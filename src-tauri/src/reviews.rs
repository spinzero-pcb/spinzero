//! Per-user append-only review event logs under .pcbreview/reviews/.
//!
//! `events.<user>.jsonl` — one JSON event per line, never rewritten. Each user
//! writes only their own file, so any dumb share (OneDrive/Syncthing) can sync
//! the folder with zero write conflicts (phase2-workflow.md §7.2). The review
//! state is the *fold* of every log sorted by `(lamport, ts, user, event_id)`:
//! a grow-only set of comments + last-writer-wins on the mutable fields
//! (status, assignee). This is a CRDT in behavior, with no CRDT runtime.
//!
//! A comment is object-anchored (component/net + sheet) and version-independent,
//! but stamped with the `base_revision` it was authored against plus a hash +
//! small meta snapshot of the anchored object. That makes drift *visible* (the
//! ⟳ re-check loop) instead of a silent misattach (phase2-workflow.md §4–§6).
//! The surface is source-agnostic from day one (`source`/`predicate`/`evidence`/
//! `severity`/`fingerprint`) so Phase 3 AI lands as another producer (§0.1).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::util::LockExt;

/// Serializes lamport-assignment + append across the whole process. Both are a
/// read-modify-write over this user's log (re-read max lamport, read the file, append,
/// rename), and Tauri commands run on a thread pool — two in flight for the same user
/// would compute the same lamport and one rename would clobber the other's appended
/// event. Appends are infrequent, so one global lock (covering comment AND session logs)
/// is simplest and cannot deadlock — it wraps only pure in-process file work.
fn write_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// A rectangle in world (sheet/board mm) coordinates — the anchor for a box-select
/// "region" comment.
#[derive(Clone, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A point in world (board/sheet mm) coordinates — the click location for an object
/// comment, so its chip pins exactly where the user clicked rather than at the object's
/// bbox corner (24.PNG).
#[derive(Clone, Serialize, Deserialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

/// Anchor — comments attach to an electrical object (`component`/`net`) or to a
/// freeform `region` box-selection. Region anchors carry a `rect` in world coordinates
/// INSTEAD of pointing at an object, so they are inherently coordinate-based and do NOT
/// participate in the ⟳ re-check drift loop (they store no object_hash). This is the one
/// deliberate exception to "anchors attach to objects, never coordinates".
#[derive(Clone, Serialize, Deserialize)]
pub struct Anchor {
    #[serde(rename = "type")]
    pub kind: String, // "component" | "net" | "region"
    #[serde(rename = "ref")]
    pub r#ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    /// Region anchors only: the box, in world (mm) coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    /// Object (net/component) anchors only: the click point in world (mm) coordinates,
    /// so the comment chip pins where the user clicked instead of at the object's bbox
    /// corner (24.PNG). Optional — older comments and region anchors carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Pt>,
}

/// One raw append-only event. A flat record (not an enum) keeps the JSONL trivial
/// to read partially / forward-compatible: unknown future fields are ignored.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub comment_id: String,
    /// "create" | "reply" | "status" | "assign" | "severity" | "delete"
    pub action: String,
    pub ts: String,
    pub lamport: u64,
    pub user: String,
    /// Display name the author chose (custom-username feature). None → fall back to the
    /// `user` identity slug. Cosmetic only: identity, attribution and the per-user log
    /// filename still key off `user`, so it stays unique regardless of this name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,

    // create
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// Which canvas this comment belongs to: "schematic" | "pcb" | "bom". The same
    /// object can carry different comments per view (item 15); legacy events default
    /// to "schematic" on fold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// Review session this comment belongs to (item 9). None = the "All comments" pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_meta: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,

    // create + reply
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,

    // status
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    // assign
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
}

/// One reply (or the opening body) in a comment's thread.
#[derive(Clone, Serialize)]
pub struct ThreadEntry {
    pub event_id: String,
    pub user: String,
    /// Chosen display name at the time this entry was written; None → show `user`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,
    pub ts: String,
    pub body: String,
}

