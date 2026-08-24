//! Crunch pipeline: run the in-process KiCad extraction into the content-keyed
//! runtime cache, snapshot the raw source as a revision, and ingest the result
//! into the SQLite index, streaming progress to the UI as it goes.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde_json::Value;
use tauri::AppHandle;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::events::{emit, CrunchEvent};
use crate::project::{self, CrunchError, DetectedDesign, ProjectHandle};
use crate::util::LockExt;
use crate::watcher::is_relevant_source;
use crate::{cache, checkpoints, index_db, rawstore, telemetry};

/// BLAKE3 every watched source file under the design folder — the hash gate.
pub fn source_hashes(design_path: &Path) -> BTreeMap<String, String> {
    let mut hashes = BTreeMap::new();
    for entry in walkdir::WalkDir::new(design_path)
        .max_depth(4)
        .into_iter()
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if !is_relevant_source(design_path, entry.path()) {
            continue;
        }
        // strip_prefix can't fail today (WalkDir roots at design_path), but the
        // project bans panic-on-fallible-path — skip rather than unwrap so a future
        // symlink/canonicalization change can't crash every crunch.
        let Ok(rel) = entry.path().strip_prefix(design_path) else {
            continue;
        };
        if let Ok(bytes) = fs::read(entry.path()) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            hashes.insert(rel, blake3::hash(&bytes).to_hex().to_string());
        }
    }
    hashes
}

/// The per-machine hash-gate state. It is regenerable and machine-specific (a hash
/// of THIS machine's last extraction), so it lives under `local_data_root` — never in
/// the synced project folder, where every crunch would churn it and two machines would
/// fight over one path (constraint 4 / the storage model).
fn last_crunch_path(project_dir: &Path) -> PathBuf {
    project::local_data_root(project_dir).join("last_crunch.json")
}

fn last_crunch_hashes(project_dir: &Path) -> Option<BTreeMap<String, String>> {
    let text = fs::read_to_string(last_crunch_path(project_dir)).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    serde_json::from_value(v.get("source_hashes")?.clone()).ok()
}

/// The revision the KiCad design folder currently corresponds to: the hash-gate
/// state (the folder's content at the last crunch/checkout on this machine) matched
/// against history. None when nothing has been crunched yet or the gate matches no
/// surviving revision. This is the history graph's "KiCad files" marker.
pub fn design_head_id(project: &ProjectHandle) -> Option<String> {
    last_crunch_hashes(&project.project_dir)
        .and_then(|prev| project.find_rev_id_by_hashes(&prev))
}

/// The parent for a new checkpoint of the live design folder: the revision the folder
/// actually descends from ([`design_head_id`]) — NOT the revision the viewer shows.
/// Merely viewing an old version never touches the disk, so an edit made in KiCad
/// descends from the folder's own last captured state; parenting it on the viewed
/// revision would draw a branch that never happened. Fallback (first crunch, or the
/// gate matches nothing after a GC / torn checkout): the effective revision.
pub fn design_parent_id(project: &ProjectHandle) -> Option<String> {
    design_head_id(project).or_else(|| project.effective_extraction_id())
}

pub fn save_last_crunch_hashes(project_dir: &Path, hashes: &BTreeMap<String, String>) {
    let path = last_crunch_path(project_dir);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        path,
        serde_json::json!({ "source_hashes": hashes }).to_string(),
    );
}

/// Project dirs with a crunch in flight, process-wide. The per-handle
/// `crunch_running` atomic only serialises ONE handle; a rapid re-open (React
/// StrictMode in dev, or the user reopening the same project) creates a SECOND
/// handle whose crunch would otherwise run concurrently on the SAME project — two
/// extractions racing on the shared `cache/<key>.tmp` staging dir, one calling
/// `remove_dir_all` on it mid-write under the other ("The system cannot find the
/// path specified. os error 3"), plus concurrent writers to the same index DB and
/// raw-store log. Keying on the project dir serialises crunches across handles too.
fn crunching() -> &'static Mutex<HashSet<PathBuf>> {
    static S: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashSet::new()))
}

/// RAII claim on a project's crunch slot, released on drop (including a panic
/// unwind) so a crunch can never wedge the project permanently.
struct CrunchClaim(PathBuf);
impl Drop for CrunchClaim {
    fn drop(&mut self) {
        crunching().lock_safe().remove(&self.0);
    }
}

