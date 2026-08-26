import type { CheckOutcome, FindingsDoc } from "../lib/findings";
import { runHealthSummary } from "../lib/reviewService";
import { useBomCheckStore } from "./bomCheckStore";
import { useReviewRunsStore } from "./reviewRunsStore";
import { useReviewStore } from "./reviewStore";
import { useToastStore } from "./toastStore";

// Everything that happens AFTER a findings document has been filed as comments.
//
// There is more than one way a review reaches the app now — the hosted service, and
// the review drop-box a local engine run or an MCP-driven agent writes into
// (`<project>/reviews/inbox/`) — and every one of them owes the user the same four
// things afterwards: the BOM strip refreshed, the comment rail reloaded and pointed
// at the run's own session, the launcher's "ran when" stamp updated, and one toast
// that is honest about whether the review was complete.
//
// Written once here because the alternative is two copies drifting: the day a review
// comes back incomplete, the surface that forgot to call `runHealthSummary` reports
// a clean run. The ingestion itself is NOT here — that is `bomcheck::ingest` on the
// Rust side, and it is already the single path.

export interface LandOptions {
  /** The document that was ingested. */
  doc: FindingsDoc;
  /** What ingestion did with it. */
  outcome: CheckOutcome;
  /** How the toast names this review, e.g. "Detailed review" or "Imported review". */
  label: string;
}

export async function landFindings({ doc, outcome, label }: LandOptions): Promise<void> {
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
    // A fresh verdict on this run's health, whatever the user acknowledged about the
    // last one.
    healthDismissed: false,
  });
  await useReviewStore.getState().load();
  // Land on the run's own session, exactly as the free check does — the findings the
  // user just waited for are what the rail should be showing.
  if (outcome.session_id) useReviewStore.getState().setActiveSession(outcome.session_id);
  // Same stamp the free check writes — the launcher's BOM row reflects whichever
  // depth ran last. Cosmetic, so a failure here never touches the result.
  void useReviewRunsStore
    .getState()
    .record("bom")
    .catch(() => {});
  // A stage that did not run makes this an incomplete result, and the toast is the
  // one surface that always appears — so it is the one that must not read like a
  // clean run. A job can say "completed" in exactly this case (the producer finished;
  // the review did not), which is why this asks the DOCUMENT and not the job.
  const health = runHealthSummary(doc);
  useToastStore.getState().push({
    kind: health ? "error" : doc.findings.length ? "info" : "success",
    title: health
      ? `${label} incomplete`
      : `${label}: ${doc.findings.length || "no"} finding${doc.findings.length === 1 ? "" : "s"}`,
    message: health
      ? `${health.text}. ${doc.findings.length} finding${doc.findings.length === 1 ? "" : "s"} filed anyway, at low confidence — treat them as unchecked.`
      : [
          outcome.filed ? `${outcome.filed} new comment${outcome.filed === 1 ? "" : "s"}` : "",
          outcome.unchanged ? `${outcome.unchanged} refined` : "",
          outcome.auto_resolved ? `${outcome.auto_resolved} auto-resolved` : "",
        ]
          .filter(Boolean)
          .join(" · "),
  });
}
