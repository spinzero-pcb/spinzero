import { create } from "zustand";
import { ipc, onAgentEvent, type AgentEvent, type OpenStepStatus, type RunStatus } from "../lib/ipc";
import { explainFailure } from "../lib/agentFailure";
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
// **Cancel stops the spending.** It kills the agent's whole process tree, so its
// sub-agents stop too and no more tokens go out. Nothing is imported, and the review
// server's status file for that run is ignored from then on: it still says "running",
// because nobody told it otherwise. Only a run this window started can be cancelled;
// one started in the user's own terminal is theirs to stop.
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
  /** When this window started the run, for the elapsed clock. Null for a run
   *  somebody started elsewhere, which this window cannot time or cancel. */
  startedAt: number | null;
  /** When the open step began, as this window saw it: the first `step_open` status, or
   *  the status where `steps_done` last changed. Null when no step is open. It is the
   *  app's clock, not the server's; the status file does not say when a step began. */
  stepStartedAt: number | null;
  /** Review ids whose status file must be ignored: runs the user cancelled. */
  cancelledIds: string[];
  start: () => Promise<void>;
  /** Stop the run this window started. Nothing is imported. */
  cancel: () => Promise<void>;
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
 * Where each stage sits on the bar, as [start, end] percentages.
 *
 * The part batches are most of the wall clock, so they get most of the bar. The
 * deterministic layer fills on the datasheet count, because that is where its time
 * goes. The board step is one step with no count inside it, so it holds one value.
 */
export const PROGRESS_SPANS = {
  /** Waiting for the agent to confirm the setup. */
  preflight: [0, 2],
  /** The deterministic layer: rules, distributor lookups, datasheets. A short stage. */
  preparing: [4, 15],
  /** The part batches, filled on parts accounted for. Most of the run. */
  batches: [15, 88],
  /** The whole-board step. */
  board: [88, 95],
  /** Assembling the report, then done. */
  assembling: [95, 100],
} as const;

/** The step id of the whole-board step, as the server names it. */
const BOARD_STEP = "board_review";

/** True when the whole-board step is the one open. An older server lists no open
 *  steps, and there the board step is the step open after every part is accounted for. */
function boardStepOpen(status: RunStatus): boolean {
  if (status.open_steps?.length) return status.open_steps.some((o) => o.step === BOARD_STEP);
  return status.phase === "step_open" && status.parts_total > 0 && status.parts_done >= status.parts_total;
}

/**
 * How far along, as a percentage. See `PROGRESS_SPANS` for the split.
 *
 * Parts move only when a batch is submitted, so with three batches running the bar
 * moves in steps. That is honest: a batch half read has accounted for nothing yet.
 */
export function percentOf(status: RunStatus | null): number {
  if (!status) return 0;
  if (status.phase === "done") return 100;
  if (status.phase === "preflight") return PROGRESS_SPANS.preflight[1];
  const partsLeft = status.parts_total > 0 && status.parts_done < status.parts_total;
  // An older server says "assembling" whenever no step is open, including the wait
  // between two batches. Parts still owed means the run is not assembling yet.
  if (status.phase === "assembling" && !partsLeft) return 97;
  if (boardStepOpen(status)) return 90;
  if (status.parts_total > 0) {
    const [from, to] = PROGRESS_SPANS.batches;
    return from + (to - from) * Math.min(1, status.parts_done / status.parts_total);
  }
  const sheets =
    status.datasheets_total > 0 ? Math.min(1, status.datasheets_read / status.datasheets_total) : 0;
  const [from, to] = PROGRESS_SPANS.preparing;
  return from + (to - from) * sheets;
}

/** "Step 4" for a batch, "Board check" for the whole-board step. */
export function stepName(step: Pick<OpenStepStatus, "step" | "index">): string {
  return step.step === BOARD_STEP ? "Board check" : `Step ${step.index}`;
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
      return stepLabel(status);
    case "assembling":
      if (status.parts_total > 0 && status.parts_done < status.parts_total) {
        return `Waiting for the next batch · ${status.parts_done} of ${status.parts_total} parts`;
      }
      return status.parts_total > 0
        ? `Assembling the report · ${status.parts_done} of ${status.parts_total} parts`
        : "Assembling the report";
    case "done":
      return "Finished";
    case "failed":
      return status.error ?? "The review failed";
    default:
      return fallback;
  }
}

/**
 * "Step 3 of about 9 · 40 of 88 parts".
 *
 * The total is "about" because the server forms batches as datasheets become ready, so
 * `steps_total` can grow during the run. Only the last step is certain: it is the
 * whole-board step, and it opens when every part is accounted for.
 */
