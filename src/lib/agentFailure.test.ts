import { describe, expect, it } from "vitest";

import { explainFailure } from "./agentFailure";

// Real lines, from agents that failed on a real machine. Each one must come back as
// something a user can act on, with the right button.

describe("explainFailure", () => {
  it("reads an expired login as signed out, and says how to sign in", () => {
    const a = explainFailure(
      "Failed to authenticate. API Error: 401 OAuth access token has expired. Re-authenticate to continue.",
      "Claude Code",
    );
    expect(a.title).toBe("Claude Code is signed out");
    expect(a.fix).toContain("run claude");
    expect(a.action).toBe("retry");
  });

  it("reads a missing program as not installed", () => {
    const a = explainFailure("could not start codex: program not found", "Codex CLI");
    expect(a.title).toBe("Codex CLI is not installed");
  });

  it("sends a missing SpinZero server to the Connect screen", () => {
    const a = explainFailure("No such tool available: mcp__spinzero__spinzero_start_review", "Claude Code");
    expect(a.action).toBe("connect");
  });

  it("reads a usage limit as one", () => {
    expect(explainFailure("Claude AI usage limit reached|1759000000", "Claude Code").title).toBe(
      "Claude Code hit its usage limit",
    );
  });

  it("keeps an unknown reason generic, so the agent's own words carry it", () => {
    const a = explainFailure("Something odd happened", "Gemini CLI");
    expect(a.title).toBe("Gemini CLI stopped before the review finished");
    expect(a.action).toBe("retry");
  });
});
