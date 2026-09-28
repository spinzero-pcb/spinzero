import { describe, expect, it } from "vitest";

import { isStalled, percentOf, progressLabel, PROGRESS_SPANS, STALL_MS } from "./agentReviewStore";
import type { RunStatus } from "../lib/ipc";

// The bar's arithmetic, and the one thing it must never do: report progress on a run
// that has stopped reporting. A watcher is not a subscription — `status.json` stops
// moving when the run dies, and a bar still reading "68%" over a dead agent is worse
// than no bar at all.

function status(patch: Partial<RunStatus> = {}): RunStatus {
  return {
    status_version: 1,
    review_id: "r1",
    pipeline: "bom-detailed",
    profile: "commercial",
    project_dir: "C:/b",
    phase: "preparing",
    stage: "fetch_datasheets",
    steps_done: 0,
    steps_total: 0,
    parts_done: 0,
    parts_total: 0,
    datasheets_read: 0,
    datasheets_total: 0,
    started_ts: "2026-09-14T10:00:00.000Z",
    updated_ts: "2026-09-14T10:00:00.000Z",
    findings_path: null,
    report_path: null,
    error: null,
    ...patch,
  };
}

describe("percentOf", () => {
  it("shows nothing before a run exists, and a sliver while the preflight is open", () => {
    expect(percentOf(null)).toBe(0);
    expect(percentOf(status({ phase: "preflight" }))).toBe(2);
  });

  it("fills the first span on datasheets, which is where that time actually goes", () => {
    // The deterministic layer used to sit at a fixed number for two minutes, which
    // reads as a hang rather than as slow.
    expect(percentOf(status({ datasheets_read: 0, datasheets_total: 88 }))).toBe(4);
    expect(percentOf(status({ datasheets_read: 44, datasheets_total: 88 }))).toBe(9.5);
    expect(percentOf(status({ datasheets_read: 88, datasheets_total: 88 }))).toBe(15);
  });

  it("fills the batch span on parts accounted for, which IS the job", () => {
    const s = (done: number) => percentOf(status({ phase: "step_open", parts_done: done, parts_total: 88 }));
    expect(s(0)).toBe(15);
    expect(s(44)).toBe(51.5);
    expect(s(87)).toBeLessThan(88);
  });

  it("holds the board step at 90, and puts assembly after it", () => {
    // Every part accounted for, and the one open step is the board step.
    expect(percentOf(status({ phase: "step_open", parts_done: 88, parts_total: 88 }))).toBe(90);
    expect(
      percentOf(
        status({
          phase: "step_open",
          parts_done: 88,
          parts_total: 88,
          open_steps: [{ step: "board_review", index: 9, opened_ts: "x", handed_out_ts: null }],
        }),
      ),
    ).toBe(90);
    expect(percentOf(status({ phase: "assembling", parts_done: 88, parts_total: 88 }))).toBe(97);
  });

  it("does not jump to assembly in the wait between two batches", () => {
    // An older server says "assembling" whenever no step is open.
    expect(Math.round(percentOf(status({ phase: "assembling", parts_done: 30, parts_total: 88 })))).toBe(40);
    expect(progressLabel(status({ phase: "assembling", parts_done: 30, parts_total: 88 }))).toBe(
      "Waiting for the next batch · 30 of 88 parts",
    );
  });

  it("never runs past its span, whatever the counts say", () => {
    // A drain batch can report more parts than the plan first estimated. With a batch
    // still open, that is not the board step.
    expect(
      percentOf(
        status({
          phase: "step_open",
          parts_done: 200,
          parts_total: 88,
          open_steps: [{ step: "verify_parts#7", index: 7, opened_ts: "x", handed_out_ts: null }],
        }),
      ),
    ).toBe(88);
    expect(percentOf(status({ phase: "done" }))).toBe(100);
  });

  it("is split into spans that meet end to end", () => {
    const spans = Object.values(PROGRESS_SPANS);
    expect(spans[0]?.[0]).toBe(0);
    expect(spans[spans.length - 1]?.[1]).toBe(100);
    for (const [from, to] of spans) expect(to).toBeGreaterThan(from);
  });
});

