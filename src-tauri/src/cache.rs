//! Runtime extraction cache (KiCad).
//!
//! KiCad extraction is in-process and fast (<1s), so we do not persist a bundle
//! per crunch. Instead we extract into `<local_data_root>/cache/<key>/` — a
//! machine-local dir under the OS local-data dir, NOT inside the synced project
//! folder (`project::local_data_root`) — keyed by the extractor's output-contract
//! version + every source file's hash. The cache is:
//!
//! - **regenerable** — gitignored, safe to delete; rebuilt on next open;
//! - **self-invalidating** — a new app release (`CARGO_PKG_VERSION`), a bump of
//!   `extract::EXTRACTOR_CACHE_EPOCH`, or an edit to any source file changes the key,
//!   so a stale entry is never served. Folding in the app version means every update
//!   re-extracts on next open even when the extractor logic looks unchanged, since the
//!   new build may extract differently;
//! - **content-addressed** — the same source + extractor reuses one dir, so storage
//!   stops growing with the number of crunches.
//!
//! Publish is create-only and atomic (`<key>.tmp` → `<key>`): a new key is always a
//! fresh dir, so we never delete-then-rename over a directory the viewer is reading.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use extract::pipeline::{run_bom, run_design, Msg};

use crate::index_db::check_manifest_schema;
use crate::sidecar::rename_retry;

/// Root of the runtime extraction cache. Machine-local and regenerable, so it
/// lives under the OS local-data dir (NOT inside the synced project folder) —
/// see `project::local_data_root`.
pub fn cache_root(project_dir: &Path) -> PathBuf {
    crate::project::local_data_root(project_dir).join("cache")
}

pub fn cache_dir(project_dir: &Path, key: &str) -> PathBuf {
    cache_root(project_dir).join(key)
}

/// Content+logic cache key: the app version and the extractor's output-contract
/// version, folded with the BLAKE3 of every relevant source file (the same map the
/// crunch hash gate builds). Deterministic — `source_hashes` is a `BTreeMap`, so
/// iteration order is stable.
///
/// The app's own `CARGO_PKG_VERSION` (bumped on every release, in `src-tauri/Cargo.toml`)
/// is part of the key, so **every app update re-extracts on the next open even if no
/// source file changed and nobody bumped `extract::EXTRACTOR_CACHE_EPOCH`**. This makes
/// the "extraction may have changed in the new version" guarantee automatic rather than
/// relying on a developer remembering the epoch. `extract::cache_version()` stays in the
/// key too, so a logic-only extractor change shipped without a version bump (dev builds)
/// still invalidates when the epoch moves.
pub fn cache_key(source_hashes: &BTreeMap<String, String>) -> String {
    let mut h = blake3::Hasher::new();
    h.update(env!("CARGO_PKG_VERSION").as_bytes());
    h.update(b"\0");
    h.update(extract::cache_version().as_bytes());
    for (path, hash) in source_hashes {
        h.update(b"\0");
        h.update(path.as_bytes());
        h.update(b"\0");
        h.update(hash.as_bytes());
    }
    h.finalize().to_hex().as_str()[..16].to_string()
}

/// True when `cache/<key>/` holds a fully-published, schema-valid bundle.
pub fn is_cached(project_dir: &Path, key: &str) -> bool {
    let dir = cache_dir(project_dir, key);
    dir.is_dir() && check_manifest_schema(&dir).is_ok()
}

/// Run the three KiCad extraction stages into `out` (design model + grouped BOM in
/// JSON and CSV). `emit` receives progress/artifact messages — the crunch maps them
/// to `CrunchEvent`s; lazy callers pass a no-op. Mirrors `sidecar::crunch_kicad` but
/// without the `AppHandle`/per-stage `CrunchError` wrapping, so it is callable from
/// command handlers (set-active, read-only resolve) that have no crunch context.
pub fn extract_into(design_file: &Path, out: &Path, emit: &mut dyn FnMut(Msg)) -> Result<(), String> {
    let design_dir = out.join("design");
    let bom_dir = out.join("bom");
    run_design(design_file, &design_dir, emit)?;
    run_bom(design_file, &bom_dir, "grouped-json", emit)?;
    run_bom(design_file, &bom_dir, "grouped-csv", emit)?;
    Ok(())
}

/// Atomically publish an already-extracted staging dir into `cache/<key>/`.
/// Create-only: if another writer (or an earlier crunch) already published this key,
/// keep it and discard `staged` — same key ⇒ byte-identical contents. Returns the
/// cache dir.
pub fn publish(project_dir: &Path, key: &str, staged: &Path) -> Result<PathBuf, String> {
    let dest = cache_dir(project_dir, key);
    if dest.is_dir() {
        let _ = fs::remove_dir_all(staged);
        return Ok(dest);
    }
    fs::create_dir_all(cache_root(project_dir)).map_err(|e| e.to_string())?;
    match rename_retry(staged, &dest) {
        Ok(()) => Ok(dest),
        // Lost a publish race (dest now exists) → fine; otherwise propagate.
        Err(e) => {
            if dest.is_dir() {
                let _ = fs::remove_dir_all(staged);
                Ok(dest)
            } else {
                Err(e)
            }
        }
    }
}

/// Ensure `cache/<key>/` exists for `design_file`, extracting on a miss with no
/// progress events. Used off the crunch path (open of a historical revision,
/// read-only re-extraction from materialized raw). Returns the cache dir.
pub fn ensure_lazy(project_dir: &Path, design_file: &Path, key: &str) -> Result<PathBuf, String> {
    if is_cached(project_dir, key) {
        return Ok(cache_dir(project_dir, key));
    }
    let staged = cache_root(project_dir).join(format!("{key}.tmp"));
    let _ = fs::remove_dir_all(&staged);
    fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
    let mut sink = |_: Msg| {};
    if let Err(e) = extract_into(design_file, &staged, &mut sink) {
        let _ = fs::remove_dir_all(&staged);
        return Err(e);
    }
    if let Err(e) = check_manifest_schema(&staged) {
        let _ = fs::remove_dir_all(&staged);
        return Err(e);
    }
    publish(project_dir, key, &staged)
}

