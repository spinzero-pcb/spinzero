import { create } from "zustand";

// Measure-tool state (docs/measure-tool-plan.md). Just the mode flag, display units
// and snap preferences live here; the ephemeral measurement points + live hover are
// kept in refs inside PcbGlView (like the camera) so the rAF loop reads them without
// re-rendering. Measure mode is PCB-only and mutually exclusive with comment mode
// (the App keymap + PcbToolbar disarm one when the other turns on).

/** Display units for the readout. World coordinates are always board millimetres;
 *  the unit is a pure display scale (1 mm = 39.3701 mil; 1 in = 25.4 mm). */
export type MeasureUnit = "mm" | "mil" | "in";

const UNIT_CYCLE: MeasureUnit[] = ["mm", "mil", "in"];

interface MeasureState {
  active: boolean;
  units: MeasureUnit;
  /** Snap the free cursor to a grid (feature 9); off by default, step in mm. Geometry
   *  snapping is always on and is momentarily disabled by holding Shift (no persisted
   *  toggle — the snap-settings UI was dropped for the Shift modifier). */
  grid: boolean;
  gridStep: number;

  toggle: () => void;
  setActive: (active: boolean) => void;
  setUnits: (units: MeasureUnit) => void;
  cycleUnits: () => void;
  setGrid: (grid: boolean) => void;
  setGridStep: (gridStep: number) => void;
}

export const useMeasureStore = create<MeasureState>((set) => ({
  active: false,
  units: "mm",
  grid: false,
  gridStep: 1,

  toggle: () => set((s) => ({ active: !s.active })),
  setActive: (active) => set({ active }),
  setUnits: (units) => set({ units }),
  cycleUnits: () =>
    set((s) => ({ units: UNIT_CYCLE[(UNIT_CYCLE.indexOf(s.units) + 1) % UNIT_CYCLE.length] })),
  setGrid: (grid) => set({ grid }),
  setGridStep: (gridStep) => set({ gridStep }),
}));
