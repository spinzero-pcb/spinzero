import { useEffect } from "react";
import { useBomCheckStore } from "../stores/bomCheckStore";
import { isAgentRunning, useAgentReviewStore } from "../stores/agentReviewStore";
import { useProjectStore } from "../stores/projectStore";
import { useRunLauncherStore } from "../stores/runLauncherStore";
import { useReviewStore } from "../stores/reviewStore";
import { executionSummary, isClaim, reviewTallies } from "../lib/findings";
import { isProjectClass, PROJECT_CLASSES } from "../lib/projectClass";
import { BOM_PROFILES, resolveBomProfile, UNSTATED_BOM_PROFILE } from "../lib/findings";
import type { CommentSeverity } from "../lib/types";
import { IconChecklist } from "./icons";
import { ReviewMapping } from "./review/ReviewMapping";
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
const CRITICAL_ROLE: CommentSeverity = "critical";
const NONCRITICAL_ROLE: CommentSeverity = "info";

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
  const detailedPhase = useAgentReviewStore((s) => s.phase);

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

  // THREE tallies, not one count. The strip used to show findings only, so a review
  // with twenty blind spots and no findings read as a pass. "Not verified" is the
  // third number, and it is the one that stops that — see `coverageGaps`.
  const tallies = reviewTallies(doc);
  // What the review ran as. The app picks it from the project class and never showed
  // the result of that choice, so a reader could not tell an automotive review from
  // one nobody stated an application for.
  const ranAs = doc ? profileLabel(doc.profile) : null;
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

      {isAgentRunning(detailedPhase) && <ReviewProgress />}

      {doc && (
        <>
          {/* A denominator, because "12 findings" does not answer the question a
              reader is actually asking. */}
          <span className="bom-check-count" title={`${doc.stats.item_count} BOM lines were read`}>
            {doc.findings.filter(isClaim).length === 0
              ? "No issues found"
              : `${rowsWithFindings(doc)} of ${doc.stats.item_count} rows have findings`}
          </span>
          <button
            className={`bom-check-sev sev-${CRITICAL_ROLE} ${tallies.critical ? "" : "none"}`}
            title="Show the critical findings in the review panel"
            onClick={() => showInReview(CRITICAL_ROLE)}
          >
            {tallies.critical} Critical
          </button>
          <button
            className={`bom-check-sev sev-${NONCRITICAL_ROLE} ${tallies.nonCritical ? "" : "none"}`}
            title="Show the non-critical findings in the review panel"
            onClick={() => showInReview(NONCRITICAL_ROLE)}
          >
            {tallies.nonCritical} Non-critical
          </button>
          {/* The third number, always shown. A zero here is news too: it is the only
              place that says the review actually checked everything it set out to. */}
          <span
            className={`bom-check-gaps ${tallies.notVerified ? "some" : "none"}`}
            title={
              tallies.notVerified
                ? "Checks this review could not make. Open “Not fully verified” for the list."
                : "Every check this review set out to make was made."
            }
          >
            {tallies.notVerified} not verified
          </span>
          {ranAs && (
            <span
              className={`bom-check-meta ${doc.column_mapping?.profile_stated === false ? "warn" : ""}`}
              title="The end application this review ran under. It decides which rules apply."
            >
              {ranAs}
            </span>
          )}
          <ReviewMapping doc={doc} />
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

/** How many BOM rows carry at least one finding. Counted over the anchors, because a
 *  finding can name several designators and several findings can land on one row.
 *
 *  Claims only. A row whose only entry is "we could not check this part" has no
 *  finding on it, and counting it here would report a blind spot as a defect. */
function rowsWithFindings(doc: { findings: { severity: string; anchors: { refdes?: string[] }[] }[] }): number {
  const rows = new Set<string>();
  for (const f of doc.findings.filter(isClaim)) {
    for (const a of f.anchors) for (const r of a.refdes ?? []) rows.add(r);
  }
  return rows.size;
}

/** The end application in the user's words. An unstated profile is named as one
 *  rather than shown as a plausible-looking setting nobody chose. */
function profileLabel(profile: string): string {
  if (!profile) return "No application stated";
  // Through the resolver, so a document that stored the retired `automotive` id still
  // reads as a label rather than as a raw word from a JSON file.
  const id = resolveBomProfile(profile);
  if (id === UNSTATED_BOM_PROFILE) return "No application stated";
  return BOM_PROFILES.find((p) => p.id === id)?.label ?? id;
}
