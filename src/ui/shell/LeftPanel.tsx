import { useReviewStore } from "../../stores/reviewStore";
import { ReviewPanel } from "../review/ReviewPanel";
import { UpdateBanner } from "./UpdateBanner";
import { IconChecklist } from "../icons";

// The LEFT rail is the global review surface — identical in every view
// (docs/phase2-ui-plan.md §1/§2). The Phase-3 AI tab was removed for now, so Review
// is the only surface here (one filterable stream of human/rule findings).
export function LeftPanel() {
  const tab = useReviewStore((s) => s.leftTab);
  const setTab = useReviewStore((s) => s.setLeftTab);

  return (
    <>
      <div className="left-tabs">
        <button
          className={`left-tab ${tab === "review" ? "active" : ""}`}
          onClick={() => setTab("review")}
        >
          <IconChecklist size={15} />
          Review
        </button>
      </div>
      <ReviewPanel />
      {/* Pinned to the bottom of the rail (batch1): a persistent update notice in place
          of the old transient toast. Renders nothing unless an update is downloaded. */}
      <UpdateBanner />
    </>
  );
}