/// A folded comment — the materialized view the UI consumes.
#[derive(Clone, Serialize)]
pub struct Comment {
    pub id: String,
    pub anchor: Anchor,
    /// "schematic" | "pcb" | "bom" — which canvas the comment is scoped to (item 15).
    pub view: String,
    /// Review session this comment belongs to (item 9); None = the "All comments" pool.
    pub session_id: Option<String>,
    pub base_revision: String,
    pub object_hash: Option<String>,
    pub object_meta: Option<Value>,
    pub source: String,
    pub severity: Option<String>,
    pub predicate: Option<Value>,
    pub evidence: Option<Value>,
    pub fingerprint: Option<String>,
    /// open | addressed | resolved | dismissed (⟳ re-check is *derived* by the UI
    /// from object_hash vs the live design — never persisted, per §5).
    pub status: String,
    pub reason: Option<String>,
    pub assignee: Option<String>,
    pub author: String,
    /// The author's chosen display name (from the create event); None → show `author`.
    pub author_name: Option<String>,
    pub created_ts: String,
    pub updated_ts: String,
    pub thread: Vec<ThreadEntry>,
}

/// Frontend-supplied action. The backend stamps user/ts/lamport/ids authoritatively
/// so identity and ordering can't be forged by a client clock.
#[derive(Deserialize)]
pub struct ActionInput {
    pub action: String,
    #[serde(default)]
    pub comment_id: Option<String>,
    /// Ids for the batch `delete_many` action (deleting a session deletes every
    /// comment it owns; one action keeps that to a single log write + fold).
    #[serde(default)]
    pub comment_ids: Option<Vec<String>>,
    #[serde(default)]
    pub anchor: Option<Anchor>,
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub base_revision: Option<String>,
    #[serde(default)]
    pub object_hash: Option<String>,
    #[serde(default)]
    pub object_meta: Option<Value>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub predicate: Option<Value>,
    #[serde(default)]
    pub evidence: Option<Value>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    /// Display name the frontend supplies (from the local user's settings). Optional.
    #[serde(default)]
    pub author_name: Option<String>,
}

fn reviews_dir(pcbreview: &Path) -> PathBuf {
    pcbreview.join("reviews")
}

fn user_log(pcbreview: &Path, user: &str) -> PathBuf {
    // Device-scoped (`events.<user>.<device>.jsonl`) so one user on two machines keeps
    // "one writer per file" — the glob in `read_all_events` still matches. See device.rs.
    reviews_dir(pcbreview).join(format!("events.{user}.{}.jsonl", crate::device::device_id()))
}

/// Read every user's event log into one unsorted vec.
fn read_all_events(pcbreview: &Path) -> Vec<Event> {
    let dir = reviews_dir(pcbreview);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.starts_with("events.") && name.ends_with(".jsonl")) {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(ev) = serde_json::from_str::<Event>(line) {
                    events.push(ev);
                }
            }
        }
    }
    events
}

/// Total-order key for the fold: lamport first, then wall clock, then user, then
/// event id — deterministic across machines regardless of arrival order.
fn order_key(e: &Event) -> (u64, String, String, String) {
    (e.lamport, e.ts.clone(), e.user.clone(), e.event_id.clone())
}

