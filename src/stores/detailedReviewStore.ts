import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { FindingsDoc } from "../lib/findings";
import {
  ackReview,
  cancelReview,
  describeProgress,
  DEFAULT_BASE_URL,
  fetchFindings,
  health,
  streamProgress,
  submitReview,
  type ActivityEntry,
  type ReviewBundle,
  type ReviewProgress,
  type ReviewServiceConfig,
} from "../lib/reviewService";
import { currentBomProfile } from "./bomCheckStore";
import { useBomMappingStore } from "./bomMappingStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";
import { landFindings } from "./landFindings";

// Detailed BOM review (paid tier) — the app's half of the service conversation.
//
// The flow, and why it is shaped this way:
//
//   prepare → submit → stream progress → fetch findings → ingest → ack
//
// * **One button, not two dialogs.** Preparation (the mapping gate, the bundle, the
//   reachability check) used to be a pre-flight dialog listing every file by name and
//   size. It now happens inside `start()`, because the file list answered a question
//   nobody was asking and the promise it existed to make — schematic and layout are
//   never sent, and the bundle is deleted when the review finishes — is a sentence,
//   not an inventory. Anything that goes wrong there surfaces as `error` on the
//   review's own setup sheet, next to the button that caused it.
// * **Progress is reported in the user's terms.** The service's stage ids name
//   pipeline internals; what the user needs is that something is happening and roughly
//   how far along it is, so stages fold onto three plain steps and a fraction.
// * **Ingestion is the free tier's path.** The findings document goes to
//   `ingest_findings`, which is `bomcheck::ingest` — so a paid finding updates the
//   comment a free finding already created instead of filing a second one.
// * **Ack means delete.** Once the findings are in the project folder, the service
//   is told to drop its copy. A failed ack is not an error the user must act on;
//   the server's TTL covers it.
// * **Nothing here is persisted.** A job id is worthless after ingestion, and the
//   findings live as review comments — the same reasoning as bomCheckStore.

/** The pipeline's stages folded onto what a customer actually wants to know. The
 *  service can add or rename a stage without this list changing; an unknown stage
 *  simply keeps the step the run is already showing. */
const STEP_OF_STAGE: Record<string, 1 | 2 | 3> = {
  validate_bundle: 1,
  deterministic_rules: 2,
  judgment_pass: 2,
  assemble: 3,
};

/** The one stage whose finding count means what the user reads it to mean.
 *
 *  Every stage reports a number and they count different things. The rule pack
 *  reports its RAW hits — 16 on a board that shipped 7 findings, because most of a
 *  rule pack's output is filtered out downstream. Showing that made the strip read
 *  "16 so far" for four solid minutes and then contradict itself at the end. Only
 *  the judgment pass counts findings that are actually going into the report, so
 *  only the judgment pass may move this number. */
const COUNTS_REPORTABLE_FINDINGS = "judgment_pass";
const STEP_LABEL: Record<1 | 2 | 3, string> = {
  1: "Preparing your BOM",
  2: "Reviewing against datasheets",
  3: "Finishing up",
};

export type ReviewPhase = "idle" | "preparing" | "submitting" | "running" | "ingesting" | "done";

/** The judgment pass, mid-flight. Counts only — the event stream carries no BOM
 *  content, so there is nothing here to leak and nothing to show but numbers. */
export interface ReviewStageProgress {
  /** Rule-pack checks ruled on so far, and how many there are. The only honest
   *  fraction the run has: turns are a budget, finishing the checks is the job. */
  reviewed: number;
  candidates: number;
  /** Datasheets opened so far. This is where most of the wall clock goes. */
  datasheetsRead: number;
}

/** How many activity lines to keep. A ten-minute run with tool events and a 15 s
 *  heartbeat lands well inside this; the cap exists so a pathological run cannot
 *  grow the store without bound, and the oldest lines are the least interesting. */
const ACTIVITY_LIMIT = 500;

