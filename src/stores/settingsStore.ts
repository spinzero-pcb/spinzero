import { create } from "zustand";
import { ipc } from "../lib/ipc";
import { usePcbViewStore } from "./pcbViewStore";
import type { AgentReviewSettings, KeymapPreset, ProjectUi, ReviewServiceSettings } from "../lib/types";

// App-level UI preferences. `keymap === null` after load means the user has
// never chosen — App shows the first-launch preset picker (spec: onboarding).
interface SettingsState {
  keymap: KeymapPreset | null;
  /** Remembered parent folder for new projects (asked once, reused after). */
  projectRoot: string | null;
  /** User-chosen accent colour (#rrggbb). `null` = the token default (blue). */
  accentColor: string | null;
  /** Display name for this user's review comments; `null` = the OS-derived slug. */
  authorName: string | null;
  /** Per-project remembered review UI (last session + tab), keyed by project dir. */
  projectUi: Record<string, ProjectUi>;
  /** Remembered PCB per-class transparency (0..1 per object class); null = defaults. */
  pcbOpacity: Record<string, number> | null;
  /** BOM quick-filter chips; null = never saved (viewStore applies its own defaults,
   *  which are NOT all-false — see `changedOnly`). */
  bomChips: Record<string, boolean> | null;
  /** PCB compare blink toggle; null = never saved (defaults off). */
  diffBlink: boolean | null;
  /** Output panel height in px; null = never saved (the component's default). */
  bottomPanelH: number | null;
  /** Downloaded-but-unapplied update version; null = nothing pending. */
  updateDeferred: string | null;
  /** Review-service endpoint + token; null = never configured (the detailed review
   *  button then explains how to point the app at a service). */
  reviewService: ReviewServiceSettings | null;
  /** How to run a review through the user's own AI assistant; null = not set up. */
  agentReview: AgentReviewSettings | null;
  /** Which surface the detailed BOM review runs on. Absent keeps the hosted service,
   *  so an existing install's button does exactly what it did yesterday. */
  reviewDriver: "service" | "agent" | null;
  loaded: boolean;
  load: () => Promise<void>;
  setKeymap: (k: KeymapPreset) => Promise<void>;
  setProjectRoot: (p: string) => Promise<void>;
  setAccentColor: (c: string | null) => Promise<void>;
  setAuthorName: (n: string | null) => Promise<void>;
  /** Merge a patch into one project's remembered review UI and persist. */
  setProjectUi: (projectDir: string, patch: Partial<ProjectUi>) => Promise<void>;
  /** Persist the PCB transparency sliders (whole map, so one class never clobbers
   *  another). Debounced: the sliders fire on every tick of a drag. */
  setPcbOpacity: (opacity: Record<string, number>) => void;
  /** Persist the BOM chip set (whole map, same reason as pcbOpacity). */
  setBomChips: (chips: Record<string, boolean>) => Promise<void>;
  /** Persist the PCB compare blink toggle. */
  setDiffBlink: (on: boolean) => Promise<void>;
  /** Persist the "run the BOM check after each extraction" toggle. */
  /** Persist the output panel height (called once per completed drag). */
  setBottomPanelH: (h: number) => Promise<void>;
  /** Persist (or clear, with null) the pending update version. */
  setUpdateDeferred: (v: string | null) => Promise<void>;
  /** Persist the review-service endpoint + token (or clear it with null). */
  setReviewService: (v: ReviewServiceSettings | null) => Promise<void>;
  setAgentReview: (v: AgentReviewSettings | null) => Promise<void>;
  setReviewDriver: (v: "service" | "agent") => Promise<void>;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  keymap: null,
  projectRoot: null,
  accentColor: null,
  authorName: null,
  projectUi: {},
  pcbOpacity: null,
  bomChips: null,
  diffBlink: null,
  bottomPanelH: null,
  updateDeferred: null,
  reviewService: null,
  agentReview: null,
  reviewDriver: null,
  loaded: false,

