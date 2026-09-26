import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useDetailedReviewStore } from "../../stores/detailedReviewStore";
import type { ActivityEntry } from "../../lib/reviewService";

// The detailed review's progress. It lives in the footer only — on screen in every
// view — so there is exactly one progress indicator for one run.
//
// **Fixed-width, so the footer holds still:** a fixed label, the bar, a percentage and
// an elapsed clock (tabular figures). The step name changes rarely and says little at
// a glance, so it is in the tooltip and at the top of the panel the bar opens. The
// elapsed clock is what tells a slow run from a hung one when the percentage cannot
// move — the datasheet stretch can hold one number for minutes.
//
// The bar used to be `step / 3`, which meant it jumped to 66% and then sat perfectly
// still for the eight to ten minutes the review itself takes. A frozen bar does not
// read as "this is slow", it reads as "this has hung", and the only remedy on offer
// was cancelling a run that was working fine. So step 2 now has an inside: the
// judgment pass reports how many of the rule pack's checks it has ruled on, and that
// fraction — the one honest measure of the work remaining — fills the middle third.
//
// Everything shown here is a count. The event stream deliberately carries no BOM
// content, so there is no part number to show even if we wanted one.

/** Where each step starts and ends on the bar. Step 2 owns the middle third because
 *  it owns almost all of the wall clock. */
const SPAN: Record<1 | 2 | 3, [number, number]> = {
  1: [4, 30],
  2: [30, 88],
  3: [88, 100],
};

// **The bar opens.** Everything above is the answer for someone waiting; it is the
// wrong answer for someone asking "where is it stuck", and that question gets asked
// of every run that takes ten minutes. Clicking the bar unfolds the event stream the
// app already receives and used to throw away: each stage, each tool the model
// reached for, and — the row that settles the question — the loop's heartbeat while
// a single model turn runs. A gap is printed beside any row that took more than a
// few seconds to arrive, because "nothing happened for six minutes" is the finding.
//
// This is not gated behind a dev flag. It shows only what the events contract already
// permits on the wire — stage names, tool names, counts, durations — so there is
// nothing here to hide from a customer, and "what is it doing?" is a support question
// in production too. Tool ARGUMENTS are the exception: they carry part numbers, so the
// engine sends them only under SPINZERO_TRACE and they surface in a row's tooltip.

export function ReviewProgress() {
  const progress = useDetailedReviewStore((s) => s.progress);
  const step = useDetailedReviewStore((s) => s.step);
  const found = useDetailedReviewStore((s) => s.liveFindings);
  const review = useDetailedReviewStore((s) => s.reviewProgress);
  const startedAt = useDetailedReviewStore((s) => s.startedAt);
  const elapsed = useElapsed(startedAt);

  const label = progress || "Starting the review";
  const [from, to] = SPAN[step ?? 1];
  // Inside step 2, interpolate on checks-reviewed. Before the first report that
  // fraction is 0, so the bar sits at the start of the span rather than jumping.
  const within =
    step === 2 && review && review.candidates > 0
      ? Math.min(1, review.reviewed / review.candidates)
      : 0;
  const pct = step ? from + (to - from) * within : 4;
  // Rounded once, for the bar and the number together: a fill at 47.4% under a label
  // reading "47%" is the kind of mismatch someone eventually files a bug about.
  const shown = Math.round(pct);

  // The step and its detail live in the tooltip; the line itself holds still.
  const detail = step === 2 && review ? stageDetail(review) : null;

  const [open, setOpen] = useState(false);

  return (
    <span className="review-progress-wrap">
      {open && <ActivityFeed label={label} detail={detail} onClose={() => setOpen(false)} />}
      <button
        type="button"
        className="review-progress"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        title={
          `${label}${detail ? ` · ${detail}` : ""}` +
          `${found > 0 ? ` · ${found} finding${found === 1 ? "" : "s"} so far` : ""}` +
          "\nClick for details"
        }
        aria-label="Detailed BOM review progress — click for details"
      >
        <span className="review-progress-label">BOM review</span>
        <span
          className="review-progress-bar"
          role="progressbar"
          aria-valuenow={shown}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          {/* The shimmer says "alive" without claiming progress the run has not
              reported — the datasheet stretch can hold one number for minutes. */}
          <span className="review-progress-fill" style={{ width: `${shown}%` }} />
        </span>
        <span className="review-progress-pct">{shown}%</span>
        {elapsed && <span className="review-progress-time">{elapsed}</span>}
      </button>
    </span>
  );
}

/** "3:07" since `since`, ticking once a second; null when there is no run. */
function useElapsed(since: number | null): string | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === null) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [since]);
  if (since === null) return null;
  const s = Math.max(0, Math.floor((now - since) / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** The step it is on, the event stream behind it, and the way out. Newest event at
 *  the bottom — the direction a log reads. */
function ActivityFeed({
  label,
  detail,
  onClose,
}: {
  label: string;
  detail: string | null;
  onClose: () => void;
}) {
  const activity = useDetailedReviewStore((s) => s.activity);
  const cancel = useDetailedReviewStore((s) => s.cancel);
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
        <span className="review-activity-step">
          {label}
          {detail && <span className="review-activity-sub"> · {detail}</span>}
        </span>
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
      <div className="review-activity-foot">
        <button
          className="btn-ghost review-activity-cancel"
          onClick={() => {
            onClose();
            void cancel();
          }}
        >
          Cancel review
        </button>
      </div>
    </div>
  );
}

function ActivityRow({ entry, previous }: { entry: ActivityEntry; previous?: ActivityEntry }) {
  const gapMs = previous ? Date.parse(entry.ts) - Date.parse(previous.ts) : 0;
  // Only gaps worth explaining. Six events in the same millisecond is normal (one
  // turn's tool calls all report at once) and printing "+0s" on each of them buries
  // the one row that says "+6m45s".
  const gap = gapMs >= 20_000 ? formatGap(gapMs) : null;
  return (
    <div className={`review-activity-row tone-${entry.tone}`} title={entry.detail ?? entry.text}>
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

/** The middle of a long run, in the fewest words that still say what is happening.
 *  Datasheet count leads before any check has been ruled on, because that is the
 *  phase where nothing else has moved yet and the silence is what worries people. */
function stageDetail(review: { reviewed: number; candidates: number; datasheetsRead: number }): string {
  if (review.reviewed > 0 && review.candidates > 0) {
    return `${review.reviewed} of ${review.candidates} checks`;
  }
  if (review.datasheetsRead > 0) {
    return `${review.datasheetsRead} datasheet${review.datasheetsRead === 1 ? "" : "s"} read`;
  }
  return "reading datasheets";
}
