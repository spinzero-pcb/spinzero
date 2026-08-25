import { useDetailedReviewStore } from "../../stores/detailedReviewStore";

// The detailed review's progress, in the two places it has to be visible: the BOM tab
// it was launched from, and the footer, which is on screen in every view.
//
// **A bar and a number, nothing else.** It used to print the step it was on beside the
// bar ("Detailed review · Reviewing against datasheets · 3 of 16 checks · 2 findings"),
// which is a sentence that changes width every time it changes, in a footer where
// everything else holds still. The words also said less than they looked like they
// said: a step name is the same for eight minutes, so the line read as frozen even
// while the bar moved. A percentage is the one thing a reader wants from a progress
// bar — is this halfway or nearly done — and it is three characters wide. The words
// are still there for anyone who wants them, in the tooltip.
//
// The bar used to be `step / 3`, which meant it jumped to 66% and then sat perfectly
// still for the eight to ten minutes the review itself takes. A frozen bar does not
// read as "this is slow", it reads as "this has hung", and the only remedy on offer
// was cancelling a run that was working fine. So step 2 now has an inside: the
// judgment pass reports how many of the rule pack's checks it has ruled on, and that
// fraction — the one honest measure of the work remaining — fills the middle third.
//
// Everything shown here is a count. The event stream deliberately carries no BOM
// content, so there is no part number to show even if we wanted one.

/** Where each step starts and ends on the bar. Step 2 owns the middle third because
 *  it owns almost all of the wall clock. */
const SPAN: Record<1 | 2 | 3, [number, number]> = {
  1: [4, 30],
  2: [30, 88],
  3: [88, 100],
};

export function ReviewProgress() {
  const progress = useDetailedReviewStore((s) => s.progress);
  const step = useDetailedReviewStore((s) => s.step);
  const found = useDetailedReviewStore((s) => s.liveFindings);
  const review = useDetailedReviewStore((s) => s.reviewProgress);

  const label = progress || "Starting the review";
  const [from, to] = SPAN[step ?? 1];
  // Inside step 2, interpolate on checks-reviewed. Before the first report that
  // fraction is 0, so the bar sits at the start of the span rather than jumping.
  const within =
    step === 2 && review && review.candidates > 0
      ? Math.min(1, review.reviewed / review.candidates)
      : 0;
  const pct = step ? from + (to - from) * within : 4;
  // Rounded once, for the bar and the number together: a fill at 47.4% under a label
  // reading "47%" is the kind of mismatch someone eventually files a bug about.
  const shown = Math.round(pct);

  // Everything the line used to print, now in the tooltip: which step, how far into
  // it, and that datasheets are being fetched (which is why it is slow, and is not a
  // hang). Nobody needs it at a glance; the people who want it know to hover.
  const detail = step === 2 && review ? stageDetail(review) : null;

  return (
    <span
      className="review-progress"
      title={
        `Detailed BOM review — ${label}${detail ? ` · ${detail}` : ""}` +
        `${found > 0 ? ` · ${found} finding${found === 1 ? "" : "s"} so far` : ""}`
      }
      role="progressbar"
      aria-valuenow={shown}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-label="Detailed BOM review progress"
    >
      <span className="review-progress-bar">
        <span className="review-progress-fill" style={{ width: `${shown}%` }} />
      </span>
      <span className="review-progress-pct">{shown}%</span>
    </span>
  );
}

/** The middle of a long run, in the fewest words that still say what is happening.
 *  Datasheet count leads before any check has been ruled on, because that is the
 *  phase where nothing else has moved yet and the silence is what worries people. */
function stageDetail(review: { reviewed: number; candidates: number; datasheetsRead: number }): string {
  if (review.reviewed > 0 && review.candidates > 0) {
    return `${review.reviewed} of ${review.candidates} checks`;
  }
  if (review.datasheetsRead > 0) {
    return `${review.datasheetsRead} datasheet${review.datasheetsRead === 1 ? "" : "s"} read`;
  }
  return "reading datasheets";
}
