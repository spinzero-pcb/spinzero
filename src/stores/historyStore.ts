import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { PresenceEntry } from "../lib/types";

/** Ephemeral state for the history-graph overlay. The revision list + mutations live
 *  in `projectStore` (single source of truth); this only owns view state: whether the
 *  overlay is open and the show-hidden toggle, plus the soft presence feed. Compare/diff
 *  is driven from the graph again (visual-diff §3) but its state lives in `diffStore`. */
interface HistoryState {
  open: boolean;
  showHidden: boolean;
  /** Other users active recently — drives the soft fork-awareness banner. */
  presence: PresenceEntry[];
  openGraph: () => void;
  closeGraph: () => void;
  toggleHidden: () => void;
  refreshPresence: () => Promise<void>;
}

export const useHistoryStore = create<HistoryState>((set) => ({
  open: false,
  showHidden: false,
  presence: [],

  openGraph: () => set({ open: true }),
  closeGraph: () => set({ open: false }),
  toggleHidden: () => set((s) => ({ showHidden: !s.showHidden })),

  // Soft/optional: a rejected get_presence is swallowed (presence must never toast or
  // crash — it's awareness, not a feature the user invoked).
  refreshPresence: async () => {
    try {
      set({ presence: await ipc.getPresence() });
    } catch {
      /* ignore — presence is advisory */
    }
  },
}));
