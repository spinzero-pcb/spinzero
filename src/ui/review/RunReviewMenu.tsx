import { useEffect, useRef } from "react";
import { useBomCheckStore } from "../../stores/bomCheckStore";
import { useDetailedReviewStore } from "../../stores/detailedReviewStore";
import { reviewRows, useReviewRunsStore } from "../../stores/reviewRunsStore";
import { useReviewStore } from "../../stores/reviewStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { formatRelative } from "../../lib/time";
import type { ReviewKindId } from "../../lib/reviewCatalog";
import { IconSparkle } from "../icons";

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

/** Open findings filed by a review, for the row's "N open" count. Today every
 *  producer files against the BOM; when a review type lands that files elsewhere it
 *  gains a discriminator here rather than in the row. */
function openCountFor(id: ReviewKindId, comments: { status: string; source: string }[]): number {
  if (id !== "bom") return 0;
  return comments.filter((c) => c.status === "open" && c.source !== "human").length;
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
  const detailedProgress = useDetailedReviewStore((s) => s.progress);
  const detailedBusy =
    detailedPhase === "submitting" || detailedPhase === "running" || detailedPhase === "ingesting";

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
  const running = [
    bomRunning ? "BOM checks" : null,
    detailedBusy ? `Detailed review${detailedProgress ? ` · ${detailedProgress}` : ""}` : null,
  ].filter(Boolean) as string[];

  return (
    <div className="run-review" ref={wrapRef}>
      {running.length > 0 && (
        <span className="run-review-active" title={running.join(" · ")}>
          <span className="status-dot running" />
          {running.length === 1 ? running[0] : `${running.length} reviews running`}
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
            const open = openCountFor(kind.id, comments);
            return (
              <button
                key={kind.id}
                role="menuitem"
                className={`run-review-row ${kind.ready ? "" : "soon"}`}
                disabled={!kind.ready}
                title={kind.blurb}
                onClick={() => openSetup(kind.id)}
              >
                <span className="run-review-name">{kind.label}</span>
                {kind.tier === "premium" && <span className="tag-premium">premium</span>}
                {!kind.ready && <span className="tag-soon">coming soon</span>}
                <span className="run-review-meta">
                  {!kind.ready ? (
                    ""
                  ) : !run ? (
                    "never run"
                  ) : (
                    <>
                      {open > 0 && `${open} open · `}
                      {`ran ${formatRelative(run.ts)}`}
                      {stale && <span className="run-review-stale"> · stale</span>}
                    </>
                  )}
                </span>
              </button>
            );
          })}
          <div className="run-review-pop-foot">Ctrl+R · each review opens its own setup</div>
        </div>
      )}
    </div>
  );
}