/// Fold all logs into the current set of comments, newest-first by creation.
pub fn list_comments(pcbreview: &Path) -> Vec<Comment> {
    let mut events = read_all_events(pcbreview);
    events.sort_by_key(order_key);

    let mut by_id: BTreeMap<String, Comment> = BTreeMap::new();
    // Tombstones (item 24): a "delete" event removes a comment permanently. Track ids
    // so any out-of-order reply/status for a deleted comment is ignored too.
    let mut deleted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in events {
        if deleted.contains(&e.comment_id) {
            continue;
        }
        match e.action.as_str() {
            "create" => {
                let anchor = match e.anchor {
                    Some(a) => a,
                    None => continue, // malformed create — skip
                };
                let mut thread = Vec::new();
                if let Some(body) = e.body.clone() {
                    thread.push(ThreadEntry {
                        event_id: e.event_id.clone(),
                        user: e.user.clone(),
                        author_name: e.author_name.clone(),
                        ts: e.ts.clone(),
                        body,
                    });
                }
                by_id.insert(
                    e.comment_id.clone(),
                    Comment {
                        id: e.comment_id.clone(),
                        anchor,
                        view: e.view.unwrap_or_else(|| "schematic".into()),
                        session_id: e.session_id,
                        base_revision: e.base_revision.unwrap_or_default(),
                        object_hash: e.object_hash,
                        object_meta: e.object_meta,
                        source: e.source.unwrap_or_else(|| "human".into()),
                        severity: e.severity,
                        predicate: e.predicate,
                        evidence: e.evidence,
                        fingerprint: e.fingerprint,
                        status: "open".into(),
                        reason: None,
                        assignee: e.assignee,
                        author: e.user.clone(),
                        author_name: e.author_name,
                        created_ts: e.ts.clone(),
                        updated_ts: e.ts,
                        thread,
                    },
                );
            }
            "reply" => {
                if let (Some(c), Some(body)) = (by_id.get_mut(&e.comment_id), e.body) {
                    c.thread.push(ThreadEntry {
                        event_id: e.event_id,
                        user: e.user,
                        author_name: e.author_name,
                        ts: e.ts.clone(),
                        body,
                    });
                    c.updated_ts = e.ts;
                }
            }
            "status" => {
                if let Some(c) = by_id.get_mut(&e.comment_id) {
                    if let Some(status) = e.status {
                        c.status = status; // LWW: events applied in sorted order
                        c.reason = e.reason;
                        c.updated_ts = e.ts;
                    }
                }
            }
            "assign" => {
                if let Some(c) = by_id.get_mut(&e.comment_id) {
                    c.assignee = e.assignee; // may be None to unassign
                    c.updated_ts = e.ts;
                }
            }
            "severity" => {
                // Item 10: change severity at any time (e.g. after a fix lands).
                if let Some(c) = by_id.get_mut(&e.comment_id) {
                    c.severity = e.severity;
                    c.updated_ts = e.ts;
                }
            }
            "delete" => {
                // Item 24: permanent tombstone (LWW by lamport order).
                by_id.remove(&e.comment_id);
                deleted.insert(e.comment_id);
            }
            _ => {}
        }
    }

    let mut comments: Vec<Comment> = by_id.into_values().collect();
    comments.sort_by(|a, b| b.created_ts.cmp(&a.created_ts));
    comments
}

fn next_lamport(pcbreview: &Path) -> u64 {
    read_all_events(pcbreview)
        .iter()
        .map(|e| e.lamport)
        .max()
        .map(|m| m + 1)
        .unwrap_or(0)
}

/// Append one or more events in a single whole-file replace. Batching matters:
/// deleting a session deletes every comment it owns, and one call per comment made
/// that N whole-file rewrites plus N full folds of every log.
fn append_events(pcbreview: &Path, user: &str, events: &[Event]) -> Result<(), String> {
    let path = user_log(pcbreview, user);
    let parent = path.parent().ok_or("review log has no parent dir")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut out = fs::read(&path).unwrap_or_default();
    for event in events {
        let mut line = serde_json::to_string(event).map_err(|e| e.to_string())?;
        line.push('\n');
        out.extend_from_slice(line.as_bytes());
    }
    // Whole-file atomic replace: a per-user log is small and only this process
    // writes it, so a tmp+rename keeps a crash from truncating prior events.
    let tmp = path.with_extension("jsonl.tmp");
    fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Apply a frontend action: stamp it, append to the user's log, return the new state.
pub fn apply_action(pcbreview: &Path, user: &str, input: ActionInput) -> Result<Vec<Comment>, String> {
    let ts = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| e.to_string())?;
    // Hold the write lock across lamport read + append so a concurrent command can't
    // assign the same lamport and clobber our appended event on rename.
    let _guard = write_lock().lock_safe();
    let lamport = next_lamport(pcbreview);

    if input.action == "delete_many" {
        let ids = input.comment_ids.unwrap_or_default();
        let events: Vec<Event> = ids
            .into_iter()
            .enumerate()
            .map(|(i, comment_id)| {
                let lamport = lamport + i as u64;
                let event_id = format!(
                    "e_{}",
                    &blake3::hash(format!("{user}{lamport}{ts}{comment_id}").as_bytes())
                        .to_hex()
                        .to_string()[..12]
                );
                Event {
                    event_id,
                    comment_id,
                    action: "delete".into(),
                    ts: ts.clone(),
                    lamport,
                    user: user.to_string(),
                    ..Event::default()
                }
            })
            .collect();
        append_events(pcbreview, user, &events)?;
        return Ok(list_comments(pcbreview));
    }

    let event = stamp_event(user, &ts, lamport, input)?;
    append_events(pcbreview, user, &[event])?;
    Ok(list_comments(pcbreview))
}

