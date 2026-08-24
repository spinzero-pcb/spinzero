import { useEffect, useRef } from "react";
import { useBomCheckStore } from "../../stores/bomCheckStore";
import { isRunning, useDetailedReviewStore } from "../../stores/detailedReviewStore";
import { reviewRows, useReviewRunsStore } from "../../stores/reviewRunsStore";
import { useReviewStore } from "../../stores/reviewStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { formatRelative } from "../../lib/time";
import type { CommentSeverity } from "../../lib/types";
import type { ReviewKindId } from "../../lib/reviewCatalog";
import { IconPremium, IconSparkle } from "../icons";
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

/** Open findings filed by a review, for the row's count. The worst severity among
 *  them rides along so the count can be coloured: "3 open" is a different fact
 *  depending on whether the worst of the three is info or critical.
 *  Today every producer files against the BOM; when a review type lands that files
 *  elsewhere it gains a discriminator here rather than in the row. */
function openFindings(
  id: ReviewKindId,
  comments: { status: string; source: string; severity: CommentSeverity | null }[],
): { count: number; worst: CommentSeverity } {
  if (id !== "bom") return { count: 0, worst: "info" };
  let count = 0;
  let worst: CommentSeverity = "info";
  for (const c of comments) {
    if (c.status !== "open" || c.source === "human") continue;
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

  const runs = useReviewRunsStore((s) => s.runs);
  const current = useReviewRunsStore((s) => s.current);
  const comments = useReviewStore((s) => s.comments);

  // Running state. The two BOM tiers are separate stores and can both be in flight,
  // so the footer counts jobs rather than showing a single boolean — concurrent runs
  // are allowed (decision 2026-08-24).
  const bomRunning = useBomCheckStore((s) => s.running);
  const detailedPhase = useDetailedReviewStore((s) => s.phase);
  const detailedBusy = isRunning(detailedPhase);

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

  const rows = reviewRows(runs, current);

  return (
    <div className="run-review" ref={wrapRef}>
      {/* The detailed run takes minutes, so it gets the bar; the instant check only ever
          needs to say that it is going. */}
      {detailedBusy && <ReviewProgress />}
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
                {kind.tier === "premium" && (
                  <span className="badge-premium" title="Premium review" aria-label="Premium review">
                    <IconPremium size={13} />
                  </span>
                )}
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
        </div>
      )}
    </div>
  );
}
