//! Read a review's progress out of the review server's own state directory.
//!
//! MCP cannot tell us. A progress notification runs from a server to ITS client,
//! which is the agent; the app is the parent of the agent and MCP has no third party.
//! A stdio server also serves only the process that started it, so the app cannot open
//! a second connection to a running review.
//!
//! So the server appends one line to `~/.spinzero/mcp-runs/<review_id>/status.jsonl` on
//! every change of state, and this module watches for it. Each line is the whole state,
//! and the file is never rewritten, so the last complete line is always a whole record.
//! Two things follow, and both are the point:
//!
//! * **The app does not have to have started the run.** A review the user started in a
//!   terminal, in Cursor, or in an editor we have never heard of shows the same bar.
//! * **Nothing here is board content.** The file holds counts, phase names, stage ids
//!   and two paths, and this module never reads anything else in that directory.
//!
//! A watcher is not a subscription: `status.jsonl` can stop moving because the run died,
//! not because it is slow. The frontend decides that from `updated_ts`, which is why
//! the whole record travels rather than a summary.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::agent::{emit, AgentEvent};
use crate::project::ProjectHandle;

/// Coalesce the burst a rewrite produces (write, rename, and the directory entry).
const DEBOUNCE: Duration = Duration::from_millis(400);
const POLL: Duration = Duration::from_millis(300);
/// Re-scan this often even with no file event: a network home directory or a
/// virus scanner can swallow notifications, and a bar that stops for that reason is
/// indistinguishable from a run that died.
const SWEEP: Duration = Duration::from_secs(10);
const REWATCH_BACKOFF: Duration = Duration::from_secs(5);

/// One review's state, exactly as the server wrote it. Mirrors `local/status.ts`.
///
/// Every field is optional to the parser except the version: this file is written by
/// a separately installed program, so a newer server writing a field we have not been
/// taught must not cost the user their progress bar.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RunStatus {
    #[serde(default)]
    pub status_version: u32,
    #[serde(default)]
    pub review_id: String,
    #[serde(default)]
    pub pipeline: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub project_dir: Option<String>,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub steps_done: u32,
    #[serde(default)]
    pub steps_total: u32,
    /// The steps open now, oldest first. Up to ten run at once. Empty from a server
    /// older than parallel steps.
    #[serde(default)]
    pub open_steps: Vec<OpenStepStatus>,
    #[serde(default)]
    pub parts_done: u32,
    #[serde(default)]
    pub parts_total: u32,
    #[serde(default)]
    pub datasheets_read: u32,
    #[serde(default)]
    pub datasheets_total: u32,
    #[serde(default)]
    pub started_ts: String,
    #[serde(default)]
    pub updated_ts: String,
    #[serde(default)]
    pub findings_path: Option<String>,
    #[serde(default)]
    pub report_path: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// One open step. `handed_out_ts` is null while no sub-agent has fetched it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct OpenStepStatus {
    #[serde(default)]
    pub step: String,
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub opened_ts: String,
    #[serde(default)]
    pub handed_out_ts: Option<String>,
}

/// The highest `status_version` this app understands. A file from a newer server is
/// ignored rather than half-read: showing a bar from fields we may be misreading is
/// worse than showing none.
const SUPPORTED_VERSION: u32 = 1;

