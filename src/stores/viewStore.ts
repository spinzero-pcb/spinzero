import { create } from "zustand";
import { useSettingsStore } from "./settingsStore";

export type MainView = "schematic" | "pcb" | "bom";

// ---------------------------------------------------------------------------
// BOM persistence (docs/storage-model.md §"UI state"). All of it lives in the
// machine-local ui_settings.json; the split is over WHO OWNS THE IDENTIFIERS:
//   • preset / hidden columns / sort / widths → ProjectUi, keyed by project dir.
//     These name things the PROJECT defines (KiCad BOM presets and their column
//     ids), so an app-global store bleeds one project's choices into another that
//     happens to share a preset name.
//   • chips → top-level UiSettings. The chip ids are the app's own, so they are
//     genuinely global.
// localStorage keys below are read once to migrate, then deleted — nothing in this
// store writes to that tier any more.
// ---------------------------------------------------------------------------

/** The BOM tab's quick-filter chips. `changedOnly` is inert unless diff mode is active. */
export type BomChip = "dnpOnly" | "missingMpn" | "changedOnly";
export type BomChips = Record<BomChip, boolean>;

const DEFAULT_CHIPS: BomChips = { dnpOnly: false, missingMpn: false, changedOnly: true };

/** Validate a stored chip map. Settings are hand-editable, so nothing is trusted.
 *  `changedOnly` is opt-OUT: absent (a fresh install, a hand-trimmed file, or a
 *  settings object written before chips existed) must read as ON, because a
 *  comparison is about the lines it changed. Only an explicit `false` turns it off. */
function sanitizeChips(v: unknown): BomChips {
  if (!v || typeof v !== "object") return { ...DEFAULT_CHIPS };
  const p = v as Partial<Record<BomChip, unknown>>;
  return {
    dnpOnly: p.dnpOnly === true,
    missingMpn: p.missingMpn === true,
    changedOnly: p.changedOnly !== false,
  };
}

/** BOM table layout the user picked, all of it per-project: the active KiCad BOM preset
 *  ("" = the built-in Default column set, null = never chose one, so the project's own
 *  default wins), which columns they hid per preset, the sort, and the dragged widths. */
export interface BomLayout {
  preset: string | null;
  /** preset name ("" for Default) → hidden column ids. */
  hidden: Record<string, string[]>;
  sort: { key: string; dir: 1 | -1 } | null;
  /** preset name ("" for Default) → column id → pixel width the user dragged to.
   *  Empty/absent = auto layout (the browser sizes the columns). */
  widths: Record<string, Record<string, number>>;
}

const EMPTY_LAYOUT: BomLayout = { preset: null, hidden: {}, sort: null, widths: {} };

/** Pre-ProjectUi keys, read once to migrate and then removed. Both can be dropped
 *  outright once no install predates the move to ui_settings.json. */
const LEGACY_LAYOUT_KEY = "bom.layout";
const LEGACY_CHIPS_KEY = "bom.chips";

function readJson(key: string): unknown {
  try {
    const raw = localStorage.getItem(key);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null; // absent, blocked, or corrupt — all mean "nothing remembered"
  }
}

/** Hostile/older payloads: keep only finite positive numbers, so a corrupt entry
 *  can't collapse a column to 0px. */
export function sanitizeWidths(v: unknown): Record<string, Record<string, number>> {
  const out: Record<string, Record<string, number>> = {};
  if (!v || typeof v !== "object") return out;
  for (const [preset, cols] of Object.entries(v as Record<string, unknown>)) {
    if (!cols || typeof cols !== "object") continue;
    const clean: Record<string, number> = {};
    for (const [id, w] of Object.entries(cols as Record<string, unknown>)) {
      if (typeof w === "number" && Number.isFinite(w) && w > 0) clean[id] = w;
    }
    out[preset] = clean;
  }
  return out;
}

function sanitizeHidden(v: unknown): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  if (!v || typeof v !== "object") return out;
  for (const [k, cols] of Object.entries(v as Record<string, unknown>)) {
    if (Array.isArray(cols)) out[k] = cols.filter((x): x is string => typeof x === "string");
  }
  return out;
}

function sanitizeSort(v: unknown): { key: string; dir: 1 | -1 } | null {
  if (!v || typeof v !== "object") return null;
  const s = v as { key?: unknown; dir?: unknown };
  if (typeof s.key !== "string") return null;
  return s.dir === -1 ? { key: s.key, dir: -1 } : { key: s.key, dir: 1 };
}

// Which content fills the main area. The schematic canvas stays mounted across
// view switches (display:none, not unmount) so its camera and highlight state
// survive a round-trip through PCB/BOM.
interface ViewState {
  view: MainView;
  setView: (v: MainView) => void;
  /** Full-screen mode collapses the left rail (activity bar + review/changes
   *  panel) so the canvas gets the width. The right panel stays. */
  fullscreen: boolean;
  setFullscreen: (v: boolean) => void;
  toggleFullscreen: () => void;
  /** BOM tab quick-filter chips (persisted app-wide in UiSettings). */
  bomChips: BomChips;
  toggleBomChip: (chip: BomChip) => void;
  setBomChip: (chip: BomChip, on: boolean) => void;
  /** BOM table preset / hidden columns / sort / widths, all per project. */
  bomLayout: BomLayout;
  setBomPreset: (preset: string) => void;
  toggleBomColumn: (preset: string, colId: string) => void;
  setBomSort: (sort: { key: string; dir: 1 | -1 }) => void;
  /** Replace the dragged column widths of one preset (all columns at once — a resize
   *  pins every column, otherwise the untouched ones would reflow). Called once per
   *  completed drag (BomTab's endResize), not per pointer-move. */
  setBomColWidths: (preset: string, widths: Record<string, number>) => void;
  /** The project whose BOM layout is loaded — the key `persistLayout` writes under.
   *  Nothing renders from it. */
  bomProjectDir: string | null;
  /** Adopt the BOM state saved for this project + the app-global chips. Called on
   *  project open, like netClassStore.hydrate. */
  hydrateBom: (projectDir: string | null) => Promise<void>;
}

