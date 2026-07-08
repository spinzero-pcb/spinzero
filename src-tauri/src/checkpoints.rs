//! Machine-local checkpoint store — the private "autosave timeline / reflog".
//!
//! Every automatic crunch (watcher / open / manual "Extract now") appends a
//! **local checkpoint** here instead of a synced revision, so 40 WIP saves an hour
//! never leak to teammates and never churn the synced folder. An explicit **Publish**
//! promotes a chosen checkpoint into the shared history (`rawstore`).
//!
//! Layout under `project::local_data_root(project_dir)/checkpoints/` (machine-local,
//! NEVER synced — sits next to `cache/`):
//! - `objects/<aa>/<blake3>` — the same zstd object layer as the raw store.
//! - `checkpoints.<device>.jsonl` — this machine's append-only checkpoint log.
//!
//! Checkpoints reuse `rawstore`'s object + event + fold machinery and the **same**
//! content-hash id space, so a checkpoint that is published keeps its id and dedupes
//! against an identical synced revision. Unlike the synced logs, the local log MAY be
//! rewritten (hard delete / GC) because this machine is its only writer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::project::{self, GitInfo};
use crate::rawstore::{self, Revision};
use crate::util::LockExt;

/// Log-filename prefix for the local checkpoint logs (fold globs `checkpoints.*`).
const CHECKPOINTS_PREFIX: &str = "checkpoints.";

/// Serializes mutations of the local checkpoint store (snapshot vs delete/GC). A GC
/// prunes objects "no surviving checkpoint references", reading survivors first; without
/// this lock a snapshot writing its blobs between GC's survivor read and its prune could
/// have those just-written blobs deleted before its create-event lands. Same-machine
/// only, so one in-process lock suffices; it wraps pure file work and can't deadlock.
fn store_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// Root of the machine-local checkpoint store (object layer + log).
pub fn checkpoints_root(project_dir: &Path) -> PathBuf {
    project::local_data_root(project_dir).join("checkpoints")
}

/// This machine's checkpoint log.
fn checkpoints_log(root: &Path) -> PathBuf {
    root.join(format!("{CHECKPOINTS_PREFIX}{}.jsonl", crate::device::device_id()))
}

/// Fold this machine's checkpoint log into a revision list, newest-first. (Reuses the
/// raw-store fold — checkpoints share the `Revision` shape; tags/hidden stay empty.)
pub fn list_checkpoints(project_dir: &Path) -> Vec<Revision> {
    rawstore::fold(rawstore::read_all_events(&checkpoints_root(project_dir), CHECKPOINTS_PREFIX))
}

pub fn find_checkpoint(project_dir: &Path, id: &str) -> Option<Revision> {
    list_checkpoints(project_dir).into_iter().find(|r| r.id == id)
}

pub fn latest_checkpoint(project_dir: &Path) -> Option<Revision> {
    list_checkpoints(project_dir).into_iter().next()
}

