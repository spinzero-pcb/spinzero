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
}));
