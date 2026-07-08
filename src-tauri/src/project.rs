//! Project model (supersedes the old vault model).
//!
//! The app manages **project folders** that point at a **design folder**. All
//! app-owned data lives in the project folder; the design folder is read-only
//! input (the extractor reads it, the watcher watches it — nothing is written
//! back). On-disk layout under a project folder:
//!
//! ```text
//! <project_dir>/              ← syncable (git/SharePoint/OneDrive)
//! ├── project.json            identity + design_path pointer
//! ├── settings.json           runtime settings (retention_days, …)
//! ├── raw/                    content-addressed raw EDA source (KiCad system of record)
//! └── reviews/                per-user review event logs (Phase 2)
//! ```
//!
//! Only `project.json` (+ `.gitignore`) is written when a project is created; the
//! folders above are created on demand by their writers the first time they store
//! something. A new project scaffolds no empty directories — none of them are
//! required to exist, so any can be safely deleted.
//!
//! Regenerable, machine-local artifacts do NOT live in the project folder — a
//! folder sync would needlessly copy multi-MB cache data and a machine-specific
//! database. They live under the OS local-data dir instead (see
//! [`local_data_root`]):
//!
//! ```text
//! <os_local_data>/spinzero/projects/<name>-<hash>/
//! ├── cache/<key>/            regenerable runtime extraction cache (KiCad)
//! ├── checkpoints/            machine-local checkpoint store (unsynced)
//! ├── last_crunch.json        per-machine source hashes for the hash gate
//! └── index.sqlite            rebuildable search index
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::util::LockExt;

pub const PROJECT_SCHEMA: u32 = 1;
pub const DEFAULT_RETENTION_DAYS: i64 = 30;

const SKIP_DIRS: &[&str] = &[
    ".pcbreview",
    ".git",
    "output",
    "History",
    "__Previews",
    "node_modules",
];

// ------------------------------------------------------------ project.json

/// `project.json` — project identity + the design-folder pointer. `design_path`
/// is absolute and machine-specific; it is handled gracefully when missing.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub schema: u32,
    pub name: String,
    pub design_path: String,
    /// Hint to skip auto-detection: "kicad" | null.
    #[serde(default)]
    pub design_tool: Option<String>,
    /// Project class: automotive | commercial | medical | industrial | space | general | null.
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// The extraction the viewer currently shows (null = latest).
    #[serde(default)]
    pub active_extraction: Option<String>,
}

fn project_json_path(project_dir: &Path) -> PathBuf {
    project_dir.join("project.json")
}

pub fn read_project_file(project_dir: &Path) -> Result<ProjectFile, String> {
    let text = fs::read_to_string(project_json_path(project_dir))
        .map_err(|e| format!("no project.json in {}: {e}", project_dir.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("project.json parse: {e}"))
}

/// Atomically write a file in the synced project folder: write a sibling `*.tmp` then
/// rename onto the target, so a sync client (SharePoint/OneDrive) or a crash can never
/// leave a half-written/zero-length file for a peer. Mirrors the `rawstore`/`reviews` logs.
/// Exported so other synced-folder writers (`highlights`) reuse it instead of a bare
/// `fs::write` that could ship a truncated file to peers.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn write_project_file(project_dir: &Path, pf: &ProjectFile) -> Result<(), String> {
    let json = serde_json::to_string_pretty(pf).map_err(|e| e.to_string())?;
    write_atomic(&project_json_path(project_dir), json.as_bytes())
}

/// Serializes `project.json` read-modify-write in-process. Sync-command serialization on
/// the main thread used to make this safe implicitly; once heavy commands moved onto worker
/// threads (async), two writers (e.g. a re-link racing the active-pointer persist) could
/// each read the old file, mutate one field, and rewrite — silently dropping the other's
/// field. This lock scopes the whole read→mutate→write so the last write reflects both.
fn project_file_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// Read `project.json`, apply `mutate`, and write it back under the RMW lock, so a
/// concurrent field update can't be lost. All single-field persisters route through this.
fn update_project_file(
    project_dir: &Path,
    mutate: impl FnOnce(&mut ProjectFile),
) -> Result<(), String> {
    let _guard = project_file_lock().lock_safe();
    let mut pf = read_project_file(project_dir)?;
    mutate(&mut pf);
    write_project_file(project_dir, &pf)
}

