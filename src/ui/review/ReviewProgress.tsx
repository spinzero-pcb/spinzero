import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  isStalled,
  percentOf,
  progressLabel,
  useAgentReviewStore,
  type ActivityEntry,
} from "../../stores/agentReviewStore";

// The detailed review's progress, in the two places it has to be visible: the BOM tab
// it was launched from, and the footer, which is on screen in every view.
//
// **A bar and a number, nothing else.** A percentage is the one thing a reader wants
// from a progress bar — is this halfway or nearly done — and it is three characters
// wide. The words are in the tooltip.
//
// **The numbers are the review server's, not ours.** They come from the run's own
// `status.json`: datasheets collected, then part numbers accounted for. The bar used
// to interpolate on rule candidates, which was the closest thing available when the
// app was running the pipeline itself. Accounting for every part IS the job, so that
// is the fraction the middle of the bar fills on.
//
// **A stalled run says so.** A watcher is not a subscription: the file stops moving
// when the run dies, and a bar that keeps saying "68%" over a dead agent is the one
// failure this surface must not have.
//
// Everything shown here is a count. `status.json` carries no BOM content, so there is
// no part number to show even if we wanted one.

export function ReviewProgress() {
  const status = useAgentReviewStore((s) => s.status);
  const line = useAgentReviewStore((s) => s.line);
  const phase = useAgentReviewStore((s) => s.phase);

  // Re-rendered on a timer, because a stall is the passage of time and nothing else
  // arrives to trigger a render. One tick a minute is enough to notice one.
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 60_000);
    return () => clearInterval(t);
  }, []);

  const stalled = isStalled(status);
  const shown = Math.round(percentOf(status));
  const label = progressLabel(status, phase === "starting" ? "Starting your agent" : "Starting the review");

  const [open, setOpen] = useState(false);

  return (
    <span className="review-progress-wrap">
      {open && <ActivityFeed onClose={() => setOpen(false)} />}
      <button
        type="button"
        className={`review-progress ${stalled ? "stalled" : ""}`}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        title={
          `Detailed BOM review — ${label}` +
          (stalled ? "\nNothing has been reported for a while. The run may have stopped." : "") +
          (line ? `\n${line}` : "") +
          `\nClick for activity`
        }
        aria-label="Detailed BOM review progress — click for activity"
      >
        <span
          className="review-progress-bar"
          role="progressbar"
          aria-valuenow={shown}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <span className="review-progress-fill" style={{ width: `${shown}%` }} />
        </span>
        <span className="review-progress-pct">{stalled ? "stalled" : `${shown}%`}</span>
      </button>
    </span>
  );
}

/** The event stream, newest at the bottom — the direction a log reads. */
function ActivityFeed({ onClose }: { onClose: () => void }) {
  const activity = useAgentReviewStore((s) => s.activity);
  const scroller = useRef<HTMLDivElement>(null);
  // Follow the tail, but only while the reader is AT the tail: yanking someone back
  // down every fifteen seconds makes the panel unusable for the one thing it is for,
  // which is scrolling back to find where the time went.
  const pinned = useRef(true);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [activity]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="review-activity" role="log" aria-label="Detailed review activity">
      <div className="review-activity-head">
        <span>Activity</span>
        <button className="btn-ghost review-activity-close" onClick={onClose} aria-label="Close activity">
          ✕
        </button>
      </div>
      <div
        className="review-activity-list"
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
      >
        {activity.length === 0 ? (
          <p className="review-activity-empty">Nothing yet — the run has not reported in.</p>
        ) : (
          activity.map((entry, i) => <ActivityRow key={entry.seq} entry={entry} previous={activity[i - 1]} />)
        )}
      </div>
    </div>
  );
}

function ActivityRow({ entry, previous }: { entry: ActivityEntry; previous?: ActivityEntry }) {
  const gapMs = previous ? Date.parse(entry.ts) - Date.parse(previous.ts) : 0;
  // Only gaps worth explaining. Several lines in the same second is normal, and
  // printing "+0s" on each of them buries the one row that says "+6m45s".
  const gap = gapMs >= 20_000 ? formatGap(gapMs) : null;
  return (
    <div className={`review-activity-row tone-${entry.tone}`} title={entry.text}>
      <span className="review-activity-time">{entry.ts.slice(11, 19)}</span>
      <span className="review-activity-text">{entry.text}</span>
      {gap && <span className="review-activity-gap">+{gap}</span>}
    </div>
  );
}

function formatGap(ms: number): string {
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
}
