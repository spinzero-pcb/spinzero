import { useReviewStore } from "../../stores/reviewStore";
import { IconChecklist, IconSparkle } from "../icons";

// The activity bar drops the old files/Explorer entry — project structure is the
// per-view RIGHT panel now — and carries the global LEFT rail's two review surfaces.
//
// They are split because they answer different questions and have different scopes.
// "Reviews" RUNS checks and is deliberately not a view of one artifact: the BOM check
// is here today, the schematic check lands beside it, and neither belongs to whichever
// canvas happens to be on screen. "Review" READS what they found, as comments anchored
// in the design.
export function ActivityBar() {
  const tab = useReviewStore((s) => s.leftTab);
  const setTab = useReviewStore((s) => s.setLeftTab);
  return (
    <nav className="activity-bar">
      <button
        className={`activity-btn ${tab === "reviews" ? "active" : ""}`}
        title="Reviews — run the BOM and design checks"
        onClick={() => setTab("reviews")}
      >
        <IconSparkle size={22} />
      </button>
      <button
        className={`activity-btn ${tab !== "reviews" ? "active" : ""}`}
        title="Review — the findings, as comments"
        onClick={() => setTab("review")}
      >
        <IconChecklist size={22} />
      </button>
    </nav>
  );
}
