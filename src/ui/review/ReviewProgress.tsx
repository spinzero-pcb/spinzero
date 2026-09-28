import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  formatDuration,
  isStalled,
  percentOf,
  progressLabel,
  stepName,
  useAgentReviewStore,
  type ActivityEntry,
} from "../../stores/agentReviewStore";
import type { OpenStepStatus } from "../../lib/ipc";

// The detailed review's progress. It lives in the footer only — on screen in every
// view — so there is exactly one progress indicator for one run.
//
// **Fixed-width, so the footer holds still:** a fixed label, the bar, a percentage and
// an elapsed clock (tabular figures). The step name is in the tooltip and at the top
// of the panel the bar opens, with Cancel. The elapsed clock is what tells a slow run
// from a hung one when the percentage cannot move — the datasheet stretch can hold
// one number for minutes.
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
// **Up to three steps run at once.** The panel lists each open step with its own clock,
// and says which ones still wait for a sub-agent. The server's timestamps drive those
// clocks, so a window opened mid-run shows the right times.
//
// Everything shown here is a count. `status.json` carries no BOM content, so there is
// no part number to show even if we wanted one.

export function ReviewProgress() {
  const status = useAgentReviewStore((s) => s.status);
  const line = useAgentReviewStore((s) => s.line);
  const phase = useAgentReviewStore((s) => s.phase);
  const startedAt = useAgentReviewStore((s) => s.startedAt);
  const stepStartedAt = useAgentReviewStore((s) => s.stepStartedAt);
  // One clock for both: the elapsed time and the time on the open step. A step can take
  // several minutes with no count moving, and this is what shows the run is not frozen.
  const openSteps = status?.open_steps ?? [];
  const now = useNow(startedAt !== null || stepStartedAt !== null || openSteps.length > 0);
  const elapsed = startedAt === null ? null : clock(now - startedAt);
  // Each open step has its own row in the panel, with its own clock. A server too old
  // to list its steps gets the window's single step clock in the header instead.
  const onStep =
    openSteps.length === 0 && stepStartedAt !== null
      ? `on this step for ${formatDuration(now - stepStartedAt)}`
      : null;

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
      {open && (
        <ActivityFeed
          label={label}
          onStep={onStep}
          steps={openSteps}
          now={now}
          canCancel={startedAt !== null}
          onClose={() => setOpen(false)}
        />
      )}
      <button
        type="button"
        className={`review-progress ${stalled ? "stalled" : ""}`}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        title={
          label +
          (onStep ? ` · ${onStep}` : "") +
          (stalled ? "\nNothing has been reported for a while. The run may have stopped." : "") +
          (line ? `\n${line}` : "") +
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
          <span className="review-progress-fill" style={{ width: `${shown}%` }} />
        </span>
        <span className="review-progress-pct">{stalled ? "stalled" : `${shown}%`}</span>
        {elapsed && <span className="review-progress-time">{elapsed}</span>}
      </button>
    </span>
  );
}

/** The current time, ticking once a second while `active`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}

/** When a step's clock starts: when a sub-agent fetched it, else when it opened. Null
 *  when the server's timestamp cannot be read. */
function stepSince(step: OpenStepStatus): number | null {
  const at = Date.parse(step.handed_out_ts ?? step.opened_ts);
  return Number.isFinite(at) ? at : null;
}

/** "3:07" for a span in milliseconds. */
function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** The step it is on, the event stream behind it, and the way out. Newest event at
 *  the bottom — the direction a log reads. */
function ActivityFeed({
  label,
  onStep,
  steps,
  now,
  canCancel,
  onClose,
}: {
  label: string;
  /** "on this step for 3m12s", from a server that does not list its steps. */
  onStep: string | null;
  /** The open steps, one row each. */
  steps: OpenStepStatus[];
  now: number;
  /** Only a run this window started can be stopped from here. */
  canCancel: boolean;
  onClose: () => void;
}) {
  const activity = useAgentReviewStore((s) => s.activity);
  const cancel = useAgentReviewStore((s) => s.cancel);
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
          {onStep && <span className="review-activity-onstep"> · {onStep}</span>}
        </span>
        <button className="btn-ghost review-activity-close" onClick={onClose} aria-label="Close activity">
          ✕
        </button>
      </div>
      {steps.length > 0 && (
        <ul className="review-activity-steps" aria-label="Steps running now">
          {steps.map((step) => {
            const since = stepSince(step);
            const time = since === null ? "" : ` · ${formatDuration(now - since)}`;
            return (
              <li key={step.step} className={step.handed_out_ts ? "working" : "waiting"}>
                <span className="review-activity-step-name">{stepName(step)}</span>
                <span className="review-activity-step-state">
                  {step.handed_out_ts ? `working${time}` : `waiting for a sub-agent${time}`}
                </span>
              </li>
            );
          })}
        </ul>
      )}
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
      {canCancel && (
        <div className="review-activity-foot">
          <button
            className="btn-ghost review-activity-cancel"
            title="Stop the assistant now. Nothing is imported."
            onClick={() => {
              onClose();
              void cancel();
            }}
          >
            Cancel review
          </button>
        </div>
      )}
    </div>
  );
}

function ActivityRow({ entry, previous }: { entry: ActivityEntry; previous?: ActivityEntry }) {
  const gapMs = previous ? Date.parse(entry.ts) - Date.parse(previous.ts) : 0;
  // Only gaps worth explaining. Several lines in the same second is normal, and
  // printing "+0s" on each of them buries the one row that says "+6m45s".
  const gap = gapMs >= 20_000 ? formatDuration(gapMs) : null;
  return (
    <div className={`review-activity-row tone-${entry.tone}`} title={entry.text}>
      {/* Local time. `ts` is ISO in UTC, so slicing it showed 13:03 at 18:33 in India. */}
      <span className="review-activity-time">
        {new Date(entry.ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false })}
      </span>
      <span className="review-activity-text">{entry.text}</span>
      {gap && <span className="review-activity-gap">+{gap}</span>}
    </div>
  );
}