/// Apply a batch of actions as one log write and one fold.
///
/// Same stamping and same semantics as `apply_action`, once per input, with
/// consecutive lamports so the batch folds in the order it was built (a status change
/// and a severity refresh on the same comment stay in that order). Filing a check run's
/// findings is the caller this exists for: one call per finding meant a whole-file
/// rewrite plus a full fold of every log per finding.
pub fn apply_actions(
    pcbreview: &Path,
    user: &str,
    inputs: Vec<ActionInput>,
) -> Result<Vec<Comment>, String> {
    if inputs.is_empty() {
        return Ok(list_comments(pcbreview));
    }
    let ts = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| e.to_string())?;
    let _guard = write_lock().lock_safe();
    let base = next_lamport(pcbreview);
    let mut events = Vec::with_capacity(inputs.len());
    for (i, input) in inputs.into_iter().enumerate() {
        events.push(stamp_event(user, &ts, base + i as u64, input)?);
    }
    append_events(pcbreview, user, &events)?;
    Ok(list_comments(pcbreview))
}

/// Turn one action into the event that records it. Ids are derived from
/// user+lamport+ts, and lamport is unique per event, so a batch sharing one `ts` still
/// gets distinct comment and event ids.
fn stamp_event(user: &str, ts: &str, lamport: u64, input: ActionInput) -> Result<Event, String> {
    // `delete_many` expands into one delete event per id; it is not itself an event,
    // and stamping it would append an action the fold does not understand.
    if input.action == "delete_many" {
        return Err("delete_many must go through apply_action".into());
    }
    let comment_id = match input.action.as_str() {
        "create" => format!(
            "c_{}",
            &blake3::hash(format!("{user}{lamport}{ts}").as_bytes()).to_hex().to_string()[..12]
        ),
        _ => input
            .comment_id
            .clone()
            .ok_or_else(|| "comment_id required".to_string())?,
    };
    let event_id = format!(
        "e_{}",
        &blake3::hash(format!("{user}{lamport}{ts}{comment_id}").as_bytes())
            .to_hex()
            .to_string()[..12]
    );

    if input.action == "create" && input.anchor.is_none() {
        return Err("create requires an anchor".into());
    }

    Ok(Event {
        event_id,
        comment_id,
        action: input.action,
        ts: ts.to_string(),
        lamport,
        user: user.to_string(),
        author_name: input.author_name,
        anchor: input.anchor,
        view: input.view,
        session_id: input.session_id,
        base_revision: input.base_revision,
        object_hash: input.object_hash,
        object_meta: input.object_meta,
        source: input.source,
        predicate: input.predicate,
        evidence: input.evidence,
        fingerprint: input.fingerprint,
        body: input.body,
        severity: input.severity,
        status: input.status,
        reason: input.reason,
        assignee: input.assignee,
    })
}

// ----------------------------------------------------------- review sessions (item 9)
// A named container for comments. A project can hold several; completing one keeps its
// comments and lets the team start the next. Stored exactly like comments — per-user
// append-only `sessions.<user>.<device>.jsonl` logs under reviews/, folded by lamport
// order — so they sync conflict-free alongside the rest of the project folder.

#[derive(Clone, Serialize, Deserialize)]
struct SessionEvent {
    event_id: String,
    session_id: String,
    /// "create" | "rename" | "status" | "delete"
    action: String,
    ts: String,
    lamport: u64,
    user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
}

/// A folded review session — the materialized view the UI consumes.
#[derive(Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// "active" | "completed"
    pub status: String,
    pub author: String,
    pub created_ts: String,
    pub updated_ts: String,
}

