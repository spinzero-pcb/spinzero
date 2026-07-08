import { create } from "zustand";

export type MainView = "schematic" | "pcb" | "bom";

// Which content fills the main area. The schematic canvas stays mounted across
// view switches (display:none, not unmount) so its camera and highlight state
// survive a round-trip through PCB/BOM.
interface ViewState {
  view: MainView;
  setView: (v: MainView) => void;
}

export const useViewStore = create<ViewState>((set) => ({
  view: "schematic",
  setView: (view) => set({ view }),
}));
