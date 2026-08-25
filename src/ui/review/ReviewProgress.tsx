import { useDetailedReviewStore } from "../../stores/detailedReviewStore";

// The detailed review's progress, in the two places it has to be visible: the BOM tab
// it was launched from, and the footer, which is on screen in every view.
//
// It reports the same three plain steps the store keeps — never a stage id, never the
// pipeline's own words.
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

export function ReviewProgress({ compact = false }: { compact?: boolean }) {
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

  // A second line the reader can act on: how much of the review is done, and that
  // datasheets are being fetched (which is why it is slow, and is not a hang).
  const detail = step === 2 && review ? stageDetail(review) : null;

  return (
    <span
      className={`review-progress ${compact ? "compact" : ""}`}
      title={`Detailed BOM review — ${label}${detail ? ` · ${detail}` : ""}`}
    >
      <span className="review-progress-bar">
        <span className="review-progress-fill" style={{ width: `${pct}%` }} />
      </span>
      <span className="review-progress-text">
        {compact ? label : `Detailed review · ${label}`}
        {detail && ` · ${detail}`}
        {found > 0 && ` · ${found} finding${found === 1 ? "" : "s"}`}
      </span>
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