/// Bound the runtime cache: keep every key in `keep` plus the most-recently-modified
/// entries up to `max_entries`, deleting the rest (and any stale `*.tmp` staging dirs).
/// `keep` protects the just-published key AND the actively-viewed revision's key, so
/// browsing history (>`max_entries` distinct revisions) can't evict the dir the viewer
/// is serving out from under a live SVG fetch. The cache is regenerable, so eviction is
/// otherwise safe — a dropped entry re-extracts on next need. Best-effort; never errors.
pub fn gc(project_dir: &Path, keep: &[String], max_entries: usize) {
    let root = cache_root(project_dir);
    let Ok(rd) = fs::read_dir(&root) else {
        return;
    };
    let mut dirs: Vec<(std::time::SystemTime, PathBuf, String)> = Vec::new();
    for e in rd.flatten() {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".tmp") {
            let _ = fs::remove_dir_all(e.path()); // abandoned staging dir
            continue;
        }
        let mtime = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        dirs.push((mtime, e.path(), name));
    }
    dirs.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    for (i, (_, path, name)) in dirs.iter().enumerate() {
        if keep.iter().any(|k| k == name) || i < max_entries {
            continue;
        }
        let _ = fs::remove_dir_all(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_version_sensitive() {
        let mut a = BTreeMap::new();
        a.insert("root.kicad_sch".to_string(), "hashA".to_string());
        a.insert("sub.kicad_sch".to_string(), "hashB".to_string());
        // Same content (regardless of insertion order) → same key.
        let mut b = BTreeMap::new();
        b.insert("sub.kicad_sch".to_string(), "hashB".to_string());
        b.insert("root.kicad_sch".to_string(), "hashA".to_string());
        assert_eq!(cache_key(&a), cache_key(&b));
        // A changed source hash → different key.
        let mut c = a.clone();
        c.insert("sub.kicad_sch".to_string(), "hashC".to_string());
        assert_ne!(cache_key(&a), cache_key(&c));
        // Keys are the 16-char prefix of a blake3 hex digest.
        assert_eq!(cache_key(&a).len(), 16);
    }

    // End-to-end on a real KiCad board (dev machine): extract into the cache via the
    // real pipeline, snapshot the raw source as a revision, and confirm the dedupe +
    // "no stored extraction bundle" invariants. Skips when the board is absent.
    #[test]
    fn kicad_runtime_cache_and_revision_end_to_end() {
        use crate::project::GitInfo;
        use crate::{rawstore, sidecar};

        // Set SPINZERO_TI_TUTORIAL to the (public) KiCad 9 TI-MSPM0 tutorial
        // board checkout to run this end-to-end test; it skips otherwise.
        let Some(board) = std::env::var("SPINZERO_TI_TUTORIAL").ok().map(PathBuf::from) else {
            return;
        };
        let pro_name = "TI-MSP-KICAD9-TUTORIAL.kicad_pro";
        if !board.join(pro_name).exists() {
            return;
        }
        let proj = std::env::temp_dir().join(format!("spinzero_e2e_{}", std::process::id()));
        let _ = fs::remove_dir_all(&proj);
        let design = proj.join("design_src");
        fs::create_dir_all(&design).unwrap();
        for entry in fs::read_dir(board).unwrap().flatten() {
            let p = entry.path();
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if p.is_file() && matches!(ext, "kicad_pro" | "kicad_sch" | "kicad_pcb" | "kicad_prl") {
                fs::copy(&p, design.join(p.file_name().unwrap())).unwrap();
            }
        }
        let project_dir = proj.join("project");
        fs::create_dir_all(&project_dir).unwrap();

        let hashes = sidecar::source_hashes(&design);
        assert!(!hashes.is_empty(), "relevant sources hashed");
        let key = cache_key(&hashes);

        // Extract into the cache through the real pipeline (no AppHandle).
        let design_file = design.join(pro_name);
        let dir = ensure_lazy(&project_dir, &design_file, &key).expect("ensure cache");
        assert!(dir.join("design").is_dir(), "cache bundle has design/");
        assert!(is_cached(&project_dir, &key), "resolves as a cache hit");

        // Snapshot the raw source; a re-snapshot of identical content dedupes to the
        // same revision id (content-addressed merge).
        let rev = rawstore::snapshot(&project_dir, &design, &hashes, "tester", &GitInfo::default(), None).unwrap();
        let rev2 = rawstore::snapshot(&project_dir, &design, &hashes, "tester", &GitInfo::default(), None).unwrap();
        assert_eq!(rev.id, rev2.id);
        assert_eq!(rawstore::list_revisions(&project_dir).len(), 1);

        // No stored extraction bundle is created for KiCad — the raw store + cache
        // are the only artifacts.
        assert!(
            !project_dir.join("extractions").exists(),
            "KiCad must not persist extractions/<id>/"
        );
        // The compressed raw store is much smaller than the extracted cache bundle.
        let raw_sz = dir_size(&rawstore::raw_dir(&project_dir));
        let cache_sz = dir_size(&dir);
        assert!(raw_sz < cache_sz, "raw store ({raw_sz}) < cache bundle ({cache_sz})");

        let _ = fs::remove_dir_all(&proj);
    }

    fn dir_size(p: &Path) -> u64 {
        walkdir::WalkDir::new(p)
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok().map(|m| m.len()))
            .sum()
    }
}
