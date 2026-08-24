import { useDiffStore } from "../../stores/diffStore";
import { useReviewStore } from "../../stores/reviewStore";
import { useViewStore } from "../../stores/viewStore";
import { ReviewPanel } from "../review/ReviewPanel";
import { ChangesPanel } from "../diff/ChangesPanel";
import { UpdateBanner } from "./UpdateBanner";
import { IconChevronLeft, IconHistory } from "../icons";

// The LEFT panel is the global comments surface — identical in every view.
//
// It is called **Comments**, not Findings: the list carries `human`, `rule` and
// `agent` items in one stream, and "findings" mislabels the notes the user wrote
// themselves. Naming it this also retires the old collision, where the rail said
// "Review" twice (once to launch, once to read) — launching now lives in the footer's
// "Run a review", and the word survives in the UI only as a verb.
//
// A second "Changes" tab appears ONLY while a visual diff is active (visual-diff §5);
// diffStore enter/exit drive the selection, so normal viewing is unchanged.
export function LeftPanel() {
  const setTab = useReviewStore((s) => s.setLeftTab);
  const tab = useReviewStore((s) => s.leftTab);
  const comments = useReviewStore((s) => s.comments);
  const diffActive = useDiffStore((s) => s.active);
  const setFullscreen = useViewStore((s) => s.setFullscreen);

  // "changes" is only meaningful while a diff is active; outside it the panel is comments.
  const showingChanges = diffActive && tab === "changes";
  const openCount = comments.filter((c) => c.status === "open").length;

  return (
    <>
      {diffActive ? (
        <div className="left-tabs">
          <button
            className={`left-tab ${showingChanges ? "active" : ""}`}
            onClick={() => setTab("changes")}
          >
            <IconHistory size={15} />
            Changes
          </button>
          <button
            className={`left-tab ${!showingChanges ? "active" : ""}`}
            onClick={() => setTab("review")}
          >
            Comments
          </button>
          <button
            className="left-collapse"
            title="Hide the panel (F11)"
            aria-label="Hide the panel"
            onClick={() => setFullscreen(true)}
          >
            <IconChevronLeft size={13} />
          </button>
        </div>
      ) : (
        <div className="left-tabs">
          <span className="left-title">
            Comments
            {openCount > 0 && <span className="left-title-n">{openCount}</span>}
          </span>
          <button
            className="left-collapse"
            title="Hide the panel (F11)"
            aria-label="Hide the panel"
            onClick={() => setFullscreen(true)}
          >
            <IconChevronLeft size={13} />
          </button>
        </div>
      )}
      {showingChanges ? <ChangesPanel /> : <ReviewPanel />}
      {/* Pinned to the bottom of the panel (batch1): a persistent update notice in place
          of the old transient toast. Renders nothing unless an update is downloaded, so
          it stays mounted through diff mode too — it's the only surface that announces a
          ready update, and a long compare session must not hide it. */}
      <UpdateBanner />
    </>
  );
}
