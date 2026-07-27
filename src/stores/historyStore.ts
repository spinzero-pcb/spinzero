import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { PresenceEntry } from "../lib/types";

/** Ephemeral state for the two version-control surfaces — the left-rail History panel
 *  and the full-width History view. The revision list + mutations live in `projectStore`
 *  (single source of truth); this owns only view state, shared by both surfaces so a
 *  selection/filter made in the rail survives a jump to the workspace and back.
 *
 *  There is no `open` flag any more: the modal overlay is gone. The rail surface is
 *  `reviewStore.leftTab === "history"`; the workspace is `viewStore.view === "history"`.
 *
 *  SELECTION IS NOT NAVIGATION. `selectedId` is the row the reader is inspecting (it
 *  drives the detail pane and the keyboard cursor); the revision actually rendered on
 *  the canvas is `projectStore.activeExtraction`, changed only by an explicit "open".
 *  Browsing history used to re-point the canvas on every click — it doesn't now. */
interface HistoryState {
  /** Include retracted (tombstoned) revisions in both surfaces. */
  showHidden: boolean;
  /** Free-text filter over subject / author / tag / short id. */
  query: string;
  /** Inspected row — detail target + keyboard cursor. NOT the viewed revision. */
  selectedId: string | null;
  /** Compare pick-mode: the first-picked revision, waiting for a second. */
  compareFrom: string | null;
  /** Other users active recently — drives the soft fork-awareness banner. */
  presence: PresenceEntry[];

  toggleHidden: () => void;
  setQuery: (q: string) => void;
  select: (id: string | null) => void;
  setCompareFrom: (id: string | null) => void;
  refreshPresence: () => Promise<void>;
}

export const useHistoryStore = create<HistoryState>((set) => ({
  showHidden: false,
  query: "",
  selectedId: null,
  compareFrom: null,
  presence: [],

  toggleHidden: () => set((s) => ({ showHidden: !s.showHidden })),
  setQuery: (query) => set({ query }),
  select: (selectedId) => set({ selectedId }),
  setCompareFrom: (compareFrom) => set({ compareFrom }),

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
