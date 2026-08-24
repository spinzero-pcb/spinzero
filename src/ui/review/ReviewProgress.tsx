import { useDetailedReviewStore } from "../../stores/detailedReviewStore";

// The detailed review's progress, in the two places it has to be visible: the BOM tab
// it was launched from, and the footer, which is on screen in every view.
//
// It reports the same three plain steps the store keeps — never a stage id, never the
// pipeline's own words. The bar is determinate because "3 of 3" is the only honest
// thing to say about a job whose remaining time nobody knows: it shows that something
// is happening and roughly how much is left, and nothing it cannot back up.

export function ReviewProgress({ compact = false }: { compact?: boolean }) {
  const progress = useDetailedReviewStore((s) => s.progress);
  const step = useDetailedReviewStore((s) => s.step);
  const found = useDetailedReviewStore((s) => s.liveFindings);

  const pct = step ? (step / 3) * 100 : 8;
  const label = progress || "Starting the review";

  return (
    <span
      className={`review-progress ${compact ? "compact" : ""}`}
      title={`Detailed BOM review — ${label}`}
    >
      <span className="review-progress-bar">
        <span className="review-progress-fill" style={{ width: `${pct}%` }} />
      </span>
      <span className="review-progress-text">
        {compact ? label : `Detailed review · ${label}`}
        {found > 0 && ` · ${found} so far`}
      </span>
    </span>
  );
}
