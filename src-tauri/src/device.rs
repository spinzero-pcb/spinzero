//! Stable per-device identity.
//!
//! Multi-machine sync (one user on two machines) needs "one writer per file" to
//! hold even within a single user, or a dumb file sync conflict-copies the shared
//! `revisions.<user>.jsonl`. Device-scoping the per-user logs to
//! `revisions.<user>.<device>.jsonl` (and `checkpoints.<device>.jsonl`,
//! `presence.<user>.<device>.json`) keeps every writer's file private to its
//! machine; the fold still unions every log, so this is purely a filename segment.
//!
//! The id is generated once and persisted under the OS local-data dir — **machine-
//! scoped, NOT per project** — so it never travels with a synced project folder.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

static DEVICE_ID: OnceLock<String> = OnceLock::new();

fn device_id_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(crate::project::APP_IDENTIFIER)
        .join("device_id")
}

/// A stable 12-hex id for this machine, created on first use and cached for the
/// process. Best-effort persistence: if the file can't be written we still return a
/// stable-for-this-process id (the `OnceLock`), so logs never collide mid-session.
pub fn device_id() -> String {
    DEVICE_ID
        .get_or_init(|| {
            let path = device_id_path();
            if let Ok(s) = fs::read_to_string(&path) {
                let t = s.trim();
                if !t.is_empty() {
                    return t.to_string();
                }
            }
            // Seed from values that differ per machine/run; the hash just yields a
            // compact, filesystem-safe token (no randomness API needed — and
            // `Math.random`/`Date::now`-style nondeterminism is irrelevant here
            // because the result is persisted and reused).
            let seed = format!(
                "{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                std::env::var("COMPUTERNAME")
                    .or_else(|_| std::env::var("HOSTNAME"))
                    .unwrap_or_default(),
            );
            let id = blake3::hash(seed.as_bytes()).to_hex().as_str()[..12].to_string();
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&path, &id);
            id
        })
        .clone()
}
