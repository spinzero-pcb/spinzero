//! Content-addressed raw-source store — the system of record for KiCad designs.
//!
//! Layout under a **store root** (`<project_dir>/raw/` for synced history, or the
//! machine-local checkpoint root — see `checkpoints.rs`, which reuses this object +
//! event layer):
//! - `objects/<aa>/<blake3-hex>` — zstd-compressed raw source bytes (immutable, deduped).
//! - `revisions.<user>.<device>.jsonl` — per-user-per-device append-only event logs.
//!
//! A revision captures the design's raw EDA source (not the extracted output, which
//! the app regenerates into `cache/`). The store is **conflict-free** the same way
//! reviews are (phase2-workflow.md §7.2): each user+device writes only its own log, so
//! a dumb share (git/OneDrive/Syncthing) merges with zero write conflicts; the history
//! is the *fold* of every log by total order `(lamport, ts, user, event_id)`.
//!
//! Revision identity is the **content hash of the root manifest** (`r_<hash[..12]>`),
//! so two engineers extracting byte-identical source independently mint the *same*
//! revision id — histories merge and dedupe automatically. This shared id space is
//! also what lets a local checkpoint **publish** into the synced store keeping its id.
//! `objects/` merges for free (immutable, hash-named). `label`/`pin`/`tag`/`hide` are
//! last-writer-wins on the fold; the append-only logs are NEVER rewritten.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::project::GitInfo;
use crate::util::LockExt;

const ZSTD_LEVEL: i32 = 3;

/// Log-filename prefix for the synced revision logs (the fold globs `revisions.*`).
pub(crate) const REVISIONS_PREFIX: &str = "revisions.";

// ------------------------------------------------------------ paths

pub fn raw_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("raw")
}

fn objects_dir(root: &Path) -> PathBuf {
    root.join("objects")
}

fn object_path(root: &Path, hash: &str) -> Result<PathBuf, String> {
    // Object hashes are blake3 hex (64 chars). Guard the 2-char shard slice so a torn or
    // malformed log line (e.g. a create event missing `root`) yields an error instead of a
    // byte-index panic when the revision is later materialized or purged.
    let shard = hash.get(..2).ok_or_else(|| format!("bad object hash {hash:?}"))?;
    Ok(objects_dir(root).join(shard).join(hash))
}

/// This machine's synced revision log under `raw`.
fn revisions_log(raw: &Path, user: &str) -> PathBuf {
    raw.join(format!("{REVISIONS_PREFIX}{user}.{}.jsonl", crate::device::device_id()))
}

/// Join a manifest-relative path under `base`, rejecting anything that would escape it.
/// Manifest paths (`RootEntry.path` / `source_hashes` keys) come from JSONL logs and root
/// manifests that sync in from OTHER machines (SharePoint/git — the whole design history),
/// so a corrupt or hostile entry (`..\..\x`, `/etc/x`, `C:\Windows\...`) must never let a
/// materialize/checkout write outside the destination tree. Platform-independent (checks
/// both separators + drive colon) since a manifest can be authored on any OS; mirrors the
/// canonicalize guard `design::read_artifact` applies on the read side.
pub(crate) fn safe_join(base: &Path, rel: &str) -> Result<PathBuf, String> {
    let unsafe_path = rel.is_empty()
        || rel.starts_with('/')
        || rel.starts_with('\\')
        || rel.contains(':') // drive letter (`C:`) or NTFS alternate data stream
        || rel.split(['/', '\\']).any(|seg| seg == "..");
    if unsafe_path {
        return Err(format!("refusing unsafe manifest path {rel:?}"));
    }
    Ok(base.join(rel))
}

// ------------------------------------------------------------ object store
// Root-parameterized so the machine-local checkpoint store reuses it verbatim.

/// Store one blob (zstd-3) under its hash. Immutable: an existing object is a no-op.
pub(crate) fn write_object(root: &Path, hash: &str, bytes: &[u8]) -> Result<(), String> {
    let path = object_path(root, hash)?;
    if path.exists() {
        return Ok(()); // dedup — objects are content-addressed and immutable
    }
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let compressed = zstd::encode_all(bytes, ZSTD_LEVEL).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, compressed).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub(crate) fn read_object(root: &Path, hash: &str) -> Result<Vec<u8>, String> {
    let bytes = fs::read(object_path(root, hash)?).map_err(|e| format!("object {hash}: {e}"))?;
    zstd::decode_all(bytes.as_slice()).map_err(|e| e.to_string())
}