interface DetailedReviewState {
  phase: ReviewPhase;
  /** The bundle actually posted. Internal — the UI no longer itemises it. */
  bundle: ReviewBundle | null;
  jobId: string | null;
  /** Latest progress line, in the user's terms (never a stage id). */
  progress: string;
  /** 1..3 while a run is in flight — the same three steps `progress` names, for a bar. */
  step: 1 | 2 | 3 | null;
  /** Findings that will be REPORTED, so far — a ticking count while it runs. Sourced
   *  from the judgment pass alone; see [[COUNTS_REPORTABLE_FINDINGS]]. */
  liveFindings: number;
  /** How far through the review stage itself we are. The three steps alone left the
   *  bar frozen for the eight-to-ten minutes the judgment pass takes, which reads as
   *  a hang; this is what moves inside step 2. Null until the stage reports. */
  reviewProgress: ReviewStageProgress | null;
  doc: FindingsDoc | null;
  error: string | null;
  /** Service reachability from the last check; null = not checked yet. */
  serviceOk: boolean | null;
  /** Every event of the current run, oldest first, capped at ACTIVITY_LIMIT.
   *
   *  The three-step bar deliberately folds four stages onto three words, which is the
   *  right answer for someone waiting and the wrong one for someone debugging: a run
   *  that spent six minutes inside one model turn and a run that died look the same
   *  through it. This is the unfolded version, and it is only ever read by the panel
   *  behind the progress bar. */
  activity: ActivityEntry[];

  /** Run the whole thing: gate the mapping, build the bundle, check the service, submit,
   *  stream, ingest, ack. Every failure lands in `error` rather than in a dialog. */
  start: () => Promise<void>;
  cancel: () => Promise<void>;
  checkService: () => Promise<void>;
  clearError: () => void;
  reset: () => void;
}

/** Read the endpoint out of settings. Missing config is not an error yet — the
 *  dialog offers to fill it in. */
export function serviceConfig(): ReviewServiceConfig | null {
  const saved = useSettingsStore.getState().reviewService;
  if (!saved?.base_url) return null;
  return { baseUrl: saved.base_url, token: saved.token };
}

export const DEFAULT_SERVICE_URL = DEFAULT_BASE_URL;

/** Is a run in flight? The one predicate the UI should ask — every surface that
 *  disables a button or shows a spinner reads this rather than listing phases. */
export function isRunning(phase: ReviewPhase): boolean {
  return phase === "preparing" || phase === "submitting" || phase === "running" || phase === "ingesting";
}