/// Claim the project's crunch slot. `None` ⇒ another handle is already crunching
/// this project, so the caller must not start a concurrent crunch.
fn claim_crunch(project_dir: &Path) -> Option<CrunchClaim> {
    let mut set = crunching().lock_safe();
    set.insert(project_dir.to_path_buf())
        .then(|| CrunchClaim(project_dir.to_path_buf()))
}

/// Public entry: serialize crunches per project. A trigger during a running
/// crunch marks it pending; the crunch re-runs once after completion.
pub fn trigger_crunch(app: AppHandle, project: Arc<ProjectHandle>, trigger: &str, force: bool) {
    // Backstop for the checkout race: while a checkout writes the design folder, never
    // start a crunch (the watcher already drops its events; this catches any already
    // queued). The checkout always clears the flag, so this can't wedge crunching.
    if project.watcher_suspended.load(Ordering::SeqCst) {
        return;
    }
    if project.crunch_running.swap(true, Ordering::SeqCst) {
        project.crunch_pending.store(true, Ordering::SeqCst);
        return;
    }
    // Cross-handle guard: don't run a second handle's crunch concurrently with an
    // in-flight crunch of the SAME project (they race on the cache staging dir → os
    // error 3). The in-flight crunch covers the same source; release our per-handle
    // flag so THIS handle can still crunch later (e.g. a real edit via its watcher).
    let Some(claim) = claim_crunch(&project.project_dir) else {
        project.crunch_running.store(false, Ordering::SeqCst);
        return;
    };
    let trigger = trigger.to_string();
    std::thread::spawn(move || {
        let _claim = claim; // slot released when the thread ends, even on panic
        let mut force = force;
        loop {
            run_one_crunch(&app, &project, &trigger, force);
            force = false; // queued re-runs go back through the hash gate
            if project.crunch_pending.swap(false, Ordering::SeqCst) {
                continue;
            }
            // No pending seen. Release the running flag, THEN re-check: a trigger
            // landing between the swap above and this store would see running==true,
            // set pending, and return — with nobody left to consume it (lost wakeup).
            // We hold the CrunchClaim for the whole thread, so no other worker can
            // race us here; re-taking `running` is safe.
            project.crunch_running.store(false, Ordering::SeqCst);
            if !project.crunch_pending.swap(false, Ordering::SeqCst) {
                break;
            }
            project.crunch_running.store(true, Ordering::SeqCst);
        }
    });
}

fn set_status(project: &ProjectHandle, f: impl FnOnce(&mut project::CrunchStatus)) {
    f(&mut project.status.lock_safe());
}

