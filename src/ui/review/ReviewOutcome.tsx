import { useEffect, useRef, useState } from "react";
import { useBomCheckStore } from "../../stores/bomCheckStore";
import { isAgentRunning, useAgentReviewStore } from "../../stores/agentReviewStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { explainFailure } from "../../lib/agentFailure";
import { coverageGaps, partCoverage, runHealthSummary, type CoverageGap } from "../../lib/findings";
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
 *  Three sources, and they are widened from one. `error` is a run that never landed.
 *  `run_health` is a run whose own stage reported that it was cut short. `bom_audit`
 *  is the rest of it — every check that could not be made — and that is the one the
 *  app used to throw away, so a review with twenty blind spots and no findings read
 *  as a pass. `run_health` now decides only whether the heading says "incomplete";
 *  the LIST is the audit, derived exactly as the review page derives it. */
export function useReviewOutcome(): {
  kind: "failed" | "incomplete";
  text: string;
  detail: string;
  gaps: CoverageGap[];
  coverage: string | null;
} | null {
  const error = useAgentReviewStore((s) => s.error);
  const phase = useAgentReviewStore((s) => s.phase);
  const doc = useBomCheckStore((s) => s.doc);
  const dismissed = useBomCheckStore((s) => s.healthDismissed);

  // A run in flight is its own story; the progress bar is already telling it.
  if (isAgentRunning(phase)) return null;
  if (error) return { kind: "failed", text: "Review failed", detail: error, gaps: [], coverage: null };
  if (dismissed) return null;
  const health = runHealthSummary(doc);
  const gaps = coverageGaps(doc);
  if (!health && !gaps.length) return null;
  return {
    kind: "incomplete",
    // A stage that died and a check that could not be made are different news, and
    // the heading is where the reader learns which this is.
    text: health ? "Review incomplete" : "Not fully verified",
    detail: health?.detail ?? "",
    gaps,
    coverage: partCoverage(doc),
  };
}

export function ReviewOutcome() {
  const outcome = useReviewOutcome();
  const clearError = useAgentReviewStore((s) => s.clearError);
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
  // A failed run says what went wrong and what to do, in that order. The agent's own
  // words stay underneath, small: they are the evidence, not the message.
  const advice = failed
    ? explainFailure(outcome.detail, useSettingsStore.getState().effectiveAgent().label)
    : null;
  return (
    <div className="review-outcome" ref={wrapRef}>
      <button
        className={`review-outcome-pill ${failed ? "failed" : "incomplete"} ${open ? "on" : ""}`}
        title={advice ? advice.title : outcome.detail}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <IconAlert size={13} />
        {outcome.text}
      </button>

      {open && (
        <div className="review-outcome-pop" role="dialog" aria-label={outcome.text}>
          <div className="review-outcome-hd">{advice ? advice.title : outcome.text}</div>
          {/* What to DO, before what it was. A stage name or an exit code answers a
              question the reader did not ask. */}
          <p className="review-outcome-what">
            {advice
              ? advice.fix
              : "Some of this board was not checked. Do not read a clean result as an all-clear — the list below is what nobody looked at."}
          </p>
          {/* The coverage sentence first, because "65 of 72 part numbers were accounted
              for" answers the question the list below only implies. */}
          {outcome.coverage && <p className="review-outcome-coverage">{outcome.coverage}</p>}
          {outcome.detail && (
            <div className={`review-outcome-detail ${failed ? "said" : ""}`}>
              {outcome.detail.split("\n").map((line, i) => (
                <div key={i}>{line}</div>
              ))}
            </div>
          )}
          {/* What the review could not check, from its own audit trail. This is the
              part the app used to throw away, which is how a review with twenty blind
              spots and no findings read as a pass. */}
          {outcome.gaps.length > 0 && (
            <ul className="review-outcome-gaps">
              {outcome.gaps.map((g) => (
                // The whole note only as a tooltip, and only when the line is shorter
                // than it — a title repeating the text under it is a second copy.
                <li key={g.item} title={g.full !== g.note ? g.full : undefined}>
                  <b>{g.item}</b>
                  {g.note ? ` — ${g.note}` : ""}
                </li>
              ))}
            </ul>
          )}
          <div className="review-outcome-acts">
            <button className="btn-ghost" onClick={dismiss}>
              Dismiss
            </button>
            <button
              className="btn-primary"
              onClick={() => {
                dismiss();
                if (advice?.action === "connect") useRunLauncherStore.getState().openConnect();
                else useRunLauncherStore.getState().openSetup("bom");
              }}
            >
              {advice?.action === "connect" ? "Connect…" : "Run again"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