/// Is this folder a project folder (has project.json)?
pub fn is_project_dir(dir: &Path) -> bool {
    project_json_path(dir).is_file()
}

// ------------------------------------------------------------ local data dir

/// The app's bundle identifier — kept in lockstep with `tauri.conf.json`'s
/// `identifier`. The single source of truth for locating this app's slot under the
/// OS data dirs from modules that have no `AppHandle` (`device`, `telemetry`), so the
/// path can't drift between modules (it did once, when `telemetry` and the rest
/// disagreed on the identifier).
pub const APP_IDENTIFIER: &str = "spinzero";

/// The machine-local home for a project's regenerable artifacts (the KiCad
/// extraction cache + the SQLite index).
///
/// This lives under the OS local-data dir (`%LOCALAPPDATA%` on Windows), NOT
/// inside the project folder, so a dumb folder sync (git / SharePoint / OneDrive)
/// never copies multi-MB regenerable cache data or a machine-specific database.
/// Everything the project folder still holds (project.json, raw/, reviews/,
/// specs/) IS meant to sync.
///
/// Keyed by the project's name plus a hash of its absolute path, so each project
/// gets a stable, collision-free slot. Moving or renaming the folder simply
/// regenerates into a new slot — both artifacts rebuild on demand from the raw
/// store, so an orphaned old slot is harmless.
pub fn local_data_root(project_dir: &Path) -> PathBuf {
    let base = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_IDENTIFIER)
        .join("projects");
    let canon = fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let key = blake3::hash(canon.to_string_lossy().as_bytes()).to_hex();
    let name = project_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    base.join(format!("{name}-{}", &key.as_str()[..12]))
}

// ------------------------------------------------------------ design detection

#[derive(Clone, Serialize)]
pub struct DetectedDesign {
    pub kind: String, // "kicad"
    /// Absolute path to the EDA project file (.kicad_pro / .PrjPcb), or for a legacy
    /// KiCad project the legacy project/schematic file we matched on.
    pub file: String,
    pub name: String,
    /// A legacy KiCad ≤5 layout (.pro/.sch, no .kicad_pro). We can detect it but not
    /// extract it — the parser reads KiCad 6+ S-expr only — so the UI tells the user to
    /// re-save it in KiCad rather than claiming nothing was found.
    #[serde(default)]
    pub legacy: bool,
}

/// Rank a legacy-KiCad candidate file: prefer the project (.pro), then the schematic
/// (.sch), then the board (.kicad_pcb). Returns None for anything else.
fn legacy_kicad_rank(ext: &str) -> Option<u8> {
    if ext.eq_ignore_ascii_case("pro") {
        Some(0)
    } else if ext.eq_ignore_ascii_case("sch") {
        Some(1)
    } else if ext.eq_ignore_ascii_case("kicad_pcb") {
        Some(2)
    } else {
        None
    }
}

/// KiCad's board file-format epoch for the 6.0 release. A `.kicad_pcb` carries a
/// monotonic date-stamped `(version …)`; anything below this was written by KiCad 5
/// or earlier — a format the extractor only partially understands. (KiCad 5 boards
/// stamp e.g. `20171130`; the first 6.0 stable is `20211014`.)
const KICAD6_PCB_EPOCH: i64 = 20211014;

/// Read the leading `(version NNNNNNNN)` integer from a KiCad S-expression file
/// (`.kicad_pcb` / `.kicad_sch`) by scanning only the file head — no full parse. The
/// `(version …)` token is the first field of the root node, well within the head.
fn kicad_sexpr_version(path: &Path) -> Option<i64> {
    use std::io::Read;
    let mut buf = [0u8; 4096];
    let mut f = fs::File::open(path).ok()?;
    let n = f.read(&mut buf).ok()?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let idx = head.find("(version")?;
    let digits: String = head[idx + "(version".len()..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Whether the design fronted by `pro` (a `.kicad_pro`) is really a pre-KiCad-6 layout.
///
/// Extension alone lies here: opening a KiCad 5 project in a newer KiCad rewrites the
/// `.kicad_pro`/`.kicad_sch` but can leave the *board* in the old format (e.g. STRF's
/// `.kicad_pcb` stays `(version 20171130) (host pcbnew "(5.1.5)")`), which we can't
/// fully extract. We judge by the board's `(version …)`: prefer the same-stem
/// `.kicad_pcb`; failing that, treat the folder as legacy only if every sibling board
/// we can read is pre-6.
fn kicad_pro_is_pre6(pro: &Path) -> bool {
    let dir = match pro.parent() {
        Some(d) => d,
        None => return false,
    };
    let stem = pro.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // The board matching the project name is authoritative when present.
    let same = dir.join(format!("{stem}.kicad_pcb"));
    if same.is_file() {
        return matches!(kicad_sexpr_version(&same), Some(v) if v < KICAD6_PCB_EPOCH);
    }
    // No same-stem board: fall back to the sibling boards. Only condemn the design when
    // there is at least one board and none of the readable ones are KiCad 6+.
    let mut saw_board = false;
    let mut all_pre6 = true;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x.eq_ignore_ascii_case("kicad_pcb")).unwrap_or(false) {
                saw_board = true;
                if !matches!(kicad_sexpr_version(&p), Some(v) if v < KICAD6_PCB_EPOCH) {
                    all_pre6 = false;
                }
            }
        }
    }
    saw_board && all_pre6
}

