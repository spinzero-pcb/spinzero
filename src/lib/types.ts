// Mirrors the serde types in src-tauri/src — keep field names in sync (snake_case over IPC).

export type ProjectKind = "kicad";

/** Functional safety / market class of the board (drives review rigor + specs). */
export type ProjectClass =
  | "general"
  | "automotive-comfort"
  | "automotive-safety"
  | "commercial"
  | "medical"
  | "industrial"
  | "space";

/** An EDA project file detected inside a design folder (mirrors project.rs). */
export interface DetectedDesign {
  kind: ProjectKind;
  /** Absolute path to the .kicad_pro / .PrjPcb file (or the legacy file we matched). */
  file: string;
  name: string;
  /** A legacy KiCad ≤5 layout (.pro/.sch) — detectable but not importable until the
   *  user re-saves it from KiCad 6+. */
  legacy?: boolean;
}

/** A project: app-owned folder that points at a design folder (mirrors project.rs). */
export interface ProjectInfo {
  project_dir: string;
  name: string;
  /** Absolute design folder, or null when not found on this machine. */
  design_path: string | null;
  design_path_exists: boolean;
  design_tool: string | null;
  class: string | null;
  /** The extraction the viewer shows (null = latest on disk). */
  active_extraction: string | null;
  extraction_count: number;
}

/** One revision/checkpoint row in the picker (mirrors project.rs ExtractionMeta). */
export interface ExtractionMeta {
  id: string;
  label: string | null;
  /** Changelog message captured at publish; shown as the row's primary text. */
  message: string | null;
  created_at: string;
  design_tool: string | null;
  git_hash: string | null;
  git_branch: string | null;
  git_dirty: boolean | null;
  /** Revision ids this derived from ([] = root). Drives the history-graph edges. */
  parents: string[];
  /** Tag names pointing here (git-tag-style ref labels). */
  tags: string[];
  /** Retracted/tombstoned — filtered from the picker by default. */
  hidden: boolean;
  /** In the synced (shared) history. Local-only checkpoints are false. */
  published: boolean;
  /** A machine-local autosave checkpoint, not yet published to the team. */
  is_checkpoint: boolean;
  /** Author of the create event (graph rows / presence). */
  author: string | null;
}

/** Fill version-control fields that an older payload might omit (defensive — the
 *  current backend always sends them). Legacy rows are treated as published. */
export function normalizeExtraction(r: ExtractionMeta): ExtractionMeta {
  return {
    ...r,
    message: r.message ?? null,
    parents: r.parents ?? [],
    tags: r.tags ?? [],
    hidden: r.hidden ?? false,
    published: r.published ?? true,
    is_checkpoint: r.is_checkpoint ?? false,
    author: r.author ?? null,
  };
}

/** Source-file delta between two revisions (mirrors rawstore::RevisionDiff). */
export interface RevisionDiff {
  added: string[];
  removed: string[];
  changed: string[];
}

/** A teammate's recent activity on this project (mirrors presence.rs Presence). */
export interface PresenceEntry {
  user: string;
  device: string;
  last_seen: string;
  revision_id: string | null;
}

/** Result of writing a revision into the design folder (updateDesignFiles). `dirty` =>
 *  un-captured on-disk edits; confirm, then retry with confirmed=true (mirrors lib.rs
 *  CheckoutResult). */
export interface CheckoutResult {
  status: "switched" | "dirty" | "busy";
  /** Checkpoint id the dirty working tree was captured into (after a confirmed update). */
  captured: string | null;
}

export type CrunchPhase = "idle" | "running" | "succeeded" | "failed" | "skipped";

export type CrunchTrigger = "open" | "watch" | "manual" | "create";

export type CrunchEvent =
  | { kind: "started"; trigger: CrunchTrigger }
  | { kind: "progress"; line: string }
  | { kind: "artifact"; path: string }
  | { kind: "succeeded"; revision_id: string; crunch_ms: number }
  | { kind: "failed"; stage: string; stderr_tail: string }
  | { kind: "skipped"; reason: string };

export interface ProjectSummary {
  name: string;
  revision_id: string;
  sheet_count: number;
  layer_count: number;
  component_count: number;
  net_count: number;
  /** BOM rows = BOM-included components (the extractor emits one row per component;
   *  grouping is the BOM table's job). Not the grouped-line count on screen. */
  bom_component_count: number;
}

export interface SheetInfo {
  number: number;
  name: string;
  sheet_path: string;
  svg_path: string;
  /** KiCad page label; empty when the project uses automatic numbering, so the
   *  viewer falls back to `number`. */
  page: string;
}

