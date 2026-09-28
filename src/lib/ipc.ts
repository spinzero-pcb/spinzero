import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  BomLine,
  BomPreset,
  Comment,
  ComponentInfo,
  CheckoutResult,
  CrunchEvent,
  CrunchStatus,
  DetectedDesign,
  ExtractionMeta,
  LayerInfo,
  NetInfo,
  PresenceEntry,
  ProjectInfo,
  ProjectSummary,
  ReviewAction,
  ReviewSession,
  RevisionDiff,
  SearchHit,
  SessionActionInput,
  SheetInfo,
  TelemetryInfo,
  UiSettings,
} from "./types";
import type { CheckOutcome, FindingsDoc, MappingView, ReviewInboxEntry } from "./findings";
import type {
  AgentProfile,

  AssistantSetup,
  RegisterOutcome,
} from "./types";
import type { DesignIndexes } from "./design";
import type { DiffHandle } from "./diff";

export const ipc = {
  // ---- projects ----
  openProject: (projectDir: string) =>
    invoke<ProjectInfo>("open_project", { projectDir }),
  createProject: (args: {
    name: string;
    designPath: string;
    projectDir: string;
    designTool?: string | null;
    class?: string | null;
  }) => invoke<ProjectInfo>("create_project", args),
  getProject: () => invoke<ProjectInfo | null>("get_project"),
  /** Change the project's end application. project.json is the only home for it. */
  setProjectClass: (cls: string | null) =>
    invoke<ProjectInfo>("set_project_class", { class: cls }),
  getRecentProjects: () => invoke<string[]>("get_recent_projects"),
  /** Classify a picked folder: "project" | "design" | "unknown"
   *  ("design" = a raw KiCad folder that isn't a SpinZero project yet). */
  inspectFolder: (path: string) => invoke<string>("inspect_folder", { path }),
  /** Detect a KiCad design inside a folder (wizard step 1). */
  detectDesign: (path: string) =>
    invoke<DetectedDesign | null>("detect_design_folder", { path }),
  /** Re-link a missing/moved design folder. */
  relinkDesignPath: (newDesignPath: string) =>
    invoke<ProjectInfo>("relink_design_path", { newDesignPath }),

  // ---- extractions ----
  crunchNow: () => invoke<void>("crunch_now"),
  getCrunchStatus: () => invoke<CrunchStatus>("get_crunch_status"),
  listExtractions: () => invoke<ExtractionMeta[]>("list_extractions"),
  /** The revision the KiCad design folder currently corresponds to (the history
   *  graph's "KiCad files" marker) — independent of the viewer's active revision. */
  getDesignHead: () => invoke<string | null>("get_design_head"),
  labelExtraction: (id: string, label: string | null) =>
    invoke<void>("label_extraction", { id, label }),
  /** Select the revision the viewer shows (null = latest). Pure viewer switch —
   *  never touches the KiCad files; `updateDesignFiles` is the explicit write. */
  setActiveExtraction: (id: string | null) =>
    invoke<void>("set_active_extraction", { id }),
  /** Write a revision's files back into the design folder (explicit action from the
   *  history graph). `confirmed` permits overwriting a dirty working tree (after
   *  capturing it as a checkpoint). */
  updateDesignFiles: (id: string, confirmed = false) =>
    invoke<CheckoutResult>("update_design_files", { id, confirmed }),

  // ---- version control (tags / hide / diff) ----
  tagRevision: (id: string, tagName: string, message?: string | null) =>
    invoke<void>("tag_revision", { id, tagName, message: message ?? null }),
  untagRevision: (tagName: string) => invoke<void>("untag_revision", { tagName }),
  hideRevision: (id: string, reason?: string | null) =>
    invoke<void>("hide_revision", { id, reason: reason ?? null }),
  unhideRevision: (id: string) => invoke<void>("unhide_revision", { id }),
  diffRevisions: (a: string, b: string) =>
    invoke<RevisionDiff>("diff_revisions", { a, b }),
  /** Semantic visual-diff: ensure both revision caches, run the diff engine, and
   *  return the changeset + both cache keys + resolved labels (visual-diff §6.1).
   *  Idempotent + cached; equal revisions short-circuit to an empty doc. */
  prepareDiff: (revA: string, revB: string) =>
    invoke<DiffHandle>("prepare_diff", { revA, revB }),
  /** Read an artifact (metadata-stripped) from a *specific* revision's cache by its
   *  cache key, so the diff view can load the A (older) side's sheets while B stays
   *  the active revision (visual-diff §6.2). */
  readArtifactFrom: (cacheKey: string, relPath: string) =>
    invoke<string>("read_artifact_from", { cacheKey, relPath }),
  /** Promote a machine-local checkpoint into the synced (shared) history (item 5:
   *  a changelog message is required, enforced in the UI and the backend). */
  publishCheckpoint: (id: string, message: string) =>
    invoke<void>("publish_checkpoint", { id, message }),
  /** Hard-delete a machine-local checkpoint (unsynced). */
  deleteCheckpoint: (id: string) => invoke<void>("delete_checkpoint", { id }),
  /** Other users who touched this project recently (soft fork-awareness). */
  getPresence: () => invoke<PresenceEntry[]>("get_presence"),

  rebuildIndex: () => invoke<void>("rebuild_index"),
  getProjectSummary: () => invoke<ProjectSummary | null>("get_project_summary"),
  listSheets: () => invoke<SheetInfo[]>("list_sheets"),
  listLayers: () => invoke<LayerInfo[]>("list_layers"),
  getComponent: (designator: string) =>
    invoke<ComponentInfo | null>("get_component", { designator }),
  getNet: (name: string) => invoke<NetInfo | null>("get_net", { name }),
  search: (q: string) => invoke<SearchHit[]>("search", { q }),

  getDesignIndexes: () => invoke<DesignIndexes>("get_design_indexes"),
  readArtifact: (relPath: string) => invoke<string>("read_artifact", { relPath }),
  getBomLines: () => invoke<BomLine[]>("get_bom_lines"),
  getBomPresets: () => invoke<BomPreset[]>("get_bom_presets"),
  /** Free tier: run the deterministic BOM rules and file their findings as review
   *  comments (dedupe/auto-resolve by fingerprint). Returns the findings document
   *  plus the refreshed comment list, so one round-trip refreshes the whole UI. */
  runBomCheck: (profile: string) => invoke<CheckOutcome>("run_bom_check", { profile }),
  /** The BOM column mapping for the approval dialog — what each rule input reads
   *  today, what else it could read, and whether the user has approved it. Pure. */
  getBomMapping: (profile: string) => invoke<MappingView>("get_bom_mapping", { profile }),
  /** Record the approved mapping (logical field → source column; "" = not in this
   *  BOM). Writing it at all is what stops the dialog interrupting the next review. */
  setBomMapping: (overrides: Record<string, string>) =>
    invoke<void>("set_bom_mapping", { overrides }),
  /** The review setup window (`SpinZero --setup <dir>`): the question an MCP review
   *  server wrote, untyped — parse it with `parseSetupRequest`. */
  setupRequest: () => invoke<unknown>("setup_request"),
  /** Save the user's answer for the review server. `mapping` holds only changed fields. */
  setupSubmit: (profile: string | null, mapping: Record<string, string>) =>
    invoke<void>("setup_submit", { profile, mapping }),
  /** A findings.json from outside, ingested through the SAME path as the free check
   *  so fingerprints reconcile against the existing comments. */
  ingestFindings: (doc: FindingsDoc) => invoke<CheckOutcome>("ingest_findings", { doc }),
  /** What is waiting in `<project>/reviews/inbox/` — findings produced by a review
   *  that ran outside the app (the engine CLI, or the user's agent over MCP). Listing
   *  parses and never files, so it is safe to call whenever the launcher opens. */
  listReviewInbox: () => invoke<ReviewInboxEntry[]>("list_review_inbox"),
  /** Import one drop-box document by name, through the same ingestion path as every
   *  other tier. A user action on purpose: filing comments into someone's project the
   *  moment a file appears is not a convenience. */
  importReviewInbox: (name: string) =>
    invoke<CheckOutcome>("import_review_inbox", { name }),

  /** Run the detailed review through the user's own agent, over MCP. Returns as soon
   *  as the agent is running; progress arrives on `agent-event` and the findings come
   *  back through the review inbox. */
  startAgentReview: (agent: AgentProfile, brief: ReviewBrief) =>
    invoke<void>("start_agent_review", { agent, brief }),
  agentReviewRunning: () => invoke<boolean>("agent_review_running"),
  /** The agent profiles SpinZero ships, for the setup screen's picker. */
  /** Stop the agent review this app started. False when none of ours is running. */
  cancelAgentReview: () => invoke<boolean>("cancel_agent_review"),
  agentProfiles: () => invoke<AgentProfile[]>("agent_profiles"),
  /** This project's newest review, whoever started it — read off the review server's
   *  own `status.json`. Asked on mount so a reopened window picks up a run in flight. */
  agentReviewStatus: () => invoke<RunStatus | null>("agent_review_status"),

  /** Everything the "Connect your AI agent" screen needs: where the server is,
   *  where the licence file is, and which assistants this machine has. */
  assistantSetup: () => invoke<AssistantSetup>("assistant_setup"),
  /** The assistants whose own config lists SpinZero. Reads files, spawns nothing. */
  assistantConnected: () => invoke<{ id: string; label: string }[]>("assistant_connected"),
  /** Run one client's own `mcp add`. We never edit its config file ourselves. */
  registerAssistant: (clientId: string) =>
    invoke<RegisterOutcome>("register_assistant", { clientId }),
  /** Write the licence key into the one file every client reads it from. */
  setLicenceKey: (key: string) => invoke<string>("set_licence_key", { key }),

  getReviewAuthor: () => invoke<string>("get_review_author"),
  listComments: () => invoke<Comment[]>("list_comments"),
  applyReviewAction: (action: ReviewAction) =>
    invoke<Comment[]>("apply_review_action", { action }),

  // ---- review sessions (item 9) ----
  listReviewSessions: () => invoke<ReviewSession[]>("list_review_sessions"),
  applySessionAction: (action: SessionActionInput) =>
    invoke<ReviewSession[]>("apply_session_action", { action }),

  // Persistent net highlights, keyed by project slug inside the per-user JSON (item 22).
  getHighlights: () => invoke<Record<string, unknown> | null>("get_highlights"),
  setHighlights: (data: Record<string, unknown>) =>
    invoke<void>("set_highlights", { data }),

  getSettings: () => invoke<UiSettings | null>("get_settings"),
  setSettings: (settings: UiSettings) => invoke<void>("set_settings", { settings }),

  /** Open an http(s) URL in the user's default browser (e.g. the releases repo
   *  from About). The webview won't follow a bare `target="_blank"`. */
  openExternal: (url: string) => invoke<void>("open_external", { url }),

  /** Persist a frontend error into the backend log file (and forward it to
   *  telemetry). Never throws — a logging failure must not cascade into the very
   *  error path that called it. */
  logError: (message: string): Promise<void> =>
    invoke<void>("log_frontend_error", { message }).catch(() => {}),

  /** Persist a frontend warning into the backend log file ONLY — not telemetry. For
   *  best-effort paths expected to fail sometimes (e.g. the launch update check when
   *  offline), so they're diagnosable in the log without spamming Sentry. Never throws. */
  logWarn: (message: string): Promise<void> =>
    invoke<void>("log_frontend_warn", { message }).catch(() => {}),

  // ---- telemetry ----
  /** Current telemetry consent + whether a collector is configured. */
  getTelemetryInfo: () => invoke<TelemetryInfo>("get_telemetry_info"),
  /** Toggle anonymized telemetry consent. Returns the new value. */
  setTelemetryEnabled: (enabled: boolean) =>
    invoke<boolean>("set_telemetry_enabled", { enabled }),
};

