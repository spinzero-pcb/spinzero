import { beforeEach, describe, expect, it, vi } from "vitest";

import type { AgentEvent, RunStatus } from "../lib/ipc";

// On project open the status watcher reports the board's newest run file. When that
// run died hours ago at `step_open`, the window must stay idle: a dead run that reads
// as "Review running…" blocks every new review of the board.

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

const { useAgentReviewStore, STALL_MS } = await import("./agentReviewStore");

function status(patch: Partial<RunStatus> = {}): RunStatus {
  return {
    status_version: 1,
    review_id: "r1",
    pipeline: "bom-detailed",
    profile: "commercial",
    project_dir: "C:/b",
    phase: "step_open",
    stage: null,
    steps_done: 3,
    steps_total: 7,
    parts_done: 45,
    parts_total: 88,
    datasheets_read: 88,
    datasheets_total: 88,
    started_ts: "2026-09-27T13:41:44.549Z",
    updated_ts: new Date().toISOString(),
    findings_path: null,
    report_path: null,
    error: null,
    ...patch,
  };
}

describe("a status file that is not a live run", () => {
  beforeEach(async () => {
    useAgentReviewStore.setState({ phase: "idle", status: null, startedAt: null, stepStartedAt: null });
    await useAgentReviewStore.getState().subscribe();
  });

  it("leaves the window idle when the run went quiet long ago", () => {
    const old = new Date(Date.now() - STALL_MS - 60_000).toISOString();
    handler!({ kind: "status", status: status({ updated_ts: old }) });
    expect(useAgentReviewStore.getState().phase).toBe("idle");
    // Still shown, so the bar can say the last run stalled.
    expect(useAgentReviewStore.getState().status?.review_id).toBe("r1");
  });

  it("leaves the window idle for a run that already finished", () => {
    handler!({ kind: "status", status: status({ phase: "done" }) });
    expect(useAgentReviewStore.getState().phase).toBe("idle");
  });

  it("marks the window busy for a run that reported just now", () => {
    // A review started in a terminal is still this board's review.
    handler!({ kind: "status", status: status() });
    expect(useAgentReviewStore.getState().phase).toBe("running");
  });
});
