import { create } from "zustand";

export type MainView = "schematic" | "pcb" | "bom";

/** The BOM tab's quick-filter chips. A remembered per-user preference
 *  (localStorage, same tier as diffStore's blink toggle) so the tab comes back
 *  the way it was left. `changedOnly` is inert unless diff mode is active. */
export type BomChip = "dnpOnly" | "missingMpn" | "changedOnly";
export type BomChips = Record<BomChip, boolean>;

const BOM_CHIPS_KEY = "bom.chips";

function loadBomChips(): BomChips {
  const empty: BomChips = { dnpOnly: false, missingMpn: false, changedOnly: true };
  try {
    const raw = localStorage.getItem(BOM_CHIPS_KEY);
    if (!raw) return empty;
    const parsed = JSON.parse(raw) as Partial<Record<BomChip, unknown>>;
    return {
      dnpOnly: parsed?.dnpOnly === true,
      missingMpn: parsed?.missingMpn === true,
      // Opt-out, not opt-in: a comparison is about the lines it changed, so the BOM tab
      // starts filtered to them unless the user has turned the chip off before.
      changedOnly: parsed?.changedOnly !== false,
    };
  } catch {
    return empty;
  }
}

/** Set one chip and remember the set. A full/blocked localStorage must never break the
 *  toggle, so the write is best-effort. */
function writeBomChips(cur: BomChips, chip: BomChip, on: boolean): { bomChips: BomChips } {
  const bomChips = { ...cur, [chip]: on };
  try {
    localStorage.setItem(BOM_CHIPS_KEY, JSON.stringify(bomChips));
  } catch {
    /* ignored */
  }
  return { bomChips };
}

/** BOM table layout the user picked: the active KiCad BOM preset ("" = the built-in
 *  Default column set, null = never chose one, so the project's own default wins), which
 *  columns they hid per preset, and the sort. Persisted in the same localStorage tier as
 *  bomChips. */
export interface BomLayout {
  preset: string | null;
  /** preset name ("" for Default) → hidden column ids. */
  hidden: Record<string, string[]>;
  sort: { key: string; dir: 1 | -1 } | null;
  /** preset name ("" for Default) → column id → pixel width the user dragged to.
   *  Empty/absent = auto layout (the browser sizes the columns). */
  widths: Record<string, Record<string, number>>;
}

const BOM_LAYOUT_KEY = "bom.layout";

function loadBomLayout(): BomLayout {
  const empty: BomLayout = { preset: null, hidden: {}, sort: null, widths: {} };
  try {
    const raw = localStorage.getItem(BOM_LAYOUT_KEY);
    if (!raw) return empty;
    const p = JSON.parse(raw) as Partial<BomLayout>;
    const hidden: Record<string, string[]> = {};
    if (p?.hidden && typeof p.hidden === "object") {
      for (const [k, v] of Object.entries(p.hidden)) {
        if (Array.isArray(v)) hidden[k] = v.filter((x): x is string => typeof x === "string");
      }
    }
    // Hostile/older payloads: keep only finite positive numbers, so a corrupt entry
    // can't collapse a column to 0px.
    const widths: Record<string, Record<string, number>> = {};
    if (p?.widths && typeof p.widths === "object") {
      for (const [preset, cols] of Object.entries(p.widths)) {
        if (!cols || typeof cols !== "object") continue;
        const clean: Record<string, number> = {};
        for (const [id, w] of Object.entries(cols)) {
          if (typeof w === "number" && Number.isFinite(w) && w > 0) clean[id] = w;
        }
        widths[preset] = clean;
      }
    }
    const sort =
      p?.sort && typeof p.sort.key === "string" && (p.sort.dir === 1 || p.sort.dir === -1)
        ? { key: p.sort.key, dir: p.sort.dir }
        : null;
    // A layout written before presets existed (or by a hide/sort save) has no preset
    // string: treat that as "never chose" so the project's default can apply.
    return { preset: typeof p?.preset === "string" ? p.preset : null, hidden, sort, widths };
  } catch {
    return empty;
  }
}

function saveBomLayout(layout: BomLayout) {
  try {
    localStorage.setItem(BOM_LAYOUT_KEY, JSON.stringify(layout));
  } catch {
    /* a full/blocked localStorage must never break the table */
  }
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
  /** BOM tab quick-filter chips (persisted). */
  bomChips: BomChips;
  toggleBomChip: (chip: BomChip) => void;
  setBomChip: (chip: BomChip, on: boolean) => void;
  /** BOM table preset / hidden columns / sort (persisted). */
  bomLayout: BomLayout;
  setBomPreset: (preset: string) => void;
  toggleBomColumn: (preset: string, colId: string) => void;
  setBomSort: (sort: { key: string; dir: 1 | -1 }) => void;
  /** Replace the dragged column widths of one preset (all columns at once — a resize
   *  pins every column, otherwise the untouched ones would reflow). */
  setBomColWidths: (preset: string, widths: Record<string, number>) => void;
}

export const useViewStore = create<ViewState>((set) => ({
  view: "schematic",
  setView: (view) => set({ view }),
  fullscreen: false,
  setFullscreen: (fullscreen) => set({ fullscreen }),
  toggleFullscreen: () => set((s) => ({ fullscreen: !s.fullscreen })),
  bomChips: loadBomChips(),
  toggleBomChip: (chip) => set((s) => writeBomChips(s.bomChips, chip, !s.bomChips[chip])),
  setBomChip: (chip, on) => set((s) => writeBomChips(s.bomChips, chip, on)),
  bomLayout: loadBomLayout(),
  setBomPreset: (preset) =>
    set((s) => {
      const bomLayout = { ...s.bomLayout, preset };
      saveBomLayout(bomLayout);
      return { bomLayout };
    }),
  toggleBomColumn: (preset, colId) =>
    set((s) => {
      const cur = s.bomLayout.hidden[preset] ?? [];
      const next = cur.includes(colId) ? cur.filter((c) => c !== colId) : [...cur, colId];
      const bomLayout = {
        ...s.bomLayout,
        hidden: { ...s.bomLayout.hidden, [preset]: next },
      };
      saveBomLayout(bomLayout);
      return { bomLayout };
    }),
  setBomSort: (sort) =>
    set((s) => {
      const bomLayout = { ...s.bomLayout, sort };
      saveBomLayout(bomLayout);
      return { bomLayout };
    }),
  setBomColWidths: (preset, widths) =>
    set((s) => {
      const bomLayout = {
        ...s.bomLayout,
        widths: { ...s.bomLayout.widths, [preset]: widths },
      };
      saveBomLayout(bomLayout);
      return { bomLayout };
    }),
}));