function stepLabel(status: RunStatus): string {
  const total = status.steps_total;
  const parts = status.parts_total > 0 ? `${status.parts_done} of ${status.parts_total} parts` : "";
  const open = status.open_steps ?? [];
  if (open.length) {
    const top = Math.max(total, ...open.map((o) => o.index));
    if (open.some((o) => o.step === BOARD_STEP)) return `Step ${top} of ${top} · board-level check`;
    const indices = open.map((o) => o.index).sort((a, b) => a - b);
    const head =
      indices.length === 1
        ? `Step ${indices[0]} of about ${top}`
        : `Steps ${indices.slice(0, -1).join(", ")} and ${indices[indices.length - 1]} of about ${top}`;
    return parts ? `${head} · ${parts}` : head;
  }
  if (total <= 0) return parts || stageLabel(status.stage) || "Reviewing against datasheets";
  const step = Math.min(status.steps_done + 1, total);
  if (step === total && status.parts_done === status.parts_total) {
    return `Step ${total} of ${total} · board-level check`;
  }
  const head = step === total ? `Step ${step} of ${total}` : `Step ${step} of about ${total}`;
  return parts ? `${head} · ${parts}` : head;
}

/** "45s" or "7m12s". Short, so it fits in a feed row. */
export function formatDuration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
}

export const useAgentReviewStore = create<AgentReviewState>((set, get) => ({
  phase: "idle",
  line: "",
  status: null,
  error: null,
  seconds: null,
  activity: [],
  startedAt: null,
  stepStartedAt: null,
  cancelledIds: [],

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
    // The same gate the instant check uses: never spend a review on a column mapping
    // nobody has looked at.
    const approved = await useBomMappingStore.getState().ensureApproved(profile);
    if (!approved) return;

    // Both preflight answers, handed over in the prompt. The server would otherwise
    // stop and ask an agent that has nobody to ask.
    const brief = {
      profile: useProjectStore.getState().project?.class ? profile : "",
      mapping: await confirmedMapping(profile),
    };

    set({
      phase: "starting",
      error: null,
      line: "",
      seconds: null,
      status: null,
      activity: [],
      startedAt: Date.now(),
      stepStartedAt: null,
    });
    try {
      await ipc.startAgentReview(agent, brief);
      set({ phase: "running" });
    } catch (e) {
      set({ phase: "failed", startedAt: null, error: e instanceof Error ? e.message : String(e) });
    }
  },

  cancel: async () => {
    try {
      const stopped = await ipc.cancelAgentReview();
      if (!stopped) {
        useToastStore.getState().push({
          kind: "warning",
          title: "Nothing to cancel here",
          message: "This review was started outside SpinZero. Stop it in the AI agent that runs it.",
        });
        return;
      }
      // The `cancelled` event resets the phase. Mark the run now so a status write
      // that lands before the event cannot bring the bar back.
      const id = get().status?.review_id;
      if (id) set({ cancelledIds: [...get().cancelledIds, id] });
    } catch (e) {
      useToastStore.getState().push({
        kind: "error",
        title: "Could not cancel the review",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  },

  refresh: async () => {
    try {
      const [running, status] = await Promise.all([ipc.agentReviewRunning(), ipc.agentReviewStatus()]);
      // A review this window did not start is still this board's review, so it is
      // shown. That is the whole point of reading a file instead of a pipe.
      const ignored = status !== null && get().cancelledIds.includes(status.review_id);
      // A stalled status file is a dead run, not a live one. A cancelled or killed run
      // leaves its file at "step_open" forever, and the server now touches the file on
      // every tool call, so five quiet minutes means nobody is working it. Counting it
      // as live blocked every new review after a cancel and a window reload.
      const live =
        !ignored && status !== null && status.phase !== "done" && status.phase !== "failed" && !isStalled(status);
      if (status && !ignored) set({ status });
      if ((running || live) && get().phase === "idle") set({ phase: "running" });
      // A run somebody else started, which has since died. This window cannot cancel it
      // and must not wait for it.
      if (!running && !live && get().phase === "running" && get().startedAt === null) {
        set({ phase: "idle", status: null, line: "" });
      }
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
          if (get().cancelledIds.includes(ev.status.review_id)) break;
          const previous = get().status;
          const now = Date.now();
          // A new run, or a step turned over: the step clock starts again. So does a
          // step that is open but was never timed, such as the first `step_open`.
          const sameRun = previous?.review_id === ev.status.review_id;
          const turned = sameRun && previous.steps_done !== ev.status.steps_done;
          const done = stepDone(previous, ev.status);
          let stepStartedAt = sameRun ? get().stepStartedAt : null;
          if (turned) stepStartedAt = now;
          if (ev.status.phase !== "step_open") stepStartedAt = null;
          else if (stepStartedAt === null) stepStartedAt = now;
          set({ status: ev.status, stepStartedAt, phase: get().phase === "idle" ? "running" : get().phase });
          for (const line of done) push(set, get, "step", line);
          const said = describeStatus(previous, ev.status);
          if (said) push(set, get, ev.status.phase === "failed" ? "error" : "step", said);
          break;
        }
        case "finished": {
          set({ phase: "done", seconds: ev.seconds, line: "", startedAt: null, stepStartedAt: null });
          push(set, get, "step", "The agent finished");
          // The findings are in the drop-box, not in the app. Refresh the inbox so the
          // launcher shows the row, and say where to click.
          void useReviewInboxStore
            .getState()
            .load()
            .then(() => {
              if (useReviewInboxStore.getState().entries.length) {
                useToastStore.getState().push({
                  kind: "info",
                  title: "Your agent finished the review",
                  message: `Import it from "Run a review" to see the findings as review comments.`,
                });
                return;
              }
              // A clean exit with nothing in the inbox is a failure. An agent exits 0
              // when a tool call is denied, so its last line is the reason, and it
              // gets the same advice as a run that exited with an error.
              const agent = useSettingsStore.getState().effectiveAgent().label;
              fail(set, get, ev.last_line?.trim() || `${agent} finished without sending any findings, and gave no reason.`);
            });
          break;
        }
        case "failed":
          fail(set, get, ev.detail);
          break;
        case "cancelled": {
          const id = get().status?.review_id;
          set({
            phase: "idle",
            status: null,
            line: "",
            error: null,
            startedAt: null,
            stepStartedAt: null,
            cancelledIds: id && !get().cancelledIds.includes(id) ? [...get().cancelledIds, id] : get().cancelledIds,
          });
          push(set, get, "step", "Cancelled");
          useToastStore.getState().push({ kind: "info", title: "Review cancelled", message: "Nothing was imported." });
          break;
        }
      }
    });
  },
}));

