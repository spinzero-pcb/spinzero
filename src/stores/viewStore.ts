import { create } from "zustand";

/** Design views render a revision; "history" is the version-control workspace that
 *  browses them (a peer tab rather than a modal, so the DAG gets full width — a 180px
 *  rail can't render a fork honestly, and the reader leaves it the same way they leave
 *  the schematic). */
export type MainView = "schematic" | "pcb" | "bom" | "history";
export type DesignView = Exclude<MainView, "history">;

const isDesignView = (v: MainView): v is DesignView => v !== "history";

// Which content fills the main area. The schematic canvas stays mounted across
// view switches (display:none, not unmount) so its camera and highlight state
// survive a round-trip through PCB/BOM/History.
interface ViewState {
  view: MainView;
  setView: (v: MainView) => void;
  /** The design view to fall back to when leaving History — so "compare these two"
   *  started in the workspace lands the reader back on the board they came from
   *  instead of a hardcoded schematic. */
  prevDesignView: DesignView;
  /** Leave the History workspace for whichever design view preceded it. No-op elsewhere. */
  exitHistory: () => void;
  /** Full-screen mode collapses the left rail (activity bar + review/history panel)
   *  so the canvas gets the width. The right panel stays. */
  fullscreen: boolean;
  setFullscreen: (v: boolean) => void;
  toggleFullscreen: () => void;
}

export const useViewStore = create<ViewState>((set) => ({
  view: "schematic",
  prevDesignView: "schematic",
  setView: (view) =>
    set((s) => ({
      view,
      // Remember where we were only when leaving a design view for History; moving
      // between design views just advances the fallback to the newest one.
      prevDesignView: isDesignView(view) ? view : s.prevDesignView,
    })),
  exitHistory: () => set((s) => (s.view === "history" ? { view: s.prevDesignView } : {})),
  fullscreen: false,
  setFullscreen: (fullscreen) => set({ fullscreen }),
  toggleFullscreen: () => set((s) => ({ fullscreen: !s.fullscreen })),
}));