export interface LayerInfo {
  name: string;
  role: string;
  svg_path: string;
}

export interface ComponentInfo {
  designator: string;
  value: string | null;
  footprint: string | null;
  mpn: string | null;
  sheet: string | null;
  dnp: boolean;
  nets: { net: string; pin: string; pin_name: string | null }[];
}

export interface NetInfo {
  name: string;
  pins: { designator: string; pin: string; pin_name: string | null }[];
}

export interface SearchHit {
  kind: "component" | "net";
  ref: string;
  detail: string;
}

export interface BomLine {
  item: number;
  qty: number;
  designators: string[];
  value: string;
  footprint: string;
  mpn: string;
  dnp: boolean;
  /** Every string field the crunched BOM carries for this line, verbatim — the source
   *  for user/custom columns named by a BOM preset. */
  fields: Record<string, string>;
}

/** One column of a KiCad BOM preset (mirrors design.rs BomPresetField). `name` may be a
 *  KiCad virtual field (`${QUANTITY}`, `${DNP}`, `${ITEM_NUMBER}`), passed through as-is. */
export interface BomPresetField {
  name: string;
  label: string;
  show: boolean;
  /** KiCad's "group by" flag: symbols coalesce into one BOM line when every field
   *  flagged here has the same value. Absent in an older payload → false. */
  group_by?: boolean;
}

/** A KiCad BOM column set from the project's .kicad_pro (mirrors design.rs BomPreset). */
export interface BomPreset {
  name: string;
  fields: BomPresetField[];
  sort_field: string;
  sort_asc: boolean;
  exclude_dnp: boolean;
  group_symbols: boolean;
  /** True for the entry built from the project's live `bom_settings` — the column set
   *  KiCad currently has selected, used as the default until the user picks one. */
  is_project_default: boolean;
}

/** Keyboard-shortcut preset. KiCad is the only preset today; kept as a
 *  one-member union for forward-compat. */
export type KeymapPreset = "kicad";

/** App-level UI preferences (NOT project settings). Stored in the OS config dir. */
export interface UiSettings {
  keymap_preset: KeymapPreset | null;
  /** Remembered parent folder for new projects (asked once, reused after). */
  project_root?: string | null;
  /** User-chosen accent colour (#rrggbb). Absent/null = built-in default. */
  accent_color?: string | null;
  /** Display name shown for this user's review comments; null = the OS-derived slug. */
  author_name?: string | null;
  /** Per-project remembered review UI (last session + status tab), keyed by project dir. */
  project_ui?: Record<string, ProjectUi>;
  /** Remembered PCB per-class transparency (object class → opacity 0..1). */
  pcb_opacity?: Record<string, number> | null;
  /** BOM tab quick-filter chips. App-global (not ProjectUi): the chip ids are fixed
   *  by the app, unlike BOM preset/column ids which each project defines. */
  bom_chips?: Record<string, boolean> | null;
  /** Blink the changed copper in the PCB compare. */
  diff_blink?: boolean | null;
  /** Run the free BOM check after every successful extraction (opt-in: the check
   *  files review comments, so it must never start doing that unasked). */
  /** Output panel height in px (drag-resized). */
  bottom_panel_h?: number | null;
  /** Update version downloaded + offered but not applied; the next launch may
   *  auto-apply it. Null = nothing pending. */
  update_deferred?: string | null;
  /** Where the paid review service lives, and the token for it. Phase 1 is a static
   *  dev token (plan §5); Phase 2 replaces it with a Clerk session whose refresh
   *  token belongs in the OS keychain, not here. */
  /** How to start SpinZero's own review server, for the block the user pastes into
   *  their agent. Null = never set up. */
  agent_review?: AgentReviewSettings | null;
  /** Which agent runs the review, and how to start it. Null = the shipped default. */
  agent_profile?: AgentProfile | null;
}

/**
 * How to start one AI agent.
 *
 * An agent is a command line program that drives a model and speaks MCP. SpinZero
 * works with any of them, so this is a profile rather than a hard-coded command.
 * `{prompt}` and `{project_dir}` are the only placeholders, and the backend does the
 * quoting. Mirrors `agent::AgentProfile`.
 */
export interface AgentProfile {
  id: string;
  label: string;
  /** The executable. Empty means this profile cannot run yet. */
  bin: string;
  /** `arg` puts the prompt in `{prompt}`; `stdin` writes it to the program's input. */
  prompt_via: "arg" | "stdin";
  args: string[];
  /** Has SpinZero run this profile end to end? A profile we have not is still
   *  offered, and the screen says so rather than presenting a guess as a fact. */
  verified: boolean;
}

