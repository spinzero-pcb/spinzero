import { useEffect } from "react";
import { useBomCheckStore } from "../stores/bomCheckStore";
import { isRunning, useDetailedReviewStore } from "../stores/detailedReviewStore";
import { useBomMappingStore } from "../stores/bomMappingStore";
import { useRunLauncherStore } from "../stores/runLauncherStore";
import { useReviewStore } from "../stores/reviewStore";
import { executionSummary, severityCounts } from "../lib/findings";
import type { FindingSeverity } from "../lib/findings";
import type { CommentSeverity } from "../lib/types";
import { IconChecklist } from "./icons";

// BOM strip — the BOM tab's door into the review, and the last run's result.
//
// The findings themselves are NOT rendered here: they are filed as review comments,
// so they appear in the review rail and as per-row chips in the table. This strip is
// one button (opens the BOM Review window) and the result as clickable severity
// chips. Everything else — application, mapping, depth — lives in that window, and
// progress and failures live in the footer, which is on screen in every view.

/** Findings-schema severity → the review UI's four-level severity vocabulary, so a
 *  finding chip is the same colour here as its comment is in the rail. Findings carry
 *  two levels and the rail's are persisted on disk, so they meet at the two ends of
 *  the rail's scale rather than in the middle. Mirrors `comment_severity` in
 *  `bomcheck.rs` — change both together. */
const SEVERITY_ROLE: Record<FindingSeverity, CommentSeverity> = {
  Critical: "critical",
  "Non-critical": "info",
};

/** One chip per findings severity, labelled in the findings vocabulary. Clicking one
 *  filters the rail by the comment severity it maps to. */
const SEVERITY_LABEL: Record<FindingSeverity, string> = {
  Critical: "Critical",
  "Non-critical": "Non-critical",
};

export function BomCheckBar() {
  const running = useBomCheckStore((s) => s.running);
  const doc = useBomCheckStore((s) => s.doc);
  const summary = useBomCheckStore((s) => s.summary);
  const unmapped = useBomCheckStore((s) => s.unmappedColumns);
  const sessionId = useBomCheckStore((s) => s.sessionId);
  const error = useBomCheckStore((s) => s.error);
  const run = useBomCheckStore((s) => s.run);
  // The detailed run's progress is in the footer; here the button just says it is busy.
  const detailedBusy = useDetailedReviewStore((s) => isRunning(s.phase));

  // Mod+Shift+B runs the check while the BOM tab is mounted. Scoped to this component
  // so it can never fire from the schematic/PCB canvases, where it would mean nothing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || !e.shiftKey) return;
      if (e.key.toLowerCase() !== "b") return;
      const el = document.activeElement;
      // Don't steal the combo from a text field the user is typing in.
      if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) return;
      e.preventDefault();
      void run();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [run]);

  /** Open the review rail on this run's session, filtered to one severity. */
  function showInReview(role: CommentSeverity) {
    const review = useReviewStore.getState();
    if (sessionId) review.setActiveSession(sessionId);
    review.setLeftTab("review");
    review.setFilterStatus("open");
    review.setFilterSeverity(role);
  }

  function openMapping() {
    useBomMappingStore.getState().setExpanded(true);
    useRunLauncherStore.getState().openSetup("bom");
  }

  // `severityCounts` walks SEVERITY_ORDER and drops empty levels, so chip order stays
  // worst-first and a clean run shows no chips at all.
  const counts = doc ? severityCounts(doc) : [];
  // What changed since the last run and which content produced it: useful when two
  // runs disagree, noise the rest of the time — so it is the result's tooltip.
  const execution = executionSummary(doc);
  const delta = summary
    ? [
        summary.filed ? `${summary.filed} new` : "",
        summary.reopened ? `${summary.reopened} reopened` : "",
        summary.auto_resolved ? `${summary.auto_resolved} auto-resolved` : "",
      ]
        .filter(Boolean)
        .join(" · ")
    : "";
  const resultTitle = [delta, execution?.text].filter(Boolean).join("\n") || undefined;

  return (
    <div className="bom-check-bar">
      <button
        className="btn-ghost bom-check-run"
        disabled={running}
        title="Set up and run a BOM review"
        onClick={() => useRunLauncherStore.getState().openSetup("bom")}
      >
        <IconChecklist size={14} />
        {running ? "Checking…" : detailedBusy ? "Review running…" : "Review BOM"}
      </button>

      {doc && counts.length === 0 && (
        <span className="bom-check-count ok" title={resultTitle}>
          No issues found
        </span>
      )}
      {counts.map((c) => (
        <button
          key={c.severity}
          className={`bom-check-sev sev-${SEVERITY_ROLE[c.severity]}`}
          title={`Show in the review panel${resultTitle ? `\n${resultTitle}` : ""}`}
          onClick={() => showInReview(SEVERITY_ROLE[c.severity])}
        >
          {c.n} {SEVERITY_LABEL[c.severity]}
        </button>
      ))}

      {/* A column the checker couldn't read looks like missing data to every rule
          that needs it. One click opens the mapping to fix it. */}
      {unmapped.length > 0 && (
        <button
          className="btn-ghost bom-check-warn"
          title={`Not read by any check: ${unmapped.join(", ")}\nClick to map them`}
          onClick={openMapping}
        >
          {unmapped.length} column{unmapped.length === 1 ? "" : "s"} unread
        </button>
      )}
      {error && (
        <span className="bom-check-warn" title={error}>
          Check failed
        </span>
      )}
    </div>
  );
}
