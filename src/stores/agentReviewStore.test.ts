import { describe, expect, it } from "vitest";

import { isStalled, percentOf, progressLabel, STALL_MS } from "./agentReviewStore";
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
    expect(percentOf(status({ datasheets_read: 44, datasheets_total: 88 }))).toBe(17);
    expect(percentOf(status({ datasheets_read: 88, datasheets_total: 88 }))).toBe(30);
  });

  it("fills the middle span on parts accounted for, which IS the job", () => {
    const s = (done: number) => percentOf(status({ phase: "step_open", parts_done: done, parts_total: 88 }));
    expect(s(0)).toBe(30);
    expect(Math.round(s(44))).toBe(63);
    expect(s(88)).toBe(95);
  });

  it("never runs past its span, whatever the counts say", () => {
    // A drain batch can report more parts than the plan first estimated.
    expect(percentOf(status({ phase: "step_open", parts_done: 200, parts_total: 88 }))).toBe(95);
    expect(percentOf(status({ phase: "done" }))).toBe(100);
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
      "12 of 88 parts accounted for",
    );
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
