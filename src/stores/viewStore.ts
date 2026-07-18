import { create } from "zustand";

export type MainView = "schematic" | "pcb" | "bom";

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
}

export const useViewStore = create<ViewState>((set) => ({
  view: "schematic",
  setView: (view) => set({ view }),
  fullscreen: false,
  setFullscreen: (fullscreen) => set({ fullscreen }),
  toggleFullscreen: () => set((s) => ({ fullscreen: !s.fullscreen })),
}));