fn run_one_crunch(app: &AppHandle, project: &Arc<ProjectHandle>, trigger: &str, force: bool) {
    // No design folder on this machine → read-only mode, nothing to crunch.
    let Some(design_path) = project.design_path_clone() else {
        return;
    };
    let Some(detected) = project::detect_design(&design_path) else {
        let err = CrunchError {
            stage: "detect".into(),
            stderr_tail: format!(
                "no KiCad project file found in {}",
                design_path.display()
            ),
        };
        log::error!("{} crunch failed at stage 'detect': {}", telemetry::LOCAL_ONLY, err.stderr_tail);
        set_status(project, |s| {
            s.phase = "failed".into();
            s.error = Some(err.clone());
            s.last_finished_ts = now_rfc3339();
        });
        emit(app, CrunchEvent::Failed { stage: err.stage, stderr_tail: err.stderr_tail });
        return;
    };
    // Legacy KiCad (≤5, .pro/.sch) can be detected but not extracted — the parser reads
    // KiCad 6+ S-expr only. Fail with an actionable message rather than a cryptic parse error.
    if detected.legacy {
        let err = CrunchError {
            stage: "detect".into(),
            stderr_tail: format!(
                "{} is a legacy KiCad project. Open it in KiCad and choose File → Save \
                 to upgrade it to the current format (.kicad_pro / .kicad_sch), then re-import.",
                design_path.display()
            ),
        };
        log::error!("{} crunch failed at stage 'detect': {}", telemetry::LOCAL_ONLY, err.stderr_tail);
        set_status(project, |s| {
            s.phase = "failed".into();
            s.error = Some(err.clone());
            s.last_finished_ts = now_rfc3339();
        });
        emit(app, CrunchEvent::Failed { stage: err.stage, stderr_tail: err.stderr_tail });
        return;
    }
    let project_dir = project.project_dir.clone();
    let hashes = source_hashes(&design_path);

    // Hash gate (skip for manual "Extract now"). "Already extracted" = the live
    // design's runtime cache exists.
    if !force {
        if let Some(prev) = last_crunch_hashes(&project_dir) {
            let extracted = cache::is_cached(&project_dir, &cache::cache_key(&hashes));
            if prev == hashes && extracted {
                set_status(project, |s| s.phase = "skipped".into());
                emit(app, CrunchEvent::Skipped { reason: "hashes_unchanged".into() });
                return;
            }
        }
    }

    set_status(project, |s| {
        s.phase = "running".into();
        s.error = None;
    });
    emit(app, CrunchEvent::Started { trigger: trigger.into() });
    log::info!("extraction started ({trigger}) for '{}'", project.name);
    let started = Instant::now();
    // Performance span (Sentry transaction): design_tool + trigger + outcome only.
    let crunch_span = telemetry::start_crunch(&detected.kind, trigger);

    let result =
        crunch_kicad_revision(app, project, &detected, &design_path, &project_dir, &hashes, trigger);
    let crunch_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(revision_id) => {
            crunch_span.finish(true, None);
            log::info!("extraction succeeded for '{}' in {crunch_ms} ms", project.name);
            save_last_crunch_hashes(&project_dir, &hashes);
            crate::presence::touch(&project_dir, Some(revision_id.clone()));
            set_status(project, |s| {
                s.phase = "succeeded".into();
                s.last_revision_id = Some(revision_id.clone());
                s.last_crunch_ms = Some(crunch_ms);
                s.last_finished_ts = now_rfc3339();
            });
            emit(app, CrunchEvent::Succeeded { revision_id, crunch_ms });
        }
        Err(err) => {
            crunch_span.finish(false, Some(err.stage.as_str()));
            // Local-only: stderr_tail carries extractor free text (design, net,
            // file names). The failing stage still reaches Sentry via the span.
            log::error!(
                "{} crunch failed at stage '{}' after {crunch_ms} ms: {}",
                telemetry::LOCAL_ONLY,
                err.stage,
                err.stderr_tail
            );
            set_status(project, |s| {
                s.phase = "failed".into();
                s.error = Some(err.clone());
                s.last_finished_ts = now_rfc3339();
            });
            emit(app, CrunchEvent::Failed { stage: err.stage, stderr_tail: err.stderr_tail });
        }
    }
}