/// Frontend-supplied session action (backend stamps ids/ts/lamport/user).
#[derive(Deserialize)]
pub struct SessionActionInput {
    pub action: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

fn session_log(pcbreview: &Path, user: &str) -> PathBuf {
    reviews_dir(pcbreview).join(format!("sessions.{user}.{}.jsonl", crate::device::device_id()))
}

fn read_all_session_events(pcbreview: &Path) -> Vec<SessionEvent> {
    let dir = reviews_dir(pcbreview);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.starts_with("sessions.") && name.ends_with(".jsonl")) {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(ev) = serde_json::from_str::<SessionEvent>(line) {
                    events.push(ev);
                }
            }
        }
    }
    events
}

fn session_order_key(e: &SessionEvent) -> (u64, String, String, String) {
    (e.lamport, e.ts.clone(), e.user.clone(), e.event_id.clone())
}

/// Fold the session logs into the current session list, oldest-first (so a picker reads
/// "Review 1, Review 2, …" in the order they were started).
pub fn list_sessions(pcbreview: &Path) -> Vec<Session> {
    let mut events = read_all_session_events(pcbreview);
    events.sort_by_key(session_order_key);
    let mut by_id: BTreeMap<String, Session> = BTreeMap::new();
    // Tombstones: a "delete" event drops the session permanently; later events for a
    // deleted id are ignored (mirrors list_comments). The session's comments are NOT
    // touched — they keep their session_id and resurface in the "All comments" pool, so
    // deleting a container never destroys review work.
    let mut deleted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in events {
        if deleted.contains(&e.session_id) {
            continue;
        }
        match e.action.as_str() {
            "create" => {
                by_id.entry(e.session_id.clone()).or_insert_with(|| Session {
                    id: e.session_id.clone(),
                    title: e.title.clone().unwrap_or_else(|| "Review".into()),
                    status: e.status.clone().unwrap_or_else(|| "active".into()),
                    author: e.user.clone(),
                    created_ts: e.ts.clone(),
                    updated_ts: e.ts.clone(),
                });
            }
            "rename" => {
                if let Some(s) = by_id.get_mut(&e.session_id) {
                    if let Some(t) = e.title {
                        s.title = t;
                    }
                    s.updated_ts = e.ts;
                }
            }
            "status" => {
                if let Some(s) = by_id.get_mut(&e.session_id) {
                    if let Some(st) = e.status {
                        s.status = st; // LWW
                    }
                    s.updated_ts = e.ts;
                }
            }
            "delete" => {
                by_id.remove(&e.session_id);
                deleted.insert(e.session_id);
            }
            _ => {}
        }
    }
    let mut sessions: Vec<Session> = by_id.into_values().collect();
    sessions.sort_by(|a, b| a.created_ts.cmp(&b.created_ts));
    sessions
}

fn next_session_lamport(pcbreview: &Path) -> u64 {
    read_all_session_events(pcbreview)
        .iter()
        .map(|e| e.lamport)
        .max()
        .map(|m| m + 1)
        .unwrap_or(0)
}

