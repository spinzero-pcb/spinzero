import { useEffect } from "react";
import { useBomCheckStore } from "../stores/bomCheckStore";
import { useBomMappingStore } from "../stores/bomMappingStore";
import { useDetailedReviewStore } from "../stores/detailedReviewStore";
import { useReviewStore } from "../stores/reviewStore";
import { BOM_PROFILES, isBomProfile, severityCounts } from "../lib/findings";
import type { FindingSeverity } from "../lib/findings";
import type { CommentSeverity } from "../lib/types";
import { IconChecklist } from "./icons";

// BOM check strip — the free deterministic review, run from the BOM tab.
//
// The findings themselves are NOT rendered here: they are filed as review comments,
// so they appear in the review rail and as per-row chips in the table, exactly like a
// human's comment. This strip is the *run* surface — pick the end application, run it,
// and read what the run did.
//
// The paid "Detailed review" sits beside the free check deliberately: the two emit the
// same findings document and land through the same ingestion path, differing only in
// what validated the findings. One strip, one severity summary — not two panels
// competing to be the review.

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
  const profile = useBomCheckStore((s) => s.profile);
  const setProfile = useBomCheckStore((s) => s.setProfile);
  const run = useBomCheckStore((s) => s.run);
  // Paid tier. `phase` drives the button: idle → open the pre-flight dialog; running
  // → show the live stage and offer a cancel.
  const detailedPhase = useDetailedReviewStore((s) => s.phase);
  const detailedProgress = useDetailedReviewStore((s) => s.progress);
  const detailedError = useDetailedReviewStore((s) => s.error);
  const liveFindings = useDetailedReviewStore((s) => s.liveFindings);
  const detailedBusy = detailedPhase === "submitting" || detailedPhase === "running" || detailedPhase === "ingesting";

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
        title="Run the deterministic BOM checks (Ctrl/⌘+Shift+B) — findings are filed as review comments"
        onClick={() => void run()}
      >
        <IconChecklist size={14} />
        {running ? "Checking…" : "Check BOM"}
      </button>
      <select
        className="bom-select"
        value={profile}
        disabled={running}
        title="End application — decides which rules apply and how severe a gap is"
        onChange={(e) => isBomProfile(e.target.value) && setProfile(e.target.value)}
      >
        {BOM_PROFILES.map((p) => (
          <option key={p.id} value={p.id}>
            {p.label}
          </option>
        ))}
      </select>

      <button
        className="btn-ghost bom-check-mapping"
        disabled={running}
        title="Show which BOM column each check reads, and correct it"
        onClick={() => void useBomMappingStore.getState().openDialog(profile)}
      >
        Mapping
      </button>

      <button
        className="btn-ghost bom-check-detailed"
        disabled={running || detailedBusy}
        title="Send the BOM to the review service for an LLM-validated review — you see the exact file list before anything is uploaded"
        onClick={() => void useDetailedReviewStore.getState().openPreflight()}
      >
        {detailedBusy ? "Reviewing…" : "Detailed review"}
      </button>

      {detailedBusy && (
        <>
          <span className="bom-check-count">
            {detailedProgress}
            {liveFindings > 0 ? ` · ${liveFindings} so far` : ""}
          </span>
          <button
            className="btn-ghost bom-check-open"
            title="Cancel the detailed review"
            onClick={() => void useDetailedReviewStore.getState().cancel()}
          >
            Cancel
          </button>
        </>
      )}

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
      {error && <span className="bom-check-warn">Check failed: {error}</span>}
      {/* The detailed review is optional by design: when the service is unreachable
          the free results above stay exactly as they were, and the reason sits here
          rather than replacing them. */}
      {detailedError && <span className="bom-check-warn">Detailed review: {detailedError}</span>}
    </div>
  );
}
