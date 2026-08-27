import { create } from "zustand";
import type { ReviewKindId } from "../lib/reviewCatalog";
import { useReviewRunsStore } from "./reviewRunsStore";

// UI state for the "Run a review" launcher: the footer popover and whichever
// review's setup sheet is open.
//
// It is a store rather than component state because three things open it — the
// footer button, Ctrl+R, and the BOM tab's own control — and they must all land on
// the same popover rather than each growing a copy.

interface RunLauncherState {
  /** The footer popover. */
  menuOpen: boolean;
  /** Which review's setup sheet is up; null = none. */
  setupFor: ReviewKindId | null;
  /** The "Connect your AI assistant" screen. Lives here rather than in component
   *  state because it is reachable from the launcher popover and from the BOM
   *  review's own setup sheet, and two copies would drift. */
  connectOpen: boolean;

  openMenu: () => void;
  closeMenu: () => void;
  toggleMenu: () => void;
  /** Open a review's setup sheet directly, skipping the picker (the BOM tab's door). */
  openSetup: (id: ReviewKindId) => void;
  closeSetup: () => void;
  openConnect: () => void;
  closeConnect: () => void;
}

export const useRunLauncherStore = create<RunLauncherState>((set, get) => ({
  menuOpen: false,
  setupFor: null,
  connectOpen: false,

  openMenu: () => {
    set({ menuOpen: true });
    // The picker shows "stale" per review, which is a comparison against the inputs
    // as they are NOW — so re-read them on open rather than trusting a digest taken
    // when the project was loaded.
    void useReviewRunsStore
      .getState()
      .refreshInputs()
      .catch(() => {});
  },
  closeMenu: () => set({ menuOpen: false }),
  toggleMenu: () => (get().menuOpen ? get().closeMenu() : get().openMenu()),

  openSetup: (id) => set({ menuOpen: false, setupFor: id }),
  closeSetup: () => set({ setupFor: null }),

  // Closes the popover and any open setup sheet: this screen is a full dialog, and
  // leaving a sheet behind it means dismissing one reveals the other.
  openConnect: () => set({ menuOpen: false, setupFor: null, connectOpen: true }),
  closeConnect: () => set({ connectOpen: false }),
}));