/// Where the review server keeps one directory per run. `SPINZERO_MCP_WORK` and
/// `SPINZERO_MCP_HOME` are the server's own overrides and are honoured here too, or
/// the app would watch the wrong place on a developer's machine.
pub fn run_root() -> Option<PathBuf> {
    if let Ok(work) = std::env::var("SPINZERO_MCP_WORK") {
        if !work.trim().is_empty() {
            return Some(PathBuf::from(work));
        }
    }
    let home = match std::env::var("SPINZERO_MCP_HOME") {
        Ok(h) if !h.trim().is_empty() => PathBuf::from(h),
        _ => dirs_home()?.join(".spinzero"),
    };
    Some(home.join("mcp-runs"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Do two paths name the same folder? Compared after normalising separators and case,
/// because the agent passes the path we gave it and the server resolves it — on
/// Windows those differ in case and in slash direction for the same folder.
fn same_dir(a: &str, b: &Path) -> bool {
    fn key(p: &str) -> String {
        p.replace('\\', "/").trim_end_matches('/').to_lowercase()
    }
    key(a) == key(&b.to_string_lossy())
}

/// The newest status this machine holds for one project.
///
/// Newest, and it has to be: a terminal run and an app run can overlap, and the app
/// allows only its own. Picking by `updated_ts` shows the one that is still moving.
pub fn newest_for(root: &Path, project_dir: &Path) -> Option<RunStatus> {
    let mut best: Option<RunStatus> = None;
    for entry in std::fs::read_dir(root).ok()? {
        let Ok(entry) = entry else { continue };
        let Some(status) = read_run(&entry.path()) else { continue };
        let Some(dir) = status.project_dir.as_deref() else { continue };
        if !same_dir(dir, project_dir) {
            continue;
        }
        if best.as_ref().is_none_or(|b| b.updated_ts < status.updated_ts) {
            best = Some(status);
        }
    }
    best
}

/// The server appends one whole state per line and never rewrites the file.
const STATUS_LOG: &str = "status.jsonl";
/// What a server before 2026-09-28 wrote instead: one state, rewritten in place. Read
/// only for runs that have no log, so an old run still shows in the launcher.
const LEGACY_STATUS: &str = "status.json";
/// How much of the log's end to read. One line is about 2 KB with ten open steps, so
/// this holds many lines, and the file is never read whole.
const TAIL_BYTES: u64 = 64 * 1024;

/// One run's current state, from its directory.
fn read_run(dir: &Path) -> Option<RunStatus> {
    let log = dir.join(STATUS_LOG);
    if log.is_file() {
        // A run with a log is read from the log only. Falling back to anything else
        // would show a state other than this run's latest.
        return last_line(&log);
    }
    let text = std::fs::read_to_string(dir.join(LEGACY_STATUS)).ok()?;
    parse(&text)
}

/// The last complete line of the log that parses.
///
/// A line counts only once its newline is written. Text after the last newline is an
/// append still in progress, and it is skipped, not read as a torn state.
fn last_line(path: &Path) -> Option<RunStatus> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let complete = &text[..text.rfind('\n')?];
    // When the tail starts mid-file, its first piece is part of a line, and it fails to
    // parse like any other broken line.
    complete.rsplit('\n').find_map(parse)
}

fn parse(text: &str) -> Option<RunStatus> {
    let status: RunStatus = serde_json::from_str(text.trim()).ok()?;
    (status.status_version <= SUPPORTED_VERSION && status.status_version > 0).then_some(status)
}

/// Watch the run directory and report the open project's newest review until the
/// project closes. Runs on its own thread, and self-heals the same way the design
/// watcher does: a missing root is a root that has not been created yet, which is the
/// normal state until the first review ever runs on this machine.
pub fn run(app: AppHandle, project: Arc<ProjectHandle>) {
    let Some(root) = run_root() else {
        log::warn!("mcpstatus: no home directory, so no review progress can be read");
        return;
    };
    let project_dir = project.project_dir.clone();
    let mut last: Option<RunStatus> = None;

    'watch: loop {
        if project.watcher_stop.load(Ordering::SeqCst) {
            return;
        }
        if std::fs::create_dir_all(&root).is_err() || !root.is_dir() {
            if backoff(&project) {
                return;
            }
            continue 'watch;
        }
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                log::error!("mcpstatus: could not create file watcher: {e}");
                return;
            }
        };
        if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
            if backoff(&project) {
                return;
            }
            continue 'watch;
        }
        log::info!("mcpstatus: watching {} for review progress", root.display());

        // Report whatever is already there. A window reopened mid-review must show the
        // run, not wait for its next write.
        report(&app, &root, &project_dir, &mut last);

        let mut dirty: Option<Instant> = None;
        let mut last_sweep = Instant::now();
        loop {
            if project.watcher_stop.load(Ordering::SeqCst) {
                return;
            }
            match rx.recv_timeout(POLL) {
                Ok(Ok(_)) => dirty = Some(Instant::now()),
                Ok(Err(_)) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => continue 'watch,
            }
            let due = dirty.is_some_and(|t| t.elapsed() >= DEBOUNCE);
            if due || last_sweep.elapsed() >= SWEEP {
                dirty = None;
                last_sweep = Instant::now();
                report(&app, &root, &project_dir, &mut last);
            }
        }
    }
}

/// Emit the newest status, but only when it changed. The file is rewritten on a
/// timer's worth of events and an unchanged record is a re-render for nothing.
fn report(app: &AppHandle, root: &Path, project_dir: &Path, last: &mut Option<RunStatus>) {
    let Some(status) = newest_for(root, project_dir) else { return };
    if last.as_ref() == Some(&status) {
        return;
    }
    *last = Some(status.clone());
    emit(app, AgentEvent::Status { status: Box::new(status) });
}