/**
 * Where SpinZero's own review server lives, and what it needs in its environment.
 *
 * The app no longer forces this on an agent — MCP registration belongs to the user,
 * and `ConnectAssistant` generates the block they paste into their own agent. This is
 * the one saved source that block is rendered from.
 */
export interface AgentReviewSettings {
  /** Path to the `claude` executable; empty means "whatever is on PATH". */
  claude_bin: string;
  /** Command that starts the MCP server, e.g. "node". */
  server_command: string;
  /** Its arguments, e.g. ["/path/to/mcp/src/server.ts"]. */
  server_args: string[];
  /** Environment for the server: credentials, binary paths. */
  server_env: Record<string, string>;
}

/** Machine-local, per-project review UI state remembered across sessions. */
export interface ProjectUi {
  /** Last-selected review session id; null = the "All comments" pool. */
  session_id?: string | null;
  /** Last-active status tab (All/Open/⟳/Done/Dismissed). */
  status_tab?: string;
  /** PCB Net Classes panel: colour picked per net class (#rrggbb). Absent classes
   *  highlight in the nets' own PCB layer colours. */
  net_class_colors?: Record<string, string>;
  /** PCB Net Classes panel: colour picked per individual net (#rrggbb). */
  net_colors?: Record<string, string>;
  /** BOM tab: active KiCad BOM preset ("" = the built-in Default column set; absent
   *  = never chose, so the project's own default wins). Per-project because presets
   *  and their column ids are defined by the project's KiCad files, not by the app. */
  bom_preset?: string | null;
  /** BOM tab: preset name ("" for Default) → hidden column ids. */
  bom_hidden?: Record<string, string[]>;
  /** BOM tab: sort column id + direction (+1/-1). */
  bom_sort?: { key: string; dir: number } | null;
  /** BOM tab: preset name ("" for Default) → column id → dragged pixel width. */
  bom_widths?: Record<string, Record<string, number>>;
  /** Retired 2026-08-24: the end application lives in project.json's `class`, which
   *  the import wizard already wrote — see lib/projectClass. Kept in the type only so
   *  an existing settings file round-trips instead of losing the key on the next save;
   *  nothing reads it. */
  bom_check_profile?: string;
  /** BOM review setup: the depth last chosen ("quick" | "detailed"). Remembered so a
   *  re-run is one click; the setup sheet says it is remembering, because a scope that
   *  persists silently is how someone reviews less than they think they did. */
  bom_review_depth?: string;
  /** Run launcher: what each review kind was last run against, keyed by review id
   *  (see lib/reviewCatalog). Machine-local on purpose — the findings themselves are
   *  comments in the project folder; this is only the "ran 23 Aug · stale" line.
   *  Validated on hydrate by `sanitizeRuns`; the shape is `ReviewRun`. */
  review_runs?: unknown;
  /** ISO timestamp of the last write for this project — the LRU key that bounds
   *  `project_ui` growth (see pruneProjectUi). */
  last_seen?: string;
}

export interface CrunchStatus {
  phase: CrunchPhase;
  last_revision_id: string | null;
  last_crunch_ms: number | null;
  last_finished_ts: string | null;
  error: { stage: string; stderr_tail: string } | null;
}

// ---------- Phase 2: review comments (mirrors src-tauri/src/reviews.rs) ----------

/** Source-agnostic from day one (phase2-workflow.md §0.1): Phase 3 AI lands as a
 *  new producer into this same record. Humans are always `human`. */
// "rule" = the deterministic checks, "agent" = the detailed LLM review — the two
// values `bomcheck::source_for` writes. This said "ai", which the backend has
// never emitted, so every detailed-review comment was outside the union.
export type CommentSource = "human" | "rule" | "agent";
/** Persisted lifecycle. ⟳ re-check is DERIVED on the frontend (object_hash vs the
 *  live design) and never stored — see deriveDisplayStatus in reviewStore. */
export type CommentStatus = "open" | "addressed" | "resolved" | "dismissed";
export type CommentSeverity = "info" | "minor" | "major" | "critical";
/** Which canvas a comment is scoped to (item 15): the same object can carry
 *  distinct schematic vs PCB vs BOM comments, and clicking one navigates there. */
export type CommentView = "schematic" | "pcb" | "bom";

