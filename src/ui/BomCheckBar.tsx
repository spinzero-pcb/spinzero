import { useEffect } from "react";
import { useBomCheckStore } from "../stores/bomCheckStore";
import { isAgentRunning, useAgentReviewStore } from "../stores/agentReviewStore";
import { useBomMappingStore } from "../stores/bomMappingStore";
import { useRunLauncherStore } from "../stores/runLauncherStore";
import { useReviewStore } from "../stores/reviewStore";
import {
  BOM_PROFILES,
  executionSummary,
  isClaim,
  resolveBomProfile,
  reviewTallies,
  UNSTATED_BOM_PROFILE,
} from "../lib/findings";
import type { CommentSeverity } from "../lib/types";
import { IconChecklist } from "./icons";

// BOM strip — the BOM tab's door into the review, and the last run's result.
//
// The findings themselves are NOT rendered here: they are filed as review comments,
// so they appear in the review rail and as per-row chips in the table. This strip is
// one button (opens the BOM Review window) and the result as clickable chips.
// Everything else — application, mapping, depth — lives in that window, and progress
// and failures live in the footer, which is on screen in every view.
//
// **What is a chip and what is a tooltip.** A chip is a number that changes what the
// user does next: fix something (Critical, Non-critical), distrust a clean result
// (not verified), or fix the setup (no application, unread columns). A chip that
// would read zero is not shown. Facts ABOUT the run — what it ran as, what changed
// since last time, how many rows it touched — are in the chips' tooltip.

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
  // The detailed run's progress is in the footer; here the button just says it is busy.
  const detailedBusy = useAgentReviewStore((s) => isAgentRunning(s.phase));

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

  // Counted by predicate — see `reviewTallies`. "Not verified" is its own number: a
  // review with blind spots and no findings must not read as a pass.
  const tallies = reviewTallies(doc);
  const unstated = doc ? profileLabel(doc.profile) === null : false;
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
  const about = doc
    ? [
        `${rowsWithFindings(doc)} of ${doc.stats.item_count} rows have findings`,
        profileLabel(doc.profile) ? `Ran as ${profileLabel(doc.profile)}` : "",
        delta,
        execution?.text ?? "",
      ]
        .filter(Boolean)
        .join("\n")
    : "";
  const clean = doc && tallies.critical === 0 && tallies.nonCritical === 0;

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

      {clean && (
        <span className="bom-check-count ok" title={about}>
          No issues found
        </span>
      )}
      {tallies.critical > 0 && (
        <button
          className={`bom-check-sev sev-${CRITICAL_ROLE}`}
          title={`Show in the review panel\n${about}`}
          onClick={() => showInReview(CRITICAL_ROLE)}
        >
          {tallies.critical} Critical
        </button>
      )}
      {tallies.nonCritical > 0 && (
        <button
          className={`bom-check-sev sev-${NONCRITICAL_ROLE}`}
          title={`Show in the review panel\n${about}`}
          onClick={() => showInReview(NONCRITICAL_ROLE)}
        >
          {tallies.nonCritical} Non-critical
        </button>
      )}
      {tallies.notVerified > 0 && (
        <span
          className="bom-check-gaps some"
          title="Checks this review could not make. Open “Not fully verified” for the list."
        >
          {tallies.notVerified} not verified
        </span>
      )}
      {unstated && (
        <button
          className="btn-ghost bom-check-warn"
          title="The review ran with no end application, so it could not tell how severe a gap is. Click to set one."
          onClick={() => useRunLauncherStore.getState().openSetup("bom")}
        >
          No application
        </button>
      )}

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

/** The end application in the user's words, or null when none was stated. */
function profileLabel(profile: string): string | null {
  if (!profile) return null;
  // Through the resolver, so a document that stored the retired `automotive` id still
  // reads as a label rather than as a raw word from a JSON file.
  const id = resolveBomProfile(profile);
  if (id === UNSTATED_BOM_PROFILE) return null;
  return BOM_PROFILES.find((p) => p.id === id)?.label ?? id;
}