  load: async () => {
    try {
      const s = await ipc.getSettings();
      const accent = normalizeHex(s?.accent_color ?? null);
      applyAccent(accent);
      const pcbOpacity =
        s && typeof s.pcb_opacity === "object" && s.pcb_opacity !== null
          ? (s.pcb_opacity as Record<string, number>)
          : null;
      set({
        keymap: s?.keymap_preset ?? null,
        projectRoot: s?.project_root ?? null,
        accentColor: accent,
        authorName: normalizeName(s?.author_name ?? null),
        projectUi:
          s && typeof s.project_ui === "object" && s.project_ui !== null
            ? (s.project_ui as Record<string, ProjectUi>)
            : {},
        pcbOpacity,
        // Kept raw here; the consuming stores validate on hydrate (settings are
        // hand-editable, so every field is untrusted at this boundary).
        bomChips:
          s && typeof s.bom_chips === "object" && s.bom_chips !== null
            ? (s.bom_chips as Record<string, boolean>)
            : null,
        diffBlink: typeof s?.diff_blink === "boolean" ? s.diff_blink : null,
        bottomPanelH:
          typeof s?.bottom_panel_h === "number" && Number.isFinite(s.bottom_panel_h)
            ? s.bottom_panel_h
            : null,
        updateDeferred: typeof s?.update_deferred === "string" ? s.update_deferred : null,
        reviewService: normalizeReviewService(s?.review_service ?? null),
        agentReview: normalizeAgentReview(s?.agent_review ?? null),
        reviewDriver: s?.review_driver === "agent" ? "agent" : s?.review_driver === "service" ? "service" : null,
        loaded: true,
      });
      // Push the saved transparency into the PCB view store so the sliders open where
      // the user left them (validated + clamped there).
      usePcbViewStore.getState().hydrateOpacity(pcbOpacity);
      void pruneProjectUi();
    } catch {
      set({ loaded: true });
    }
  },

  setKeymap: async (keymap) => {
    await ensureLoaded();
    set({ keymap });
    await persist();
  },

  setProjectRoot: async (projectRoot) => {
    await ensureLoaded();
    set({ projectRoot });
    await persist();
  },

  setAccentColor: async (accentColor) => {
    await ensureLoaded();
    const next = normalizeHex(accentColor);
    applyAccent(next);
    set({ accentColor: next });
    await persist();
  },

  setAuthorName: async (name) => {
    await ensureLoaded();
    set({ authorName: normalizeName(name) });
    await persist();
  },

  setProjectUi: async (projectDir, patch) => {
    await ensureLoaded();
    const prev = get().projectUi[projectDir] ?? {};
    // Stamped on every write so pruneProjectUi has an LRU key to fall back on.
    const next = { ...prev, ...patch, last_seen: new Date().toISOString() };
    set({ projectUi: { ...get().projectUi, [projectDir]: next } });
    await persist();
  },

  setPcbOpacity: (opacity) => {
    set({ pcbOpacity: opacity });
    // Debounced: the Appearance sliders fire setOpacity on every tick of a drag, and
    // each persist() is a whole-file rewrite. The UI already shows `opacity` from the
    // store, so delaying only the disk write costs nothing visible.
    persistSoon();
  },

  setBomChips: async (chips) => {
    await ensureLoaded();
    set({ bomChips: chips });
    await persist();
  },

  setDiffBlink: async (on) => {
    await ensureLoaded();
    set({ diffBlink: on });
    await persist();
  },

  setBottomPanelH: async (h) => {
    await ensureLoaded();
    set({ bottomPanelH: h });
    await persist();
  },

  setUpdateDeferred: async (v) => {
    await ensureLoaded();
    if (get().updateDeferred === v) return; // no-op clears run on every launch
    set({ updateDeferred: v });
    await persist();
  },

  setReviewService: async (v) => {
    await ensureLoaded();
    set({ reviewService: normalizeReviewService(v) });
    await persist();
  },

  setAgentReview: async (v) => {
    await ensureLoaded();
    set({ agentReview: normalizeAgentReview(v) });
    await persist();
  },

  setReviewDriver: async (v) => {
    await ensureLoaded();
    set({ reviewDriver: v });
    await persist();
  },
}));

/** Read the file before the first write of a session. Every setter mutates state and
 *  then persists the WHOLE object, so writing before `load()` would push this store's
 *  defaults over the real file and wipe every preference. Setters call this before
 *  they mutate — after would be too late, since `load()` overwrites what they set.
 *  A launch-time setter (the updater's deferral) can genuinely beat App's load(). */
