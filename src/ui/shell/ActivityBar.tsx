import { useReviewStore } from "../../stores/reviewStore";
import { IconChecklist } from "../icons";

// The activity bar drops the old files/Explorer entry — project structure is the
// per-view RIGHT panel now — and focuses the global LEFT rail's Review surface (the
// Phase-3 AI entry was removed for now).
export function ActivityBar() {
  const tab = useReviewStore((s) => s.leftTab);
  const setTab = useReviewStore((s) => s.setLeftTab);
  return (
    <nav className="activity-bar">
      <button
        className={`activity-btn ${tab === "review" ? "active" : ""}`}
        title="Review"
        onClick={() => setTab("review")}
      >
        <IconChecklist size={22} />
      </button>
    </nav>
  );
}
