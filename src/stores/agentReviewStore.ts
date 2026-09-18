import { create } from "zustand";
import { ipc, onAgentEvent, type AgentEvent, type RunStatus } from "../lib/ipc";
import { stageLabel } from "../lib/findings";
import { currentBomProfile } from "./bomCheckStore";
import { useBomMappingStore } from "./bomMappingStore";
import { useProjectStore } from "./projectStore";
import { useReviewInboxStore } from "./reviewInboxStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";

// The detailed BOM review — run by the user's own agent, over MCP.
//
// This is the ONLY detailed review now. The hosted tier is gone, and with it the
// store that posted a bundle to a service.
//
// The shape of this store is the shape of the surface: **we do not run this review.**
// SpinZero starts an agent, the agent's model does the reasoning on the user's own
// subscription, and the findings come back through the review drop-box like every
// other review that ran outside this window.
//
// **Progress is real, and it is not the agent's.** The review server writes
// `status.json` for its run; the backend watches it and forwards it here. So the bar
// moves on counts the server actually holds — parts accounted for, datasheets read —
// and not on a percentage invented over somebody else's loop. It also moves for a
// review the user started in a terminal, because the file does not care who started
// the run. The agent's own output is kept as a sign of life and never parsed.
//
// **Finishing is not ingesting.** The agent writes `findings.json` into
// `<project>/reviews/inbox/`; the user still imports it.
//
// **Nothing is persisted.** A run is a subprocess and a file on disk; the durable part
// is the drop-box.

export type AgentPhase = "idle" | "starting" | "running" | "done" | "failed";

/** How many activity lines to keep. The cap exists so a pathological run cannot grow
 *  the store without bound, and the oldest lines are the least interesting. */
const ACTIVITY_LIMIT = 500;

/** A gap this long in `updated_ts` is a stalled run, not a slow one. A watcher is not
 *  a subscription: the file stops moving when the run dies, and saying "still going"
 *  over a dead run is the one thing this surface must not do. The judgment pass can
 *  genuinely sit inside one model turn for minutes, so the threshold is generous. */
export const STALL_MS = 5 * 60_000;

export interface ActivityEntry {
  /** Sequence number, for React keys: two lines can share a millisecond. */
  seq: number;
  ts: string;
  tone: "step" | "agent" | "error";
  text: string;
}

interface AgentReviewState {
  phase: AgentPhase;
  /** The agent's most recent line. Never a finding, never parsed. */
  line: string;
  /** The review server's own account of the run, or null when none is in flight. */
  status: RunStatus | null;
  error: string | null;
  /** Wall clock of the last completed run, for the "took Ns" note. */
  seconds: number | null;
  activity: ActivityEntry[];
  start: () => Promise<void>;
  /** Subscribe to `agent-event`. Called once from the shell; returns the unsubscribe. */
  subscribe: () => Promise<() => void>;
  /** Ask the backend what it can see: a run of ours that survived a window reload, and
   *  any review of this project that somebody started elsewhere. */
  refresh: () => Promise<void>;
  clearError: () => void;
}

/** Is a run in flight? The one predicate every surface should ask. */
export function isAgentRunning(phase: AgentPhase): boolean {
  return phase === "starting" || phase === "running";
}

/** Has this run stopped reporting? Reported separately from the phase because the
 *  remedy differs: a stalled run is one the user has to go and look at. */
export function isStalled(status: RunStatus | null, now = Date.now()): boolean {
  if (!status || status.phase === "done" || status.phase === "failed") return false;
  const at = Date.parse(status.updated_ts);
  return Number.isFinite(at) && now - at > STALL_MS;
}

/**
 * How far along, as a percentage.
 *
 * Three spans, and one honest denominator in each. The middle one is the whole run as
 * far as a person waiting is concerned, so it gets almost all of the bar; the first is
 * the deterministic layer, weighted by the datasheet count because that is where its
 * time goes.
 */
export function percentOf(status: RunStatus | null): number {
  if (!status) return 0;
  if (status.phase === "done") return 100;
  if (status.phase === "preflight") return 2;
  const sheets =
    status.datasheets_total > 0 ? Math.min(1, status.datasheets_read / status.datasheets_total) : 0;
  if (status.phase === "assembling") return 97;
  if (status.parts_total > 0) {
    return 30 + 65 * Math.min(1, status.parts_done / status.parts_total);
  }
  // The deterministic layer. Datasheet collection is most of it and is the only part
  // that reports a fraction, so the span fills on that and nothing else pretends to.
  return 4 + 26 * sheets;
}

/** The line under the bar. Counts, never a part number — the status file carries none. */
export function progressLabel(status: RunStatus | null, fallback = "Starting the review"): string {
  if (!status) return fallback;
  switch (status.phase) {
    case "preflight":
      return "Waiting for the agent to confirm the setup";
    case "preparing":
      return status.datasheets_total > 0
        ? `Collecting datasheets · ${status.datasheets_read} of ${status.datasheets_total}`
        : stageLabel(status.stage) || "Preparing your BOM";
    case "step_open":
    case "assembling":
      return status.parts_total > 0
        ? `${status.parts_done} of ${status.parts_total} parts accounted for`
        : stageLabel(status.stage) || "Reviewing against datasheets";
    case "done":
      return "Finished";
    case "failed":
      return status.error ?? "The review failed";
    default:
      return fallback;
  }
}

