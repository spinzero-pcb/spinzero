import { beforeEach, describe, expect, it, vi } from "vitest";

import type { AgentEvent } from "../lib/ipc";

// A clean exit is not a review. Claude Code in print mode exits 0 when a tool call is
// denied, so "finished" with an empty inbox must read as a failure, with the agent's
// last line as the reason. This run happened on 2026-09-27 and told the user to read
// an app log that held nothing.

let handler: ((ev: AgentEvent) => void) | null = null;
let inbox: unknown[] = [];

vi.mock("../lib/ipc", () => ({
  ipc: {},
  onAgentEvent: async (h: (ev: AgentEvent) => void) => {
    handler = h;
    return () => {};
  },
}));

vi.mock("./reviewInboxStore", () => ({
  useReviewInboxStore: {
    getState: () => ({ entries: inbox, load: async () => {} }),
  },
}));

const { useAgentReviewStore } = await import("./agentReviewStore");
const { useToastStore } = await import("./toastStore");

const DENIED =
  "Permission for `spinzero_start_review` was denied, so I can't proceed with the review. Please grant access to the spinzero MCP tools and I'll continue.";

async function settle(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0));
}

describe("an agent that finishes", () => {
  beforeEach(async () => {
    inbox = [];
    useToastStore.setState({ toasts: [] });
    useAgentReviewStore.setState({ phase: "running", error: null, activity: [], startedAt: Date.now() });
    await useAgentReviewStore.getState().subscribe();
  });

  it("with nothing in the inbox fails, and gives the agent's own reason", async () => {
    handler!({ kind: "finished", seconds: 17, last_line: DENIED });
    await settle();
    const s = useAgentReviewStore.getState();
    expect(s.phase).toBe("failed");
    expect(s.error).toBe(DENIED);
    const toast = useToastStore.getState().toasts.at(-1);
    expect(toast?.title).toContain("blocked the SpinZero tools");
  });

  it("with nothing in the inbox and nothing said still fails, and says so", async () => {
    handler!({ kind: "finished", seconds: 3, last_line: null });
    await settle();
    expect(useAgentReviewStore.getState().phase).toBe("failed");
    expect(useAgentReviewStore.getState().error).toContain("gave no reason");
  });

  it("with findings in the inbox is done", async () => {
    inbox = [{}];
    handler!({ kind: "finished", seconds: 900, last_line: "Findings landed in reviews/inbox." });
    await settle();
    expect(useAgentReviewStore.getState().phase).toBe("done");
    expect(useAgentReviewStore.getState().error).toBeNull();
    expect(useToastStore.getState().toasts.at(-1)?.title).toBe("Your agent finished the review");
  });
});