/// KiCad crunch: extract the live design into the content-keyed runtime cache, then
/// snapshot the raw source as a revision in the conflict-free raw store and ingest
/// the cache into SQLite under that revision id. No extracted bundle is persisted —
/// the cache is regenerable, the raw store is the system of record. Returns the id.
fn crunch_kicad_revision(
    app: &AppHandle,
    project: &Arc<ProjectHandle>,
    detected: &DetectedDesign,
    design_path: &Path,
    project_dir: &Path,
    hashes: &BTreeMap<String, String>,
    trigger: &str,
) -> Result<String, CrunchError> {
    let design_file = PathBuf::from(&detected.file);
    let key = cache::cache_key(hashes);

    // Extract into cache/<key>/ unless already present (an epoch bump or a source
    // edit moves the key, so a forced re-extract of unchanged source still re-runs).
    let cache_dir = if cache::is_cached(project_dir, &key) {
        cache::cache_dir(project_dir, &key)
    } else {
        let staged = cache::cache_root(project_dir).join(format!("{key}.tmp"));
        let _ = fs::remove_dir_all(&staged);
        fs::create_dir_all(&staged).map_err(|e| CrunchError {
            stage: "prepare".into(),
            stderr_tail: e.to_string(),
        })?;
        if let Err(e) = crunch_kicad(app, &design_file, &staged) {
            let _ = fs::remove_dir_all(&staged);
            return Err(e);
        }
        index_db::check_manifest_schema(&staged).map_err(|e| {
            let _ = fs::remove_dir_all(&staged);
            CrunchError { stage: "validate".into(), stderr_tail: e }
        })?;
        cache::publish(project_dir, &key, &staged)
            .map_err(|e| CrunchError { stage: "swap".into(), stderr_tail: e })?
    };

    // Auto-crunches append a machine-local checkpoint (private WIP) — the user later
    // Publishes a chosen one into the synced history. The very first crunch of a
    // brand-new project auto-publishes a root so the shared history is never empty.
    // The parent is what the design folder actually descended from (the hash-gate
    // state, read before this crunch overwrites it) — never the viewed revision.
    let git = project::git_info(design_path);
    let author = project::author_slug();
    let parent = design_parent_id(project);
    let cp = checkpoints::snapshot_local(project_dir, design_path, hashes, &author, &git, parent.as_deref())
        .map_err(|e| CrunchError { stage: "snapshot".into(), stderr_tail: e })?;
    let rev = if trigger == "create" && rawstore::latest_revision(project_dir).is_none() {
        checkpoints::publish(project_dir, &cp.id, &author, &git, Some("Initial import"))
            .map_err(|e| CrunchError { stage: "publish".into(), stderr_tail: e })?
    } else {
        cp
    };

    // Drop a dangling active pointer (a pre-upgrade extraction id, or a pruned
    // revision/checkpoint) so the viewer follows the latest instead of a missing one.
    let active_id = project.active_extraction.lock_safe().clone();
    if let Some(a) = active_id {
        if project.resolve_rev(&a).is_none() {
            // Re-check under the lock before clearing: a set_active landing after our
            // snapshot above must not have its fresh pointer clobbered to None.
            let mut guard = project.active_extraction.lock_safe();
            if guard.as_deref() == Some(a.as_str()) {
                let _ = project::set_active_extraction(project_dir, None);
                *guard = None;
            }
        }
    }

    // Ingest the cache under the revision id (rebuildable search/summary index).
    let mut conn = index_db::open(project_dir)
        .map_err(|e| CrunchError { stage: "index".into(), stderr_tail: e })?;
    index_db::ingest(&mut conn, &cache_dir, &rev.id, &rev.ts, &project.name)
        .map_err(|e| CrunchError { stage: "index".into(), stderr_tail: e })?;

    // Bound the regenerable cache (keep the just-published key + recent entries). Also
    // protect the actively-viewed revision's key so browsing history can't evict the dir
    // the viewer is serving right now.
    let mut keep = vec![key.clone()];
    if let Some(r) = project.effective_resolved() {
        keep.push(cache::cache_key(&r.revision().source_hashes));
    }
    cache::gc(project_dir, &keep, 8);

    Ok(rev.id)
}

fn now_rfc3339() -> Option<String> {
    OffsetDateTime::now_utc().format(&Rfc3339).ok()
}

/// In-process KiCad extraction: design model + BOM (grouped JSON & CSV), each as
/// its own stage so the UI shows progress and a single stage can fail cleanly.
fn crunch_kicad(app: &AppHandle, design_file: &Path, tmp: &Path) -> Result<(), CrunchError> {
    use extract::pipeline::{run_bom, run_design};

    let design_dir = tmp.join("design");
    let bom_dir = tmp.join("bom");

    run_extract_stage(app, "design", |emit| run_design(design_file, &design_dir, emit))?;
    run_extract_stage(app, "bom-json", |emit| {
        run_bom(design_file, &bom_dir, "grouped-json", emit)
    })?;
    run_extract_stage(app, "bom-csv", |emit| {
        run_bom(design_file, &bom_dir, "grouped-csv", emit)
    })?;
    // The enriched BOM is what `reviewbundle::build` uploads for a detailed review;
    // it must be written by the normal crunch or the paid tier is unreachable.
    run_extract_stage(app, "bom-enriched", |emit| {
        run_bom(design_file, &bom_dir, "enriched-csv", emit)
    })?;
    Ok(())
}

