import { create } from "zustand";

export type MainView = "schematic" | "pcb" | "bom";

/** The BOM tab's quick-filter chips. A remembered per-user preference
 *  (localStorage, same tier as diffStore's blink toggle) so the tab comes back
 *  the way it was left. `changedOnly` is inert unless diff mode is active. */
export type BomChip = "dnpOnly" | "missingMpn" | "changedOnly";
export type BomChips = Record<BomChip, boolean>;

const BOM_CHIPS_KEY = "bom.chips";

function loadBomChips(): BomChips {
  const empty: BomChips = { dnpOnly: false, missingMpn: false, changedOnly: false };
  try {
    const raw = localStorage.getItem(BOM_CHIPS_KEY);
    if (!raw) return empty;
    const parsed = JSON.parse(raw) as Partial<Record<BomChip, unknown>>;
    return {
      dnpOnly: parsed?.dnpOnly === true,
      missingMpn: parsed?.missingMpn === true,
      changedOnly: parsed?.changedOnly === true,
    };
  } catch {
    return empty;
  }
}

/** BOM table layout the user picked: the active KiCad BOM preset ("" = the built-in
 *  Default column set), which columns they hid per preset, and the sort. Persisted in the
 *  same localStorage tier as bomChips. */
export interface BomLayout {
  preset: string;
  /** preset name ("" for Default) → hidden column ids. */
  hidden: Record<string, string[]>;
  sort: { key: string; dir: 1 | -1 } | null;
}

const BOM_LAYOUT_KEY = "bom.layout";

function loadBomLayout(): BomLayout {
  const empty: BomLayout = { preset: "", hidden: {}, sort: null };
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
    const sort =
      p?.sort && typeof p.sort.key === "string" && (p.sort.dir === 1 || p.sort.dir === -1)
        ? { key: p.sort.key, dir: p.sort.dir }
        : null;
    return { preset: typeof p?.preset === "string" ? p.preset : "", hidden, sort };
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
  /** BOM table preset / hidden columns / sort (persisted). */
  bomLayout: BomLayout;
  setBomPreset: (preset: string) => void;
  toggleBomColumn: (preset: string, colId: string) => void;
  setBomSort: (sort: { key: string; dir: 1 | -1 }) => void;
}

export const useViewStore = create<ViewState>((set) => ({
  view: "schematic",
  setView: (view) => set({ view }),
  fullscreen: false,
  setFullscreen: (fullscreen) => set({ fullscreen }),
  toggleFullscreen: () => set((s) => ({ fullscreen: !s.fullscreen })),
  bomChips: loadBomChips(),
  toggleBomChip: (chip) =>
    set((s) => {
      const bomChips = { ...s.bomChips, [chip]: !s.bomChips[chip] };
      try {
        localStorage.setItem(BOM_CHIPS_KEY, JSON.stringify(bomChips));
      } catch {
        /* a full/blocked localStorage must never break the toggle */
      }
      return { bomChips };
    }),
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
}));
