import { useEffect, useRef } from "react";
import { isAgentRunning, useAgentReviewStore } from "../../stores/agentReviewStore";
import { useBomCheckStore } from "../../stores/bomCheckStore";
import { isRunning, useDetailedReviewStore } from "../../stores/detailedReviewStore";
import { reviewRows, useReviewRunsStore } from "../../stores/reviewRunsStore";
import { useReviewInboxStore } from "../../stores/reviewInboxStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { useReviewStore } from "../../stores/reviewStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { formatRelative } from "../../lib/time";
import type { CommentSeverity } from "../../lib/types";
import type { ReviewKindId } from "../../lib/reviewCatalog";
import { IconSparkle } from "../icons";
import { ReviewOutcome } from "./ReviewOutcome";
import { ReviewProgress } from "./ReviewProgress";

// "Run a review" — the single launcher, in the status bar.
//
// The footer rather than the sidebar for one decisive reason: the comments panel can
// be collapsed (that is the whole point of the collapse control), and a launcher that
// disappears when you go heads-down on the canvas disappears exactly when you want it.
// The footer is present on every view, in full screen, and beside the progress the
// button turns into while a run is going.
//
// Rows are one line each because scope moved into the review's own setup sheet — so
// this list doubles as the coverage readout: what has run, when, and what has gone
// stale since. Unbuilt reviews are listed too (decision 2026-08-24): the picker is the
// answer to "what has this board been through", and a hidden review reads like one
// that passed.

const SEV_RANK: Record<CommentSeverity, number> = { info: 0, minor: 1, major: 2, critical: 3 };

/** Which review filed this comment, from the pipeline the backend stamps into every
 *  machine-filed comment's predicate (`bomcheck::ingest`). Pipelines are named
 *  `<kind>-<tier>` — "bom-rules" and "bom-detailed" are both the BOM review — so the
 *  kind is the part before the dash and a second review type needs nothing here.
 *  Human comments and anything unattributable belong to no review. */
function filedBy(c: { source: string; predicate: unknown }): string | null {
  if (c.source === "human") return null;
  const pipeline = (c.predicate as { pipeline?: unknown } | null)?.pipeline;
  if (typeof pipeline !== "string" || !pipeline) return null;
  return pipeline.split("-")[0];
}

/** Open findings this review filed, for the row's count. The worst severity among
 *  them rides along so the count can be coloured: "3 open" is a different fact
 *  depending on whether the worst of the three is info or critical. */
function openFindings(
  id: ReviewKindId,
  comments: { status: string; source: string; severity: CommentSeverity | null; predicate: unknown }[],
): { count: number; worst: CommentSeverity } {
  let count = 0;
  let worst: CommentSeverity = "info";
  for (const c of comments) {
    if (c.status !== "open" || filedBy(c) !== id) continue;
    count++;
    const sev = c.severity ?? "info";
    if (SEV_RANK[sev] > SEV_RANK[worst]) worst = sev;
  }
  return { count, worst };
}

