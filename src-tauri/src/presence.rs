//! Soft presence / awareness — NOT a lock.
//!
//! Hard locking is unreliable over async file sync (the lock file itself syncs with
//! latency, so two people can both acquire it). Instead each user+device writes a tiny
//! synced heartbeat `presence.<user>.<device>.json` on open and on every crunch; the
//! app reads the others to warn "⚠️ Priya crunched a revision 4 min ago — you may be
//! about to fork." It never blocks editing; a fork, if it happens, is surfaced loudly
//! in the history graph and reconciled by a human (see version-control-plan.md §1).
//!
//! Device-scoped filename keeps "one writer per file" true (no sync conflict). The file
//! lives at the project root next to `highlights.<user>.json`, and is synced.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Serialize, Deserialize)]
pub struct Presence {
    pub user: String,
    pub device: String,
    pub last_seen: String,
    #[serde(default)]
    pub revision_id: Option<String>,
}

fn presence_path(project_dir: &Path, user: &str, device: &str) -> PathBuf {
    project_dir.join(format!("presence.{user}.{device}.json"))
}

/// Heartbeat: record that this user+device is active now. Best-effort — never errors
/// a crunch or an open.
pub fn touch(project_dir: &Path, revision_id: Option<String>) {
    let user = crate::project::author_slug();
    let device = crate::device::device_id();
    let Ok(last_seen) = OffsetDateTime::now_utc().format(&Rfc3339) else {
        return;
    };
    let p = Presence { user: user.clone(), device: device.clone(), last_seen, revision_id };
    if let Ok(json) = serde_json::to_string(&p) {
        // tmp + rename: this file is synced and read by other machines — a bare
        // fs::write can be observed half-written (torn read). Rename is atomic, so a
        // reader always sees the whole old or whole new file. Best-effort as before.
        let path = presence_path(project_dir, &user, &device);
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

/// Other users active within `within_hours` (excludes this user+device), newest-first.
/// Globs `presence.*.json`; skips unparseable / stale / self entries.
pub fn recent_editors(project_dir: &Path, within_hours: i64) -> Vec<Presence> {
    let me_user = crate::project::author_slug();
    let me_device = crate::device::device_id();
    let now = OffsetDateTime::now_utc();
    let mut out: Vec<Presence> = Vec::new();
    let Ok(entries) = fs::read_dir(project_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.starts_with("presence.") && name.ends_with(".json")) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(p) = serde_json::from_str::<Presence>(&text) else {
            continue;
        };
        if p.user == me_user && p.device == me_device {
            continue; // self
        }
        let recent = OffsetDateTime::parse(&p.last_seen, &Rfc3339)
            .map(|ts| (now - ts).whole_hours() < within_hours)
            .unwrap_or(false);
        if recent {
            out.push(p);
        }
    }
    out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
    out
}
