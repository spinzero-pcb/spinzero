import { create } from "zustand";
import { ipc, onAgentEvent, type AgentEvent } from "../lib/ipc";
import { currentBomProfile } from "./bomCheckStore";
import { useBomMappingStore } from "./bomMappingStore";
import { useReviewInboxStore } from "./reviewInboxStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";

// A detailed review run by the user's own AI assistant, over MCP.
//
// The shape of this store is the shape of the surface: **we do not run this review.**
// SpinZero spawns the assistant against its own MCP server, the assistant's model
// does the reasoning on the user's own subscription, and the findings come back
// through the review drop-box like every other review that ran outside this window.
//
// What follows from that, and why this is not just another copy of
// `detailedReviewStore`:
//
// * **No stages, no percentage.** Our own pipeline reports stage-by-stage progress
//   because we are running it. Here the honest signal is the assistant's own last
//   line, and inventing a progress bar over somebody else's loop would be a
//   decoration, not information.
// * **Finishing is not ingesting.** The assistant writes `findings.json` into
//   `<project>/reviews/inbox/`; the user still imports it. So `finished` refreshes
//   the inbox and points at it rather than filing anything.
// * **Nothing is persisted.** A run is a subprocess; when it is gone it is gone, and
//   the findings live in the drop-box, which is the durable part.

export type AgentPhase = "idle" | "starting" | "running" | "done" | "failed";

interface AgentReviewState {
  phase: AgentPhase;
  /** The assistant's most recent line, for the status bar. Never a finding. */
  line: string;
  error: string | null;
  /** Wall clock of the last completed run, for the "took Ns" note. */
  seconds: number | null;
  start: () => Promise<void>;
  /** Subscribe to `agent-event`. Called once from the shell; returns the unsubscribe. */
  subscribe: () => Promise<() => void>;
  /** Ask the backend whether a run survived a window reload. */
  refresh: () => Promise<void>;
  clearError: () => void;
}

export const useAgentReviewStore = create<AgentReviewState>((set, get) => ({
  phase: "idle",
  line: "",
  error: null,
  seconds: null,

  clearError: () => set({ error: null }),

  start: async () => {
    if (get().phase === "starting" || get().phase === "running") return;
    const config = useSettingsStore.getState().agentReview;
    if (!config) {
      set({
        phase: "failed",
        error: "tell SpinZero how to start its MCP server first, below.",
      });
      return;
    }
    const profile = currentBomProfile();
    // The same gate the other two tiers use: never spend a review on a column mapping
    // nobody has looked at. The dialog takes over and re-enters here once approved.
    const approved = await useBomMappingStore.getState().ensureApproved(profile, () => void get().start());
    if (!approved) return;

    set({ phase: "starting", error: null, line: "", seconds: null });
    try {
      await ipc.startAgentReview(profile, config);
      set({ phase: "running" });
    } catch (e) {
      set({ phase: "failed", error: e instanceof Error ? e.message : String(e) });
    }
  },

  refresh: async () => {
    try {
      const running = await ipc.agentReviewRunning();
      if (running && get().phase === "idle") set({ phase: "running" });
    } catch {
      // The backend not answering is not something to put in front of anyone; the
      // launcher simply offers to start a review, and a second one is refused there.
    }
  },

  subscribe: async () => {
    return onAgentEvent((ev: AgentEvent) => {
      switch (ev.kind) {
        case "started":
          set({ phase: "running", line: `Handed the review to ${ev.assistant}`, error: null });
          break;
        case "progress":
          // Last line wins. The assistant narrates at its own pace and a transcript
          // in the status bar helps nobody; what the user needs is a sign of life.
          set({ line: ev.line.slice(0, 200) });
          break;
        case "finished": {
          set({ phase: "done", seconds: ev.seconds, line: "" });
          // The findings are in the drop-box, not in the app. Refresh the inbox so the
          // launcher shows the row, and say where to click.
          void useReviewInboxStore
            .getState()
            .load()
            .then(() => {
              const waiting = useReviewInboxStore.getState().entries.length;
              useToastStore.getState().push({
                kind: waiting ? "info" : "error",
                title: waiting ? "Your assistant finished the review" : "Your assistant finished, with nothing to import",
                message: waiting
                  ? `Import it from "Run a review" to see the findings as review comments.`
                  : "No findings document reached the review inbox. The assistant may have stopped early; its output is in the app log.",
              });
            });
          break;
        }
        case "failed":
          set({ phase: "failed", error: ev.detail, line: "" });
          useToastStore.getState().push({
            kind: "error",
            title: "The assistant review did not finish",
            message: ev.detail,
          });
          break;
      }
    });
  },
}));

/** Is a run in flight? Same predicate shape as `isRunning` in detailedReviewStore, so
 *  the launcher can treat all three tiers alike. */
export function isAgentRunning(phase: AgentPhase): boolean {
  return phase === "starting" || phase === "running";
}