export function RunReviewMenu() {
  const menuOpen = useRunLauncherStore((s) => s.menuOpen);
  const toggleMenu = useRunLauncherStore((s) => s.toggleMenu);
  const closeMenu = useRunLauncherStore((s) => s.closeMenu);
  const openSetup = useRunLauncherStore((s) => s.openSetup);
  const openConnect = useRunLauncherStore((s) => s.openConnect);

  const runs = useReviewRunsStore((s) => s.runs);
  const current = useReviewRunsStore((s) => s.current);
  const comments = useReviewStore((s) => s.comments);
  // What is waiting in the project's review drop-box. Listed here rather than in a
  // corner of the BOM tab because this popover already answers "what has this board
  // been through" — and a review that ran on the user's own agent, outside this
  // window, is exactly that question's newest answer.
  const inbox = useReviewInboxStore((s) => s.entries);
  const importing = useReviewInboxStore((s) => s.importing);
  const loadInbox = useReviewInboxStore((s) => s.load);
  const importInbox = useReviewInboxStore((s) => s.importOne);

  // Running state. The two BOM tiers are separate stores and can both be in flight,
  // so the footer counts jobs rather than showing a single boolean — concurrent runs
  // are allowed (decision 2026-08-24).
  const bomRunning = useBomCheckStore((s) => s.running);
  const detailedPhase = useDetailedReviewStore((s) => s.phase);
  const detailedBusy = isRunning(detailedPhase);
  // A review running through the user's own assistant has no stages of ours to
  // report, so it gets a line of its own rather than the detailed review's bar.
  const agentPhase = useAgentReviewStore((s) => s.phase);
  const agentLine = useAgentReviewStore((s) => s.line);
  const agentBusy = isAgentRunning(agentPhase);

  // Whether the assistant has been set up at all. Shown on the row rather than
  // hidden behind a click: "set up" and "configured" answer different questions, and
  // a row that reads the same either way is a row nobody revisits when it breaks.
  const agentConfigured = useSettingsStore((s) => s.agentReview !== null);

  const wrapRef = useRef<HTMLDivElement | null>(null);

  // Dismiss on an outside click or Esc — a popover anchored to the status bar has no
  // backdrop, so it must not survive the next thing the user does.
  useEffect(() => {
    if (!menuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) closeMenu();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        closeMenu();
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [menuOpen, closeMenu]);

  // Read on open rather than polled: the drop-box changes when a review the user
  // started elsewhere finishes, and opening the launcher is when they ask.
  useEffect(() => {
    if (menuOpen) void loadInbox();
  }, [menuOpen, loadInbox]);

  const rows = reviewRows(runs, current);

  return (
    <div className="run-review" ref={wrapRef}>
      {/* The detailed run takes minutes, so it gets the bar; the instant check only ever
          needs to say that it is going. */}
      {detailedBusy && <ReviewProgress />}
      {/* The footer is on screen in every view, which is exactly why the last run's
          failure belongs here: it renders nothing on a healthy run, and does not go
          away by itself on a bad one. */}
      <ReviewOutcome />
      {agentBusy && (
        <span className="run-review-active" title={agentLine || "Your assistant is reviewing this BOM"}>
          <span className="status-dot running" />
          {/* The assistant's own last line, when it has said something. It narrates at
              its own pace, and a sign of life beats a bar we would have to invent. */}
          {agentLine ? agentLine.slice(0, 60) : "Assistant reviewing"}
        </span>
      )}
      {bomRunning && (
        <span className="run-review-active">
          <span className="status-dot running" />
          Instant check
        </span>
      )}
      <button
        className={`run-review-btn ${menuOpen ? "on" : ""}`}
        title="Run a review (Ctrl+R)"
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={() => toggleMenu()}
      >
        <IconSparkle size={13} />
        Run a review
        <span className="run-review-caret">▴</span>
      </button>

      {menuOpen && (
        <div className="run-review-pop" role="menu">
          <div className="run-review-pop-hd">Run a review</div>
          {rows.map(({ kind, run, stale }) => {
            const { count, worst } = openFindings(kind.id, comments);
            return (
              <button
                key={kind.id}
                role="menuitem"
                className={`run-review-row ${kind.ready ? "" : "soon"}`}
                disabled={!kind.ready}
                // Unbuilt reviews carry no tooltip: describing what a row cannot do yet
                // is an explanation nobody asked for.
                title={kind.ready ? kind.blurb : undefined}
                onClick={() => openSetup(kind.id)}
              >
                <span className="run-review-name">{kind.label}</span>
                <span className="run-review-meta">
                  {!kind.ready ? (
                    // Right-hand column, with the other rows' status: "coming soon" IS
                    // this row's status, and beside the name it left the column ragged.
                    <span className="tag-soon">coming soon</span>
                  ) : !run ? (
                    "never run"
                  ) : (
                    <>
                      {count > 0 && (
                        <span className={`run-review-open sev-${worst}`}>{count} open</span>
                      )}
                      <span>{`ran ${formatRelative(run.ts)}`}</span>
                      {stale && <span className="run-review-stale">stale</span>}
                    </>
                  )}
                </span>
              </button>
            );
          })}
          {/* Below the reviews rather than above them: this is setup, and setup that
              sits at the top of a list of actions reads as the first step every time
              you open it, which it is exactly once. */}
          <button
            role="menuitem"
            className="run-review-row"
            title="Run SpinZero reviews through Claude Code, Cursor, or any MCP client — on your own subscription"
            onClick={() => openConnect()}
          >
            <span className="run-review-name">Connect your AI assistant</span>
            <span className="run-review-meta">{agentConfigured ? "configured" : "set up"}</span>
          </button>
          {inbox.length > 0 && (
            <>
              <div className="run-review-pop-hd sub">Findings waiting to import</div>
              {inbox.map((entry) => (
                <button
                  key={entry.name}
                  role="menuitem"
                  className="run-review-row"
                  disabled={Boolean(entry.error) || importing !== null}
                  // The file name is the only handle on WHICH run this was, and the
                  // rows are one line, so it lives in the tooltip with the reason a
                  // broken one cannot be imported.
                  title={entry.error ? `${entry.name} — ${entry.error}` : entry.name}
                  onClick={() => void importInbox(entry.name)}
                >
                  <span className="run-review-name">
                    {entry.error ? entry.name : `Import ${entry.pipeline || "review"}`}
                  </span>
                  <span className="run-review-meta">
                    {entry.error ? (
                      <span className="run-review-stale">unreadable</span>
                    ) : importing === entry.name ? (
                      "importing…"
                    ) : (
                      `${entry.finding_count} finding${entry.finding_count === 1 ? "" : "s"}`
                    )}
                  </span>
                </button>
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}