/// One file in a revision's root manifest.
#[derive(Serialize, Deserialize)]
pub(crate) struct RootEntry {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

/// Write every source blob + the sorted root manifest under `root`; return the
/// content-derived `(revision_id, root_hash)`. Shared by `snapshot` (synced) and
/// `checkpoints::snapshot_local` (machine-local) — same hashing ⇒ same id space.
pub(crate) fn build_root(
    root: &Path,
    source_dir: &Path,
    source_hashes: &BTreeMap<String, String>,
) -> Result<(String, String), String> {
    let mut entries: Vec<RootEntry> = Vec::new();
    for (rel, hash) in source_hashes {
        let bytes = fs::read(source_dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
        write_object(root, hash, &bytes)?;
        entries.push(RootEntry { path: rel.clone(), hash: hash.clone(), size: bytes.len() as u64 });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let root_bytes = serde_json::to_vec(&entries).map_err(|e| e.to_string())?;
    let root_hash = blake3::hash(&root_bytes).to_hex().to_string();
    write_object(root, &root_hash, &root_bytes)?;
    let id = format!("r_{}", &root_hash[..12]);
    Ok((id, root_hash))
}

// ------------------------------------------------------------ events / fold

/// One append-only revision event. Flat (not an enum) so partial reads stay trivial
/// and unknown future fields are ignored — same shape discipline as `reviews::Event`.
/// Every optional field is `#[serde(default)]` so older logs (lacking parent/tag/…)
/// keep folding and older app builds ignore newer fields.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RevEvent {
    event_id: String,
    lamport: u64,
    ts: String,
    user: String,
    /// "create" | "label" | "pin" | "tag" | "untag" | "hide" | "unhide"
    action: String,
    revision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    source_hashes: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_dirty: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pinned: Option<bool>,
    /// Revision this was derived from (what the design folder descended from at
    /// crunch time — the hash-gate state, not the viewed revision). None = root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    /// Tag name for "tag"/"untag" actions (git-tag semantics, unique by name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag_name: Option<String>,
    /// Optional human message — annotated-tag note / hide-retract reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// A folded revision — the materialized view callers consume.
#[derive(Clone, Serialize)]
pub struct Revision {
    pub id: String,
    pub ts: String,
    pub author: String,
    pub root: String,
    pub source_hashes: BTreeMap<String, String>,
    pub label: Option<String>,
    /// Changelog/commit message captured at publish time (None for local checkpoints
    /// bundles). Shown as the row's primary text in the history graph.
    pub message: Option<String>,
    pub pinned: bool,
    /// Revisions this one derived from (normally one; a convergence node has two).
    pub parents: BTreeSet<String>,
    /// Tag names pointing at this revision (movable, unique by name across history).
    pub tags: Vec<String>,
    /// Tombstoned/retracted — filtered from the picker by default; reversible.
    pub hidden: bool,
    pub git_hash: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
}

/// Read every `<prefix>*.jsonl` log under `dir` into one unsorted vec. The prefix
/// segments synced revisions (`revisions.`) from local checkpoints (`checkpoints.`)
/// while sharing all the fold/append machinery. Device-scoped filenames
/// (`revisions.<user>.<device>.jsonl`) still match the prefix — no change here.
pub(crate) fn read_all_events(dir: &Path, prefix: &str) -> Vec<RevEvent> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.starts_with(prefix) && name.ends_with(".jsonl")) {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(ev) = serde_json::from_str::<RevEvent>(line) {
                    events.push(ev);
                }
            }
        }
    }
    events
}

fn order_key(e: &RevEvent) -> (u64, String, String, String) {
    (e.lamport, e.ts.clone(), e.user.clone(), e.event_id.clone())
}