export const useAgentReviewStore = create<AgentReviewState>((set, get) => ({
  phase: "idle",
  line: "",
  status: null,
  error: null,
  seconds: null,
  activity: [],

  clearError: () => set({ error: null }),

  start: async () => {
    if (isAgentRunning(get().phase)) return;
    const agent = useSettingsStore.getState().effectiveAgent();
    if (!agent.bin.trim()) {
      set({
        phase: "failed",
        error: "tell SpinZero which agent to run first, below.",
      });
      return;
    }
    const profile = currentBomProfile();
    // The same gate every tier used: never spend a review on a column mapping nobody
    // has looked at. The dialog takes over and re-enters here once approved.
    const approved = await useBomMappingStore.getState().ensureApproved(profile, () => void get().start());
    if (!approved) return;

    // Both preflight answers, handed over in the prompt. The server would otherwise
    // stop and ask an agent that has nobody to ask.
    const brief = {
      profile: useProjectStore.getState().project?.class ? profile : "",
      mapping: await confirmedMapping(profile),
    };

    set({ phase: "starting", error: null, line: "", seconds: null, status: null, activity: [] });
    try {
      await ipc.startAgentReview(agent, brief);
      set({ phase: "running" });
    } catch (e) {
      set({ phase: "failed", error: e instanceof Error ? e.message : String(e) });
    }
  },

  refresh: async () => {
    try {
      const [running, status] = await Promise.all([ipc.agentReviewRunning(), ipc.agentReviewStatus()]);
      // A review this window did not start is still this board's review, so it is
      // shown. That is the whole point of reading a file instead of a pipe.
      const live = status !== null && status.phase !== "done" && status.phase !== "failed";
      if (status) set({ status });
      if ((running || live) && get().phase === "idle") set({ phase: "running" });
    } catch {
      // The backend not answering is not something to put in front of anyone; the
      // launcher simply offers to start a review, and a second one is refused there.
    }
  },

  subscribe: async () => {
    return onAgentEvent((ev: AgentEvent) => {
      switch (ev.kind) {
        case "started":
          set({ phase: "running", line: `Handed the review to ${ev.agent}`, error: null });
          push(set, get, "step", `Handed the review to ${ev.agent}`);
          break;
        case "progress":
          // Last line wins for the status bar. The agent narrates at its own pace and
          // a transcript in the footer helps nobody; the feed keeps the rest.
          set({ line: ev.line.slice(0, 200) });
          push(set, get, "agent", ev.line.slice(0, 200));
          break;
        case "status": {
          const previous = get().status;
          set({ status: ev.status, phase: get().phase === "idle" ? "running" : get().phase });
          const said = describeStatus(previous, ev.status);
          if (said) push(set, get, ev.status.phase === "failed" ? "error" : "step", said);
          break;
        }
        case "finished": {
          set({ phase: "done", seconds: ev.seconds, line: "" });
          push(set, get, "step", "The agent finished");
          // The findings are in the drop-box, not in the app. Refresh the inbox so the
          // launcher shows the row, and say where to click.
          void useReviewInboxStore
            .getState()
            .load()
            .then(() => {
              const waiting = useReviewInboxStore.getState().entries.length;
              useToastStore.getState().push({
                kind: waiting ? "info" : "error",
                title: waiting ? "Your agent finished the review" : "Your agent finished, with nothing to import",
                message: waiting
                  ? `Import it from "Run a review" to see the findings as review comments.`
                  : "No findings document reached the review inbox. The agent may have stopped early; its output is in the app log.",
              });
            });
          break;
        }
        case "failed":
          set({ phase: "failed", error: ev.detail, line: "" });
          push(set, get, "error", ev.detail);
          useToastStore.getState().push({
            kind: "error",
            title: "The review did not finish",
            message: ev.detail,
          });
          break;
      }
    });
  },
}));

type Setter = (partial: Partial<AgentReviewState>) => void;

function push(set: Setter, get: () => AgentReviewState, tone: ActivityEntry["tone"], text: string): void {
  const activity = get().activity;
  const seq = (activity[activity.length - 1]?.seq ?? 0) + 1;
  set({
    activity: [...activity.slice(-(ACTIVITY_LIMIT - 1)), { seq, ts: new Date().toISOString(), tone, text }],
  });
}

/**
 * What changed since the last status, in one line — or nothing.
 *
 * The file is rewritten every few seconds and most rewrites move a counter by one.
 * A feed with a row for each of those buries the two rows that matter: a stage
 * starting, and a phase turning over.
 */
function describeStatus(previous: RunStatus | null, next: RunStatus): string | null {
  if (previous?.phase !== next.phase) {
    switch (next.phase) {
      case "preparing":
        return "The deterministic layer is running";
      case "step_open":
        return next.parts_total > 0 ? `Reviewing ${next.parts_total} parts` : "Reviewing";
      case "assembling":
        return "Assembling the report";
      case "done":
        return next.report_path ? `Report written to ${next.report_path}` : "The review is complete";
      case "failed":
        return `The review failed: ${next.error ?? "no reason given"}`;
      default:
        return null;
    }
  }
  if (previous.stage !== next.stage && next.stage) return stageLabel(next.stage);
  return null;
}

/**
 * The column mapping the user has already approved, as the review server's own field
 * names.
 *
 * Sent as overrides rather than left to the server's resolver, because the user has
 * answered this question once already and a second guess that disagrees with the app's
 * own BOM tab is the confusing outcome. A failure here is not fatal: the server falls
 * back to its own resolution, which is what an agent in a chat window gets anyway.
 */
async function confirmedMapping(profile: string): Promise<{ field: string; column: string }[]> {
  try {
    const view = await ipc.getBomMapping(profile);
    if (!view.approved) return [];
    return view.fields
      .filter((f) => f.overridden)
      .map((f) => ({ field: f.logical, column: f.column }));
  } catch {
    return [];
  }
}
