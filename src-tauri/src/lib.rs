mod bomcheck;
mod cache;
mod checkpoints;
mod design;
mod device;
mod diff;
mod events;
mod index_db;
mod logging;
mod presence;
mod project;
mod rawstore;
mod reviewbundle;
mod reviews;
mod sidecar;
mod telemetry;
mod util;
mod watcher;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, State};

use project::{ProjectHandle, ProjectInfo};
use util::LockExt;

#[derive(Default)]
struct AppState {
    project: Mutex<Option<Arc<ProjectHandle>>>,
}

fn current_project(state: &State<AppState>) -> Result<Arc<ProjectHandle>, String> {
    state
        .project
        .lock_safe()
        .clone()
        .ok_or_else(|| "no project open".to_string())
}

// ------------------------------------------------------------ recents

fn recents_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("recent_projects.json"))
}

fn read_recents(app: &AppHandle) -> Vec<String> {
    recents_path(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn push_recent(app: &AppHandle, path: &str) {
    let mut recents = read_recents(app);
    recents.retain(|r| r != path);
    recents.insert(0, path.to_string());
    recents.truncate(8);
    if let Some(p) = recents_path(app) {
        let _ = fs::create_dir_all(p.parent().unwrap());
        let _ = fs::write(p, serde_json::to_string(&recents).unwrap_or_default());
    }
}

/// Forget a project path (e.g. its folder was deleted/moved and no longer opens),
/// so it stops resurfacing in Open Recent. No-op if it isn't in the list.
fn remove_recent(app: &AppHandle, path: &str) {
    let mut recents = read_recents(app);
    let before = recents.len();
    recents.retain(|r| r != path);
    if recents.len() == before {
        return;
    }
    if let Some(p) = recents_path(app) {
        let _ = fs::create_dir_all(p.parent().unwrap());
        let _ = fs::write(p, serde_json::to_string(&recents).unwrap_or_default());
    }
}

// ------------------------------------------------------------ open helpers

/// Build a `ProjectHandle` for an existing project folder, install it as the
/// active project, start its watcher, and kick off a background index rebuild if
/// the SQLite cache is empty but extractions exist on disk. Does NOT trigger a
/// crunch — callers decide whether to (open reconciles, create forces).
fn open_handle(
    app: &AppHandle,
    state: &State<AppState>,
    project_dir: &Path,
) -> Result<Arc<ProjectHandle>, String> {
    // Stop the previous project's watcher, if any.
    if let Some(old) = state.project.lock_safe().take() {
        old.watcher_stop.store(true, Ordering::SeqCst);
    }

    let handle = Arc::new(project::open_project(project_dir)?);

    *state.project.lock_safe() = Some(handle.clone());
    push_recent(app, &handle.project_dir.to_string_lossy());

    // Watcher thread (no-ops when design_path is None).
    {
        let (app, p) = (app.clone(), handle.clone());
        let generation = p.watcher_gen.load(Ordering::SeqCst);
        std::thread::spawn(move || watcher::run(app, p, generation));
    }

    // Background: a schema bump empties the DB — refill it from the system of record
    // (the DB is a cache, never truth).
    {
        let handle = handle.clone();
        std::thread::spawn(move || {
            // Soft presence heartbeat: record that we opened this project.
            presence::touch(&handle.project_dir, None);
            let empty = index_db::open(&handle.project_dir)
                .map(|conn| index_db::is_empty(&conn))
                .unwrap_or(false);
            if empty && !handle.list_extractions_meta().is_empty() {
                if let Err(e) = rebuild_index_from_store(&handle) {
                    log::warn!("background index rebuild failed: {e}");
                }
            }
            // Reclaim orphaned raw objects (raw is the only copy now),
            // keeping any revision a review pins through its base_revision.
            {
                let retention = project::load_settings(&handle.project_dir)
                    .retention_days
                    .unwrap_or(project::DEFAULT_RETENTION_DAYS);
                let keep: std::collections::HashSet<String> =
                    reviews::list_comments(&handle.project_dir)
                        .into_iter()
                        .map(|c| c.base_revision)
                        .filter(|s| !s.is_empty())
                        .collect();
                if let Err(e) = rawstore::prune(&handle.project_dir, retention, &keep) {
                    log::warn!("raw-store prune failed: {e}");
                }
                // Bound the machine-local checkpoint timeline (keep newest 50 / 14
                // days / the active revision). Hard delete — local + unsynced.
                let mut protect = std::collections::HashSet::new();
                if let Some(r) = handle.effective_resolved() {
                    protect.insert(r.revision().id.clone());
                }
                let _ = checkpoints::gc_local(&handle.project_dir, 50, 14, &protect);
            }
        });
    }

    Ok(handle)
}

/// Rebuild the SQLite index from the system of record: re-ingest the effective
/// revision's cache when it is already extracted (the crunch / open-ensure path
/// extracts on a miss — we never extract inside a rebuild).
fn rebuild_index_from_store(handle: &ProjectHandle) -> Result<(), String> {
    index_db::drop_all(&handle.project_dir)?;
    let mut conn = index_db::open(&handle.project_dir)?;
    if let Some(resolved) = handle.effective_resolved() {
        let rev = resolved.revision();
        let key = cache::cache_key(&rev.source_hashes);
        if cache::is_cached(&handle.project_dir, &key) {
            let dir = cache::cache_dir(&handle.project_dir, &key);
            index_db::ingest(&mut conn, &dir, &rev.id, &rev.ts, &handle.name)?;
        }
    }
    Ok(())
}

/// A collision-free temp dir under the OS temp root. Keyed by pid + a process-wide
/// counter (not just pid + rev.id) so two code paths materializing the SAME revision
/// concurrently (e.g. the read-only background thread and a set-active) don't
/// `remove_dir_all` + write into the same dir and feed a torn tree to the extractor.
fn unique_tmp_dir(prefix: &str, rev_id: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("{prefix}_{}_{}_{}", std::process::id(), rev_id, n))
}

/// Surfaced by the commands that would trigger a fresh extraction (viewer switch, diff
/// prepare) while a crunch is staging into cache/<key>.tmp — blocking avoids the
/// remove_dir_all race on the shared staging dir. The frontend toasts it verbatim.
const EXTRACTION_BUSY: &str = "extraction in progress — try again in a moment";

/// Ensure the runtime cache holds `rev`'s extracted bundle, extracting on a miss —
/// from the live design folder when it still matches the revision, else by
/// materializing the revision's raw source into a temp dir and extracting that.
/// Used off the crunch path (set-active to a historical revision, read-only open).
fn ensure_revision_cache(
    handle: &ProjectHandle,
    resolved: &project::ResolvedRev,
) -> Result<PathBuf, String> {
    let rev = resolved.revision();
    let key = cache::cache_key(&rev.source_hashes);
    if cache::is_cached(&handle.project_dir, &key) {
        return Ok(cache::cache_dir(&handle.project_dir, &key));
    }
    // Live design folder, when present and byte-for-byte this revision.
    if let Some(design_path) = handle.design_path_clone() {
        if cache::cache_key(&sidecar::source_hashes(&design_path)) == key {
            if let Some(detected) = project::detect_design(&design_path) {
                return cache::ensure_lazy(&handle.project_dir, &PathBuf::from(detected.file), &key);
            }
        }
    }
    // Otherwise materialize the raw source from the right store and extract from it.
    let tmp = unique_tmp_dir("spinzero_mat", &rev.id);
    let _ = fs::remove_dir_all(&tmp);
    // Scope the fallible work so the temp tree is always cleaned up, even on error.
    let result = (|| -> Result<PathBuf, String> {
        match resolved {
            project::ResolvedRev::Synced(r) => rawstore::materialize(&handle.project_dir, r, &tmp)?,
            project::ResolvedRev::Local(r) => checkpoints::materialize_local(&handle.project_dir, r, &tmp)?,
        }
        let detected = project::detect_design(&tmp)
            .ok_or_else(|| "materialized revision has no design file".to_string())?;
        cache::ensure_lazy(&handle.project_dir, &PathBuf::from(detected.file), &key)
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

/// Write a revision's raw source back into the design folder so the EDA tool and the
/// app agree (the checkout-to-disk write). Materializes to a temp dir FIRST — so a
/// pruned/missing blob fails before any on-disk file is touched — then reconciles into
/// `design_path`: overwrite every manifest file and delete any relevant-source file not
/// in the manifest (a checkout that dropped a sheet drops it on disk). Non-source files
/// (`output/`, `.git/`, …) are left untouched. Caveat: if the board is open in KiCad,
/// the OS may lock a file and a copy can fail — the pre-checkout state is always
/// recoverable (clean, or captured as a checkpoint by the caller).
fn write_revision_into_design(
    project_dir: &Path,
    resolved: &project::ResolvedRev,
    design_path: &Path,
) -> Result<(), String> {
    let rev = resolved.revision();
    let tmp = unique_tmp_dir("spinzero_checkout", &rev.id);
    let _ = fs::remove_dir_all(&tmp);
    let manifest: HashSet<String> = rev.source_hashes.keys().cloned().collect();
    // Scope the fallible work so the temp tree is always removed, even on an early error
    // return (it otherwise litters %TEMP%).
    let result = (|| -> Result<(), String> {
        match resolved {
            project::ResolvedRev::Synced(r) => rawstore::materialize(project_dir, r, &tmp)?,
            project::ResolvedRev::Local(r) => checkpoints::materialize_local(project_dir, r, &tmp)?,
        }
        // Overwrite every file the revision contains. `safe_join` rejects any manifest key
        // that would escape `design_path` (`..`/absolute/drive-rooted) — the same synced-data
        // path-traversal guard the materialize side applies — so a hostile revision synced
        // from a peer can't overwrite files outside the design folder.
        for rel in &manifest {
            let dst = rawstore::safe_join(design_path, rel)?;
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{rel}: {e}"))?;
            }
            fs::copy(tmp.join(rel), &dst).map_err(|e| format!("write {rel}: {e}"))?;
        }
        // Remove relevant-source files on disk that this revision doesn't have.
        for entry in walkdir::WalkDir::new(design_path).max_depth(4).into_iter().flatten() {
            if !entry.file_type().is_file() || !watcher::is_relevant_source(design_path, entry.path()) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(design_path)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if !manifest.contains(&rel) {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

/// Read-only KiCad open: extract the active revision from the synced raw store into
/// the cache, ingest it, and signal the frontend to load. Runs on its own thread so
/// the (possibly multi-second) materialize+extract never blocks the open command.
fn ensure_readonly_kicad_cache(app: &AppHandle, handle: Arc<ProjectHandle>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(resolved) = handle.effective_resolved() else {
            return; // nothing extracted yet on any machine
        };
        let rev = resolved.revision();
        // Ingest must succeed for the viewer to have data; on any failure emit Failed
        // (not Succeeded) so the frontend reports the problem instead of showing an
        // empty/stale viewer under a "loaded" banner.
        let outcome = ensure_revision_cache(&handle, &resolved).and_then(|dir| {
            let mut conn = index_db::open(&handle.project_dir)
                .map_err(|e| format!("index open: {e}"))?;
            index_db::ingest(&mut conn, &dir, &rev.id, &rev.ts, &handle.name)
                .map_err(|e| format!("ingest: {e}"))
        });
        match outcome {
            Ok(()) => events::emit(
                &app,
                events::CrunchEvent::Succeeded { revision_id: rev.id.clone(), crunch_ms: 0 },
            ),
            Err(e) => {
                log::error!("read-only load failed: {e}");
                events::emit(
                    &app,
                    events::CrunchEvent::Failed { stage: "read-only load".into(), stderr_tail: e },
                );
            }
        }
    });
}

// ------------------------------------------------------------ project commands

#[tauri::command]
async fn open_project(
    app: AppHandle,
    state: State<'_, AppState>,
    project_dir: String,
) -> Result<ProjectInfo, String> {
    let dir = PathBuf::from(&project_dir);
    if !project::is_project_dir(&dir) {
        // The folder is gone (or never was a project) — drop it from recents so it
        // stops reappearing in Open Recent and erroring on every click.
        remove_recent(&app, &project_dir);
        return Err(format!("not a project folder (no project.json): {project_dir}"));
    }
    let handle = open_handle(&app, &state, &dir)?;
    let tool = handle.design_tool.lock_safe().clone();
    log::info!("opened project '{}' ({tool})", handle.name);
    telemetry::bump("projects_opened");
    if handle.design_path_clone().is_some() {
        // Startup reconciliation: hash-gated crunch catches edits made while closed.
        sidecar::trigger_crunch(app, handle.clone(), "open", false);
    } else {
        // Read-only (no design folder on this machine): extract the active
        // revision from the synced raw store so the viewer can still render.
        ensure_readonly_kicad_cache(&app, handle.clone());
    }
    Ok(handle.info())
}

#[tauri::command]
async fn create_project(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    design_path: String,
    project_dir: String,
    design_tool: Option<String>,
    class: Option<String>,
) -> Result<ProjectInfo, String> {
    let pdir = PathBuf::from(&project_dir);
    let dpath = PathBuf::from(&design_path);
    project::create_project(&pdir, &name, &dpath, design_tool, class)?;
    let handle = open_handle(&app, &state, &pdir)?;
    let tool = handle.design_tool.lock_safe().clone();
    log::info!("created project '{}' ({tool})", handle.name);
    telemetry::bump("projects_opened");
    // First extraction (forced — there is nothing to hash-gate against yet).
    sidecar::trigger_crunch(app, handle.clone(), "create", true);
    Ok(handle.info())
}

/// Set the project's end application (market class). project.json is the single home
/// for it — the New Project wizard writes it once, and the BOM review's setup sheet
/// edits it later, so the review can never run against a class the project doesn't
/// claim. Returns the refreshed info so the UI reflects what was actually written.
#[tauri::command]
fn set_project_class(state: State<AppState>, class: Option<String>) -> Result<ProjectInfo, String> {
    let handle = current_project(&state)?;
    project::set_class(&handle.project_dir, class.as_deref())?;
    *handle.class.lock_safe() = class;
    Ok(handle.info())
}

#[tauri::command]
fn get_project(state: State<AppState>) -> Option<ProjectInfo> {
    state.project.lock_safe().as_ref().map(|p| p.info())
}

#[tauri::command]
fn get_recent_projects(app: AppHandle) -> Vec<String> {
    read_recents(&app)
}

/// Classify a folder the user picked in "Open Project…": an existing project,
/// a raw EDA design folder (offer to create a project from it), or neither.
#[tauri::command]
fn inspect_folder(path: String) -> String {
    let dir = PathBuf::from(&path);
    if project::is_project_dir(&dir) {
        "project".into()
    } else if project::detect_design(&dir).is_some() {
        // A raw KiCad design folder (e.g. a KiCad demo) — not a SpinZero
        // project yet. The shell jumps into New Project instead of erroring out.
        "design".into()
    } else {
        "unknown".into()
    }
}

/// Probe a folder for a KiCad design (New Project wizard, step 1).
#[tauri::command]
fn detect_design_folder(path: String) -> Option<project::DetectedDesign> {
    project::detect_design(&PathBuf::from(path))
}

/// Re-link a project whose design folder moved or is missing on this machine.
#[tauri::command]
async fn relink_design_path(
    app: AppHandle,
    state: State<'_, AppState>,
    new_design_path: String,
) -> Result<ProjectInfo, String> {
    let handle = current_project(&state)?;
    let dpath = PathBuf::from(&new_design_path);
    if !dpath.is_dir() {
        return Err(format!("not a folder: {new_design_path}"));
    }
    let detected = project::detect_design(&dpath)
        .ok_or_else(|| format!("no KiCad project file found in {new_design_path}"))?;
    if detected.legacy {
        return Err(format!(
            "{new_design_path} was built with KiCad 5 or older. Open the board in KiCad's \
             PCB editor and choose File → Save to upgrade it to the current format, then \
             try again."
        ));
    }
    project::set_design_path(&handle.project_dir, &dpath, &detected.kind)?;
    *handle.design_path.lock_safe() = Some(dpath);
    *handle.design_tool.lock_safe() = detected.kind;
    log::info!("re-linked design path for '{}'", handle.name);

    // Retire the previous watcher (it captured the old path — or a None design_path —
    // at spawn) and start a fresh one on the new path. Bumping the generation makes the
    // old watcher exit on its next poll, so re-linking can't leak it or double-crunch.
    {
        let (app2, p) = (app.clone(), handle.clone());
        let generation = p.watcher_gen.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::spawn(move || watcher::run(app2, p, generation));
    }
    sidecar::trigger_crunch(app, handle.clone(), "open", false);
    Ok(handle.info())
}

/// Outcome of writing a revision back into the design folder (`update_design_files`):
/// `dirty` means there are un-captured on-disk edits and the caller must confirm; on a
/// confirmed retry they're captured as a checkpoint, then overwritten.
#[derive(serde::Serialize, Clone)]
struct CheckoutResult {
    status: String, // "switched" | "dirty" | "busy"
    captured: Option<String>,
}

/// Point the viewer at a resolved revision: ensure its cache, ingest it, persist the
/// active pointer. Never touches the design folder — used by every viewer switch and
/// after an explicit `update_design_files` checkout.
fn view_resolved(
    handle: &ProjectHandle,
    id: Option<&str>,
    resolved: &project::ResolvedRev,
) -> Result<(), String> {
    let dir = ensure_revision_cache(handle, resolved)?;
    let rev = resolved.revision();
    let mut conn = index_db::open(&handle.project_dir)?;
    index_db::ingest(&mut conn, &dir, &rev.id, &rev.ts, &handle.name)?;
    project::set_active_extraction(&handle.project_dir, id)?;
    *handle.active_extraction.lock_safe() = id.map(|s| s.to_string());
    Ok(())
}

/// Select the revision the viewer shows. This is a **pure viewer switch** — the design
/// folder on disk is never touched, so it needs no confirmation and is always safe.
/// `id = None` means "latest". Writing a revision back into the KiCad files is a
/// separate, explicit action: `update_design_files`.
#[tauri::command]
async fn set_active_extraction(
    state: State<'_, AppState>,
    id: Option<String>,
) -> Result<(), String> {
    let handle = current_project(&state)?;
    // A viewer switch to an un-cached revision re-extracts into cache/<key>.tmp — the same
    // staging path a live crunch uses — so ensure_lazy's remove_dir_all would race the
    // crunch (sidecar 'os error 3'). Bail while a crunch runs; the switch is retried once
    // it settles. (Restores the 'Extraction in progress' guard the pure-viewer-switch path
    // dropped; the crunch's own re-extract lands the latest revision anyway.)
    if handle.crunch_running.load(Ordering::SeqCst) {
        return Err(EXTRACTION_BUSY.into());
    }
    if let Some(rid) = id.as_deref() {
        let resolved = handle.resolve_rev(rid).ok_or_else(|| format!("unknown revision {rid}"))?;
        view_resolved(&handle, Some(rid), &resolved)?;
    } else {
        project::set_active_extraction(&handle.project_dir, None)?;
        *handle.active_extraction.lock_safe() = None;
    }
    Ok(())
}

/// Write a revision's source files back into the design folder, so KiCad shows the same
/// thing as the app. An explicit user action (history graph → "Update KiCad files") —
/// never triggered by merely viewing a version. Guarded so un-captured edits are never
/// silently lost: `dirty` asks the caller to confirm; `confirmed = true` captures them
/// as a checkpoint, then overwrites.
#[tauri::command]
async fn update_design_files(
    state: State<'_, AppState>,
    id: String,
    confirmed: bool,
) -> Result<CheckoutResult, String> {
    let handle = current_project(&state)?;
    let design_path = handle.design_path_clone().ok_or_else(|| {
        "the design folder is missing on this machine — re-link it first".to_string()
    })?;

    // Suspend the watcher BEFORE the checkout: otherwise a watch-triggered crunch can
    // slip into the gap between the busy-check and the write (TOCTOU) and hash a
    // half-overwritten design folder into a torn revision. Resume unconditionally, even
    // on an early return or error.
    handle.watcher_suspended.store(true, Ordering::SeqCst);
    let result = checkout_to_disk(&handle, &design_path, &id, confirmed);
    handle.watcher_suspended.store(false, Ordering::SeqCst); // always resume
    if matches!(&result, Ok(r) if r.status == "switched") {
        log::info!("design files updated to revision {id}");
        telemetry::bump("update_design_files");
    }
    result
}

/// The KiCad checkout-to-disk body of `update_design_files`, extracted so the policy
/// (busy/dirty/switched outcomes, capture-before-overwrite, hash-gate seeding) is a named,
/// testable unit and the command wrapper is just the watcher-suspend bracket. The caller
/// MUST have suspended the watcher; this runs the whole resolve→capture→write→view flow.
fn checkout_to_disk(
    handle: &ProjectHandle,
    design_path: &Path,
    id: &str,
    confirmed: bool,
) -> Result<CheckoutResult, String> {
    let switched = CheckoutResult { status: "switched".into(), captured: None };
    if handle.crunch_running.load(Ordering::SeqCst) {
        return Ok(CheckoutResult { status: "busy".into(), captured: None });
    }

    let resolved = handle.resolve_rev(id).ok_or_else(|| format!("unknown revision {id}"))?;
    let target_hashes = resolved.revision().source_hashes.clone();
    let live = sidecar::source_hashes(design_path);

    // Disk already matches the target → no write, just point the viewer.
    if live == target_hashes {
        view_resolved(handle, Some(id), &resolved)?;
        return Ok(switched);
    }

    // Guard: refuse to clobber un-captured edits unless the user confirmed.
    let clean = handle.source_is_captured(&live);
    if !clean && !confirmed {
        return Ok(CheckoutResult { status: "dirty".into(), captured: None });
    }

    let mut captured = None;
    if !clean {
        // Capture the dirty working tree FIRST so it is never lost. Its parent is the
        // folder's own last captured state (the hash gate), not the viewed revision —
        // the edits were made on top of what was on disk.
        let git = project::git_info(design_path);
        let parent = sidecar::design_parent_id(handle);
        let cp = checkpoints::snapshot_local(
            &handle.project_dir,
            design_path,
            &live,
            &project::author_slug(),
            &git,
            parent.as_deref(),
        )?;
        captured = Some(cp.id);
    }
    if let Err(e) = write_revision_into_design(&handle.project_dir, &resolved, design_path) {
        // A mid-copy failure (e.g. KiCad holding a file lock) leaves the design folder
        // half-old/half-new. Re-seed the hash gate to the ON-DISK state so the watcher,
        // once resumed, sees "no change" and doesn't auto-capture the torn tree as a
        // garbage revision. The user's real edits were already captured above; a later
        // genuine edit re-triggers a clean crunch.
        sidecar::save_last_crunch_hashes(&handle.project_dir, &sidecar::source_hashes(design_path));
        return Err(e);
    }
    // Seed the hash gate so any stray post-resume trigger no-ops.
    sidecar::save_last_crunch_hashes(&handle.project_dir, &target_hashes);

    view_resolved(handle, Some(id), &resolved)?;
    Ok(CheckoutResult { status: "switched".into(), captured })
}

#[tauri::command]
fn list_extractions(state: State<AppState>) -> Result<Vec<project::ExtractionMeta>, String> {
    let handle = current_project(&state)?;
    Ok(handle.list_extractions_meta())
}

/// The revision id the KiCad design folder currently corresponds to (the history
/// graph's "KiCad files" marker). Independent of the viewer's active revision —
/// this is exactly the divergence the marker exists to show.
#[tauri::command]
fn get_design_head(state: State<AppState>) -> Result<Option<String>, String> {
    let handle = current_project(&state)?;
    Ok(sidecar::design_head_id(&handle))
}

#[tauri::command]
fn label_extraction(
    state: State<AppState>,
    id: String,
    label: Option<String>,
) -> Result<(), String> {
    let handle = current_project(&state)?;
    let user = project::author_slug();
    // Route the label to the store that holds the row's create event: a label folded
    // in the synced log lands on nothing for an unpublished local checkpoint (its
    // create lives in the machine-local checkpoint log), so renaming silently no-oped.
    if rawstore::find_revision(&handle.project_dir, &id).is_some() {
        return rawstore::set_label(&handle.project_dir, &user, &id, label);
    }
    if checkpoints::find_checkpoint(&handle.project_dir, &id).is_some() {
        log::info!("labeling local checkpoint {id}");
        return checkpoints::set_label_local(&handle.project_dir, &user, &id, label);
    }
    Err(format!("unknown revision {id}"))
}

// ------------------------------------------------------------ version control
// Tags, hide/retract, and revision diff over the raw store.

#[tauri::command]
fn tag_revision(
    state: State<AppState>,
    id: String,
    tag_name: String,
    message: Option<String>,
) -> Result<(), String> {
    let handle = current_project(&state)?;
    rawstore::set_tag(&handle.project_dir, &project::author_slug(), &id, &tag_name, message)
}

#[tauri::command]
fn untag_revision(state: State<AppState>, tag_name: String) -> Result<(), String> {
    let handle = current_project(&state)?;
    rawstore::remove_tag(&handle.project_dir, &project::author_slug(), &tag_name)
}

#[tauri::command]
fn hide_revision(state: State<AppState>, id: String, reason: Option<String>) -> Result<(), String> {
    let handle = current_project(&state)?;
    rawstore::set_hidden(&handle.project_dir, &project::author_slug(), &id, true, reason)
}

#[tauri::command]
fn unhide_revision(state: State<AppState>, id: String) -> Result<(), String> {
    let handle = current_project(&state)?;
    rawstore::set_hidden(&handle.project_dir, &project::author_slug(), &id, false, None)
}

#[tauri::command]
fn diff_revisions(
    state: State<AppState>,
    a: String,
    b: String,
) -> Result<rawstore::RevisionDiff, String> {
    let handle = current_project(&state)?;
    // Resolves synced revisions today; Step D generalizes this to also resolve local
    // checkpoints via `ProjectHandle::resolve_rev`.
    let from = handle
        .resolve_source_hashes(&a)
        .ok_or_else(|| format!("unknown revision {a}"))?;
    let to = handle
        .resolve_source_hashes(&b)
        .ok_or_else(|| format!("unknown revision {b}"))?;
    Ok(rawstore::diff_source_hashes(&from, &to))
}

/// What `prepare_diff` hands back: the changeset plus the two cache keys (so the
/// frontend can lazily read B's artifacts via `read_artifact_from` while A stays
/// active) and the resolved side labels. Field names cross to `src/lib/diff.ts`.
#[derive(serde::Serialize)]
struct DiffHandle {
    doc: diff::DiffDoc,
    /// Machine-local path of the cached `diff.json` (regenerable, never synced).
    path: String,
    cache_key_a: String,
    cache_key_b: String,
    label_a: String,
    label_b: String,
}

/// Human-facing label for a revision row: its explicit label, else its message's
/// first line, else the short revision id. Mirrors the history graph's row text.
fn revision_label(rev: &rawstore::Revision) -> String {
    if let Some(l) = rev.label.as_ref().filter(|l| !l.is_empty()) {
        return l.clone();
    }
    if let Some(m) = rev.message.as_ref().and_then(|m| m.lines().next()).filter(|m| !m.is_empty()) {
        return m.to_string();
    }
    rev.id.clone()
}

/// The `.kicad_pcb` source file in a revision's source-hash map (design-relative),
/// for the diff engine's PCB-pass pruning. `None` for a board-less (schematic-only)
/// revision.
fn pcb_source_file(hashes: &std::collections::BTreeMap<String, String>) -> Option<String> {
    hashes
        .keys()
        .find(|k| k.ends_with(".kicad_pcb"))
        .cloned()
}

/// Prepare a semantic diff of two revisions (§6.1). Ensures both revision caches
/// (short-circuits to an empty diff when the cache keys are equal), runs the pure
/// engine, writes `diff.json` to the machine-local diff cache, GCs it, and returns
/// the doc + both cache keys + labels. Read-only: no viewer state changes.
///
/// The body is multi-second on a cold cache (two extractions + a full board diff), so
/// the command is `async` and runs it on a blocking thread — the webview event loop and
/// all other IPC (incl. the "Preparing comparison…" spinner) stay responsive.
#[tauri::command]
async fn prepare_diff(
    state: State<'_, AppState>,
    rev_a: String,
    rev_b: String,
) -> Result<DiffHandle, String> {
    let handle = current_project(&state)?;
    tauri::async_runtime::spawn_blocking(move || prepare_diff_blocking(&handle, rev_a, rev_b))
        .await
        .map_err(|e| format!("prepare_diff task: {e}"))?
}

/// The synchronous body of [`prepare_diff`], run on a blocking thread.
fn prepare_diff_blocking(
    handle: &Arc<ProjectHandle>,
    rev_a: String,
    rev_b: String,
) -> Result<DiffHandle, String> {
    // Same staging-dir race as set_active_extraction: prepare_diff materializes both
    // revisions' caches (ensure_revision_cache below), which shares cache/<key>.tmp with a
    // live crunch. Don't start while one runs.
    if handle.crunch_running.load(Ordering::SeqCst) {
        return Err(EXTRACTION_BUSY.into());
    }

    let resolved_a = handle
        .resolve_rev(&rev_a)
        .ok_or_else(|| format!("unknown revision {rev_a}"))?;
    let resolved_b = handle
        .resolve_rev(&rev_b)
        .ok_or_else(|| format!("unknown revision {rev_b}"))?;
    let rev_meta_a = resolved_a.revision().clone();
    let rev_meta_b = resolved_b.revision().clone();
    log::info!("prepare_diff: {} → {}", rev_meta_a.id, rev_meta_b.id);
    // Counted once both revisions resolve, so every path below (byte-identical
    // short-circuit, cached doc, freshly computed) reports one diff prepared.
    telemetry::bump("diffs_prepared");

    let key_a = cache::cache_key(&rev_meta_a.source_hashes);
    let key_b = cache::cache_key(&rev_meta_b.source_hashes);
    let label_a = revision_label(&rev_meta_a);
    let label_b = revision_label(&rev_meta_b);

    // Both bundles are extracted at the current EXTRACTOR_CACHE_EPOCH, so equal cache
    // keys ⇒ byte-identical bundles ⇒ empty diff. Short-circuit before any extraction.
    if key_a == key_b {
        log::info!("prepare_diff: revisions are byte-identical — empty diff");
        let doc = diff::empty_doc(&rev_meta_a.id, &label_a, &rev_meta_b.id, &label_b);
        let dkey = diff::diff_key(&key_a, &key_b);
        let path = write_diff_cache(&handle.project_dir, &dkey, &doc)?;
        return Ok(DiffHandle { doc, path, cache_key_a: key_a, cache_key_b: key_b, label_a, label_b });
    }

    // Materialize both bundles' caches up front, even when the diff doc itself is
    // served from cache below: the frontend reads both sides' artifacts by cache key
    // right after this returns, and the bundle cache GCs independently of the diff
    // cache, so a cached doc must not outlive its bundles. The two caches key off
    // disjoint dirs, so extract them in parallel — on a cold cache this is ~1 s instead
    // of ~1 s + ~1 s back-to-back.
    let (res_a, res_b) = std::thread::scope(|s| {
        let ta = s.spawn(|| ensure_revision_cache(handle, &resolved_a));
        let tb = s.spawn(|| ensure_revision_cache(handle, &resolved_b));
        (ta.join(), tb.join())
    });
    let cache_dir_a = res_a.map_err(|_| "extraction A panicked".to_string())??;
    let cache_dir_b = res_b.map_err(|_| "extraction B panicked".to_string())??;

    // Serve a cached diff.json when both bundles are unchanged. The changeset is a
    // pure function of the two source-identical bundles, but the row labels/revs are
    // per-request metadata (a revision can be relabeled), so refresh the sides.
    let dkey = diff::diff_key(&key_a, &key_b);
    let cache_path = diff::diff_cache_path(&handle.project_dir, &dkey);
    if let Ok(text) = fs::read_to_string(&cache_path) {
        if let Ok(mut doc) = serde_json::from_str::<diff::DiffDoc>(&text) {
            log::info!("prepare_diff: served cached diff.json ({} changes)", doc.changes.len());
            doc.a = diff::DiffSide { rev: rev_meta_a.id.clone(), label: label_a.clone() };
            doc.b = diff::DiffSide { rev: rev_meta_b.id.clone(), label: label_b.clone() };
            return Ok(DiffHandle {
                doc,
                path: cache_path.to_string_lossy().into_owned(),
                cache_key_a: key_a,
                cache_key_b: key_b,
                label_a,
                label_b,
            });
        }
    }

    let bundle_a = load_diff_bundle(&cache_dir_a, &rev_meta_a, label_a.clone())?;
    let bundle_b = load_diff_bundle(&cache_dir_b, &rev_meta_b, label_b.clone())?;

    // Cheap per-file source-hash delta feeds the engine's pruning (§6.4). Altium
    // prunes nothing (the delta is still computed and simply won't match .kicad_*).
    let source_diff = rawstore::diff_source_hashes(&rev_meta_a.source_hashes, &rev_meta_b.source_hashes);

    let doc = diff::diff_bundles(&bundle_a, &bundle_b, &source_diff);
    log::info!("prepare_diff: computed {} changes", doc.changes.len());
    let path = write_diff_cache(&handle.project_dir, &dkey, &doc)?;
    diff::gc(&handle.project_dir, &dkey, 8); // keep the doc we just published + serve

    Ok(DiffHandle { doc, path, cache_key_a: key_a, cache_key_b: key_b, label_a, label_b })
}

/// Assemble a diff `Bundle` from a revision's cache dir: viewer indexes + the
/// backend-only extras (sheet→file map, geometry IR).
fn load_diff_bundle(
    cache_dir: &Path,
    rev: &rawstore::Revision,
    label: String,
) -> Result<diff::Bundle, String> {
    let indexes = design::build_indexes(Some(cache_dir.to_path_buf()))?;
    let extras = design::load_diff_extras(cache_dir)?;
    let geometry = match extras.geometry_json {
        Some(text) => serde_json::from_str::<diff::Geometry>(&text)
            .map_err(|e| format!("geometry parse: {e}"))
            .map(Some)?,
        None => None,
    };
    // Schematic geometry is best-effort: a parse failure (or an older cache without the
    // artifact) simply leaves the diff engine on its one-row-per-sheet fallback. Log the
    // fallback branch so a corrupt geometry.json is distinguishable from an old cache in a
    // bug report (mirrors the extractor's 'schematic geometry skipped').
    let sch_geometry = extras.sch_geometry_json.and_then(|text| {
        match serde_json::from_str::<diff::SchGeometry>(&text) {
            Ok(g) => Some(g),
            Err(e) => {
                log::info!("diff: schematic geometry skipped for {} ({e})", rev.id);
                None
            }
        }
    });
    Ok(diff::Bundle {
        rev: rev.id.clone(),
        label,
        indexes,
        sheet_files: extras.sheet_files,
        geometry,
        sch_geometry,
        pcb_file: pcb_source_file(&rev.source_hashes),
        comp_params: extras.comp_params,
    })
}

/// Serialize + write a diff doc into the machine-local cache, returning its path.
fn write_diff_cache(project_dir: &Path, dkey: &str, doc: &diff::DiffDoc) -> Result<String, String> {
    let root = diff::diff_cache_root(project_dir);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let path = diff::diff_cache_path(project_dir, dkey);
    let text = serde_json::to_string(doc).map_err(|e| e.to_string())?;
    // Atomic-ish publish: write a temp then rename, so a reader never sees a torn file.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Read an artifact from a *specific* revision's cache by its cache key (§6.2), so the
/// frontend can load the comparison (B) side's sheets/geometry while A stays active.
/// Read-only; mirrors `read_artifact`'s metadata-stripping + path sanitization.
#[tauri::command]
fn read_artifact_from(
    state: State<AppState>,
    cache_key: String,
    rel_path: String,
) -> Result<String, String> {
    let handle = current_project(&state)?;
    // Reject a key that isn't a plain cache-key token, so it can't escape the cache root.
    if cache_key.is_empty() || !cache_key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("invalid cache key".into());
    }
    let dir = cache::cache_dir(&handle.project_dir, &cache_key);
    if !dir.is_dir() {
        return Err(format!("no cached bundle for key {cache_key}"));
    }
    design::read_artifact(Some(dir), &rel_path)
}

/// Promote a machine-local checkpoint into the synced (shared) history. Idempotent —
/// the content id dedupes, so re-publishing is a no-op.
#[tauri::command]
fn publish_checkpoint(state: State<AppState>, id: String, message: String) -> Result<(), String> {
    let handle = current_project(&state)?;
    // A changelog message is required (item 5): no blank publishes into shared history.
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("a changelog message is required to publish".into());
    }
    let git = handle
        .design_path_clone()
        .map(|p| project::git_info(&p))
        .unwrap_or_default();
    checkpoints::publish(&handle.project_dir, &id, &project::author_slug(), &git, Some(&message))?;
    Ok(())
}

/// Other users who touched this project recently (soft awareness for the fork banner).
#[tauri::command]
fn get_presence(state: State<AppState>) -> Result<Vec<presence::Presence>, String> {
    let handle = current_project(&state)?;
    Ok(presence::recent_editors(&handle.project_dir, 24))
}

/// Hard-delete a machine-local checkpoint (it's unsynced — rewriting the local log is
/// safe). A published revision can't be deleted this way; hide or purge it instead.
#[tauri::command]
fn delete_checkpoint(state: State<AppState>, id: String) -> Result<(), String> {
    let handle = current_project(&state)?;
    checkpoints::delete_local(&handle.project_dir, &id)
}

/// "Purge locally now" — for a leaked secret. Writes a synced hide tombstone (so the
/// revision leaves every UI and isn't re-kept by retention) AND deletes this revision's
/// now-orphan blobs on THIS machine immediately. Honest serverless caveat (surfaced in
/// the UI): peers who already synced keep their copies until their own GC — we cannot
/// reach them. Refuses to purge the revision currently being viewed.
#[tauri::command]
fn purge_revision_local(state: State<AppState>, id: String) -> Result<(), String> {
    let handle = current_project(&state)?;
    if handle.effective_resolved().map(|r| r.revision().id.clone()) == Some(id.clone()) {
        return Err("can't purge the revision you're viewing — switch to another first".into());
    }
    let user = project::author_slug();
    rawstore::set_hidden(&handle.project_dir, &user, &id, true, Some("purged locally".into()))?;
    rawstore::purge_objects(&handle.project_dir, &id)?;
    let _ = checkpoints::delete_local(&handle.project_dir, &id);
    Ok(())
}

#[tauri::command]
fn crunch_now(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let handle = current_project(&state)?;
    if handle.design_path_clone().is_none() {
        return Err("design folder not found — re-link it before extracting".into());
    }
    sidecar::trigger_crunch(app, handle, "manual", true);
    Ok(())
}

#[tauri::command]
fn get_crunch_status(state: State<AppState>) -> Result<project::CrunchStatus, String> {
    let handle = current_project(&state)?;
    let status = handle.status.lock_safe().clone();
    Ok(status)
}

#[tauri::command]
fn rebuild_index(state: State<AppState>) -> Result<(), String> {
    let handle = current_project(&state)?;
    log::info!("manual index rebuild for '{}'", handle.name);
    rebuild_index_from_store(&handle)
}

// ------------------------------------------------------------ index reads

#[tauri::command]
fn get_project_summary(
    state: State<AppState>,
) -> Result<Option<index_db::ProjectSummary>, String> {
    let handle = current_project(&state)?;
    let conn = index_db::open(&handle.project_dir)?;
    Ok(index_db::project_summary(&conn, handle.effective_extraction_id().as_deref()))
}

#[tauri::command]
fn list_sheets(state: State<AppState>) -> Result<Vec<index_db::SheetInfo>, String> {
    let handle = current_project(&state)?;
    let conn = index_db::open(&handle.project_dir)?;
    Ok(index_db::list_sheets(&conn, handle.effective_extraction_id().as_deref()))
}

#[tauri::command]
fn list_layers(state: State<AppState>) -> Result<Vec<index_db::LayerInfo>, String> {
    let handle = current_project(&state)?;
    let conn = index_db::open(&handle.project_dir)?;
    Ok(index_db::list_layers(&conn, handle.effective_extraction_id().as_deref()))
}

#[tauri::command]
fn get_component(
    state: State<AppState>,
    designator: String,
) -> Result<Option<index_db::ComponentInfo>, String> {
    let handle = current_project(&state)?;
    let conn = index_db::open(&handle.project_dir)?;
    Ok(index_db::get_component(&conn, &designator))
}

#[tauri::command]
fn get_net(state: State<AppState>, name: String) -> Result<Option<index_db::NetInfo>, String> {
    let handle = current_project(&state)?;
    let conn = index_db::open(&handle.project_dir)?;
    Ok(index_db::get_net(&conn, &name))
}

#[tauri::command]
fn search(state: State<AppState>, q: String) -> Result<Vec<index_db::SearchHit>, String> {
    let handle = current_project(&state)?;
    let conn = index_db::open(&handle.project_dir)?;
    // Count searches only — never log or transmit the query text (it could carry
    // a net/component name).
    telemetry::bump("searches");
    Ok(index_db::search(&conn, &q))
}

/// The active extraction's bundle dir, if a project is open. None lets `design`'s
/// `PCBREVIEW_CACHE_DIR` dev override stand alone (canvas against a crunched dir).
fn opt_active_extraction(state: &State<AppState>) -> Option<PathBuf> {
    state
        .project
        .lock_safe()
        .as_ref()
        .and_then(|p| p.active_extraction_dir())
}

#[tauri::command]
fn get_design_indexes(state: State<AppState>) -> Result<design::DesignIndexes, String> {
    design::build_indexes(opt_active_extraction(&state))
}

#[tauri::command]
fn read_artifact(state: State<AppState>, rel_path: String) -> Result<String, String> {
    design::read_artifact(opt_active_extraction(&state), &rel_path)
}

#[tauri::command]
fn get_bom_lines(state: State<AppState>) -> Result<Vec<design::BomLine>, String> {
    design::bom_lines(opt_active_extraction(&state))
}

/// KiCad BOM column sets, read from the live `.kicad_pro` (the crunch cache holds only
/// extractor output, not the raw project file). Read-only mode / a missing or legacy
/// project file yields an empty list — presets are a viewing aid, never fatal.
#[tauri::command]
fn get_bom_presets(state: State<AppState>) -> Result<Vec<design::BomPreset>, String> {
    let pro = state
        .project
        .lock_safe()
        .as_ref()
        .and_then(|p| p.design_path_clone())
        .and_then(|dir| project::detect_design(&dir))
        .map(|d| PathBuf::from(d.file));
    Ok(design::bom_presets(pro))
}

// ------------------------------------------------------ BOM check (free tier)
// Deterministic rules over the crunched BOM (`crates/bom-rules`), whose findings.json
// is ingested as review comments. The paid detailed review emits the same document,
// so it lands through the same path — see bomcheck.rs.

#[tauri::command]
fn run_bom_check(
    state: State<AppState>,
    profile: Option<String>,
) -> Result<bomcheck::CheckOutcome, String> {
    let handle = current_project(&state)?;
    let lines = design::bom_lines(opt_active_extraction(&state))?;
    let profile = profile.unwrap_or_else(|| "default".to_string());
    let overrides = saved_bom_mapping(&handle.project_dir).unwrap_or_default();
    let (doc, mapping) = bomcheck::run_rules(&lines, &profile, &overrides);
    telemetry::bump("bom_checks");
    bomcheck::ingest(
        &handle.project_dir,
        &project::author_slug(),
        handle.effective_extraction_id(),
        doc,
        &mapping,
    )
}

/// The approved column mapping from project.json, or None when the user has never
/// been through the dialog. A project file that will not parse is not worth failing a
/// review over — the aliases still work.
fn saved_bom_mapping(project_dir: &Path) -> Option<BTreeMap<String, String>> {
    project::read_project_file(project_dir)
        .ok()?
        .bom_mapping
        .map(|m| m.overrides)
}

/// The BOM column mapping for the approval dialog: what each logical field reads
/// today, the columns to choose between, and whether the user has approved it yet.
/// Pure — nothing is written.
#[tauri::command]
fn get_bom_mapping(
    state: State<AppState>,
    profile: Option<String>,
) -> Result<bomcheck::MappingView, String> {
    let handle = current_project(&state)?;
    let lines = design::bom_lines(opt_active_extraction(&state))?;
    let profile = profile.unwrap_or_else(|| "default".to_string());
    let saved = saved_bom_mapping(&handle.project_dir);
    Ok(bomcheck::mapping_view(&lines, &profile, saved.as_ref()))
}

/// Record the mapping the user approved. Skipping the dialog saves an empty map —
/// the point of the write is the record that they were asked, so the app stops
/// interrupting reviews to ask again.
#[tauri::command]
fn set_bom_mapping(
    state: State<AppState>,
    overrides: BTreeMap<String, String>,
) -> Result<(), String> {
    let handle = current_project(&state)?;
    let read = overrides.values().filter(|c| !c.is_empty()).count();
    log::info!("BOM column mapping approved: {read}/{} fields read", overrides.len());
    project::set_bom_mapping(&handle.project_dir, overrides)
}

// ------------------------------------------- detailed review (paid tier, Phase 1)
// The service does the reviewing; the app's job is to send the smallest possible
// bundle, and to land the result through the SAME ingestion path as the free check
// so a paid finding refines the free comment instead of duplicating it.
//
// The HTTP conversation itself (submit, SSE progress, findings, ack) lives in the
// frontend (`src/lib/reviewService.ts`): it is plain fetch against a configurable
// base URL, and keeping it there means no provider token, no job state and no retry
// policy in the Rust process.

/// Exactly what a detailed review would upload, for the pre-flight dialog. Pure —
/// nothing is sent, nothing is written.
#[tauri::command]
fn build_review_bundle(
    state: State<AppState>,
    profile: Option<String>,
) -> Result<reviewbundle::ReviewBundle, String> {
    let handle = current_project(&state)?;
    let profile = profile.unwrap_or_else(|| "default".to_string());
    let design_tool = handle.design_tool.lock_safe().clone();
    let component_count = design::bom_lines(opt_active_extraction(&state))
        .map(|lines| lines.iter().map(|l| l.designators.len()).sum())
        .unwrap_or(0);
    reviewbundle::build(
        opt_active_extraction(&state),
        &profile,
        &handle.name,
        &design_tool,
        handle.effective_extraction_id(),
        component_count,
    )
}

/// Ingest a findings document the review service produced.
///
/// The document is untrusted input from the network, so it is parsed into the same
/// typed shape the free tier emits (`findings-1.0.json`) and rejected if it is not
/// that — a malformed or hostile document must not be able to file review comments
/// with arbitrary shapes into the user's project folder.
#[tauri::command]
fn ingest_findings(
    state: State<AppState>,
    doc: serde_json::Value,
) -> Result<bomcheck::CheckOutcome, String> {
    let handle = current_project(&state)?;
    let doc: bom_rules::FindingsDoc =
        serde_json::from_value(doc).map_err(|e| format!("not a findings.json document: {e}"))?;
    if doc.schema_version != "1.0" {
        return Err(format!(
            "findings schema_version {} is not supported by this app (expected 1.0)",
            doc.schema_version
        ));
    }
    if doc.pipeline == "bom-rules" {
        // The free tier has its own command; letting a network document file as the
        // local checker would let it auto-resolve the checker's comments.
        return Err("a bom-rules document must come from the local check, not the service".into());
    }
    telemetry::bump("detailed_reviews");
    log::info!(
        "ingesting {} findings from pipeline {} ({})",
        doc.findings.len(),
        doc.pipeline,
        doc.engine_version
    );
    // A stage the service could not run (provider rate limit, cost cap, timeout) is
    // the difference between "checked and clean" and "never checked". The UI shows it
    // on the BOM strip; log it too, so a user reporting "the review missed X" can be
    // answered from the app log without the service's job directory.
    for h in &doc.run_health {
        log::warn!(
            "{} detailed review incomplete: stage '{}' {} — {}",
            telemetry::LOCAL_ONLY,
            h.stage,
            h.status,
            h.detail
        );
    }
    // Counted as well as logged: "how often does a paid review come back incomplete"
    // is a question about the service, and a log line only answers it for the one user
    // who thought to send their log. A count, never the reason — the detail quotes a
    // provider error and names parts, and neither leaves the machine.
    if !doc.run_health.is_empty() {
        telemetry::bump("detailed_reviews_incomplete");
    }
    bomcheck::ingest(
        &handle.project_dir,
        &project::author_slug(),
        handle.effective_extraction_id(),
        doc,
        &bom_rules::load::MappingReport::default(),
    )
}

// ------------------------------------------------------------ reviews (Phase 2)
// Object-anchored review comments synced as per-user append-only logs under the
// project folder's reviews/ dir. ⟳ re-check is derived on the frontend from
// object_hash vs the live design, never persisted here.

#[tauri::command]
fn get_review_author() -> String {
    project::author_slug()
}

#[tauri::command]
fn list_comments(state: State<AppState>) -> Result<Vec<reviews::Comment>, String> {
    let handle = current_project(&state)?;
    Ok(reviews::list_comments(&handle.project_dir))
}

#[tauri::command]
fn apply_review_action(
    state: State<AppState>,
    action: reviews::ActionInput,
) -> Result<Vec<reviews::Comment>, String> {
    let handle = current_project(&state)?;
    // Count only new comments, not edits/resolves/re-anchors.
    if action.action == "create" {
        telemetry::bump("review_comments");
    }
    reviews::apply_action(&handle.project_dir, &project::author_slug(), action)
}

#[tauri::command]
fn list_review_sessions(state: State<AppState>) -> Result<Vec<reviews::Session>, String> {
    let handle = current_project(&state)?;
    Ok(reviews::list_sessions(&handle.project_dir))
}

#[tauri::command]
fn apply_session_action(
    state: State<AppState>,
    action: reviews::SessionActionInput,
) -> Result<Vec<reviews::Session>, String> {
    let handle = current_project(&state)?;
    // Count only newly started review sessions, not rename/status/delete.
    if action.action == "create" {
        telemetry::bump("review_sessions");
    }
    reviews::apply_session_action(&handle.project_dir, &project::author_slug(), action)
}

// ------------------------------------------------------------ net highlights (item 22)
// Persistent net/component highlights, scoped per user: stored in the project
// folder as `highlights.<user>.json`. One board per project, so they are keyed by
// a fixed `"design"` key inside the file (the project folder IS the scope).

fn highlights_path(project_dir: &Path, user: &str) -> PathBuf {
    project_dir.join(format!("highlights.{user}.json"))
}

#[tauri::command]
fn get_highlights(state: State<AppState>) -> Result<serde_json::Value, String> {
    let handle = current_project(&state)?;
    let path = highlights_path(&handle.project_dir, &project::author_slug());
    Ok(fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null))
}

#[tauri::command]
fn set_highlights(state: State<AppState>, data: serde_json::Value) -> Result<(), String> {
    let handle = current_project(&state)?;
    let path = highlights_path(&handle.project_dir, &project::author_slug());
    let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
    // Atomic write (tmp + rename) like every other synced-folder writer, so a crash or a
    // sync-client snapshot mid-write can't ship a truncated highlights file to peers — a
    // half-written JSON here is silently swallowed by get_highlights (returns Null), losing
    // the user's saved highlights with no error.
    project::write_atomic(&path, json.as_bytes())
}

// ------------------------------------------------------------ app settings
// App-level UI preferences (keymap preset, default project root, …) — distinct
// from per-project settings. Stored next to recent_projects.json.

fn ui_settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("ui_settings.json"))
}

#[tauri::command]
fn get_settings(app: AppHandle) -> serde_json::Value {
    ui_settings_path(&app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: serde_json::Value) -> Result<(), String> {
    let path = ui_settings_path(&app).ok_or("no config dir")?;
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    // Atomic write (tmp + rename) like set_highlights and every other whole-file writer.
    // This file is the ONLY copy of the user's prefs — keymap, accent, and every
    // project's remembered UI — and the frontend persists the WHOLE object on every
    // setter, so a bare fs::write reopens a truncation window on each one (the opacity
    // slider fires one per drag tick). A torn file parses as invalid and get_settings
    // swallows it into Null: every preference gone, silently.
    project::write_atomic(&path, json.as_bytes())
}

// ------------------------------------------------------------ external links

/// Open an http(s) URL in the user's default browser — the About dialog's link to
/// the public releases repo (README / downloads / changelog). Tauri's webview
/// won't open a bare `target="_blank"`, and a few lines beat pulling in a plugin
/// for one link. Scheme-restricted to http(s) so a click can't launch a local
/// program (the URL crosses the IPC boundary).
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("refusing to open a non-http(s) url".into());
    }
    // `explorer <url>` misparses URLs — a query string (`?a=1&b=2`) or an unusual
    // scheme handler makes it fall back to opening a File Explorer window (at the user's
    // Documents), never reaching the browser. `rundll32 url.dll,FileProtocolHandler`
    // hands the URL straight to the registered http(s) handler (the default browser) and
    // handles `&`/query strings correctly — the standard dependency-free way to open a URL.
    #[cfg(target_os = "windows")]
    std::process::Command::new("rundll32")
        .arg("url.dll,FileProtocolHandler")
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(all(unix, not(target_os = "macos")))]
    std::process::Command::new("xdg-open")
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ------------------------------------------------------------ telemetry

/// Persist a frontend error (caught by the React error boundary or a global
/// `error`/`unhandledrejection` handler) into the same log file the backend
/// writes. The telemetry log bridge (`telemetry::wrap_logger`) forwards every
/// `error!` to Sentry, so no explicit capture is needed here. Best-effort:
/// never returns an error, so a logging failure can't itself break the UI.
#[tauri::command]
fn log_frontend_error(message: String) {
    log::error!("[frontend] {message}");
}

/// Persist a frontend WARNING into the log file only — deliberately NOT forwarded to
/// telemetry. For best-effort paths that are *expected* to fail sometimes (the launch
/// update check when offline / before any release exists), so the failure is
/// diagnosable in `spinzero.log` without spamming Sentry at ERROR on every normal
/// offline launch. Never returns an error.
#[tauri::command]
fn log_frontend_warn(message: String) {
    log::warn!("[frontend] {message}");
}

/// Current telemetry consent + whether a collector is configured — the only
/// facts the Privacy toggle needs. No design data, no diagnostics bundle.
#[tauri::command]
fn get_telemetry_info() -> Option<telemetry::TelemetryInfo> {
    telemetry::info()
}

/// Flip telemetry consent from the Privacy dialog. Returns the new value.
#[tauri::command]
fn set_telemetry_enabled(enabled: bool) -> bool {
    telemetry::set_enabled(enabled)
}

/// A process launched with a missing or invalid current working directory — some
/// launchers, and certain GUI automation, do this — makes every working-directory-
/// relative path resolution fail with "os error 3 / the system cannot find the path
/// specified": the updater's launch `check()` and relative path ops during extraction
/// included (see [[project-dist-release]] memory). If the CWD isn't a usable directory,
/// pin it to the executable's own folder, which is always valid for an installed app.
fn ensure_valid_cwd() {
    if std::env::current_dir().map(|d| d.is_dir()).unwrap_or(false) {
        return;
    }
    match std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        Some(dir) if std::env::set_current_dir(&dir).is_ok() => {
            log::warn!("startup: invalid working directory; reset CWD to {}", dir.display());
        }
        _ => log::warn!("startup: invalid working directory and could not reset it"),
    }
}