/// Fold events into the revision list, newest-first by creation. Create events are
/// grouped by `revision_id` (= content hash), so duplicate creates of the same source
/// dedupe; `parent` unions across all creates for an id (convergence ⇒ two parents);
/// `label`/`pin`/`hide` are last-writer-wins; tags are a name→id map (LWW by name).
pub(crate) fn fold(mut events: Vec<RevEvent>) -> Vec<Revision> {
    events.sort_by_key(order_key);

    let mut by_id: BTreeMap<String, Revision> = BTreeMap::new();
    let mut tag_map: BTreeMap<String, String> = BTreeMap::new();
    for e in events {
        match e.action.as_str() {
            "create" => {
                // Earliest create wins for scalar fields (events are in total order);
                // `parent` unions across every create for this id.
                let parent = e.parent.clone();
                let r = by_id.entry(e.revision_id.clone()).or_insert_with(|| Revision {
                    id: e.revision_id.clone(),
                    ts: e.ts.clone(),
                    author: e.user.clone(),
                    root: e.root.clone().unwrap_or_default(),
                    source_hashes: e.source_hashes.clone(),
                    label: e.label.clone(),
                    message: e.message.clone(),
                    pinned: e.pinned.unwrap_or(false),
                    parents: BTreeSet::new(),
                    tags: Vec::new(),
                    hidden: false,
                    git_hash: e.git_hash.clone(),
                    git_branch: e.git_branch.clone(),
                    git_dirty: e.git_dirty,
                });
                if let Some(p) = parent {
                    r.parents.insert(p);
                }
            }
            "label" => {
                if let Some(r) = by_id.get_mut(&e.revision_id) {
                    r.label = e.label; // LWW
                }
            }
            "pin" => {
                if let Some(r) = by_id.get_mut(&e.revision_id) {
                    r.pinned = e.pinned.unwrap_or(false); // LWW
                }
            }
            "tag" => {
                if let Some(n) = e.tag_name {
                    tag_map.insert(n, e.revision_id); // move a tag = re-point it (LWW by name)
                }
            }
            "untag" => {
                if let Some(n) = e.tag_name {
                    tag_map.remove(&n);
                }
            }
            "hide" => {
                if let Some(r) = by_id.get_mut(&e.revision_id) {
                    r.hidden = true; // LWW tombstone
                }
            }
            "unhide" => {
                if let Some(r) = by_id.get_mut(&e.revision_id) {
                    r.hidden = false;
                }
            }
            _ => {}
        }
    }

    // Attach tags to their revisions (a tag pointing at a not-yet-synced revision is
    // silently skipped and re-attaches once the create syncs).
    for (name, rid) in tag_map {
        if let Some(r) = by_id.get_mut(&rid) {
            r.tags.push(name);
        }
    }
    for r in by_id.values_mut() {
        r.tags.sort();
        r.tags.dedup();
    }

    let mut revs: Vec<Revision> = by_id.into_values().collect();
    revs.sort_by(|a, b| b.ts.cmp(&a.ts)); // newest-first (ids are content hashes, not sortable)
    revs
}

/// Fold every per-user-per-device synced log into the revision list, newest-first.
pub fn list_revisions(project_dir: &Path) -> Vec<Revision> {
    fold(read_all_events(&raw_dir(project_dir), REVISIONS_PREFIX))
}

pub fn find_revision(project_dir: &Path, id: &str) -> Option<Revision> {
    list_revisions(project_dir).into_iter().find(|r| r.id == id)
}

pub fn latest_revision(project_dir: &Path) -> Option<Revision> {
    list_revisions(project_dir).into_iter().next()
}

/// Next lamport across every `<prefix>*` log under `dir` (monotonic across device logs).
pub(crate) fn next_lamport(dir: &Path, prefix: &str) -> u64 {
    read_all_events(dir, prefix)
        .iter()
        .map(|e| e.lamport)
        .max()
        .map(|m| m + 1)
        .unwrap_or(0)
}

fn now_rfc3339() -> Result<String, String> {
    OffsetDateTime::now_utc().format(&Rfc3339).map_err(|e| e.to_string())
}

/// Serializes in-process appends across ALL event logs. `append_event` reads the whole
/// file → appends in memory → atomic-renames; two threads appending to the SAME log (e.g.
/// an open-time prune-bookkeeping write racing a user `tag_revision`, or a crunch-thread
/// publish racing a label edit — both target this user+device's log) would each read the
/// old file and the loser's line would be silently erased from the system of record. One
/// process-wide lock is enough: cross-process safety already comes from each device owning
/// its own log file exclusively. Wraps pure file work, so it can't deadlock.
fn append_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// Append one event to a specific log file (whole-file atomic replace, like reviews).
pub(crate) fn append_event(log_path: &Path, event: &RevEvent) -> Result<(), String> {
    let _guard = append_lock().lock_safe();
    fs::create_dir_all(log_path.parent().unwrap()).map_err(|e| e.to_string())?;
    let mut line = serde_json::to_string(event).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut out = fs::read(log_path).unwrap_or_default();
    out.extend_from_slice(line.as_bytes());
    let tmp = log_path.with_extension("jsonl.tmp");
    fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    fs::rename(&tmp, log_path).map_err(|e| e.to_string())
}

fn make_id(prefix: &str, parts: &[&str]) -> String {
    let joined = parts.join("");
    format!("{prefix}_{}", &blake3::hash(joined.as_bytes()).to_hex().as_str()[..12])
}