type Setter = (partial: Partial<AgentReviewState>) => void;

/** Mark the run failed, and toast the advice for `detail`. The raw line stays in the
 *  footer's Review failed panel; the toast says what to do. */
function fail(set: Setter, get: () => AgentReviewState, detail: string): void {
  set({ phase: "failed", error: detail, line: "", startedAt: null, stepStartedAt: null });
  push(set, get, "error", detail);
  const advice = explainFailure(detail, useSettingsStore.getState().effectiveAgent().label);
  useToastStore.getState().push({ kind: "error", title: advice.title, message: advice.fix });
}

function push(set: Setter, get: () => AgentReviewState, tone: ActivityEntry["tone"], text: string): void {
  const activity = get().activity;
  const seq = (activity[activity.length - 1]?.seq ?? 0) + 1;
  set({
    activity: [...activity.slice(-(ACTIVITY_LIMIT - 1)), { seq, ts: new Date().toISOString(), tone, text }],
  });
}

/**
 * "Step 2 done · 12 parts" when `steps_done` went up, or null. One line per step.
 *
 * It compares two statuses, so a file rewritten with the same counts says nothing, and
 * each step prints once. When the server lists its open steps, the steps that closed
 * are the ones that left the list. The time a step took is not shown.
 */
function stepDone(previous: RunStatus | null, next: RunStatus): string[] {
  if (!previous || previous.review_id !== next.review_id) return [];
  if (next.steps_done <= previous.steps_done) return [];
  const parts = Math.max(0, next.parts_done - previous.parts_done);
  const still = new Set((next.open_steps ?? []).map((o) => o.step));
  const closed = (previous.open_steps ?? []).filter((o) => !still.has(o.step));
  // The parts count is the change across the whole status write. When two steps
  // closed in one write it cannot be split between them, so it is left out.
  const partsText = (n: number) => ` · ${n} ${n === 1 ? "part" : "parts"}`;
  if (closed.length) {
    return closed.map((o) => `${stepName(o)} done${closed.length === 1 ? partsText(parts) : ""}`);
  }
  const count = next.steps_done - previous.steps_done;
  return Array.from(
    { length: count },
    (_, i) => `Step ${previous.steps_done + i + 1} done${count === 1 ? partsText(parts) : ""}`,
  );
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