/// Find the EDA project file inside a design folder (depth 3, skipping our own
/// and EDA-internal dirs). Returns the shallowest modern match (ties broken by name);
/// failing that, a legacy KiCad layout if one is present (so the UI can prompt an
/// upgrade instead of reporting "no design found").
pub fn detect_design(design_path: &Path) -> Option<DetectedDesign> {
    let mut best: Option<(usize, DetectedDesign)> = None;
    // Legacy KiCad fallback, keyed by (depth, rank) so a .pro beats a stray .sch/.kicad_pcb.
    let mut legacy: Option<(usize, u8, DetectedDesign)> = None;
    for entry in WalkDir::new(design_path)
        .max_depth(3)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && SKIP_DIRS.iter().any(|s| {
                    e.file_name().to_string_lossy().eq_ignore_ascii_case(s)
                        || e.file_name().to_string_lossy().starts_with("Project Outputs")
                }))
        })
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let depth = path.strip_prefix(design_path).map(|r| r.components().count()).unwrap_or(99);
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let kind = if ext.eq_ignore_ascii_case("kicad_pro") {
            "kicad"
        } else {
            // Not a modern KiCad project file — remember a legacy KiCad candidate
            // for the fallback. (Only KiCad projects are detected.)
            if let Some(rank) = legacy_kicad_rank(ext) {
                let cand = DetectedDesign {
                    kind: "kicad".into(),
                    file: path.to_string_lossy().into_owned(),
                    name,
                    legacy: true,
                };
                let better = match &legacy {
                    None => true,
                    Some((d, r, _)) => depth < *d || (depth == *d && rank < *r),
                };
                if better {
                    legacy = Some((depth, rank, cand));
                }
            }
            continue;
        };
        let cand = DetectedDesign {
            kind: kind.into(),
            file: path.to_string_lossy().into_owned(),
            name,
            legacy: false,
        };
        let better = match &best {
            None => true,
            Some((d, b)) => depth < *d || (depth == *d && cand.file < b.file),
        };
        if better {
            best = Some((depth, cand));
        }
    }
    // A modern-looking project (found a .kicad_pro) can still front a pre-6 board that a
    // newer KiCad left in the old format. Verify by the board version before accepting it,
    // so the wizard refuses it instead of importing an incomplete extraction.
    if let Some((_, d)) = best.as_mut() {
        if kicad_pro_is_pre6(Path::new(&d.file)) {
            d.legacy = true;
        }
    }
    best.map(|(_, d)| d).or_else(|| legacy.map(|(_, _, d)| d))
}

// ------------------------------------------------------------ extractions

fn default_true() -> bool {
    true
}

/// The picker's row shape, synthesized from a folded `rawstore::Revision` or
/// `checkpoints::Checkpoint`.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct ExtractionMeta {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    /// Changelog message captured at publish. None for local checkpoints.
    #[serde(default)]
    pub message: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub design_tool: Option<String>,
    #[serde(default)]
    pub git_hash: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_dirty: Option<bool>,
    #[serde(default)]
    pub source_hashes: BTreeMap<String, String>,
    /// Revisions this derived from (`[]` = root). Drives the history-graph edges.
    #[serde(default)]
    pub parents: Vec<String>,
    /// Tag names pointing here (git-tag-style ref labels).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Retracted/tombstoned — filtered from the picker by default.
    #[serde(default)]
    pub hidden: bool,
    /// In the synced (shared) history. Local-only checkpoints are `false`.
    #[serde(default = "default_true")]
    pub published: bool,
    /// A machine-local autosave checkpoint (not yet published to the team).
    #[serde(default)]
    pub is_checkpoint: bool,
    /// Author of the create event (graph rows / presence).
    #[serde(default)]
    pub author: Option<String>,
}