fn backoff(project: &ProjectHandle) -> bool {
    let deadline = Instant::now() + REWATCH_BACKOFF;
    while Instant::now() < deadline {
        if project.watcher_stop.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(POLL);
    }
    project.watcher_stop.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// One state as the server writes it: a single line of JSON.
    fn line(id: &str, project: &str, phase: &str, updated: &str) -> String {
        // A backslash is an escape in JSON, so the fixture has to escape it the way
        // the server's own writer does. `"C:\boards"` would parse as a backspace.
        let project = project.replace('\\', "\\\\");
        format!(
            r#"{{"status_version":1,"review_id":"{id}","pipeline":"bom-detailed","profile":"commercial","project_dir":"{project}","phase":"{phase}","stage":null,"steps_done":1,"steps_total":4,"parts_done":12,"parts_total":88,"datasheets_read":80,"datasheets_total":88,"started_ts":"2026-09-14T00:00:00.000Z","updated_ts":"{updated}","findings_path":null,"report_path":null,"error":null}}"#
        )
    }

    fn write(dir: &Path, id: &str, project: &str, updated: &str) {
        let run = dir.join(id);
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join(STATUS_LOG), format!("{}\n", line(id, project, "step_open", updated))).unwrap();
    }

    #[test]
    fn a_half_appended_line_is_skipped_and_the_run_keeps_its_last_state() {
        let tmp = std::env::temp_dir().join(format!("sz-status-a-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // An older run that finished. Before the log, a torn file in the live run made
        // the watcher show this one, and the feed said "Report written" mid-run.
        let done = tmp.join("old");
        fs::create_dir_all(&done).unwrap();
        fs::write(done.join(STATUS_LOG), format!("{}\n", line("old", "/b", "done", "2026-09-14T01:00:00.000Z"))).unwrap();
        let live = tmp.join("live");
        fs::create_dir_all(&live).unwrap();
        fs::write(
            live.join(STATUS_LOG),
            format!(
                "{}\n{}\n{{\"status_version\":1,\"review_id\":\"li",
                line("live", "/b", "preparing", "2026-09-14T02:00:00.000Z"),
                line("live", "/b", "step_open", "2026-09-14T02:01:00.000Z"),
            ),
        )
        .unwrap();
        let found = newest_for(&tmp, Path::new("/b")).unwrap();
        assert_eq!(found.review_id, "live");
        assert_eq!(found.phase, "step_open", "the last COMPLETE line is the state");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn an_old_run_without_a_log_is_still_read() {
        let tmp = std::env::temp_dir().join(format!("sz-status-l-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("r")).unwrap();
        fs::write(tmp.join("r").join(LEGACY_STATUS), line("r", "/b", "done", "2026-09-14T01:00:00.000Z")).unwrap();
        assert_eq!(newest_for(&tmp, Path::new("/b")).unwrap().phase, "done");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn picks_the_newest_run_for_this_project_and_ignores_other_boards() {
        let tmp = std::env::temp_dir().join(format!("sz-status-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        write(&tmp, "old", "C:/boards/MC-02", "2026-09-14T01:00:00.000Z");
        write(&tmp, "new", "C:\\boards\\mc-02", "2026-09-14T02:00:00.000Z");
        write(&tmp, "other", "C:/boards/XX-01", "2026-09-14T03:00:00.000Z");

        let found = newest_for(&tmp, Path::new("C:/boards/MC-02")).unwrap();
        assert_eq!(found.review_id, "new", "a different case and slash is the same folder");
        assert_eq!(found.parts_total, 88);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn reads_the_open_steps_and_defaults_them_when_absent() {
        let with: RunStatus = serde_json::from_str(
            r#"{"status_version":1,"open_steps":[
                {"step":"verify_parts#2","index":2,"opened_ts":"a","handed_out_ts":"b"},
                {"step":"verify_parts#3","index":3,"opened_ts":"c","handed_out_ts":null}]}"#,
        )
        .unwrap();
        assert_eq!(with.open_steps.len(), 2);
        assert_eq!(with.open_steps[1].handed_out_ts, None);
        // An older server writes no list, and that must not cost the progress bar.
        let without: RunStatus = serde_json::from_str(r#"{"status_version":1}"#).unwrap();
        assert!(without.open_steps.is_empty());
    }

    #[test]
    fn a_file_from_a_newer_server_is_ignored_rather_than_half_read() {
        let tmp = std::env::temp_dir().join(format!("sz-status-v-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("r")).unwrap();
        fs::write(
            tmp.join("r").join(STATUS_LOG),
            "{\"status_version\":99,\"project_dir\":\"/b\",\"updated_ts\":\"z\"}\n",
        )
        .unwrap();
        assert!(newest_for(&tmp, Path::new("/b")).is_none());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_torn_file_is_skipped_without_taking_the_others_with_it() {
        let tmp = std::env::temp_dir().join(format!("sz-status-t-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        write(&tmp, "good", "/b", "2026-09-14T01:00:00.000Z");
        fs::create_dir_all(tmp.join("torn")).unwrap();
        fs::write(tmp.join("torn").join(LEGACY_STATUS), "{\"status_ver").unwrap();
        assert_eq!(newest_for(&tmp, Path::new("/b")).unwrap().review_id, "good");
        let _ = fs::remove_dir_all(&tmp);
    }
}
