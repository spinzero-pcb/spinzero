import { useEffect } from "react";
import { useBomCheckStore } from "../stores/bomCheckStore";
import { isRunning, useDetailedReviewStore } from "../stores/detailedReviewStore";
import { useProjectStore } from "../stores/projectStore";
import { useRunLauncherStore } from "../stores/runLauncherStore";
import { useReviewStore } from "../stores/reviewStore";
import { executionSummary, severityCounts } from "../lib/findings";
import { isProjectClass, PROJECT_CLASSES } from "../lib/projectClass";
import type { FindingSeverity } from "../lib/findings";
import type { CommentSeverity } from "../lib/types";
import { IconChecklist } from "./icons";
import { ReviewOutcome } from "./review/ReviewOutcome";
import { ReviewProgress } from "./review/ReviewProgress";

// BOM check strip — the free deterministic review, run from the BOM tab.
//
// The findings themselves are NOT rendered here: they are filed as review comments,
// so they appear in the review rail and as per-row chips in the table, exactly like a
// human's comment. This strip is the *run* surface — pick the end application, run it,
// and read what the run did.
//
// "Review BOM" is the visible door into the SAME setup sheet the footer's "Run a
// review" opens, pre-picked to this review — so the paid tier, the column mapping and
// the end application are all one dialog away from the table they describe. It is two
// entrances to one action, not two actions: nothing here runs a check by itself.

/** Findings-schema severity → the review UI's four-level severity vocabulary, so a
 *  finding chip is the same colour here as its comment is in the rail. Findings carry
 *  two levels and the rail's are persisted on disk, so they meet at the two ends of
 *  the rail's scale rather than in the middle. Mirrors `comment_severity` in
 *  `bomcheck.rs` — change both together. */
const SEVERITY_ROLE: Record<FindingSeverity, CommentSeverity> = {
  Important: "critical",
  Observation: "info",
};

/** One chip per findings severity, labelled in the findings vocabulary. Clicking one
 *  filters the rail by the comment severity it maps to. */
const SEVERITY_LABEL: Record<FindingSeverity, string> = {
  Important: "Important",
  Observation: "Observation",
};

export function BomCheckBar() {
  const running = useBomCheckStore((s) => s.running);
  const doc = useBomCheckStore((s) => s.doc);
  const summary = useBomCheckStore((s) => s.summary);
  const unmapped = useBomCheckStore((s) => s.unmappedColumns);
  const sessionId = useBomCheckStore((s) => s.sessionId);
  const error = useBomCheckStore((s) => s.error);
  const run = useBomCheckStore((s) => s.run);
  const cls = useProjectStore((s) => s.project?.class) ?? "general";
  const setClass = useProjectStore((s) => s.setClass);
  // The detailed run is the one that takes minutes, so the tab it belongs to says so
  // rather than leaving the footer as the only place it is visible.
  const detailedPhase = useDetailedReviewStore((s) => s.phase);

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

  // `severityCounts` walks SEVERITY_ORDER and drops empty levels, so chip order stays
  // worst-first and a clean run shows no chips at all.
  const counts = doc ? severityCounts(doc) : [];
  // Which content produced this. Absent on the free tier, which is deterministic and
  // has nothing to disclose; on the paid tiers it is the first thing to compare when
  // two runs of one board disagree.
  const execution = executionSummary(doc);

  return (
    <div className="bom-check-bar">
      <button
        className="btn-ghost bom-check-run"
        disabled={running}
        title="Set up and run a BOM review — depth, end application and column mapping"
        onClick={() => useRunLauncherStore.getState().openSetup("bom")}
      >
        <IconChecklist size={14} />
        {running ? "Checking…" : "Review BOM"}
      </button>
      <select
        className="bom-select"
        value={cls}
        disabled={running}
        title="End application — decides which rules apply and how severe a gap is"
        onChange={(e) => isProjectClass(e.target.value) && void setClass(e.target.value)}
      >
        {PROJECT_CLASSES.map((c) => (
          <option key={c.value} value={c.value}>
            {c.label}
          </option>
        ))}
      </select>

      {isRunning(detailedPhase) && <ReviewProgress />}

      {doc && (
        <>
          <span className="bom-check-count">
            {doc.findings.length === 0
              ? "No issues found"
              : `${doc.findings.length} finding${doc.findings.length === 1 ? "" : "s"}`}
          </span>
          {counts.map((c) => (
            <button
              key={c.severity}
              className={`bom-check-sev sev-${SEVERITY_ROLE[c.severity]}`}
              title={`Show the ${SEVERITY_LABEL[c.severity].toLowerCase()} findings in the review panel`}
              onClick={() => showInReview(SEVERITY_ROLE[c.severity])}
            >
              {c.n} {SEVERITY_LABEL[c.severity]}
            </button>
          ))}
          {execution && (
            <span className="bom-check-meta" title={execution.detail}>
              {execution.text}
            </span>
          )}
          {summary && (summary.filed > 0 || summary.auto_resolved > 0 || summary.reopened > 0) && (
            <span className="bom-check-delta">
              {[
                summary.filed ? `${summary.filed} new` : "",
                summary.reopened ? `${summary.reopened} reopened` : "",
                summary.auto_resolved ? `${summary.auto_resolved} auto-resolved` : "",
              ]
                .filter(Boolean)
                .join(" · ")}
            </span>
          )}
        </>
      )}

      {/* A column the checker couldn't map reads as "this data is missing" in every
          rule that needs it — say so out loud rather than letting the user trust a
          false all-clear. */}
      {unmapped.length > 0 && (
        <span
          className="bom-check-warn"
          title="These columns are well filled but did not map to a known BOM field, so the checks could not read them."
        >
          Unmapped: {unmapped.slice(0, 3).join(", ")}
          {unmapped.length > 3 ? ` +${unmapped.length - 3}` : ""}
        </span>
      )}
      {/* A review whose judgment stage died (provider rate limit, cost cap, timeout,
          a model that ruled on nothing) still returns findings, and the job itself
          reports "completed" — so without this the user reads an incomplete review as
          a full one. This used to be a grey chip here and nowhere else, which meant
          the failure was invisible from every other tab; `ReviewOutcome` is the same
          verdict, rendered as an alert, and it is in the footer too. */}
      <ReviewOutcome />
      {error && <span className="bom-check-warn">Check failed: {error}</span>}
    </div>
  );
}
