use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Crunch lifecycle events streamed to the frontend (`crunch-event`).
/// Phase 1's reload banner consumes `Succeeded` unchanged.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CrunchEvent {
    Started { trigger: String },
    Progress { line: String },
    Artifact { path: String },
    Succeeded { revision_id: String, crunch_ms: u64 },
    Failed { stage: String, stderr_tail: String },
    Skipped { reason: String },
}

pub fn emit(app: &AppHandle, ev: CrunchEvent) {
    let _ = app.emit("crunch-event", ev);
}
