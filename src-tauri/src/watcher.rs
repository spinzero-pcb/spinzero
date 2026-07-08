//! Design-folder watcher: notify → relevance filter → 2 s debounce → crunch
//! trigger (the hash gate inside the crunch handles no-op saves). The project
//! folder itself is never watched — only the design source is.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use tauri::AppHandle;

use crate::project::ProjectHandle;
use crate::sidecar;

const DEBOUNCE: Duration = Duration::from_secs(2);
/// Event-channel poll cadence (also the suspended-drain sleep).
const POLL: Duration = Duration::from_millis(300);
/// Wait between re-watch attempts after a watch failure / missing folder.
const REWATCH_BACKOFF: Duration = Duration::from_secs(5);
/// How often (while idle) to confirm the watched root still exists — a delete/rename
/// makes notify go silent without disconnecting.
const HEALTH_CHECK: Duration = Duration::from_secs(10);

const SOURCE_EXTENSIONS: &[&str] = &[
    // KiCad sources + project libraries
    "kicad_pro", "kicad_sch", "kicad_pcb", "kicad_prl", "kicad_sym", "kicad_mod",
];

const LIB_TABLE_NAMES: &[&str] = &["sym-lib-table", "fp-lib-table"];

const IGNORE_DIRS: &[&str] = &[
    ".pcbreview", ".pcbreview-project", ".git", "output", "history", "__previews", "node_modules",
];

/// Is this file an EDA source that should trigger (and be hashed for) a crunch?
pub fn is_relevant_source(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return false };
    for component in rel.components() {
        let name = component.as_os_str().to_string_lossy().to_lowercase();
        if IGNORE_DIRS.contains(&name.as_str()) || name.starts_with("project outputs") {
            return false;
        }
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // EDA backup/lock churn
    if file_name.starts_with('~')
        || file_name.starts_with(".~lock")
        || file_name.starts_with("_autosave-")
        || file_name.ends_with(".bak")
        || file_name.ends_with("-bak")
        || file_name.ends_with(".lck")
    {
        return false;
    }
    if LIB_TABLE_NAMES.contains(&file_name.as_str()) {
        return true;
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    SOURCE_EXTENSIONS.contains(&ext.as_str())
}

/// Exit condition for the watcher thread: a full stop (project closed/switched) or a
/// re-link having started a newer watcher for this handle (generation bumped past ours).
fn should_stop(project: &ProjectHandle, generation: u64) -> bool {
    project.watcher_stop.load(Ordering::SeqCst)
        || project.watcher_gen.load(Ordering::SeqCst) != generation
}

/// Sleep ~`REWATCH_BACKOFF` before a re-watch attempt, staying responsive to
/// stop/retire. Returns true if the watcher should exit instead of retrying.
fn backoff(project: &ProjectHandle, generation: u64) -> bool {
    let deadline = Instant::now() + REWATCH_BACKOFF;
    while Instant::now() < deadline {
        if should_stop(project, generation) {
            return true;
        }
        std::thread::sleep(POLL);
    }
    should_stop(project, generation)
}

/// Watch the project's design folder until `watcher_stop` flips or this watcher's
/// `generation` is retired by a re-link. Runs on its own thread. No design folder on
/// this machine (read-only mode) → exits immediately.
///
/// The watch self-heals: a failed `watch()` (folder briefly missing during a sync, a
/// network-drive hiccup) or a deleted/renamed root no longer kills auto-crunch — the
/// outer loop re-establishes the watch, backing off until the folder is reachable again.
pub fn run(app: AppHandle, project: Arc<ProjectHandle>, generation: u64) {
    let Some(design_path) = project.design_path_clone() else {
        return;
    };

    'watch: loop {
        if should_stop(&project, generation) {
            return;
        }
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                // Constructing a watcher at all failed — retrying won't help.
                log::error!("watcher: could not create file watcher: {e}");
                return;
            }
        };
        if let Err(e) = watcher.watch(&design_path, RecursiveMode::Recursive) {
            log::warn!(
                "watcher: could not watch '{}' ({e}) — retrying in {}s",
                project.name,
                REWATCH_BACKOFF.as_secs()
            );
            if backoff(&project, generation) {
                return;
            }
            continue 'watch;
        }
        log::info!("watcher: watching design folder for '{}'", project.name);

        let mut last_relevant: Option<Instant> = None;
        let mut was_suspended = false;
        let mut last_health = Instant::now();
        loop {
            if should_stop(&project, generation) {
                return;
            }
            // A checkout is writing the design folder — ignore its writes (and drop any
            // pending debounce) so it doesn't spawn a spurious revision of itself. Drain
            // the WHOLE backlog: a checkout enqueues dozens of events and a one-per-tick
            // trickle would leak past resume into a spurious crunch + checkpoint.
            if project.watcher_suspended.load(Ordering::SeqCst) {
                last_relevant = None;
                was_suspended = true;
                while rx.try_recv().is_ok() {}
                std::thread::sleep(POLL);
                continue;
            }
            if was_suspended {
                // Just resumed: clear anything queued in the gap between the last drain
                // and the flag flipping, so the checkout's tail can't trigger a crunch.
                while rx.try_recv().is_ok() {}
                was_suspended = false;
                last_relevant = None;
            }
            match rx.recv_timeout(POLL) {
                Ok(Ok(event)) => {
                    if event
                        .paths
                        .iter()
                        .any(|p| is_relevant_source(&design_path, p))
                    {
                        last_relevant = Some(Instant::now());
                    }
                }
                Ok(Err(_)) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                // The backend dropped its sender — re-establish the watch.
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    log::warn!("watcher: event channel closed for '{}' — re-watching", project.name);
                    continue 'watch;
                }
            }
            // Confirm the root still exists: a delete/rename stops notify from delivering
            // without disconnecting, so the loop would otherwise spin forever doing
            // nothing. If it's gone, re-watch (the outer loop backs off until it returns).
            if last_health.elapsed() >= HEALTH_CHECK {
                last_health = Instant::now();
                if !design_path.is_dir() {
                    log::warn!("watcher: design folder for '{}' is missing — re-watching", project.name);
                    if backoff(&project, generation) {
                        return;
                    }
                    continue 'watch;
                }
            }
            if let Some(t) = last_relevant {
                if t.elapsed() >= DEBOUNCE {
                    last_relevant = None;
                    log::debug!("watcher: change debounced, triggering crunch");
                    sidecar::trigger_crunch(app.clone(), project.clone(), "watch", false);
                }
            }
        }
    }
}
