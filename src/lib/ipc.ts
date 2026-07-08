import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  BomLine,
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
import type { DesignIndexes } from "./design";

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
  labelExtraction: (id: string, label: string | null) =>
    invoke<void>("label_extraction", { id, label }),
  /** Select a revision (null = latest). For KiCad this is a checkout-to-disk;
   *  `confirmed` permits overwriting a dirty working tree (after capturing it). */
  setActiveExtraction: (id: string | null, confirmed = false) =>
    invoke<CheckoutResult>("set_active_extraction", { id, confirmed }),

  // ---- version control (tags / hide / diff) ----
  tagRevision: (id: string, tagName: string, message?: string | null) =>
    invoke<void>("tag_revision", { id, tagName, message: message ?? null }),
  untagRevision: (tagName: string) => invoke<void>("untag_revision", { tagName }),
  hideRevision: (id: string, reason?: string | null) =>
    invoke<void>("hide_revision", { id, reason: reason ?? null }),
  unhideRevision: (id: string) => invoke<void>("unhide_revision", { id }),
  diffRevisions: (a: string, b: string) =>
    invoke<RevisionDiff>("diff_revisions", { a, b }),
  /** Promote a machine-local checkpoint into the synced (shared) history (item 5:
   *  a changelog message is required, enforced in the UI and the backend). */
  publishCheckpoint: (id: string, message: string) =>
    invoke<void>("publish_checkpoint", { id, message }),
  /** Hard-delete a machine-local checkpoint (unsynced). */
  deleteCheckpoint: (id: string) => invoke<void>("delete_checkpoint", { id }),
  /** Hide everywhere + delete this revision's bytes on this machine (leaked secret). */
  purgeRevisionLocal: (id: string) => invoke<void>("purge_revision_local", { id }),
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