export const useViewStore = create<ViewState>((set, get) => ({
  view: "schematic",
  setView: (view) => set({ view }),
  fullscreen: false,
  setFullscreen: (fullscreen) => set({ fullscreen }),
  toggleFullscreen: () => set((s) => ({ fullscreen: !s.fullscreen })),

  // Defaults until hydrateBom lands (it runs on project open, long before the BOM
  // tab can be reached).
  bomChips: { ...DEFAULT_CHIPS },
  bomLayout: { ...EMPTY_LAYOUT },
  bomProjectDir: null,

  toggleBomChip: (chip) => get().setBomChip(chip, !get().bomChips[chip]),
  setBomChip: (chip, on) => {
    const bomChips = { ...get().bomChips, [chip]: on };
    set({ bomChips });
    void useSettingsStore.getState().setBomChips(bomChips);
  },

  setBomPreset: (preset) => {
    set((s) => ({ bomLayout: { ...s.bomLayout, preset } }));
    persistLayout();
  },

  toggleBomColumn: (preset, colId) => {
    set((s) => {
      const cur = s.bomLayout.hidden[preset] ?? [];
      const next = cur.includes(colId) ? cur.filter((c) => c !== colId) : [...cur, colId];
      return { bomLayout: { ...s.bomLayout, hidden: { ...s.bomLayout.hidden, [preset]: next } } };
    });
    persistLayout();
  },

  setBomSort: (sort) => {
    set((s) => ({ bomLayout: { ...s.bomLayout, sort } }));
    persistLayout();
  },

  setBomColWidths: (preset, widths) => {
    set((s) => ({
      bomLayout: { ...s.bomLayout, widths: { ...s.bomLayout.widths, [preset]: widths } },
    }));
    persistLayout();
  },

  hydrateBom: async (projectDir) => {
    // Settings may not have been read yet on a startup that restores a project.
    const settings = useSettingsStore.getState();
    if (!settings.loaded) {
      try {
        await settings.load();
      } catch {
        /* fall back to the defaults below */
      }
    }
    const st = useSettingsStore.getState();

    // Chips: saved value wins; otherwise adopt the pre-settings localStorage blob once
    // so an existing install doesn't silently lose its filter set. The key is dropped
    // only after the settings write lands, so a failed write retries next launch.
    const legacyChips = st.bomChips === null ? readJson(LEGACY_CHIPS_KEY) : null;
    const bomChips = sanitizeChips(st.bomChips ?? legacyChips);
    if (legacyChips) {
      void st.setBomChips(bomChips).then(() => localStorage.removeItem(LEGACY_CHIPS_KEY));
    }

    // Layout: same one-shot migration. The legacy blob was app-global, so the first
    // project opened after the upgrade inherits it — which is also what un-bleeds it,
    // since every project writes its own copy from then on.
    const ui = projectDir ? st.projectUi[projectDir] : undefined;
    const saved =
      ui?.bom_preset !== undefined ||
      ui?.bom_hidden !== undefined ||
      ui?.bom_sort !== undefined ||
      ui?.bom_widths !== undefined;
    const legacy = saved ? null : (readJson(LEGACY_LAYOUT_KEY) as Partial<BomLayout> | null);
    const src = legacy ?? {
      preset: ui?.bom_preset,
      hidden: ui?.bom_hidden,
      sort: ui?.bom_sort,
      widths: ui?.bom_widths,
    };

    const bomLayout: BomLayout = {
      // A layout written before presets existed has no preset string: treat that as
      // "never chose" so the project's default can apply.
      preset: typeof src.preset === "string" ? src.preset : null,
      hidden: sanitizeHidden(src.hidden),
      sort: sanitizeSort(src.sort),
      widths: sanitizeWidths(src.widths),
    };
    set({ bomChips, bomLayout, bomProjectDir: projectDir });
    if (legacy && projectDir) {
      // The blob is app-global: keep it until a project has adopted it, then drop it.
      void persistLayout()?.then(() => localStorage.removeItem(LEGACY_LAYOUT_KEY));
    }
  },
}));

/** Write preset/hidden/sort back to this project's ProjectUi. No-op with no project
 *  open (the BOM tab can't be reached then, but hydrate may run with a null dir). */
function persistLayout(): Promise<void> | undefined {
  const { bomProjectDir, bomLayout } = useViewStore.getState();
  if (!bomProjectDir) return undefined;
  return useSettingsStore.getState().setProjectUi(bomProjectDir, {
    bom_preset: bomLayout.preset,
    bom_hidden: bomLayout.hidden,
    bom_sort: bomLayout.sort,
    bom_widths: bomLayout.widths,
  });
}