export function onCrunchEvent(
  handler: (ev: CrunchEvent) => void,
): Promise<UnlistenFn> {
  return listen<CrunchEvent>("crunch-event", (e) => handler(e.payload));
}

/**
 * One review's state, exactly as the review server wrote it into `status.json`.
 * Mirrors `mcpstatus::RunStatus` and `local/status.ts`.
 *
 * Counts, phase names, stage ids and two paths. Nothing here is board content, which
 * is what makes the file safe to write on a customer's machine.
 */
/** One open step in `status.json`. `handed_out_ts` is null while no sub-agent has
 *  fetched it. Step ids are the server's ("verify_parts#3", "board_review"). */
export interface OpenStepStatus {
  step: string;
  index: number;
  opened_ts: string;
  handed_out_ts: string | null;
}

export interface RunStatus {
  status_version: number;
  review_id: string;
  pipeline: string;
  profile: string;
  project_dir: string | null;
  phase: "preflight" | "preparing" | "step_open" | "assembling" | "done" | "failed";
  stage: string | null;
  steps_done: number;
  steps_total: number;
  /** The steps open now, oldest first. Up to three run at once. Absent from a server
   *  older than parallel steps. */
  open_steps?: OpenStepStatus[];
  parts_done: number;
  parts_total: number;
  datasheets_read: number;
  datasheets_total: number;
  started_ts: string;
  /** A gap here is a stalled run — see `isStalled` in `agentReviewStore`. */
  updated_ts: string;
  findings_path: string | null;
  report_path: string | null;
  error: string | null;
}

/** The preflight answers SpinZero already holds, handed to the agent in its prompt.
 *  Mirrors `agent::ReviewBrief`. */
export interface ReviewBrief {
  /** The end application profile id, or "" when the user has not stated one. */
  profile: string;
  /** Only the corrections. An empty column means "this BOM has no such column". */
  mapping: { field: string; column: string }[];
}

/** One line of life from a review running through the user's agent. Mirrors
 *  `agent::AgentEvent`. */
export type AgentEvent =
  | { kind: "started"; agent: string }
  | { kind: "progress"; line: string }
  | { kind: "status"; status: RunStatus }
  | { kind: "finished"; seconds: number; last_line: string | null }
  | { kind: "failed"; detail: string }
  | { kind: "cancelled" };

export function onAgentEvent(handler: (ev: AgentEvent) => void): Promise<UnlistenFn> {
  return listen<AgentEvent>("agent-event", (e) => handler(e.payload));
}