export interface CommentAnchor {
  /** "bom" is the BOM-check scope anchor: a document-level finding ("this BOM has no
   *  lifecycle column") points at no electrical object, so — like "region" — it never
   *  participates in the ⟳ re-check drift loop. */
  type: "component" | "net" | "region" | "bom";
  ref: string;
  sheet?: string | null;
  /** Region (box-select) anchors only: a rectangle in world (sheet/board mm) coords.
   *  Coordinate-based, so region comments never participate in ⟳ re-check. */
  rect?: { x: number; y: number; w: number; h: number } | null;
  /** Object (net/component) anchors only: the click point in world (sheet/board mm)
   *  coords, so the PCB comment chip pins where the user clicked rather than at the
   *  object's bbox corner (24.PNG). */
  at?: { x: number; y: number } | null;
}

export interface ThreadEntry {
  event_id: string;
  user: string;
  /** Chosen display name at write time; null → show the `user` identity slug. */
  author_name?: string | null;
  ts: string;
  body: string;
}

export interface Comment {
  id: string;
  anchor: CommentAnchor;
  view: CommentView;
  /** Review session this comment belongs to (item 9); null = the "All comments" pool. */
  session_id: string | null;
  base_revision: string;
  object_hash: string | null;
  object_meta: Record<string, unknown> | null;
  source: CommentSource;
  severity: CommentSeverity | null;
  predicate: unknown | null;
  evidence: unknown | null;
  fingerprint: string | null;
  status: CommentStatus;
  reason: string | null;
  assignee: string | null;
  author: string;
  /** The author's chosen display name (from the create event); null → show `author`. */
  author_name?: string | null;
  created_ts: string;
  updated_ts: string;
  thread: ThreadEntry[];
}

/** What the frontend sends to `apply_review_action`; the backend stamps
 *  user/ts/lamport/ids authoritatively. */
export interface ReviewAction {
  action: "create" | "reply" | "status" | "assign" | "severity" | "delete" | "delete_many";
  comment_id?: string;
  /** Ids for `delete_many` — deleting a session deletes every comment it owns in one call. */
  comment_ids?: string[];
  anchor?: CommentAnchor;
  view?: CommentView;
  session_id?: string | null;
  base_revision?: string;
  object_hash?: string;
  object_meta?: Record<string, unknown>;
  source?: CommentSource;
  severity?: CommentSeverity;
  body?: string;
  status?: CommentStatus;
  reason?: string;
  assignee?: string | null;
  /** Local user's chosen display name, stamped onto create/reply events. */
  author_name?: string | null;
}

// ---------- telemetry (mirrors TelemetryInfo in src-tauri/src/telemetry.rs) ----------

/** Telemetry consent state for the Privacy toggle. Anonymized — no design data. */
export interface TelemetryInfo {
  enabled: boolean;
  /** Whether a Sentry DSN is configured (without one, nothing is ever sent). */
  dsn_configured: boolean;
}

/** A review session (item 9): a named container for comments. A project can have many;
 *  completing one keeps its comments and the team starts the next. */
export interface ReviewSession {
  id: string;
  title: string;
  status: "active" | "completed";
  author: string;
  created_ts: string;
  updated_ts: string;
}

/** What the frontend sends to `apply_session_action` (backend stamps id/ts/user). */
export interface SessionActionInput {
  action: "create" | "rename" | "status" | "delete";
  session_id?: string;
  title?: string;
  status?: string;
}

// ------------------------------------------------ connecting an AI assistant
// Mirrors `assistant.rs`. SpinZero never edits another product's config file: where a
// client has its own `mcp add` we run that, and where it does not we show a block and
// the path to paste it into.

/** How a client is told about an MCP server. */
export type HowToAdd = "command" | "config_file";

export interface AssistantClient {
  id: string;
  label: string;
  how: HowToAdd;
  /** Found on this machine. A client we cannot see is still listed — somebody about to
   *  install Cursor should not have to wonder whether SpinZero works with it. */
  installed: boolean;
  /** `how: "command"` — the line we would run, ready to show. */
  command: string;
  /** `how: "config_file"` — where that client keeps its MCP servers. */
  config_path: string;
  /** `mcpServers` for almost everyone, `servers` for VS Code. Getting this wrong makes
   *  the client ignore the block without an error. */
  config_key: string;
  /** Its own config already lists SpinZero. Read, never written; a hint, not a check
   *  that the server starts. */
  connected: boolean;
}

export interface AssistantSetup {
  /** The review server beside this app, resolved rather than typed. */
  server_command: string;
  /** Empty when it was found; otherwise why not, in a sentence. */
  server_problem: string;
  licence_file: string;
  /** Is there a key in that file? Never the key itself. */
  licence_present: boolean;
  clients: AssistantClient[];
}

export interface RegisterOutcome {
  ok: boolean;
  /** What we ran, so the user can run it themselves if it failed. */
  command: string;
  /** The client's own words about its own config. */
  detail: string;
}
