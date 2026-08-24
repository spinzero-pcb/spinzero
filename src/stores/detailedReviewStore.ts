import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { FindingsDoc } from "../lib/findings";
import {
  ackReview,
  cancelReview,
  DEFAULT_BASE_URL,
  fetchFindings,
  health,
  streamProgress,
  submitReview,
  type ReviewBundle,
  type ReviewProgress,
  type ReviewServiceConfig,
} from "../lib/reviewService";
import { currentBomProfile, useBomCheckStore } from "./bomCheckStore";
import { useBomMappingStore } from "./bomMappingStore";
import { useReviewRunsStore } from "./reviewRunsStore";
import { useReviewStore } from "./reviewStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";

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
  fp_validation: 2,
  judgment_pass: 2,
  assemble: 3,
};
const STEP_LABEL: Record<1 | 2 | 3, string> = {
  1: "Preparing your BOM",
  2: "Reviewing against datasheets",
  3: "Finishing up",
};

export type ReviewPhase = "idle" | "preparing" | "submitting" | "running" | "ingesting" | "done";

interface DetailedReviewState {
  phase: ReviewPhase;
  /** The bundle actually posted. Internal — the UI no longer itemises it. */
  bundle: ReviewBundle | null;
  jobId: string | null;
  /** Latest progress line, in the user's terms (never a stage id). */
  progress: string;
  /** 1..3 while a run is in flight — the same three steps `progress` names, for a bar. */
  step: 1 | 2 | 3 | null;
  /** Findings emitted so far, from stage_progress — a ticking count while it runs. */
  liveFindings: number;
  doc: FindingsDoc | null;
  error: string | null;
  /** Service reachability from the last check; null = not checked yet. */
  serviceOk: boolean | null;

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
  doc: null,
  error: null,
  serviceOk: null,

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
    set({ phase: "preparing", error: null, progress: "Preparing your BOM", step: 1, liveFindings: 0 });

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
      // The BOM tab's summary strip renders whatever ran last, free or paid.
      useBomCheckStore.setState({
        doc,
        summary: {
          filed: outcome.filed,
          reopened: outcome.reopened,
          unchanged: outcome.unchanged,
          auto_resolved: outcome.auto_resolved,
        },
        sessionId: outcome.session_id,
        error: null,
      });
      await useReviewStore.getState().load();
      // Land on the run's own session, exactly as the free check does — the findings
      // the user just paid to wait for are what the rail should be showing.
      if (outcome.session_id) useReviewStore.getState().setActiveSession(outcome.session_id);
      // Same stamp the free check writes — the launcher's BOM row reflects whichever
      // depth ran last. Cosmetic, so a failure here never touches the result.
      void useReviewRunsStore
        .getState()
        .record("bom")
        .catch(() => {});
      useToastStore.getState().push({
        kind: doc.findings.length ? "info" : "success",
        title: `Detailed review: ${doc.findings.length || "no"} finding${doc.findings.length === 1 ? "" : "s"}`,
        message: [
          outcome.filed ? `${outcome.filed} new comment${outcome.filed === 1 ? "" : "s"}` : "",
          outcome.unchanged ? `${outcome.unchanged} refined` : "",
          outcome.auto_resolved ? `${outcome.auto_resolved} auto-resolved` : "",
        ]
          .filter(Boolean)
          .join(" · "),
      });
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
      doc: null,
      error: null,
    }),
}));

type Setter = (partial: Partial<DetailedReviewState>) => void;

function onProgress(set: Setter, get: () => DetailedReviewState, event: ReviewProgress): void {
  // The stage's own message is deliberately dropped: it narrates the pipeline, and a
  // customer reading "fp_validation: 12/40 kept" learns nothing they can act on. What
  // travels is the step it belongs to and how many findings exist so far.
  switch (event.type) {
    case "stage_started":
    case "stage_done":
    case "stage_progress": {
      const step = STEP_OF_STAGE[event.stage ?? ""] ?? get().step ?? 1;
      const findings = (event.data as { findings?: number } | undefined)?.findings;
      set({
        step,
        progress: STEP_LABEL[step],
        ...(typeof findings === "number" ? { liveFindings: findings } : {}),
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