/// Build + append one `create` event to `log_path` (lamport supplied by the caller so
/// synced and checkpoint logs keep independent clocks), returning the folded revision.
pub(crate) fn append_create_event(
    log_path: &Path,
    lamport: u64,
    id: &str,
    root_hash: &str,
    source_hashes: &BTreeMap<String, String>,
    user: &str,
    git: &GitInfo,
    parent: Option<&str>,
    message: Option<&str>,
) -> Result<Revision, String> {
    let ts = now_rfc3339()?;
    let event = RevEvent {
        event_id: make_id("e", &[user, &lamport.to_string(), &ts, id]),
        lamport,
        ts: ts.clone(),
        user: user.to_string(),
        action: "create".into(),
        revision_id: id.to_string(),
        root: Some(root_hash.to_string()),
        source_hashes: source_hashes.clone(),
        git_hash: git.hash.clone(),
        git_branch: git.branch.clone(),
        git_dirty: Some(git.dirty),
        label: None,
        pinned: None,
        parent: parent.map(|s| s.to_string()),
        tag_name: None,
        message: message.map(|s| s.to_string()),
    };
    append_event(log_path, &event)?;
    Ok(Revision {
        id: id.to_string(),
        ts,
        author: user.to_string(),
        root: root_hash.to_string(),
        source_hashes: source_hashes.clone(),
        label: None,
        message: message.map(|s| s.to_string()),
        pinned: false,
        parents: parent.into_iter().map(|s| s.to_string()).collect(),
        tags: Vec::new(),
        hidden: false,
        git_hash: git.hash.clone(),
        git_branch: git.branch.clone(),
        git_dirty: Some(git.dirty),
    })
}

// ------------------------------------------------------------ snapshot / restore

/// Snapshot the design's raw source directly as a synced revision. `source_hashes` is
/// the relevant file map already computed by the caller, so the stored root and the
/// cache key see the identical file set. `parent` is the active revision at crunch time.
/// Idempotent: identical content returns the existing revision without a duplicate create.
///
/// Retained as the tested synced-store create primitive; the app itself routes
/// auto-crunches through `checkpoints::snapshot_local` and promotes via `publish`
/// (which uses `snapshot_from_blobs`), so this has no production caller today —
/// hence `#[cfg(test)]` (compiled only for tests) rather than `#[allow(dead_code)]`.
#[cfg(test)]
pub fn snapshot(
    project_dir: &Path,
    source_dir: &Path,
    source_hashes: &BTreeMap<String, String>,
    user: &str,
    git: &GitInfo,
    parent: Option<&str>,
) -> Result<Revision, String> {
    let raw = raw_dir(project_dir);
    let (id, root_hash) = build_root(&raw, source_dir, source_hashes)?;
    if let Some(existing) = find_revision(project_dir, &id) {
        return Ok(existing); // same content → same revision, no duplicate create
    }
    let lamport = next_lamport(&raw, REVISIONS_PREFIX);
    append_create_event(
        &revisions_log(&raw, user),
        lamport,
        &id,
        &root_hash,
        source_hashes,
        user,
        git,
        parent,
        None,
    )
}

/// Append a synced `create` for content whose blobs already live in `raw/objects`
/// (used by `checkpoints::publish`, which copies the blobs across first). Idempotent.
pub fn snapshot_from_blobs(
    project_dir: &Path,
    id: &str,
    root_hash: &str,
    source_hashes: &BTreeMap<String, String>,
    user: &str,
    git: &GitInfo,
    parent: Option<&str>,
    message: Option<&str>,
) -> Result<Revision, String> {
    if let Some(existing) = find_revision(project_dir, id) {
        return Ok(existing);
    }
    let raw = raw_dir(project_dir);
    let lamport = next_lamport(&raw, REVISIONS_PREFIX);
    append_create_event(
        &revisions_log(&raw, user),
        lamport,
        id,
        root_hash,
        source_hashes,
        user,
        git,
        parent,
        message,
    )
}

/// Restore a revision's raw source tree into `dest` (for read-only re-extraction).
pub fn materialize(project_dir: &Path, revision: &Revision, dest: &Path) -> Result<(), String> {
    materialize_from(&raw_dir(project_dir), &revision.root, dest)
}