/// Drive one in-process extraction stage, mapping its progress/artifact messages
/// onto crunch events and logging a failure to the app log.
fn run_extract_stage(
    app: &AppHandle,
    stage: &str,
    run: impl FnOnce(&mut dyn FnMut(extract::pipeline::Msg)) -> Result<(), String>,
) -> Result<(), CrunchError> {
    use extract::pipeline::Msg;
    emit(app, CrunchEvent::Progress { line: format!("▸ {stage}") });
    let mut on_msg = |m: Msg| match m {
        Msg::Artifact(path) => emit(app, CrunchEvent::Artifact { path }),
        Msg::Progress(line) => emit(app, CrunchEvent::Progress { line }),
    };
    run(&mut on_msg).map_err(|e| {
        log::error!("{} extract stage '{stage}' failed: {e}", telemetry::LOCAL_ONLY);
        CrunchError { stage: stage.into(), stderr_tail: e }
    })
}

pub(crate) fn rename_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut last_err = String::new();
    for _ in 0..5 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e.to_string();
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
    Err(format!("rename {} -> {}: {last_err}", from.display(), to.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};

    fn handle_for(dir: &Path) -> ProjectHandle {
        ProjectHandle {
            project_dir: dir.to_path_buf(),
            name: "t".into(),
            class: Mutex::new(None),
            design_path: Mutex::new(None),
            design_tool: Mutex::new("kicad".into()),
            active_extraction: Mutex::new(None),
            status: Mutex::new(Default::default()),
            crunch_running: AtomicBool::new(false),
            crunch_pending: AtomicBool::new(false),
            watcher_stop: AtomicBool::new(false),
            watcher_suspended: AtomicBool::new(false),
            watcher_gen: AtomicU64::new(0),
        }
    }

    // The bug this guards against: user views an OLD revision in the app while the
    // KiCad folder sits on a NEWER one; an edit in KiCad must parent on what the
    // folder actually contained (the hash-gate state), not on the viewed revision.
    #[test]
    fn crunch_parent_follows_the_design_folder_not_the_viewer() {
        let proj = std::env::temp_dir()
            .join(format!("spinzero_parent_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&proj);
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();

        fs::write(src.join("a.kicad_sch"), b"v1").unwrap();
        let h1 = source_hashes(&src);
        let cp_a = crate::checkpoints::snapshot_local(
            &proj, &src, &h1, "a", &project::GitInfo::default(), None,
        )
        .unwrap();
        fs::write(src.join("a.kicad_sch"), b"v2").unwrap();
        let h2 = source_hashes(&src);
        let cp_b = crate::checkpoints::snapshot_local(
            &proj, &src, &h2, "a", &project::GitInfo::default(), Some(&cp_a.id),
        )
        .unwrap();

        // Disk (hash gate) is at B, but the viewer shows A.
        save_last_crunch_hashes(&proj, &h2);
        let handle = handle_for(&proj);
        *handle.active_extraction.lock_safe() = Some(cp_a.id.clone());

        assert_eq!(design_head_id(&handle).as_deref(), Some(cp_b.id.as_str()));
        assert_eq!(
            design_parent_id(&handle).as_deref(),
            Some(cp_b.id.as_str()),
            "the next checkpoint parents on the folder's state, not the viewed revision"
        );

        // Gate matching nothing (e.g. torn checkout re-seed) → fall back to effective.
        let mut unknown = BTreeMap::new();
        unknown.insert("a.kicad_sch".to_string(), "0".repeat(64));
        save_last_crunch_hashes(&proj, &unknown);
        assert_eq!(design_head_id(&handle), None);
        assert_eq!(
            design_parent_id(&handle).as_deref(),
            Some(cp_a.id.as_str()),
            "fallback is the effective (viewed) revision"
        );

        let _ = fs::remove_dir_all(&proj);
        let _ = fs::remove_dir_all(crate::checkpoints::checkpoints_root(&proj));
    }

    // The cross-handle crunch guard (fix for the dev double-open "os error 3" race):
    // the same project dir can't be claimed twice at once, different projects are
    // independent, and the slot frees on drop so a project is never wedged.
    #[test]
    fn crunch_claim_serialises_per_project_dir() {
        let a = std::env::temp_dir().join("spinzero_claim_test_a");
        let b = std::env::temp_dir().join("spinzero_claim_test_b");
        let held = claim_crunch(&a).expect("first claim on A succeeds");
        assert!(claim_crunch(&a).is_none(), "A is blocked while its claim is held");
        let other = claim_crunch(&b).expect("a different project claims independently");
        drop(held);
        assert!(claim_crunch(&a).is_some(), "A is claimable again after release");
        drop(other);
    }
}
