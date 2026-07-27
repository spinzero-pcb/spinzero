import { useReviewStore } from "../../stores/reviewStore";
import { useDiffStore } from "../../stores/diffStore";
import { ReviewPanel } from "../review/ReviewPanel";
import { ChangesPanel } from "../diff/ChangesPanel";
import { HistoryPanel } from "../history/HistoryPanel";
import { UpdateBanner } from "./UpdateBanner";
import { IconChecklist, IconHistory } from "../icons";

// The LEFT rail carries the two global surfaces, one per activity-bar icon: Review
// (docs/phase2-ui-plan.md §1/§2) and History. Within the Review surface a "Changes" tab
// appears ONLY while a visual diff is active (visual-diff §5); diffStore enter/exit
// drive the selection (auto-Changes on enter, back to Review on exit), so normal viewing
// is unchanged. The selected surface is reviewStore.leftTab — one source of truth shared
// with ActivityBar and diffStore.
export function LeftPanel() {
  const setTab = useReviewStore((s) => s.setLeftTab);
  const tab = useReviewStore((s) => s.leftTab);
  const diffActive = useDiffStore((s) => s.active);

  // "changes" is only meaningful while a diff is active; outside it the rail is Review.
  const showingChanges = diffActive && tab === "changes";
  const showingHistory = tab === "history";

  return (
    <>
      {!showingHistory && (
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
      )}
      {showingHistory ? <HistoryPanel /> : showingChanges ? <ChangesPanel /> : <ReviewPanel />}
      {/* Pinned to the bottom of the rail (batch1): a persistent update notice in place
          of the old transient toast. Renders nothing unless an update is downloaded, so
          it stays mounted through diff mode too — it's the only surface that announces a
          ready update, and a long compare session must not hide it. */}
      <UpdateBanner />
    </>
  );
}