/// Snapshot the live design as a machine-local checkpoint. Idempotent by content id
/// (same source ⇒ same checkpoint, no duplicate). `parent` is the effective revision
/// at crunch time (a prior checkpoint or the last published revision).
pub fn snapshot_local(
    project_dir: &Path,
    source_dir: &Path,
    source_hashes: &std::collections::BTreeMap<String, String>,
    user: &str,
    git: &GitInfo,
    parent: Option<&str>,
) -> Result<Revision, String> {
    let _guard = store_lock().lock_safe();
    let root_dir = checkpoints_root(project_dir);
    let (id, root_hash) = rawstore::build_root(&root_dir, source_dir, source_hashes)?;
    if let Some(existing) = find_checkpoint(project_dir, &id) {
        return Ok(existing);
    }
    let lamport = rawstore::next_lamport(&root_dir, CHECKPOINTS_PREFIX);
    rawstore::append_create_event(
        &checkpoints_log(&root_dir),
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

/// Restore a checkpoint's raw source tree into `dest` (from the local object store).
pub fn materialize_local(project_dir: &Path, checkpoint: &Revision, dest: &Path) -> Result<(), String> {
    rawstore::materialize_from(&checkpoints_root(project_dir), &checkpoint.root, dest)
}

/// Promote a checkpoint into the synced history: copy its blobs local→synced
/// `raw/objects/` (dedupe — a no-op for any blob already there), then append one synced
/// `create` event whose parent is the nearest **already-published** ancestor. Idempotent
/// (content-id dedupe): publishing twice returns the same revision with no duplicate.
pub fn publish(
    project_dir: &Path,
    checkpoint_id: &str,
    user: &str,
    git: &GitInfo,
    message: Option<&str>,
) -> Result<Revision, String> {
    let cp = find_checkpoint(project_dir, checkpoint_id)
        .ok_or_else(|| format!("unknown checkpoint {checkpoint_id}"))?;
    let local_root = checkpoints_root(project_dir);
    let raw = rawstore::raw_dir(project_dir);

    // Copy the root manifest + every referenced blob into the synced object store.
    let keep = rawstore::referenced_objects_in(&local_root, std::slice::from_ref(&cp));
    for hash in &keep {
        let bytes = rawstore::read_object(&local_root, hash)?;
        rawstore::write_object(&raw, hash, &bytes)?;
    }

    let parent = nearest_published_ancestor(project_dir, &cp);
    rawstore::snapshot_from_blobs(
        project_dir,
        &cp.id,
        &cp.root,
        &cp.source_hashes,
        user,
        git,
        parent.as_deref(),
        message,
    )
}

/// Walk the checkpoint's local-parent chain until we hit an id that already exists in
/// the synced store — the previously-published ancestor a publish should point at.
/// None ⇒ this is the first published revision (a root).
fn nearest_published_ancestor(project_dir: &Path, cp: &Revision) -> Option<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut frontier: Vec<String> = cp.parents.iter().cloned().collect();
    while let Some(id) = frontier.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if rawstore::find_revision(project_dir, &id).is_some() {
            return Some(id); // already published
        }
        if let Some(parent_cp) = find_checkpoint(project_dir, &id) {
            frontier.extend(parent_cp.parents.iter().cloned());
        }
    }
    None
}

/// Hard-delete one checkpoint: drop its create line from the local log, then prune any
/// local object no surviving checkpoint references. Allowed because the local log is
/// unsynced and this machine is its only writer. A published checkpoint's bytes survive
/// in the synced store regardless.
pub fn delete_local(project_dir: &Path, id: &str) -> Result<(), String> {
    let _guard = store_lock().lock_safe();
    let root = checkpoints_root(project_dir);
    let mut drop = HashSet::new();
    drop.insert(id.to_string());
    rawstore::rewrite_log_dropping(&checkpoints_log(&root), &drop)?;
    let survivors = list_checkpoints(project_dir);
    let keep = rawstore::referenced_objects_in(&root, &survivors);
    rawstore::prune_orphan_objects(&root, &keep);
    Ok(())
}