describe("isStalled", () => {
  const now = Date.parse("2026-09-14T10:00:00.000Z");

  it("is false while the run is reporting", () => {
    expect(isStalled(status({ updated_ts: "2026-09-14T09:59:00.000Z" }), now)).toBe(false);
  });

  it("is true once nothing has been written for long enough", () => {
    const old = new Date(now - STALL_MS - 1_000).toISOString();
    expect(isStalled(status({ updated_ts: old }), now)).toBe(true);
  });

  it("is never true of a run that has ended — that is not a stall, it is over", () => {
    const old = new Date(now - STALL_MS - 1_000).toISOString();
    expect(isStalled(status({ phase: "done", updated_ts: old }), now)).toBe(false);
    expect(isStalled(status({ phase: "failed", updated_ts: old }), now)).toBe(false);
    expect(isStalled(null, now)).toBe(false);
  });
});

describe("progressLabel", () => {
  it("counts in the terms of whatever the run is actually doing", () => {
    expect(progressLabel(status({ datasheets_read: 30, datasheets_total: 88 }))).toBe(
      "Collecting datasheets · 30 of 88",
    );
    expect(progressLabel(status({ phase: "step_open", parts_done: 12, parts_total: 88 }))).toBe(
      "12 of 88 parts",
    );
  });

  it("says 'about' for the step total while more steps can still be formed", () => {
    // The server forms batches as datasheets arrive, so 7 steps can become 9.
    expect(
      progressLabel(status({ phase: "step_open", steps_done: 2, steps_total: 7, parts_done: 30, parts_total: 88 })),
    ).toBe("Step 3 of about 7 · 30 of 88 parts");
  });

  it("names the last step as the board-level check once every part is accounted for", () => {
    expect(
      progressLabel(status({ phase: "step_open", steps_done: 8, steps_total: 9, parts_done: 88, parts_total: 88 })),
    ).toBe("Step 9 of 9 · board-level check");
    // Never past the total, even if the counts run ahead of it.
    expect(
      progressLabel(status({ phase: "step_open", steps_done: 9, steps_total: 9, parts_done: 88, parts_total: 88 })),
    ).toBe("Step 9 of 9 · board-level check");
  });

  it("drops 'about' on the last step even with parts left", () => {
    expect(
      progressLabel(status({ phase: "step_open", steps_done: 3, steps_total: 4, parts_done: 80, parts_total: 88 })),
    ).toBe("Step 4 of 4 · 80 of 88 parts");
  });

  it("names every step running at once", () => {
    const open = (step: string, index: number, handed = true) => ({
      step,
      index,
      opened_ts: "2026-09-14T10:00:00.000Z",
      handed_out_ts: handed ? "2026-09-14T10:00:05.000Z" : null,
    });
    expect(
      progressLabel(
        status({
          phase: "step_open",
          steps_done: 3,
          steps_total: 9,
          parts_done: 36,
          parts_total: 88,
          open_steps: [open("verify_parts#4", 4), open("verify_parts#5", 5), open("verify_parts#6", 6, false)],
        }),
      ),
    ).toBe("Steps 4, 5 and 6 of about 9 · 36 of 88 parts");
    expect(
      progressLabel(
        status({
          phase: "step_open",
          steps_done: 8,
          steps_total: 9,
          parts_done: 88,
          parts_total: 88,
          open_steps: [open("board_review", 9)],
        }),
      ),
    ).toBe("Step 9 of 9 · board-level check");
  });

  it("says what a run parked on the preflight is waiting for", () => {
    // The one state a user cannot diagnose on their own: the agent was started and
    // has not answered the setup question yet.
    expect(progressLabel(status({ phase: "preflight" }))).toContain("confirm the setup");
  });

  it("carries the failure sentence rather than a phase name", () => {
    expect(progressLabel(status({ phase: "failed", error: "the agent exited" }))).toBe("the agent exited");
  });
});
