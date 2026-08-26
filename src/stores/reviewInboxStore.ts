import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { ReviewInboxEntry } from "../lib/findings";
import { landFindings } from "./landFindings";
import { useToastStore } from "./toastStore";

// The review drop-box, from the app's side.
//
// A detailed review no longer has to run inside this window. The engine CLI can run
// one on this machine, and — the point of the whole MCP harness — so can the user's
// own coding agent, on their own model subscription, with the design never leaving
// the disk. Neither of those has a window to hand findings to, so they write
// `findings-1.0.json` into `<project>/reviews/inbox/` and the app picks it up.
//
// Three decisions worth keeping:
//
// * **Importing is a click, never automatic.** The backend refuses to let the folder
//   watcher do it (see `bomcheck::inbox_dir`): filing comments into someone's project
//   the instant a file appears is not a convenience. What the app does by itself is
//   NOTICE — the launcher shows what is waiting.
// * **A file that cannot be imported is shown, not skipped.** A review the user
//   believes ran and cannot find anywhere is worse than an error line.
// * **Nothing here is persisted.** The drop-box is the state; this store is a view of
//   it, refreshed whenever the launcher opens.

interface ReviewInboxState {
  entries: ReviewInboxEntry[];
  /** Name currently being imported, so its row can show it and the others stay live. */
  importing: string | null;
  /** Why the last listing failed. Absent on the common path — a missing drop-box is
   *  an empty list, not an error. */
  error: string | null;
  load: () => Promise<void>;
  importOne: (name: string) => Promise<void>;
}

export const useReviewInboxStore = create<ReviewInboxState>((set, get) => ({
  entries: [],
  importing: null,
  error: null,

  async load() {
    try {
      set({ entries: await ipc.listReviewInbox(), error: null });
    } catch (e) {
      // Listing is a background convenience; a failure must not put an error in front
      // of a user who was doing something else. It is recorded for the row to show.
      set({ entries: [], error: e instanceof Error ? e.message : String(e) });
    }
  },

  async importOne(name) {
    if (get().importing) return;
    set({ importing: name });
    try {
      const outcome = await ipc.importReviewInbox(name);
      // Same landing as a hosted review: the strip, the rail, the launcher stamp and
      // one toast that says so if the run came back incomplete.
      await landFindings({ doc: outcome.findings, outcome, label: "Imported review" });
    } catch (e) {
      useToastStore.getState().push({
        kind: "error",
        title: "Could not import the review",
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      set({ importing: null });
      // The imported file has been archived out of the drop-box, so re-listing is what
      // removes the row — and it is also how a failed import keeps its row.
      await get().load();
    }
  },
}));