export const useDetailedReviewStore = create<DetailedReviewState>((set, get) => ({
  phase: "idle",
  bundle: null,
  jobId: null,
  progress: "",
  step: null,
  liveFindings: 0,
  reviewProgress: null,
  doc: null,
  error: null,
  serviceOk: null,
  activity: [],

  checkService: async () => {
    const config = serviceConfig();
    if (!config) {
      set({ serviceOk: null });
      return;
    }
    const { ok } = await health(config);
    set({ serviceOk: ok });
  },

  start: async () => {
    if (isRunning(get().phase)) return;
    const profile = currentBomProfile();

    // Everything the old pre-flight dialog did, in order, before a byte is posted.
    set({
      phase: "preparing",
      error: null,
      progress: "Preparing your BOM",
      step: 1,
      liveFindings: 0,
      // Last run's feed is not this run's feed, and a stale one is worse than none.
      activity: [],
    });

    const config = serviceConfig();
    if (!config) {
      fail(set, "the review service is not set up yet — add its address below.");
      return;
    }

    // Same gate as the free check: never spend a paid review on a mapping nobody has
    // looked at. The dialog takes over and re-enters here once one is approved.
    const approved = await useBomMappingStore.getState().ensureApproved(profile, () => void get().start());
    if (!approved) {
      set({ phase: "idle", progress: "", step: null });
      return;
    }

    let bundle: ReviewBundle;
    try {
      bundle = await ipc.buildReviewBundle(profile);
      set({ bundle });
    } catch (e) {
      // No enriched BOM yet is the common case (project never extracted).
      fail(set, `there is nothing to review yet: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }

    // Reachability is checked HERE, not while a sheet sits open: a service that was up
    // a minute ago proves nothing, and this is the moment the user asked to send.
    const { ok } = await health(config);
    set({ serviceOk: ok });
    if (!ok) {
      fail(set, "the review service is not reachable. Check your connection and try again.");
      return;
    }

    set({ phase: "submitting", progress: "Preparing your BOM", step: 1 });
    let jobId: string;
    try {
      ({ job_id: jobId } = await submitReview(config, { profile, files: bundle.files }));
    } catch (e) {
      fail(set, `${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    set({ phase: "running", jobId, progress: "Waiting for a reviewer", step: 1 });

    try {
      const terminal = await streamProgress(config, jobId, (event) => onProgress(set, get, event));
      if (terminal.type === "failed") {
        const detail = (terminal.data as { error?: string } | undefined)?.error ?? terminal.message ?? "";
        fail(set, `the review failed${detail ? `: ${detail}` : ""}`);
        return;
      }
    } catch (e) {
      // Losing the progress stream does not mean losing the review: the job may well
      // have finished, so fall through and try to collect the findings anyway — and
      // say nothing alarming about it while doing so.
      set({ progress: "Finishing up", step: 3 });
    }

    set({ phase: "ingesting", progress: "Finishing up", step: 3 });
    try {
      const doc = await fetchFindings(config, jobId);
      const outcome = await ipc.ingestFindings(doc);
      set({ phase: "done", doc, progress: "", step: null });
      // Everything a landed review owes the user — BOM strip, rail, launcher stamp,
      // one honest toast — is shared with the drop-box import (`landFindings`), so a
      // review that came back incomplete cannot report clean on one surface only.
      await landFindings({ doc, outcome, label: "Detailed review" });
      // The result is safely in the project folder; tell the service to delete its
      // copy (plan §6.1). Fire-and-forget: a failed ack costs a TTL, not the result.
      void ackReview(config, jobId);
    } catch (e) {
      fail(set, `the findings could not be filed: ${e instanceof Error ? e.message : String(e)}`);
    }
  },

  cancel: async () => {
    const config = serviceConfig();
    const jobId = get().jobId;
    if (config && jobId) await cancelReview(config, jobId);
    set({ phase: "idle", jobId: null, progress: "", step: null, liveFindings: 0 });
  },

  clearError: () => set({ error: null }),

  reset: () =>
    set({
      phase: "idle",
      bundle: null,
      jobId: null,
      progress: "",
      step: null,
      liveFindings: 0,
      reviewProgress: null,
      doc: null,
      error: null,
      activity: [],
    }),
}));

type Setter = (partial: Partial<DetailedReviewState>) => void;

function onProgress(set: Setter, get: () => DetailedReviewState, event: ReviewProgress): void {
  // EVERY event lands in the feed, including the ones the switch below ignores —
  // `log` lines are the whole reason the feed exists. The bar's reading of an event
  // and the feed's are independent on purpose: folding a stage onto a step is a
  // product decision, and the feed must not inherit it.
  const entry = describeProgress(event);
  if (entry) {
    const activity = get().activity;
    const seq = (activity[activity.length - 1]?.seq ?? 0) + 1;
    set({
      activity: [...activity.slice(-(ACTIVITY_LIMIT - 1)), { ...entry, seq }],
    });
  }

  // The stage's own message is deliberately dropped: it narrates the pipeline, and a
  // customer reading "judgment_pass: shard 2 of 3" learns nothing they can act on.
  // What travels is the step it belongs to and how many findings exist so far.
  switch (event.type) {
    case "stage_started":
    case "stage_done":
    case "stage_progress": {
      const step = STEP_OF_STAGE[event.stage ?? ""] ?? get().step ?? 1;
      const data = event.data as
        | { findings?: number; reviewed?: number; candidates?: number; datasheets_read?: number }
        | undefined;
      const fromJudgment = event.stage === COUNTS_REPORTABLE_FINDINGS;
      const findings = fromJudgment ? data?.findings : undefined;
      // Only the judgment pass knows how much of the review is done, and only while
      // it is running: a `stage_done` from a later stage must not reset the bar.
      const reviewProgress =
        fromJudgment && typeof data?.candidates === "number"
          ? {
              reviewed: data.reviewed ?? 0,
              candidates: data.candidates,
              datasheetsRead: data.datasheets_read ?? 0,
            }
          : undefined;
      set({
        step,
        progress: STEP_LABEL[step],
        ...(typeof findings === "number" ? { liveFindings: findings } : {}),
        ...(reviewProgress ? { reviewProgress } : {}),
      });
      break;
    }
    case "queued":
      set({ progress: "Waiting for a reviewer", step: 1 });
      break;
    default:
      // `log` lines are diagnostics, not user-facing narration; the completed/failed
      // events are handled by the caller, which owns the phase transition.
      break;
  }
}

function fail(set: Setter, message: string): void {
  set({ phase: "idle", error: message, progress: "", step: null });
  useToastStore.getState().push({
    kind: "error",
    title: "Detailed review failed",
    message,
  });
}
