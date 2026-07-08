import { useEffect, useState } from "react";
import { useReviewStore } from "../../stores/reviewStore";
import { useDiffStore } from "../../stores/diffStore";
import { ReviewPanel } from "../review/ReviewPanel";
import { ChangesPanel } from "../diff/ChangesPanel";
import { UpdateBanner } from "./UpdateBanner";
import { IconChecklist, IconHistory } from "../icons";

// The LEFT rail is the global review surface — identical in every view
// (docs/phase2-ui-plan.md §1/§2). A second "Changes" tab appears ONLY while a visual
// diff is active (visual-diff §5); it auto-activates on enter and disappears on exit,
// so normal viewing is unchanged.
export function LeftPanel() {
  const setReviewTab = useReviewStore((s) => s.setLeftTab);
  const diffActive = useDiffStore((s) => s.active);
  // While diff mode is active the rail exposes both tabs; "changes" auto-selects on
  // enter. On exit we always fall back to the Review surface.
  const [tab, setTab] = useState<"review" | "changes">("review");
  useEffect(() => {
    setTab(diffActive ? "changes" : "review");
  }, [diffActive]);

  const showingChanges = diffActive && tab === "changes";

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
          onClick={() => {
            setTab("review");
            setReviewTab("review");
          }}
        >
          <IconChecklist size={15} />
          Review
        </button>
      </div>
      {showingChanges ? <ChangesPanel /> : <ReviewPanel />}
      {/* Pinned to the bottom of the rail (batch1): a persistent update notice in place
          of the old transient toast. Renders nothing unless an update is downloaded. */}
      {!diffActive && <UpdateBanner />}
    </>
  );
}