// ------------------------------------------------------------ git info

#[derive(Clone, Serialize, Default)]
pub struct GitInfo {
    pub hash: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
}

/// Best-effort git metadata for the design folder. Git not installed or not a
/// repo → all fields null/false, never an error.
pub fn git_info(design_path: &Path) -> GitInfo {
    let run = |args: &[&str]| -> Option<String> {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(design_path).args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let hash = run(&["rev-parse", "--short", "HEAD"]).filter(|s| !s.is_empty());
    if hash.is_none() {
        return GitInfo::default(); // not a repo (or git missing)
    }
    let branch = run(&["branch", "--show-current"]).filter(|s| !s.is_empty());
    let dirty = run(&["status", "--porcelain"]).map(|s| !s.is_empty()).unwrap_or(false);
    GitInfo { hash, branch, dirty }
}

// ------------------------------------------------------------ handle / info

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct CrunchError {
    pub stage: String,
    pub stderr_tail: String,
}

#[derive(Clone, Serialize, Default)]
pub struct CrunchStatus {
    pub phase: String, // idle|running|succeeded|failed|skipped
    pub last_revision_id: Option<String>,
    pub last_crunch_ms: Option<u64>,
    pub last_finished_ts: Option<String>,
    pub error: Option<CrunchError>,
}

/// One open project. Lives behind an Arc; the watcher thread holds a clone.
pub struct ProjectHandle {
    pub project_dir: PathBuf,
    pub name: String,
    pub class: Option<String>,
    /// Absolute design folder, or None when it does not exist on this machine
    /// (read-only mode until re-linked).
    pub design_path: Mutex<Option<PathBuf>>,
    /// "kicad".
    pub design_tool: Mutex<String>,
    /// The extraction the viewer shows (None = latest on disk).
    pub active_extraction: Mutex<Option<String>>,
    pub status: Mutex<CrunchStatus>,
    /// Crunch serialization: at most one extraction per project at a time; a
    /// second trigger while running sets `crunch_pending` and re-runs once.
    pub crunch_running: AtomicBool,
    pub crunch_pending: AtomicBool,
    /// Set to true to stop this project's watcher thread.
    pub watcher_stop: AtomicBool,
    /// Set while a checkout writes the selected revision into the design folder, so
    /// the watcher ignores its own writes (and `trigger_crunch` no-ops) — otherwise
    /// the checkout would immediately spawn a spurious revision of itself.
    pub watcher_suspended: AtomicBool,
    /// Bumped to retire this handle's current watcher and start a fresh one (re-link
    /// points the project at a new design folder). Each watcher captures the value at
    /// spawn and exits once it no longer matches, so a re-link can't leak the old
    /// watcher onto the stale path or leave two watchers running at once.
    pub watcher_gen: AtomicU64,
}

#[derive(Clone, Serialize)]
pub struct ProjectInfo {
    pub project_dir: String,
    pub name: String,
    pub design_path: Option<String>,
    pub design_path_exists: bool,
    pub design_tool: Option<String>,
    pub class: Option<String>,
    pub active_extraction: Option<String>,
    pub extraction_count: usize,
}

/// A revision resolved from either store. Synced-first: a published revision (which
/// lives in both stores) resolves as `Synced`; a purely machine-local checkpoint as
/// `Local`. Both wrap the same `Revision` shape; the variant only tells the cache /
/// materialize path which object store to read from.
pub enum ResolvedRev {
    Synced(crate::rawstore::Revision),
    Local(crate::rawstore::Revision),
}

impl ResolvedRev {
    pub fn revision(&self) -> &crate::rawstore::Revision {
        match self {
            ResolvedRev::Synced(r) | ResolvedRev::Local(r) => r,
        }
    }
}

impl ProjectHandle {
    /// The source-hash map for any revision id — the diff/checkout input. Resolves a
    /// synced revision or a local checkpoint (synced-first).
    pub fn resolve_source_hashes(&self, id: &str) -> Option<BTreeMap<String, String>> {
        self.resolve_rev(id).map(|r| r.revision().source_hashes.clone())
    }

    /// Resolve an id from either store, **synced-first** (a published revision wins
    /// over its local twin), else a purely-local checkpoint.
    pub fn resolve_rev(&self, id: &str) -> Option<ResolvedRev> {
        if let Some(r) = crate::rawstore::find_revision(&self.project_dir, id) {
            return Some(ResolvedRev::Synced(r));
        }
        crate::checkpoints::find_checkpoint(&self.project_dir, id).map(ResolvedRev::Local)
    }

    /// The revision the viewer should show, resolved from either store: the active id
    /// if it resolves, else the newest of (latest synced revision, latest checkpoint).
    pub fn effective_resolved(&self) -> Option<ResolvedRev> {
        let active = self.active_extraction.lock_safe().clone();
        if let Some(id) = active {
            if let Some(r) = self.resolve_rev(&id) {
                return Some(r);
            }
        }
        self.latest_resolved()
    }

    /// The newest revision across both stores (ignores the active pointer) — the
    /// checkout target when the user picks "latest".
    pub fn latest_resolved(&self) -> Option<ResolvedRev> {
        let synced = crate::rawstore::latest_revision(&self.project_dir);
        let local = crate::checkpoints::latest_checkpoint(&self.project_dir);
        match (synced, local) {
            (Some(s), Some(l)) => {
                if l.ts > s.ts {
                    Some(ResolvedRev::Local(l))
                } else {
                    Some(ResolvedRev::Synced(s))
                }
            }
            (Some(s), None) => Some(ResolvedRev::Synced(s)),
            (None, Some(l)) => Some(ResolvedRev::Local(l)),
            (None, None) => None,
        }
    }

    /// Is the given live source already captured as some revision or checkpoint? When
    /// true, a checkout can overwrite the working tree without losing anything (a
    /// "clean" tree); when false there are un-captured edits and the checkout must warn.
    pub fn source_is_captured(&self, live: &BTreeMap<String, String>) -> bool {
        crate::rawstore::list_revisions(&self.project_dir)
            .iter()
            .any(|r| &r.source_hashes == live)
            || crate::checkpoints::list_checkpoints(&self.project_dir)
                .iter()
                .any(|r| &r.source_hashes == live)
    }

    /// The effective revision id served to the viewer. None if nothing has been
    /// extracted yet.
    pub fn effective_extraction_id(&self) -> Option<String> {
        self.effective_resolved().map(|r| r.revision().id.clone())
    }

    /// The bundle dir the viewer reads: the runtime cache dir for the effective
    /// revision (`cache/<key>/`) — which the crunch/open/set-active flows are
    /// responsible for having extracted. The path is returned whether or not it
    /// exists yet; serving fails gracefully until the background crunch publishes
    /// it, exactly as a first open behaves today.
    pub fn active_extraction_dir(&self) -> Option<PathBuf> {
        let resolved = self.effective_resolved()?;
        Some(crate::cache::cache_dir(
            &self.project_dir,
            &crate::cache::cache_key(&resolved.revision().source_hashes),
        ))
    }

    pub fn design_path_clone(&self) -> Option<PathBuf> {
        self.design_path.lock_safe().clone()
    }

    /// The revision history as `ExtractionMeta`, newest-first — the picker's
    /// data source: raw-store revisions mapped onto the picker row shape.
    pub fn list_extractions_meta(&self) -> Vec<ExtractionMeta> {
        {
            // Merge the synced (published) history with this machine's local-only
            // checkpoints. A published checkpoint shares its id with the synced
            // revision, so it appears once (as published); only checkpoints with no
            // synced twin are added as local-only rows.
            let to_meta = |r: crate::rawstore::Revision, published: bool, is_checkpoint: bool| {
                ExtractionMeta {
                    id: r.id,
                    label: r.label,
                    message: r.message,
                    created_at: r.ts,
                    design_tool: Some("kicad".into()),
                    git_hash: r.git_hash,
                    git_branch: r.git_branch,
                    git_dirty: r.git_dirty,
                    source_hashes: r.source_hashes,
                    parents: r.parents.into_iter().collect(),
                    tags: r.tags,
                    hidden: r.hidden,
                    published,
                    is_checkpoint,
                    author: Some(r.author),
                }
            };
            let synced = crate::rawstore::list_revisions(&self.project_dir);
            let synced_ids: std::collections::HashSet<String> =
                synced.iter().map(|r| r.id.clone()).collect();
            let mut metas: Vec<ExtractionMeta> =
                synced.into_iter().map(|r| to_meta(r, true, false)).collect();
            for cp in crate::checkpoints::list_checkpoints(&self.project_dir) {
                if !synced_ids.contains(&cp.id) {
                    metas.push(to_meta(cp, false, true));
                }
            }
            metas.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            metas
        }
    }

    pub fn info(&self) -> ProjectInfo {
        let design_path = self.design_path_clone();
        // Bind each lock result to its own statement so the `design_tool` guard is
        // released before the next field runs (holding a guard across the sibling
        // fields' method calls risks a self-deadlock if one re-locks it).
        let design_tool = self.design_tool.lock_safe().clone();
        let active_extraction = self.effective_extraction_id();
        let extraction_count = self.list_extractions_meta().len();
        ProjectInfo {
            project_dir: self.project_dir.to_string_lossy().into_owned(),
            name: self.name.clone(),
            design_path: design_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            design_path_exists: design_path.as_ref().map(|p| p.is_dir()).unwrap_or(false),
            design_tool: Some(design_tool),
            class: self.class.clone(),
            active_extraction,
            extraction_count,
        }
    }
}

// ------------------------------------------------------------ create / open

/// Create the on-disk project folder structure + project.json + a specs starter.
/// Does not run the first extraction (the caller opens and triggers it).
pub fn create_project(
    project_dir: &Path,
    name: &str,
    design_path: &Path,
    design_tool: Option<String>,
    class: Option<String>,
) -> Result<(), String> {
    if !design_path.is_dir() {
        return Err(format!("design folder not found: {}", design_path.display()));
    }
    if is_project_dir(project_dir) {
        return Err(format!("a project already exists at {}", project_dir.display()));
    }
    // No empty scaffold dirs: `reviews/` and `raw/` are
    // created on demand by their writers when they first have something to store, and
    // the reserved `sessions/` / `knowledge_base/` / `specs/` dirs were unused clutter.
    // A new project is just project.json (+ .gitignore) until real content lands, and any
    // of those folders is safe to delete.

    // Detect the design tool if the caller didn't pass one.
    let tool = design_tool.or_else(|| detect_design(design_path).map(|d| d.kind));

    let created_at = OffsetDateTime::now_utc().format(&Rfc3339).ok();
    let pf = ProjectFile {
        schema: PROJECT_SCHEMA,
        name: name.to_string(),
        design_path: design_path.to_string_lossy().into_owned(),
        design_tool: tool,
        class: class.clone(),
        created_at,
        active_extraction: None,
    };
    write_project_file(project_dir, &pf)?;
    write_gitignore(project_dir);
    Ok(())
}

/// Keep transient staging dirs out of version control. Regenerable, machine-local
/// artifacts (the extraction cache + SQLite index) already live OUTSIDE the project
/// folder (see [`local_data_root`]), so they don't need ignoring here — only the
/// short-lived `*.tmp` publish dirs do. The raw store, reviews, and project.json ARE
/// meant to sync. Best-effort, never clobbers an existing file.
pub fn write_gitignore(project_dir: &Path) {
    let path = project_dir.join(".gitignore");
    if path.exists() {
        return;
    }
    let _ = fs::write(
        &path,
        "# SpinZero — transient publish staging (regenerable cache + index live\n\
         # under the OS local-data dir, never inside this folder)\n\
         *.tmp\n",
    );
}

/// Open an existing project folder. A missing `design_path` yields a handle with
/// `design_path = None` (read-only mode — the last extraction still serves).
pub fn open_project(project_dir: &Path) -> Result<ProjectHandle, String> {
    let pf = read_project_file(project_dir)?;
    let design_path = PathBuf::from(&pf.design_path);
    let exists = design_path.is_dir();

    // Resolve the tool: stored hint, else detect (only possible when the folder exists).
    let tool = pf
        .design_tool
        .clone()
        .or_else(|| if exists { detect_design(&design_path).map(|d| d.kind) } else { None })
        .unwrap_or_else(|| "kicad".into());

    Ok(ProjectHandle {
        project_dir: project_dir.to_path_buf(),
        name: pf.name,
        class: pf.class,
        design_path: Mutex::new(if exists { Some(design_path) } else { None }),
        design_tool: Mutex::new(tool),
        active_extraction: Mutex::new(pf.active_extraction),
        status: Mutex::new(Default::default()),
        crunch_running: AtomicBool::new(false),
        crunch_pending: AtomicBool::new(false),
        watcher_stop: AtomicBool::new(false),
        watcher_suspended: AtomicBool::new(false),
        watcher_gen: AtomicU64::new(0),
    })
}

/// Persist `active_extraction` back to project.json (best-effort merge that
/// preserves the rest of the file).
pub fn set_active_extraction(project_dir: &Path, id: Option<&str>) -> Result<(), String> {
    update_project_file(project_dir, |pf| pf.active_extraction = id.map(|s| s.to_string()))
}

/// Persist a new design path + tool to project.json (used by re-link).
pub fn set_design_path(project_dir: &Path, design_path: &Path, tool: &str) -> Result<(), String> {
    update_project_file(project_dir, |pf| {
        pf.design_path = design_path.to_string_lossy().into_owned();
        pf.design_tool = Some(tool.to_string());
    })
}

// ------------------------------------------------------------ settings

#[derive(Serialize, Deserialize, Default)]
pub struct Settings {
    pub schema: String,
    pub retention_days: Option<i64>,
}

/// Read project settings (the retention window). Returns defaults when
/// `settings.json` is absent.
pub fn load_settings(project_dir: &Path) -> Settings {
    let Ok(text) = fs::read_to_string(project_dir.join("settings.json")) else {
        return Settings::default(); // absent → defaults (the common, expected case)
    };
    match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            // A malformed file silently reverting to defaults could let prune delete
            // revisions the user configured a longer retention to keep — make it diagnosable.
            log::warn!("settings.json parse failed ({e}); using defaults");
            Settings::default()
        }
    }
}