async function ensureLoaded(): Promise<void> {
  if (useSettingsStore.getState().loaded) return;
  try {
    await useSettingsStore.getState().load();
  } catch {
    /* load() swallows its own errors and still marks loaded */
  }
}

// Persist the full settings object so one setter never clobbers another's field.
async function persist() {
  pending = false;
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
  // Safety net for the same hazard `ensureLoaded` handles — the debounced path can
  // fire without a setter in front of it. Every real setter awaits ensureLoaded, so
  // this returning early means there was nothing worth writing yet.
  if (!useSettingsStore.getState().loaded) return;
  const {
    keymap, projectRoot, accentColor, authorName, projectUi, pcbOpacity, bomChips,
    diffBlink, bottomPanelH, updateDeferred, reviewService, agentReview, reviewDriver,
  } = useSettingsStore.getState();
  try {
    await ipc.setSettings({
      keymap_preset: keymap,
      project_root: projectRoot,
      accent_color: accentColor,
      author_name: authorName,
      project_ui: projectUi,
      pcb_opacity: pcbOpacity,
      bom_chips: bomChips,
      diff_blink: diffBlink,
      bottom_panel_h: bottomPanelH,
      update_deferred: updateDeferred,
      review_service: reviewService,
      agent_review: agentReview,
      review_driver: reviewDriver,
    });
  } catch {
    // Persisting failed (e.g. read-only config dir) — the in-memory choice stands.
  }
}

/** Settings are hand-editable, so the service config is untrusted at load: an
 *  endpoint that is not http(s) is dropped rather than handed to fetch. */
/** Vet the assistant config on the way in. A half-filled one is null rather than a
 *  config that fails at spawn time: the UI offers "set this up" for null, which is a
 *  better answer than a subprocess error a minute later. */
function normalizeAgentReview(v: unknown): AgentReviewSettings | null {
  if (typeof v !== "object" || v === null) return null;
  const o = v as { claude_bin?: unknown; server_command?: unknown; server_args?: unknown; server_env?: unknown };
  const command = typeof o.server_command === "string" ? o.server_command.trim() : "";
  const args = Array.isArray(o.server_args) ? o.server_args.filter((a): a is string => typeof a === "string") : [];
  // Arguments are NOT required. They were when the only way to run the server was
  // `node …/mcp/src/server.ts` from a checkout; the shipped build is a single
  // executable that takes none, so demanding them silently discarded the setup every
  // customer actually has — saved, then normalised back to null on the way in.
  if (!command) return null;
  const env: Record<string, string> = {};
  if (typeof o.server_env === "object" && o.server_env !== null) {
    for (const [k, val] of Object.entries(o.server_env as Record<string, unknown>)) {
      if (typeof val === "string") env[k] = val;
    }
  }
  return {
    claude_bin: typeof o.claude_bin === "string" ? o.claude_bin.trim() : "",
    server_command: command,
    server_args: args,
    server_env: env,
  };
}

