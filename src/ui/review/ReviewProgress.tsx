import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useDetailedReviewStore } from "../../stores/detailedReviewStore";
import type { ActivityEntry } from "../../lib/reviewService";

// The detailed review's progress, in the two places it has to be visible: the BOM tab
// it was launched from, and the footer, which is on screen in every view.
//
// **A bar and a number, nothing else.** It used to print the step it was on beside the
// bar ("Detailed review · Reviewing against datasheets · 3 of 16 checks · 2 findings"),
// which is a sentence that changes width every time it changes, in a footer where
// everything else holds still. The words also said less than they looked like they
// said: a step name is the same for eight minutes, so the line read as frozen even
// while the bar moved. A percentage is the one thing a reader wants from a progress
// bar — is this halfway or nearly done — and it is three characters wide. The words
// are still there for anyone who wants them, in the tooltip.
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

  // Everything the line used to print, now in the tooltip: which step, how far into
  // it, and that datasheets are being fetched (which is why it is slow, and is not a
  // hang). Nobody needs it at a glance; the people who want it know to hover.
  const detail = step === 2 && review ? stageDetail(review) : null;

  const [open, setOpen] = useState(false);

  return (
    <span className="review-progress-wrap">
      {open && <ActivityFeed onClose={() => setOpen(false)} />}
      <button
      type="button"
      className="review-progress"
      onClick={() => setOpen((v) => !v)}
      aria-expanded={open}
      title={
        `Detailed BOM review — ${label}${detail ? ` · ${detail}` : ""}` +
        `${found > 0 ? ` · ${found} finding${found === 1 ? "" : "s"} so far` : ""}` +
        `
Click for activity`
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
      <span className="review-progress-pct">{shown}%</span>
      </button>
    </span>
  );
}

/** The event stream, newest at the bottom — the direction a log reads. */
function ActivityFeed({ onClose }: { onClose: () => void }) {
  const activity = useDetailedReviewStore((s) => s.activity);
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
