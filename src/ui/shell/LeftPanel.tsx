import { useReviewStore } from "../../stores/reviewStore";
import { useDiffStore } from "../../stores/diffStore";
import { ReviewPanel } from "../review/ReviewPanel";
import { ReviewsPanel } from "../review/ReviewsPanel";
import { ChangesPanel } from "../diff/ChangesPanel";
import { UpdateBanner } from "./UpdateBanner";
import { IconChecklist, IconHistory } from "../icons";

// The LEFT rail is the global review surface — identical in every view
// (docs/phase2-ui-plan.md §1/§2). A second "Changes" tab appears ONLY while a visual
// diff is active (visual-diff §5); diffStore enter/exit drive the selection (auto-Changes
// on enter, back to Review on exit), so normal viewing is unchanged. The selected tab is
// reviewStore.leftTab — one source of truth shared with ActivityBar and diffStore.
export function LeftPanel() {
  const setTab = useReviewStore((s) => s.setLeftTab);
  const tab = useReviewStore((s) => s.leftTab);
  const diffActive = useDiffStore((s) => s.active);

  // "changes" is only meaningful while a diff is active; outside it the rail is Review.
  const showingChanges = diffActive && tab === "changes";

  // The Reviews surface is chosen from the activity bar, not from these tabs: it is a
  // different rail entry, not a third view of the same comments.
  if (tab === "reviews") {
    return (
      <>
        <ReviewsPanel />
        <UpdateBanner />
      </>
    );
  }

  return (
    <>
      <div className="left-tabs">
        {diffActive && (
          <button
            className={`left-tab ${showingChanges ? "active" : ""}`}
            onClick={() => setTab("changes")}
          >
            <IconHistory size={15} />
            Changes
          </button>
        )}
        <button
          className={`left-tab ${!showingChanges ? "active" : ""}`}
          onClick={() => setTab("review")}
        >
          <IconChecklist size={15} />
          Review
        </button>
      </div>
      {showingChanges ? <ChangesPanel /> : <ReviewPanel />}
      {/* Pinned to the bottom of the rail (batch1): a persistent update notice in place
          of the old transient toast. Renders nothing unless an update is downloaded, so
          it stays mounted through diff mode too — it's the only surface that announces a
          ready update, and a long compare session must not hide it. */}
      <UpdateBanner />
    </>
  );
}