// ------------------------------------------------------------ misc

fn slugify(s: &str) -> String {
    // Collapse every run of non-alphanumerics to a single dash in one pass — a
    // `replace("--", "-")` is non-overlapping, so "a---b" would leave "a--b".
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// OS username sanitized to a slug — the review author identity (spec §4).
pub fn author_slug() -> String {
    let raw = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into());
    let slug = slugify(&raw);
    if slug.is_empty() {
        "user".into()
    } else {
        slug
    }
}

#[cfg(test)]
mod detect_tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sz-detect-{}-{}",
            std::process::id(),
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn pcb(version: i64) -> String {
        format!("(kicad_pcb (version {version}) (host pcbnew \"(x)\")\n)\n")
    }

    #[test]
    fn modern_kicad_pro_is_accepted() {
        let d = tmp();
        fs::write(d.join("b.kicad_pro"), "{}").unwrap();
        fs::write(d.join("b.kicad_pcb"), pcb(20241229)).unwrap();
        let det = detect_design(&d).expect("detected");
        assert_eq!(det.kind, "kicad");
        assert!(!det.legacy, "KiCad 9 board must not be flagged legacy");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn pre6_board_behind_fresh_project_is_legacy() {
        // The STRF case: a newer KiCad rewrote .kicad_pro but the board stayed KiCad 5.
        let d = tmp();
        fs::write(d.join("b.kicad_pro"), "{}").unwrap();
        fs::write(d.join("b.kicad_pcb"), pcb(20171130)).unwrap();
        let det = detect_design(&d).expect("detected");
        assert!(det.legacy, "pre-6 board must be flagged legacy despite .kicad_pro");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn exactly_the_epoch_is_modern() {
        let d = tmp();
        fs::write(d.join("b.kicad_pro"), "{}").unwrap();
        fs::write(d.join("b.kicad_pcb"), pcb(KICAD6_PCB_EPOCH)).unwrap();
        let det = detect_design(&d).expect("detected");
        assert!(!det.legacy, "the 6.0 epoch itself is KiCad 6, not legacy");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn legacy_only_folder_is_legacy() {
        // No .kicad_pro at all — the extension fallback still flags it.
        let d = tmp();
        fs::write(d.join("b.pro"), "version=1\n").unwrap();
        fs::write(d.join("b.sch"), "EESchema Schematic File Version 4\n").unwrap();
        let det = detect_design(&d).expect("detected");
        assert!(det.legacy);
        fs::remove_dir_all(&d).ok();
    }
}
