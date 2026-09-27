import { beforeEach, describe, expect, it, vi } from "vitest";

import type { AgentEvent, RunStatus } from "../lib/ipc";

// Cancel stops the spending and files nothing. The review server's status file still
// says "running" afterwards, because nobody told it otherwise, so what is pinned here
// is that the bar does not come back from it.

let handler: ((ev: AgentEvent) => void) | null = null;
const cancelAgentReview = vi.fn(async () => true);

vi.mock("../lib/ipc", () => ({
  ipc: {
    cancelAgentReview: () => cancelAgentReview(),
    agentReviewRunning: async () => false,
    agentReviewStatus: async () => status(),
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
    steps_done: 1,
    steps_total: 4,
    parts_done: 10,
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

describe("cancelling an agent review", () => {
  beforeEach(async () => {
    useAgentReviewStore.setState({
      phase: "running",
      status: status(),
      startedAt: Date.now(),
      cancelledIds: [],
      activity: [],
    });
    await useAgentReviewStore.getState().subscribe();
  });

  it("goes back to idle and forgets the run", async () => {
    await useAgentReviewStore.getState().cancel();
    handler!({ kind: "cancelled" });
    const s = useAgentReviewStore.getState();
    expect(s.phase).toBe("idle");
    expect(s.status).toBeNull();
    expect(s.startedAt).toBeNull();
    expect(s.cancelledIds).toEqual(["r1"]);
  });

  it("ignores the cancelled run's status file from then on", async () => {
    await useAgentReviewStore.getState().cancel();
    handler!({ kind: "cancelled" });
    handler!({ kind: "status", status: status({ parts_done: 11 }) });
    await useAgentReviewStore.getState().refresh();
    expect(useAgentReviewStore.getState().phase).toBe("idle");
    expect(useAgentReviewStore.getState().status).toBeNull();
  });

  it("still follows a new run", async () => {
    await useAgentReviewStore.getState().cancel();
    handler!({ kind: "cancelled" });
    handler!({ kind: "status", status: status({ review_id: "r2" }) });
    expect(useAgentReviewStore.getState().status?.review_id).toBe("r2");
  });
});