function normalizeReviewService(v: unknown): ReviewServiceSettings | null {
  if (typeof v !== "object" || v === null) return null;
  const o = v as { base_url?: unknown; token?: unknown };
  const baseUrl = typeof o.base_url === "string" ? o.base_url.trim().replace(/\/+$/, "") : "";
  if (!/^https?:\/\//i.test(baseUrl)) return null;
  return { base_url: baseUrl, token: typeof o.token === "string" ? o.token : "" };
}

// ---- debounced persist ----------------------------------------------------
// For state that changes continuously (a slider drag). Trailing edge only: the
// in-memory value is already live, so the disk write just needs to land eventually.

const PERSIST_DEBOUNCE_MS = 250;
let timer: ReturnType<typeof setTimeout> | null = null;
let pending = false;

function persistSoon() {
  pending = true;
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(() => void persist(), PERSIST_DEBOUNCE_MS);
}

/** Flush a debounced write immediately. Exported for tests; also wired to page
 *  teardown so quitting mid-drag doesn't drop the last slider position. */
export function flushSettings(): Promise<void> {
  return pending ? persist() : Promise.resolve();
}

if (typeof window !== "undefined") {
  // `pagehide` fires on webview teardown; the IPC may not complete if the process
  // dies immediately, which is why the debounce is short rather than lazy.
  window.addEventListener("pagehide", () => void flushSettings());
}

// ---- project_ui pruning ---------------------------------------------------

/** `project_ui` is keyed by project dir and nothing ever removed an entry, so it grew
 *  without bound as projects came and went. Bounded here rather than pruned eagerly:
 *  a project can be legitimately absent right now (unplugged drive, offline network
 *  share) and its remembered UI is worth more than the bytes. So do nothing until the
 *  map is well past the recents cap (8), then drop entries whose folder is no longer a
 *  project, oldest-first, and only if still over. */
const PROJECT_UI_CAP = 24;

async function pruneProjectUi(): Promise<void> {
  const current = useSettingsStore.getState().projectUi;
  const dirs = Object.keys(current);
  if (dirs.length <= PROJECT_UI_CAP) return;

  const alive = new Set<string>();
  for (const dir of dirs) {
    try {
      // "unknown" = neither a SpinZero project nor a design folder — i.e. gone. Any
      // IPC failure counts as alive, so a transient error never deletes state.
      if ((await ipc.inspectFolder(dir)) !== "unknown") alive.add(dir);
    } catch {
      alive.add(dir);
    }
  }

  let keep = dirs.filter((d) => alive.has(d));
  if (keep.length > PROJECT_UI_CAP) {
    // Still over: oldest last_seen goes first. Entries written before last_seen
    // existed sort oldest, which is the correct guess for them.
    keep = keep
      .sort((a, b) => (current[b].last_seen ?? "").localeCompare(current[a].last_seen ?? ""))
      .slice(0, PROJECT_UI_CAP);
  }
  if (keep.length === dirs.length) return;

  const projectUi: Record<string, ProjectUi> = {};
  for (const d of keep) projectUi[d] = current[d];
  useSettingsStore.setState({ projectUi });
  await persist();
}

// A display name is trimmed; blank collapses to null (fall back to the identity slug).
function normalizeName(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const t = v.trim();
  return t.length ? t.slice(0, 60) : null;
}

// ---- accent colour --------------------------------------------------------
// The accent is a single token trio (--accent / --accent-fg / --accent-dim).
// A user choice is applied as inline properties on :root, overriding tokens.css;
// clearing it (null) removes the overrides so the CSS default (blue) takes over.

function normalizeHex(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const m = /^#?([0-9a-fA-F]{6})$/.exec(v.trim());
  return m ? `#${m[1].toLowerCase()}` : null;
}

/** Apply an accent colour to :root, deriving a readable fg and a dim tint.
 *  `null` removes the overrides, falling back to the tokens.css default. */
export function applyAccent(hex: string | null) {
  const root = document.documentElement;
  if (!hex) {
    root.style.removeProperty("--accent");
    root.style.removeProperty("--accent-fg");
    root.style.removeProperty("--accent-dim");
    return;
  }
  const n = parseInt(hex.slice(1), 16);
  const r = (n >> 16) & 255;
  const g = (n >> 8) & 255;
  const b = n & 255;
  // Perceived brightness: light accents get dark text, dark accents get white.
  const light = (0.299 * r + 0.587 * g + 0.114 * b) / 255 > 0.55;
  root.style.setProperty("--accent", hex);
  root.style.setProperty("--accent-fg", light ? "#0b1210" : "#ffffff");
  root.style.setProperty("--accent-dim", `rgba(${r}, ${g}, ${b}, 0.16)`);
}

/** Accent presets shown in the Appearance dialog. `null` = the built-in default. */
export const ACCENT_PRESETS: { label: string; value: string | null }[] = [
  { label: "Blue (default)", value: null },
  { label: "Green", value: "#3fb950" },
  { label: "Copper", value: "#c87f42" },
  { label: "Violet", value: "#a371f7" },
  { label: "Teal", value: "#2bb3c0" },
  { label: "Rose", value: "#f06f92" },
];

/** The swatch colour to render for a preset (the default renders as blue). */
export const ACCENT_DEFAULT = "#4f8cff";
