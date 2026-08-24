import { useEffect } from "react";
import { useBomCheckStore } from "../stores/bomCheckStore";
import { isRunning, useDetailedReviewStore } from "../stores/detailedReviewStore";
import { useProjectStore } from "../stores/projectStore";
import { useRunLauncherStore } from "../stores/runLauncherStore";
import { useReviewStore } from "../stores/reviewStore";
import { severityCounts } from "../lib/findings";
import { isProjectClass, PROJECT_CLASSES } from "../lib/projectClass";
import { runHealthSummary } from "../lib/reviewService";
import type { FindingSeverity } from "../lib/findings";
import type { CommentSeverity } from "../lib/types";
import { IconChecklist } from "./icons";
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
 *  finding chip is the same colour here as its comment is in the rail. */
const SEVERITY_ROLE: Record<FindingSeverity, CommentSeverity> = {
  Critical: "critical",
  Major: "major",
  Medium: "minor",
  Low: "info",
  Question: "info",
};

/** The chips are labelled in that same vocabulary. The strip used to print the raw
 *  findings-schema words ("Medium", "Low") while the rail said "Minor"/"Info" for the
 *  very same comments — two names for one level, and two chips ("Low", "Question")
 *  that both filtered the rail to the same "info" set. One vocabulary, one chip
 *  per level. */
const ROLE_LABEL: Record<CommentSeverity, string> = {
  critical: "Critical",
  major: "Major",
  minor: "Minor",
  info: "Info",
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

  // Findings-severity counts folded onto the review vocabulary, so "Low" and
  // "Question" land in one "Info" chip instead of two chips filtering to the same
  // rail. `severityCounts` walks SEVERITY_ORDER, so Map order stays worst-first.
  // Stages that did not fully run in the review that produced `doc` (paid tier only
  // — the free deterministic check has nothing to degrade). null on a clean run.
  const health = runHealthSummary(doc);

  const counts = (() => {
    if (!doc) return [] as { role: CommentSeverity; n: number }[];
    const byRole = new Map<CommentSeverity, number>();
    for (const c of severityCounts(doc)) {
      const role = SEVERITY_ROLE[c.severity];
      byRole.set(role, (byRole.get(role) ?? 0) + c.n);
    }
    return [...byRole].map(([role, n]) => ({ role, n }));
  })();

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
              key={c.role}
              className={`bom-check-sev sev-${c.role}`}
              title={`Show the ${c.role} findings in the review panel`}
              onClick={() => showInReview(c.role)}
            >
              {c.n} {ROLE_LABEL[c.role]}
            </button>
          ))}
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
      {/* A review whose validation or judgment stage died (provider rate limit, cost
          cap, timeout) still returns findings, and the job itself reports "completed"
          — so without this the user reads an incomplete review as a full one. The
          reason travels in the document's `run_health`; the tooltip carries every
          stage verbatim. */}
      {health && (
        <span className="bom-check-warn" title={`This review is incomplete.\n\n${health.detail}`}>
          Incomplete review: {health.text}
        </span>
      )}
      {error && <span className="bom-check-warn">Check failed: {error}</span>}
    </div>
  );
}