fn append_session_event(pcbreview: &Path, user: &str, event: &SessionEvent) -> Result<(), String> {
    let path = session_log(pcbreview, user);
    let parent = path.parent().ok_or("session log has no parent dir")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut line = serde_json::to_string(event).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut out = fs::read(&path).unwrap_or_default();
    out.extend_from_slice(line.as_bytes());
    let tmp = path.with_extension("jsonl.tmp");
    fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Apply a session action (create / rename / status): stamp it, append, return the list.
pub fn apply_session_action(
    pcbreview: &Path,
    user: &str,
    input: SessionActionInput,
) -> Result<Vec<Session>, String> {
    let ts = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| e.to_string())?;
    let _guard = write_lock().lock_safe();
    let lamport = next_session_lamport(pcbreview);
    let session_id = match input.action.as_str() {
        "create" => format!(
            "s_{}",
            &blake3::hash(format!("{user}{lamport}{ts}").as_bytes()).to_hex().to_string()[..12]
        ),
        _ => input
            .session_id
            .clone()
            .ok_or_else(|| "session_id required".to_string())?,
    };
    let event_id = format!(
        "e_{}",
        &blake3::hash(format!("{user}{lamport}{ts}{session_id}").as_bytes())
            .to_hex()
            .to_string()[..12]
    );
    let event = SessionEvent {
        event_id,
        session_id,
        action: input.action,
        ts,
        lamport,
        user: user.to_string(),
        title: input.title,
        status: input.status,
    };
    append_session_event(pcbreview, user, &event)?;
    Ok(list_sessions(pcbreview))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spinzero_rev_{}_{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn blank() -> ActionInput {
        ActionInput {
            action: String::new(),
            comment_id: None,
            comment_ids: None,
            anchor: None,
            view: None,
            session_id: None,
            base_revision: None,
            object_hash: None,
            object_meta: None,
            source: None,
            predicate: None,
            evidence: None,
            fingerprint: None,
            body: None,
            severity: None,
            status: None,
            reason: None,
            assignee: None,
            author_name: None,
        }
    }

    #[test]
    fn create_reply_resolve_folds() {
        let root = temp_root("fold");
        let pcb = root.join(".pcbreview");

        let create = ActionInput {
            action: "create".into(),
            anchor: Some(Anchor { kind: "component".into(), r#ref: "R12".into(), sheet: None, rect: None, at: None }),
            base_revision: Some("r1".into()),
            object_hash: Some("h1".into()),
            body: Some("needs a snubber".into()),
            severity: Some("major".into()),
            ..blank()
        };
        let comments = apply_action(&pcb, "alice", create).unwrap();
        assert_eq!(comments.len(), 1);
        let id = comments[0].id.clone();
        assert_eq!(comments[0].status, "open");
        assert_eq!(comments[0].source, "human");
        assert_eq!(comments[0].thread.len(), 1);

        // Bob replies from his own log — no write conflict.
        let reply = ActionInput {
            action: "reply".into(),
            comment_id: Some(id.clone()),
            body: Some("on it".into()),
            ..blank()
        };
        let comments = apply_action(&pcb, "bob", reply).unwrap();
        assert_eq!(comments[0].thread.len(), 2);

        // Resolve.
        let resolve = ActionInput {
            action: "status".into(),
            comment_id: Some(id.clone()),
            status: Some("resolved".into()),
            ..blank()
        };
        let comments = apply_action(&pcb, "alice", resolve).unwrap();
        assert_eq!(comments[0].status, "resolved");

        // Two user logs exist; the fold is stable across both.
        assert!(user_log(&pcb, "alice").exists());
        assert!(user_log(&pcb, "bob").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_create_rename_delete_folds() {
        let root = temp_root("sessions");
        let pcb = root.join(".pcbreview");

        // Create two sessions.
        let sessions = apply_session_action(
            &pcb,
            "alice",
            SessionActionInput { action: "create".into(), session_id: None, title: Some("Review 1".into()), status: None },
        )
        .unwrap();
        let s1 = sessions[0].id.clone();
        let sessions = apply_session_action(
            &pcb,
            "alice",
            SessionActionInput { action: "create".into(), session_id: None, title: Some("Review 2".into()), status: None },
        )
        .unwrap();
        assert_eq!(sessions.len(), 2);
        let s2 = sessions.iter().find(|s| s.id != s1).unwrap().id.clone();

        // Rename the second, then delete it.
        let sessions = apply_session_action(
            &pcb,
            "alice",
            SessionActionInput { action: "rename".into(), session_id: Some(s2.clone()), title: Some("Pre-tapeout".into()), status: None },
        )
        .unwrap();
        assert_eq!(sessions.iter().find(|s| s.id == s2).unwrap().title, "Pre-tapeout");

        let sessions = apply_session_action(
            &pcb,
            "alice",
            SessionActionInput { action: "delete".into(), session_id: Some(s2.clone()), title: None, status: None },
        )
        .unwrap();
        // The deleted session is gone; the survivor remains.
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, s1);

        // A late event for the tombstoned session is ignored (stays deleted).
        let sessions = apply_session_action(
            &pcb,
            "alice",
            SessionActionInput { action: "rename".into(), session_id: Some(s2.clone()), title: Some("Zombie".into()), status: None },
        )
        .unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions.iter().all(|s| s.id != s2));
        let _ = fs::remove_dir_all(&root);
    }
}
