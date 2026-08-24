import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { BomProfile, FindingsDoc } from "../lib/findings";
import {
  ackReview,
  cancelReview,
  DEFAULT_BASE_URL,
  fetchFindings,
  health,
  stageLabel,
  streamProgress,
  submitReview,
  type ReviewBundle,
  type ReviewProgress,
  type ReviewServiceConfig,
} from "../lib/reviewService";
import { useBomCheckStore } from "./bomCheckStore";
import { useBomMappingStore } from "./bomMappingStore";
import { useReviewRunsStore } from "./reviewRunsStore";
import { useReviewStore } from "./reviewStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";

// Detailed BOM review (paid tier) — the app's half of the service conversation.
//
// The flow, and why it is shaped this way:
//
//   preflight → submit → stream progress → fetch findings → ingest → ack
//
// * **Pre-flight is not optional.** The user sees the exact file list before
//   anything is uploaded (plan §4.2), and the bundle shown is the bundle sent —
//   both come from one `build_review_bundle` call.
// * **Ingestion is the free tier's path.** The findings document goes to
//   `ingest_findings`, which is `bomcheck::ingest` — so a paid finding updates the
//   comment a free finding already created instead of filing a second one.
// * **Ack means delete.** Once the findings are in the project folder, the service
//   is told to drop its copy. A failed ack is not an error the user must act on;
//   the server's TTL covers it.
// * **Nothing here is persisted.** A job id is worthless after ingestion, and the
//   findings live as review comments — the same reasoning as bomCheckStore.

export type ReviewPhase = "idle" | "preflight" | "submitting" | "running" | "ingesting" | "done";

interface DetailedReviewState {
  phase: ReviewPhase;
  /** The bundle the dialog renders; null until pre-flight has been opened. */
  bundle: ReviewBundle | null;
  bundleError: string | null;
  jobId: string | null;
  /** Latest progress line, already humanized for display. */
  progress: string;
  stage: string | null;
  /** Findings emitted so far, from stage_progress — a ticking count while it runs. */
  liveFindings: number;
  doc: FindingsDoc | null;
  error: string | null;
  /** Service reachability from the last check; null = not checked yet. */
  serviceOk: boolean | null;

  openPreflight: () => Promise<void>;
  closePreflight: () => void;
  start: () => Promise<void>;
  cancel: () => Promise<void>;
  checkService: () => Promise<void>;
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

export const useDetailedReviewStore = create<DetailedReviewState>((set, get) => ({
  phase: "idle",
  bundle: null,
  bundleError: null,
  jobId: null,
  progress: "",
  stage: null,
  liveFindings: 0,
  doc: null,
  error: null,
  serviceOk: null,

  openPreflight: async () => {
    const profile = useBomCheckStore.getState().profile;
    // Same gate as the free check: never spend a paid review on a mapping nobody
    // has looked at. The dialog re-enters here once the user has approved one.
    const approved = await useBomMappingStore
      .getState()
      .ensureApproved(profile, () => void get().openPreflight());
    if (!approved) return;
    set({ phase: "preflight", bundle: null, bundleError: null, error: null, doc: null });
    try {
      const bundle = await ipc.buildReviewBundle(profile);
      set({ bundle });
    } catch (e) {
      // No enriched BOM yet is the common case (project never extracted) — say so
      // in the dialog rather than as a toast the user has to correlate.
      set({ bundleError: String(e) });
    }
    void get().checkService();
  },

  closePreflight: () => {
    if (get().phase === "preflight") set({ phase: "idle", bundle: null, bundleError: null });
  },

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
    const config = serviceConfig();
    const bundle = get().bundle;
    if (!config || !bundle || get().phase === "running" || get().phase === "submitting") return;
    const profile = useBomCheckStore.getState().profile as BomProfile;

    set({ phase: "submitting", error: null, progress: "Uploading the bundle…", liveFindings: 0, stage: null });
    let jobId: string;
    try {
      ({ job_id: jobId } = await submitReview(config, { profile, files: bundle.files }));
    } catch (e) {
      fail(set, `${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    set({ phase: "running", jobId, progress: "Queued…" });

    try {
      const terminal = await streamProgress(config, jobId, (event) => onProgress(set, get, event));
      if (terminal.type === "failed") {
        const detail = (terminal.data as { error?: string } | undefined)?.error ?? terminal.message ?? "";
        fail(set, `the review failed${detail ? `: ${detail}` : ""}`);
        return;
      }
    } catch (e) {
      // Losing the progress stream does not mean losing the review: the job may well
      // have finished, so fall through and try to collect the findings anyway.
      set({ progress: `Progress stream lost (${String(e)}); collecting the result…` });
    }

    set({ phase: "ingesting", progress: "Filing the findings…" });
    try {
      const doc = await fetchFindings(config, jobId);
      const outcome = await ipc.ingestFindings(doc);
      set({ phase: "done", doc, progress: "" });
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
    set({ phase: "idle", jobId: null, progress: "", stage: null, liveFindings: 0 });
  },

  reset: () =>
    set({
      phase: "idle",
      bundle: null,
      bundleError: null,
      jobId: null,
      progress: "",
      stage: null,
      liveFindings: 0,
      doc: null,
      error: null,
    }),
}));

type Setter = (partial: Partial<DetailedReviewState>) => void;

function onProgress(set: Setter, get: () => DetailedReviewState, event: ReviewProgress): void {
  switch (event.type) {
    case "stage_started":
      set({ stage: event.stage ?? null, progress: `${stageLabel(event.stage)}…` });
      break;
    case "stage_progress": {
      const findings = (event.data as { findings?: number } | undefined)?.findings;
      set({
        progress: event.message ? `${stageLabel(event.stage)}: ${event.message}` : get().progress,
        ...(typeof findings === "number" ? { liveFindings: findings } : {}),
      });
      break;
    }
    case "stage_done":
      set({ progress: `${stageLabel(event.stage)} — done` });
      break;
    case "queued":
      set({ progress: "Queued…" });
      break;
    default:
      // `log` lines are diagnostics, not user-facing narration; the completed/failed
      // events are handled by the caller, which owns the phase transition.
      break;
  }
}

function fail(set: Setter, message: string): void {
  set({ phase: "idle", error: message, progress: "", stage: null });
  useToastStore.getState().push({
    kind: "error",
    title: "Detailed review failed",
    message,
  });
}