/// Bound the local checkpoint timeline: keep the newest `keep_n`, anything within
/// `keep_days`, and anything in `protect` (the active/effective revision). Drop the
/// rest (hard delete — rewrite the log + prune orphan objects). Best-effort.
pub fn gc_local(
    project_dir: &Path,
    keep_n: usize,
    keep_days: i64,
    protect: &HashSet<String>,
) -> Result<(), String> {
    let _guard = store_lock().lock_safe();
    let checkpoints = list_checkpoints(project_dir); // newest-first
    if checkpoints.len() <= keep_n {
        return Ok(());
    }
    let now = OffsetDateTime::now_utc();
    let mut drop: HashSet<String> = HashSet::new();
    for (i, cp) in checkpoints.iter().enumerate() {
        if i < keep_n || protect.contains(&cp.id) {
            continue;
        }
        let recent = OffsetDateTime::parse(&cp.ts, &Rfc3339)
            .map(|ts| (now - ts).whole_days() < keep_days)
            .unwrap_or(true); // unparseable ts → keep, never silently destroy
        if !recent {
            drop.insert(cp.id.clone());
        }
    }
    if drop.is_empty() {
        return Ok(());
    }
    let root = checkpoints_root(project_dir);
    rawstore::rewrite_log_dropping(&checkpoints_log(&root), &drop)?;
    let survivors: Vec<Revision> =
        list_checkpoints(project_dir).into_iter().filter(|r| !drop.contains(&r.id)).collect();
    let keep = rawstore::referenced_objects_in(&root, &survivors);
    rawstore::prune_orphan_objects(&root, &keep);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    fn hashes(src: &Path) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for entry in fs::read_dir(src).unwrap().flatten() {
            let bytes = fs::read(entry.path()).unwrap();
            let rel = entry.file_name().to_string_lossy().into_owned();
            m.insert(rel, blake3::hash(&bytes).to_hex().to_string());
        }
        m
    }

    // Checkpoints live under local_data_root, which is keyed by the project dir's
    // absolute path — so a temp project dir gives each test its own isolated store.
    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ckpt_test_{}_{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn local_checkpoint_does_not_touch_the_synced_store() {
        let proj = temp_project("isolation");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v1").unwrap();
        let cp = snapshot_local(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), None).unwrap();

        // The synced raw store has NO revisions and NO objects yet (no byte leak).
        assert!(rawstore::list_revisions(&proj).is_empty(), "checkpoint is not synced");
        assert!(!rawstore::raw_dir(&proj).join("objects").exists(), "no synced blobs");
        // But it is materializable from the local store.
        let out = proj.join("out");
        materialize_local(&proj, &cp, &out).unwrap();
        assert_eq!(fs::read(out.join("a.kicad_sch")).unwrap(), b"v1");
        let _ = fs::remove_dir_all(&proj);
        let _ = fs::remove_dir_all(checkpoints_root(&proj));
    }

    #[test]
    fn publish_promotes_with_same_id_and_is_idempotent() {
        let proj = temp_project("publish");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v1").unwrap();
        let cp = snapshot_local(&proj, &src, &hashes(&src), "alice", &GitInfo::default(), None).unwrap();

        let rev = publish(&proj, &cp.id, "alice", &GitInfo::default(), None).unwrap();
        assert_eq!(rev.id, cp.id, "publish keeps the content id");
        assert!(rawstore::find_revision(&proj, &cp.id).is_some(), "now in synced history");
        // Idempotent: publishing again is a no-op (no duplicate synced revision).
        publish(&proj, &cp.id, "bob", &GitInfo::default(), None).unwrap();
        assert_eq!(rawstore::list_revisions(&proj).len(), 1);
        let _ = fs::remove_dir_all(&proj);
        let _ = fs::remove_dir_all(checkpoints_root(&proj));
    }

    #[test]
    fn publish_parent_is_nearest_published_ancestor() {
        let proj = temp_project("pubparent");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        // cp1 -> cp2 -> cp3 (a local chain).
        fs::write(src.join("a.kicad_sch"), b"v1").unwrap();
        let cp1 = snapshot_local(&proj, &src, &hashes(&src), "a", &GitInfo::default(), None).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v2").unwrap();
        let cp2 = snapshot_local(&proj, &src, &hashes(&src), "a", &GitInfo::default(), Some(&cp1.id)).unwrap();
        fs::write(src.join("a.kicad_sch"), b"v3").unwrap();
        let cp3 = snapshot_local(&proj, &src, &hashes(&src), "a", &GitInfo::default(), Some(&cp2.id)).unwrap();

        // Publish cp1, then cp3 — cp3's synced parent must skip the unpublished cp2.
        publish(&proj, &cp1.id, "a", &GitInfo::default(), None).unwrap();
        let r3 = publish(&proj, &cp3.id, "a", &GitInfo::default(), None).unwrap();
        assert!(r3.parents.contains(&cp1.id), "publish parent skips unpublished cp2 to cp1");
        let _ = fs::remove_dir_all(&proj);
        let _ = fs::remove_dir_all(checkpoints_root(&proj));
    }

    #[test]
    fn gc_local_keeps_recent_and_protected() {
        let proj = temp_project("gc");
        let src = proj.join("src");
        fs::create_dir_all(&src).unwrap();
        let mut ids = Vec::new();
        for i in 0..5 {
            fs::write(src.join("a.kicad_sch"), format!("v{i}")).unwrap();
            let cp = snapshot_local(&proj, &src, &hashes(&src), "a", &GitInfo::default(), None).unwrap();
            ids.push(cp.id);
        }
        // Keep newest 2; keep_days 0 so older non-protected ones drop — but protect ids[0].
        let mut protect = HashSet::new();
        protect.insert(ids[0].clone());
        gc_local(&proj, 2, 0, &protect).unwrap();
        let surviving: HashSet<String> = list_checkpoints(&proj).into_iter().map(|r| r.id).collect();
        assert!(surviving.contains(&ids[4]) && surviving.contains(&ids[3]), "newest 2 kept");
        assert!(surviving.contains(&ids[0]), "protected id kept");
        let _ = fs::remove_dir_all(&proj);
        let _ = fs::remove_dir_all(checkpoints_root(&proj));
    }
}
