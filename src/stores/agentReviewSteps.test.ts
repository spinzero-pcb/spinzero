import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AgentEvent, RunStatus } from "../lib/ipc";

// A step takes minutes and moves no count until it is submitted. What is pinned here is
// that the feed says so once when a step finishes, and that the step clock restarts at
// the right moments.

let handler: ((ev: AgentEvent) => void) | null = null;

vi.mock("../lib/ipc", () => ({
  ipc: {
    cancelAgentReview: async () => true,
    agentReviewRunning: async () => false,
    agentReviewStatus: async () => null,
  },
  onAgentEvent: async (h: (ev: AgentEvent) => void) => {
    handler = h;
    return () => {};
  },
}));

const { useAgentReviewStore } = await import("./agentReviewStore");

function status(patch: Partial<RunStatus> = {}): RunStatus {
  return {
    status_version: 1,
    review_id: "r1",
    pipeline: "bom-detailed",
    profile: "commercial",
    project_dir: "C:/b",
    phase: "step_open",
    stage: null,
    steps_done: 0,
    steps_total: 7,
    parts_done: 0,
    parts_total: 88,
    datasheets_read: 88,
    datasheets_total: 88,
    started_ts: "2026-09-27T10:00:00.000Z",
    updated_ts: new Date().toISOString(),
    findings_path: null,
    report_path: null,
    error: null,
    ...patch,
  };
}

const T0 = Date.parse("2026-09-27T10:00:00.000Z");
const send = (patch: Partial<RunStatus>) => handler!({ kind: "status", status: status(patch) });
const stepLines = () =>
  useAgentReviewStore
    .getState()
    .activity.map((a) => a.text)
    .filter((t) => /^Step \d+ done/.test(t));

describe("step progress", () => {
  beforeEach(async () => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
    useAgentReviewStore.setState({
      phase: "running",
      status: null,
      startedAt: T0,
      stepStartedAt: null,
      cancelledIds: [],
      activity: [],
    });
    await useAgentReviewStore.getState().subscribe();
  });
  afterEach(() => vi.useRealTimers());

  it("starts the step clock on the first step_open status", () => {
    send({ phase: "preparing", steps_total: 0, parts_total: 0 });
    expect(useAgentReviewStore.getState().stepStartedAt).toBeNull();
    vi.setSystemTime(T0 + 5_000);
    send({});
    expect(useAgentReviewStore.getState().stepStartedAt).toBe(T0 + 5_000);
  });

  it("prints one feed line per finished step, however often the file is rewritten", () => {
    send({});
    vi.setSystemTime(T0 + 60_000);
    send({}); // same counts
    vi.setSystemTime(T0 + 432_000);
    send({ steps_done: 1, parts_done: 12, steps_total: 9 });
    send({ steps_done: 1, parts_done: 12, steps_total: 9 });
    send({ steps_done: 1, parts_done: 12, steps_total: 9 });
    expect(stepLines()).toEqual(["Step 1 done · 12 parts"]);

    vi.setSystemTime(T0 + 432_000 + 45_000);
    send({ steps_done: 2, parts_done: 13, steps_total: 9 });
    expect(stepLines()).toEqual(["Step 1 done · 12 parts", "Step 2 done · 1 part"]);
  });

  it("prints a separate line for each step that closed, when several run at once", () => {
    const open = (n: number) => ({ step: `verify_parts#${n}`, index: n, opened_ts: "a", handed_out_ts: "b" });
    send({ open_steps: [open(1), open(2), open(3)] });
    send({ steps_done: 2, parts_done: 8, open_steps: [open(2), open(4), open(5)] });
    expect(stepLines()).toEqual(["Step 1 done", "Step 3 done"]);
    send({ steps_done: 3, parts_done: 12, open_steps: [open(4), open(5)] });
    expect(stepLines()).toEqual(["Step 1 done", "Step 3 done", "Step 2 done · 4 parts"]);
  });

  it("restarts the step clock when steps_done changes, and not otherwise", () => {
    send({});
    vi.setSystemTime(T0 + 30_000);
    send({ parts_done: 0 });
    expect(useAgentReviewStore.getState().stepStartedAt).toBe(T0);
    vi.setSystemTime(T0 + 90_000);
    send({ steps_done: 1, parts_done: 12 });
    expect(useAgentReviewStore.getState().stepStartedAt).toBe(T0 + 90_000);
  });

  it("restarts the step clock for a new run", () => {
    send({});
    vi.setSystemTime(T0 + 90_000);
    send({ review_id: "r2", steps_done: 3, parts_done: 40 });
    // A different run is not a finished step of the old one.
    expect(stepLines()).toEqual([]);
    expect(useAgentReviewStore.getState().stepStartedAt).toBe(T0 + 90_000);
  });

  it("clears the step clock on cancel", () => {
    send({});
    handler!({ kind: "cancelled" });
    expect(useAgentReviewStore.getState().stepStartedAt).toBeNull();
  });

  it("clears the step clock when the steps are over", () => {
    send({ steps_done: 6, parts_done: 88 });
    send({ phase: "assembling", steps_done: 7, parts_done: 88 });
    expect(useAgentReviewStore.getState().stepStartedAt).toBeNull();
    expect(stepLines()).toHaveLength(1);
  });
});
