import { create } from "zustand";
import type { Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ipc } from "../lib/ipc";

// Auto-update UI state (batch1). The old flow surfaced a transient "Relaunch to update"
// toast; an update only applies on the user's nod, so the prompt must persist until they
// take it. This store holds the downloaded build; UpdateBanner renders it pinned to the
// bottom of the left rail (no dismiss control, no changelog — just the version).
type UpdatePhase = "idle" | "ready" | "installing" | "error";

interface UpdateState {
  phase: UpdatePhase;
  /** Version of the downloaded build (e.g. "0.0.3"), once ready. */
  version: string | null;
  /** Failure text — only set when phase === "error". */
  error: string | null;
  /** The downloaded Update handle, held so apply() can install it on the user's nod. */
  update: Update | null;
  /** Flag that a signed build was downloaded and is ready to apply. */
  setReady: (update: Update) => void;
  /** Install a downloaded build (headless — installMode "quiet") and relaunch. Returns
   *  false if the install/relaunch failed (so a launch-time auto-apply can fall back to
   *  just offering the banner); never returns on success — relaunch ends the process. */
  applyNow: (update: Update) => Promise<boolean>;
  /** Banner button: install the build held in the store, on the user's nod. */
  apply: () => Promise<void>;
}

export const useUpdateStore = create<UpdateState>((set, get) => ({
  phase: "idle",
  version: null,
  error: null,
  update: null,

  setReady: (update) =>
    set({ phase: "ready", version: update.version, error: null, update }),

  applyNow: async (update) => {
    if (get().phase === "installing") return false;
    set({ phase: "installing", version: update.version, update, error: null });
    try {
      await update.install();
      await relaunch(); // restarts into the new build — nothing below runs on success
      return true;
    } catch (e) {
      // A failed install IS a real error (unlike a routine offline check) — capture it.
      void ipc.logError(`update install/relaunch failed: ${String(e)}`);
      set({ phase: "error", error: String(e) });
      return false;
    }
  },

  apply: async () => {
    const { update } = get();
    if (!update) return;
    await get().applyNow(update);
  },
}));
