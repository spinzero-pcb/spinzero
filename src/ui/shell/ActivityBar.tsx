import { useProjectStore } from "../../stores/projectStore";
import { useReviewStore } from "../../stores/reviewStore";
import { IconChecklist, IconHistory } from "../icons";

// The activity bar drops the old files/Explorer entry — project structure is the
// per-view RIGHT panel now — and carries the two persistent global surfaces: Review
// and History. History earned a rail slot because it had none: version control used to
// live behind a footer chip labelled with a timestamp, which is to say nowhere.
export function ActivityBar() {
  const tab = useReviewStore((s) => s.leftTab);
  const setTab = useReviewStore((s) => s.setLeftTab);
  const extractions = useProjectStore((s) => s.extractions);
  // Unpublished work is the one thing about history worth interrupting for, so the rail
  // carries its count — the reader shouldn't have to open the panel to learn they have
  // checkpoints their team can't see.
  const localCount = extractions.filter((e) => e.is_checkpoint && !e.published).length;

  return (
    <nav className="activity-bar">
      <button
        className={`activity-btn ${tab === "review" || tab === "changes" ? "active" : ""}`}
        title="Review"
        aria-label="Review"
        onClick={() => setTab("review")}
      >
        <IconChecklist size={22} />
      </button>
      <button
        className={`activity-btn ${tab === "history" ? "active" : ""}`}
        title={
          localCount > 0
            ? `Revision history — ${localCount} local checkpoint${localCount === 1 ? "" : "s"} not yet published`
            : "Revision history"
        }
        aria-label="Revision history"
        onClick={() => setTab("history")}
      >
        <IconHistory size={22} />
        {localCount > 0 && <span className="activity-badge">{localCount}</span>}
      </button>
    </nav>
  );
}
