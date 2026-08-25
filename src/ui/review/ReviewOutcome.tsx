import { useEffect, useRef, useState } from "react";
import { useBomCheckStore } from "../../stores/bomCheckStore";
import { isRunning, useDetailedReviewStore } from "../../stores/detailedReviewStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { runHealthSummary } from "../../lib/reviewService";
import { IconAlert } from "../icons";

// What the last review did wrong, kept on screen until it is dealt with.
//
// This exists because of a run that failed in the worst available way: the judgment
// pass ruled on none of its 16 candidates, the service still reported the job
// "completed", and the app filed 16 raw rule hits as a finished detailed review. The
// engine said so — `run_health` carried the failed stage — and the app showed it in
// exactly one place: a grey chip on the BOM strip, which is not on screen unless the
// BOM tab is. The completion toast said "Detailed review: 16 findings", the same
// sentence a healthy run produces. So the user paid for a review, got an unreviewed
// one, and had no way to know.
//
// Three things follow from that, and they are the whole design here:
//
//  * **A failure is not a caveat.** It renders as an alert, not as another grey
//    string in a row of grey strings, and it says what is untrustworthy about the
//    result rather than naming a stage.
//  * **It outlives the toast.** A toast is gone in seconds and the findings it
//    describes stay in the project forever. This sits in the footer — on screen in
//    every view — until the user dismisses it or runs another review.
//  * **Two failures, one surface.** A run that never delivered ("the service is not
//    reachable") and a run that delivered something incomplete are different
//    sentences but the same question: can I trust what is in the rail right now?
//
// Not persisted, deliberately: it describes THIS session's run. The durable record is
// the findings themselves, each of which carries its own "not reviewed" note.

/** The last run's failure, or null when there is nothing wrong to report.
 *
 *  `error` is a run that never landed — the store already holds the sentence. The
 *  health summary is the other one: a run that landed and should not be trusted. */
export function useReviewOutcome(): { kind: "failed" | "incomplete"; text: string; detail: string } | null {
  const error = useDetailedReviewStore((s) => s.error);
  const phase = useDetailedReviewStore((s) => s.phase);
  const doc = useBomCheckStore((s) => s.doc);
  const dismissed = useBomCheckStore((s) => s.healthDismissed);

  // A run in flight is its own story; the progress bar is already telling it.
  if (isRunning(phase)) return null;
  if (error) return { kind: "failed", text: "Review failed", detail: error };
  if (dismissed) return null;
  const health = runHealthSummary(doc);
  if (!health) return null;
  return { kind: "incomplete", text: "Review incomplete", detail: health.detail };
}

export function ReviewOutcome() {
  const outcome = useReviewOutcome();
  const clearError = useDetailedReviewStore((s) => s.clearError);
  const dismissHealth = useBomCheckStore((s) => s.dismissHealth);
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  // Same dismiss idiom as the other footer popovers: click away or Escape.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  // The outcome can clear underneath an open panel (a new run starts), and a popover
  // describing nothing is worse than no popover.
  useEffect(() => {
    if (!outcome) setOpen(false);
  }, [outcome]);

  if (!outcome) return null;

  function dismiss() {
    setOpen(false);
    clearError();
    dismissHealth();
  }

  const failed = outcome.kind === "failed";
  return (
    <div className="review-outcome" ref={wrapRef}>
      <button
        className={`review-outcome-pill ${failed ? "failed" : "incomplete"} ${open ? "on" : ""}`}
        title={outcome.detail}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <IconAlert size={13} />
        {outcome.text}
      </button>

      {open && (
        <div className="review-outcome-pop" role="dialog" aria-label={outcome.text}>
          <div className="review-outcome-hd">{outcome.text}</div>
          {/* What it MEANS, before what it was. A stage name answers a question the
              reader did not ask; "do not trust the all-clear" is the one they did. */}
          <p className="review-outcome-what">
            {failed
              ? "This review did not finish, so nothing from it has been filed. The findings in the review panel are from an earlier run."
              : "Part of this review did not run. Its findings were filed anyway, marked low confidence — treat them as unchecked, and do not read a clean result as an all-clear."}
          </p>
          <div className="review-outcome-detail">
            {outcome.detail.split("\n").map((line, i) => (
              <div key={i}>{line}</div>
            ))}
          </div>
          <div className="review-outcome-acts">
            <button className="btn-ghost" onClick={dismiss}>
              Dismiss
            </button>
            <button
              className="btn-primary"
              onClick={() => {
                dismiss();
                useRunLauncherStore.getState().openSetup("bom");
              }}
            >
              Run again
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