/// Restore the tree rooted at `root_hash` from the object store at `store_root` into
/// `dest`. Root-parameterized so checkpoints materialize from the local store.
pub(crate) fn materialize_from(store_root: &Path, root_hash: &str, dest: &Path) -> Result<(), String> {
    let root_bytes = read_object(store_root, root_hash)?;
    let entries: Vec<RootEntry> = serde_json::from_slice(&root_bytes).map_err(|e| e.to_string())?;
    for e in entries {
        // Validate the relative path BEFORE reading the blob: reject any `..`/absolute/
        // drive-rooted entry that would escape `dest` (path traversal from synced data),
        // and use `if let` on the parent so a pathological entry can't panic (never-crash).
        let path = safe_join(dest, &e.path)?;
        let bytes = read_object(store_root, &e.hash)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ------------------------------------------------------------ mutations (LWW events)

#[allow(clippy::too_many_arguments)]
fn mutate_event(
    project_dir: &Path,
    user: &str,
    id: &str,
    action: &str,
    label: Option<String>,
    pinned: Option<bool>,
    tag_name: Option<String>,
    message: Option<String>,
) -> Result<(), String> {
    let raw = raw_dir(project_dir);
    let log = revisions_log(&raw, user);
    mutate_event_in(&raw, REVISIONS_PREFIX, &log, user, id, action, label, pinned, tag_name, message)
}

/// Append a mutate event into an arbitrary store (dir + log prefix + target log) —
/// shared by the synced revisions log and the machine-local checkpoint log, so
/// label/pin/tag mutations fold identically in both stores.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mutate_event_in(
    store_dir: &Path,
    prefix: &str,
    log: &Path,
    user: &str,
    id: &str,
    action: &str,
    label: Option<String>,
    pinned: Option<bool>,
    tag_name: Option<String>,
    message: Option<String>,
) -> Result<(), String> {
    let lamport = next_lamport(store_dir, prefix);
    let ts = now_rfc3339()?;
    let event = RevEvent {
        event_id: make_id("e", &[user, &lamport.to_string(), &ts, id, action]),
        lamport,
        ts,
        user: user.to_string(),
        action: action.into(),
        revision_id: id.to_string(),
        root: None,
        source_hashes: BTreeMap::new(),
        git_hash: None,
        git_branch: None,
        git_dirty: None,
        label,
        pinned,
        parent: None,
        tag_name,
        message,
    };
    append_event(log, &event)
}

pub fn set_label(project_dir: &Path, user: &str, id: &str, label: Option<String>) -> Result<(), String> {
    mutate_event(project_dir, user, id, "label", label, None, None, None)
}

/// Point/move a named tag at a revision (git-tag semantics; LWW by name on the fold).
pub fn set_tag(
    project_dir: &Path,
    user: &str,
    revision_id: &str,
    tag_name: &str,
    message: Option<String>,
) -> Result<(), String> {
    mutate_event(project_dir, user, revision_id, "tag", None, None, Some(tag_name.to_string()), message)
}

/// Remove a named tag (the revision id is irrelevant for the fold — keyed by name).
pub fn remove_tag(project_dir: &Path, user: &str, tag_name: &str) -> Result<(), String> {
    mutate_event(project_dir, user, "", "untag", None, None, Some(tag_name.to_string()), None)
}

/// Hide (retract) or unhide a revision — a reversible tombstone, never a log rewrite.
pub fn set_hidden(
    project_dir: &Path,
    user: &str,
    id: &str,
    hidden: bool,
    reason: Option<String>,
) -> Result<(), String> {
    let action = if hidden { "hide" } else { "unhide" };
    mutate_event(project_dir, user, id, action, None, None, None, reason)
}

// ------------------------------------------------------------ diff

/// The source-file delta between two revisions — a cheap, extraction-free diff.
#[derive(Clone, Serialize, Default)]
pub struct RevisionDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

/// Compare two revisions' source-hash maps: in `to` not `from` = added; differing hash
/// = changed; in `from` not `to` = removed. Pure on the maps so either side can be a
/// synced revision or a local checkpoint (the command resolves each id first).
pub fn diff_source_hashes(
    from: &BTreeMap<String, String>,
    to: &BTreeMap<String, String>,
) -> RevisionDiff {
    let mut d = RevisionDiff::default();
    for (path, hash) in to {
        match from.get(path) {
            None => d.added.push(path.clone()),
            Some(fh) if fh != hash => d.changed.push(path.clone()),
            _ => {}
        }
    }
    for path in from.keys() {
        if !to.contains_key(path) {
            d.removed.push(path.clone());
        }
    }
    d.added.sort();
    d.removed.sort();
    d.changed.sort();
    d
}

// ------------------------------------------------------------ prune / purge

/// Every object hash referenced by the given revisions (root + its files), reading
/// manifests from `store_root` — the keep-set for GC. Root-parameterized so the
/// machine-local checkpoint store reuses it.
pub(crate) fn referenced_objects_in(store_root: &Path, revisions: &[Revision]) -> HashSet<String> {
    let mut keep = HashSet::new();
    for r in revisions {
        keep.insert(r.root.clone());
        if let Ok(root_bytes) = read_object(store_root, &r.root) {
            if let Ok(entries) = serde_json::from_slice::<Vec<RootEntry>>(&root_bytes) {
                keep.extend(entries.into_iter().map(|e| e.hash));
            }
        }
    }
    keep
}

fn referenced_objects(project_dir: &Path, revisions: &[Revision]) -> HashSet<String> {
    referenced_objects_in(&raw_dir(project_dir), revisions)
}

/// Delete every blob under `store_root/objects` whose hash isn't in `keep`.
pub(crate) fn prune_orphan_objects(store_root: &Path, keep: &HashSet<String>) {
    for entry in WalkDir::new(objects_dir(store_root)).into_iter().flatten() {
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !keep.contains(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Rewrite `log_path` keeping only events whose `revision_id` is NOT in `drop_ids`.
/// Valid ONLY for a log the caller owns exclusively (the machine-local checkpoint
/// log) — NEVER call this on a synced revision log: rewriting breaks the
/// conflict-free merge (a peer with the old line resurrects it on next sync).
pub(crate) fn rewrite_log_dropping(log_path: &Path, drop_ids: &HashSet<String>) -> Result<(), String> {
    let Ok(text) = fs::read_to_string(log_path) else {
        return Ok(());
    };
    let mut out = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<RevEvent>(line) {
            Ok(ev) if drop_ids.contains(&ev.revision_id) => continue,
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    let tmp = log_path.with_extension("jsonl.tmp");
    fs::write(&tmp, out.as_bytes()).map_err(|e| e.to_string())?;
    fs::rename(&tmp, log_path).map_err(|e| e.to_string())
}

/// Reclaim disk by deleting blobs no surviving revision needs. A revision survives if
/// it is pinned, **tagged**, the latest, or referenced by a review (`keep_ids`, from a
/// comment's `base_revision`); a **hidden** revision loses the retention grace (but a
/// hidden-yet-pinned/tagged/latest one is still kept). The append-only logs are NEVER
/// rewritten (that would break the conflict-free merge) — only orphaned objects are
/// removed; a revision whose blobs are gone simply can't materialize until it is
/// re-synced from a peer or re-extracted.
pub fn prune(
    project_dir: &Path,
    retention_days: i64,
    keep_ids: &HashSet<String>,
) -> Result<(), String> {
    let revisions = list_revisions(project_dir);
    if revisions.is_empty() {
        return Ok(());
    }
    let latest_id = revisions.first().map(|r| r.id.clone()); // list is newest-first
    let now = OffsetDateTime::now_utc();
    let kept: Vec<Revision> = revisions
        .into_iter()
        .filter(|r| {
            // Hard keeps first, so a hidden-but-tagged/pinned/latest revision survives.
            if r.pinned
                || !r.tags.is_empty()
                || Some(&r.id) == latest_id.as_ref()
                || keep_ids.contains(&r.id)
            {
                return true;
            }
            if r.hidden {
                return false; // retracted ⇒ drops the retention grace
            }
            match OffsetDateTime::parse(&r.ts, &Rfc3339) {
                Ok(ts) => (now - ts).whole_days() < retention_days,
                Err(_) => true, // unparseable ts → keep, never silently destroy
            }
        })
        .collect();

    let referenced = referenced_objects(project_dir, &kept);
    prune_orphan_objects(&raw_dir(project_dir), &referenced);
    Ok(())
}

/// Immediately delete the objects referenced ONLY by `id` (a leaked-secret purge on
/// this machine). Honest serverless caveat: peers that already synced keep their bytes
/// until their own GC — we cannot reach them. The log entry stays (never rewritten);
/// callers pair this with a `hide` tombstone so the revision leaves every UI.
pub fn purge_objects(project_dir: &Path, id: &str) -> Result<(), String> {
    let Some(target) = find_revision(project_dir, id) else {
        return Ok(());
    };
    let others: Vec<Revision> =
        list_revisions(project_dir).into_iter().filter(|r| r.id != id).collect();
    let keep = referenced_objects(project_dir, &others);
    let raw = raw_dir(project_dir);
    for hash in referenced_objects(project_dir, std::slice::from_ref(&target)) {
        if !keep.contains(&hash) {
            if let Ok(p) = object_path(&raw, &hash) {
                let _ = fs::remove_file(p);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rawstore_test_{}_{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hashes(src: &Path) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for entry in fs::read_dir(src).unwrap().flatten() {
            let bytes = fs::read(entry.path()).unwrap();
            let rel = entry.file_name().to_string_lossy().into_owned();
            m.insert(rel, blake3::hash(&bytes).to_hex().to_string());
        }
        m
    }

    #[test]
    fn snapshot_roundtrip_and_content_id() {
        let proj = temp_project("roundtrip");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.kicad_sch"), b"sheet A").unwrap();
        fs::write(src.join("b.kicad_sch"), vec![b'x'; 10_000]).unwrap();
        let git = GitInfo::default();

        let h = hashes(&src);
        let rev = snapshot(&proj, &src, &h, "alice", &git, None).unwrap();
        assert!(rev.id.starts_with("r_"));
        assert!(rev.parents.is_empty(), "root revision has no parent");

        // Same content → same id, no duplicate revision (idempotent / auto-merge).
        let rev2 = snapshot(&proj, &src, &h, "bob", &git, None).unwrap();
        assert_eq!(rev.id, rev2.id);
        assert_eq!(list_revisions(&proj).len(), 1);

        // Roundtrip materialize is byte-exact.
        let out = proj.join("out");
        materialize(&proj, &rev, &out).unwrap();
        assert_eq!(fs::read(out.join("a.kicad_sch")).unwrap(), b"sheet A");
        assert_eq!(fs::read(out.join("b.kicad_sch")).unwrap(), vec![b'x'; 10_000]);
        let _ = fs::remove_dir_all(&proj);
    }

    #[test]
    fn parent_links_child_to_its_base() {
        let proj = temp_project("parent");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v1").unwrap();
        let r1 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), None).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v2").unwrap();
        let r2 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), Some(&r1.id)).unwrap();
        let folded = find_revision(&proj, &r2.id).unwrap();
        assert!(folded.parents.contains(&r1.id), "child records its parent");
        assert_eq!(folded.parents.len(), 1);
        let _ = fs::remove_dir_all(&proj);
    }

    #[test]
    fn distinct_content_makes_distinct_revisions_and_label_is_lww() {
        let proj = temp_project("distinct");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v1").unwrap();
        let r1 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), None).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v2").unwrap();
        let r2 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), Some(&r1.id)).unwrap();
        assert_ne!(r1.id, r2.id);
        assert_eq!(list_revisions(&proj).len(), 2);

        // Label is last-writer-wins across users in total (lamport) order.
        set_label(&proj, "alice", &r1.id, Some("first".into())).unwrap();
        set_label(&proj, "bob", &r1.id, Some("renamed".into())).unwrap();
        let folded = find_revision(&proj, &r1.id).unwrap();
        assert_eq!(folded.label.as_deref(), Some("renamed"), "last label wins");
        let _ = fs::remove_dir_all(&proj);
    }

    #[test]
    fn tags_are_lww_by_name_and_keep_from_gc() {
        let proj = temp_project("tags");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v1").unwrap();
        let r1 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), None).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v2").unwrap();
        let r2 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), Some(&r1.id)).unwrap();

        set_tag(&proj, "alice", &r1.id, "fab", None).unwrap();
        assert_eq!(find_revision(&proj, &r1.id).unwrap().tags, vec!["fab".to_string()]);

        // Moving the tag to r2 re-points it (LWW by name); r1 loses it.
        set_tag(&proj, "bob", &r2.id, "fab", None).unwrap();
        assert!(find_revision(&proj, &r1.id).unwrap().tags.is_empty());
        assert_eq!(find_revision(&proj, &r2.id).unwrap().tags, vec!["fab".to_string()]);

        // Tag r1 again, then prune with zero retention: tagged r1 survives, untagged
        // non-latest revisions would not (r2 is latest so also survives).
        set_tag(&proj, "alice", &r1.id, "keepme", None).unwrap();
        prune(&proj, 0, &HashSet::new()).unwrap();
        let out = proj.join("out_r1");
        materialize(&proj, &find_revision(&proj, &r1.id).unwrap(), &out).expect("tagged blobs survive");
        let _ = fs::remove_dir_all(&proj);
    }

    #[test]
    fn hide_drops_retention_but_pin_tag_latest_win() {
        let proj = temp_project("hide");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v1").unwrap();
        let r1 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), None).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v2").unwrap();
        let _r2 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), Some(&r1.id)).unwrap();

        set_hidden(&proj, "alice", &r1.id, true, Some("retracted".into())).unwrap();
        assert!(find_revision(&proj, &r1.id).unwrap().hidden);
        // Hidden + not latest + 0 retention ⇒ its unique blob is purged by prune.
        prune(&proj, 0, &HashSet::new()).unwrap();
        assert!(materialize(&proj, &find_revision(&proj, &r1.id).unwrap(), &proj.join("gone")).is_err());
        let _ = fs::remove_dir_all(&proj);
    }

    #[test]
    fn diff_reports_added_removed_changed() {
        let mut from = BTreeMap::new();
        from.insert("a".to_string(), "h1".to_string());
        from.insert("b".to_string(), "h2".to_string());
        let mut to = BTreeMap::new();
        to.insert("a".to_string(), "h1".to_string()); // unchanged
        to.insert("b".to_string(), "h2x".to_string()); // changed
        to.insert("c".to_string(), "h3".to_string()); // added
        let d = diff_source_hashes(&from, &to);
        assert_eq!(d.added, vec!["c".to_string()]);
        assert_eq!(d.changed, vec!["b".to_string()]);
        assert!(d.removed.is_empty());
    }

    #[test]
    fn device_scoped_logs_fold_together() {
        // Two logs that differ only by device segment must fold into one history with
        // monotonic lamports (the multi-machine must-fix).
        let proj = temp_project("device");
        let raw = raw_dir(&proj);
        fs::create_dir_all(&raw).unwrap();
        let ev = |lamport: u64, id: &str| {
            format!(
                r#"{{"event_id":"e_{lamport}","lamport":{lamport},"ts":"2026-06-22T00:00:0{lamport}Z","user":"alice","action":"create","revision_id":"{id}","root":"{id}root"}}"#
            )
        };
        fs::write(raw.join("revisions.alice.devA.jsonl"), format!("{}\n", ev(0, "r_aaaaaaaaaaaa"))).unwrap();
        fs::write(raw.join("revisions.alice.devB.jsonl"), format!("{}\n", ev(1, "r_bbbbbbbbbbbb"))).unwrap();
        assert_eq!(list_revisions(&proj).len(), 2, "both device logs fold in");
        assert_eq!(next_lamport(&raw, REVISIONS_PREFIX), 2, "lamport is max+1 across devices");
        let _ = fs::remove_dir_all(&proj);
    }

    fn object_count(proj: &Path) -> usize {
        WalkDir::new(objects_dir(&raw_dir(proj)))
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .count()
    }

    #[test]
    fn editing_one_file_only_adds_its_blob() {
        // The headline storage win: a design change re-stores only what changed.
        let proj = temp_project("dedup");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.kicad_sch"), b"sheet A, untouched").unwrap();
        fs::write(src.join("b.kicad_sch"), b"sheet B, v1").unwrap();
        let r1 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), None).unwrap();
        let n1 = object_count(&proj);

        // Edit only b; a is byte-identical.
        fs::write(src.join("b.kicad_sch"), b"sheet B, v2 (changed)").unwrap();
        let r2 = snapshot(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), Some(&r1.id)).unwrap();
        let n2 = object_count(&proj);

        assert_ne!(r1.id, r2.id, "changed source → new revision");
        // a's blob is shared; only b's new blob + the new root manifest were written.
        assert_eq!(n2 - n1, 2, "an edit adds exactly the changed file's blob + new root");
        let _ = fs::remove_dir_all(&proj);
    }

    #[test]
    fn safe_join_rejects_traversal_and_absolute_paths() {
        let base = Path::new("/tmp/dest");
        // Normal relative paths (incl. subdirs and `.`) are accepted and stay under base.
        assert!(safe_join(base, "a.kicad_sch").is_ok());
        assert!(safe_join(base, "sub/b.kicad_sch").is_ok());
        // Escapes are rejected: parent traversal, absolute, drive-rooted, empty.
        assert!(safe_join(base, "../evil").is_err());
        assert!(safe_join(base, "..\\evil").is_err());
        assert!(safe_join(base, "a/../../evil").is_err());
        assert!(safe_join(base, "/etc/passwd").is_err());
        assert!(safe_join(base, "\\\\server\\share").is_err());
        assert!(safe_join(base, "C:\\Windows\\system32").is_err());
        assert!(safe_join(base, "").is_err());
    }

    #[test]
    fn materialize_rejects_a_traversing_manifest_entry() {
        // A hostile/corrupt root manifest (as could sync in from a peer) with a `..` path
        // must fail the materialize instead of writing outside `dest`.
        let proj = temp_project("traversal");
        let raw = raw_dir(&proj);
        let evil = vec![RootEntry { path: "../escaped.txt".into(), hash: "deadbeef".into(), size: 0 }];
        let root_bytes = serde_json::to_vec(&evil).unwrap();
        let root_hash = blake3::hash(&root_bytes).to_hex().to_string();
        write_object(&raw, &root_hash, &root_bytes).unwrap();
        let dest = proj.join("dest");
        let err = materialize_from(&raw, &root_hash, &dest).unwrap_err();
        assert!(err.contains("unsafe"), "traversing entry is rejected: {err}");
        assert!(!proj.join("escaped.txt").exists(), "nothing was written outside dest");
        let _ = fs::remove_dir_all(&proj);
    }

    #[test]
    fn purge_objects_removes_only_this_revisions_unique_blobs() {
        let proj = temp_project("purge");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("shared.kicad_sch"), b"shared, untouched").unwrap();
        fs::write(src.join("a.kicad_sch"), b"only in r1").unwrap();
        let r1 = snapshot(&proj, &src, &hashes(&src), "a", &GitInfo::default(), None).unwrap();
        fs::write(src.join("a.kicad_sch"), b"changed in r2").unwrap();
        let r2 = snapshot(&proj, &src, &hashes(&src), "a", &GitInfo::default(), Some(&r1.id)).unwrap();

        // Purge r1: its unique blobs (old a + r1's root) go; the shared blob (still
        // referenced by r2) is kept, so r2 still materializes.
        purge_objects(&proj, &r1.id).unwrap();
        assert!(
            materialize(&proj, &find_revision(&proj, &r1.id).unwrap(), &proj.join("g1")).is_err(),
            "purged revision no longer materializes"
        );
        materialize(&proj, &find_revision(&proj, &r2.id).unwrap(), &proj.join("g2"))
            .expect("a revision sharing blobs still materializes after a sibling purge");
        let _ = fs::remove_dir_all(&proj);
    }
}