pub fn run() {
    // Initialise Sentry FIRST and keep its guard for the entire run — it flushes
    // pending telemetry on drop and installs the panic-capture integration that
    // our logging panic hook (set up later) chains onto. DSN-gated, so this stays
    // inert unless PCBREVIEW_SENTRY_DSN is set. See telemetry.rs.
    let _sentry_guard = telemetry::init();

    // `mut` is only exercised by the debug-only plugin block below.
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    // tauri-pilot: AI/E2E testing bridge, debug-only so it never ships in a
    // release build. Lets an agent drive the live app over a local socket
    // (snapshot/click/fill/assert) — see docs/testing.md.
    #[cfg(debug_assertions)]
    {
        builder = builder.plugin(tauri_plugin_pilot::init());
    }

    // Verbose in dev, lean in release.
    let log_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    // Auto-update from GitHub Releases (item 6). Desktop-only; the JS side calls
    // `check()` on launch (see lib/updater.ts) and the signature is verified against
    // the pubkey in tauri.conf.json before anything installs.
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
    }

    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(move |app| {
            // Logging: official Tauri plugin → rotating file `<app_log_dir>/spinzero.log`
            // plus a stdout mirror for `cargo tauri dev`. All code logs via `log::*`.
            // `split()` (instead of `build()`) hands us the plugin's logger so the
            // telemetry bridge can wrap it: error! also becomes a Sentry event,
            // warn!/info! become crash-context breadcrumbs. Consent-gated as always.
            let (log_plugin, max_level, logger) = tauri_plugin_log::Builder::new()
                .level(log_level)
                // Log in local time WITH an explicit UTC offset (see logging::format_line):
                // the plugin's default is bare UTC, which is ambiguous to a user reading
                // their own log and forces everyone to convert.
                .format(logging::format_line)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some(logging::LOG_FILE_STEM.into()),
                    }),
                ])
                .split(app.handle())?;
            log::set_boxed_logger(telemetry::wrap_logger(logger))?;
            log::set_max_level(max_level);
            app.handle().plugin(log_plugin)?;
            // Install the panic hook now that the log plugin is active, so a panic
            // on any thread is recorded. Chains onto Sentry's panic integration.
            logging::init();
            // A process launched with a missing/invalid working directory makes every
            // CWD-relative path resolution fail with "os error 3" (the updater check and
            // extraction included). Pin it to a valid directory now, before either runs.
            ensure_valid_cwd();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_project,
            create_project,
            get_project,
            get_recent_projects,
            inspect_folder,
            detect_design_folder,
            relink_design_path,
            set_active_extraction,
            set_project_class,
            update_design_files,
            list_extractions,
            get_design_head,
            label_extraction,
            tag_revision,
            untag_revision,
            hide_revision,
            unhide_revision,
            diff_revisions,
            prepare_diff,
            read_artifact_from,
            publish_checkpoint,
            get_presence,
            delete_checkpoint,
            purge_revision_local,
            crunch_now,
            get_crunch_status,
            rebuild_index,
            get_project_summary,
            list_sheets,
            list_layers,
            get_component,
            get_net,
            search,
            get_design_indexes,
            read_artifact,
            get_bom_lines,
            get_bom_presets,
            run_bom_check,
            get_bom_mapping,
            set_bom_mapping,
            build_review_bundle,
            ingest_findings,
            get_review_author,
            list_comments,
            apply_review_action,
            list_review_sessions,
            apply_session_action,
            get_highlights,
            set_highlights,
            get_settings,
            set_settings,
            open_external,
            log_frontend_error,
            log_frontend_warn,
            get_telemetry_info,
            set_telemetry_enabled,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // The runtime never came up, so record the fatal and exit non-zero
            // rather than panicking out of a half-dead process.
            logging::fatal(&format!("tauri failed to build: {e}"));
            telemetry::record_fatal(&format!("tauri failed to build: {e}"));
            std::process::exit(1);
        });

    app.run(|_handle, event| {
        // The event loop may terminate the process without unwinding, so ship
        // the usage summary + flush pending telemetry on the Exit event rather
        // than trusting the Sentry guard's drop.
        if let tauri::RunEvent::Exit = event {
            telemetry::on_exit();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Hash every file under `dir` into the {rel-path -> blake3} map a revision keys on.
    fn hash_tree(dir: &Path) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(entry.path()).unwrap();
            m.insert(rel, blake3::hash(&bytes).to_hex().to_string());
        }
        m
    }

    // The one destructive, GUI-only path: selecting a revision reconciles the live
    // design folder to that revision's exact tree. Verify it writes the revision's
    // files (incl. nested ones), prunes stray EDA-source files the revision lacks, and
    // never touches non-source files. Exercises write_revision_into_design directly
    // (no ProjectHandle needed — that was the point of taking project_dir).
    #[test]
    fn checkout_reconciles_design_folder_to_the_revision() {
        let base = std::env::temp_dir().join(format!("ckout_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let project_dir = base.join("project");
        let source = base.join("source");
        let design = base.join("design");
        fs::create_dir_all(source.join("lib")).unwrap();
        fs::create_dir_all(&design).unwrap();

        // Capture a two-file revision (one nested) into the synced raw store.
        fs::write(source.join("board.kicad_sch"), b"REVISION").unwrap();
        fs::write(source.join("lib/parts.kicad_sym"), b"LIB-REV").unwrap();
        let rev = rawstore::snapshot(
            &project_dir,
            &source,
            &hash_tree(&source),
            "tester",
            &project::GitInfo::default(),
            None,
        )
        .unwrap();

        // A dirty live design folder: stale content, an extra EDA-source file not in
        // the revision, and a non-source file that must survive untouched.
        fs::write(design.join("board.kicad_sch"), b"STALE-WORKING-COPY").unwrap();
        fs::write(design.join("extra.kicad_sch"), b"NOT-IN-REVISION").unwrap();
        fs::write(design.join("notes.txt"), b"keep me").unwrap();

        let resolved = project::ResolvedRev::Synced(rev);
        write_revision_into_design(&project_dir, &resolved, &design).unwrap();

        assert_eq!(fs::read(design.join("board.kicad_sch")).unwrap(), b"REVISION");
        assert_eq!(fs::read(design.join("lib/parts.kicad_sym")).unwrap(), b"LIB-REV");
        assert!(!design.join("extra.kicad_sch").exists(), "stray source file pruned");
        assert_eq!(fs::read(design.join("notes.txt")).unwrap(), b"keep me");

        let _ = fs::remove_dir_all(&base);
    }
}
