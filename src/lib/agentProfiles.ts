// The agent a review runs on, before the backend has been asked.
//
// The real list is the backend's (`agent::builtin_profiles`), because the backend is
// what starts the process. This is the one entry the app must be able to name without
// an IPC round trip: what a fresh install runs with, so a store's default and a
// picker's initial value agree.

import type { AgentProfile } from "./types";

/** Claude Code, which is the profile SpinZero has run end to end. Keep in step with
 *  the first entry of `agent::builtin_profiles`. */
export const DEFAULT_AGENT_PROFILE: AgentProfile = {
  id: "claude-code",
  label: "Claude Code",
  bin: "claude",
  prompt_via: "arg",
  // Mirrors `builtin_profiles()` in agent.rs, which says why the flag is here and why
  // it comes after the prompt.
  args: ["-p", "{prompt}", "--allowedTools", "mcp__spinzero"],
  verified: true,
};

/** What a profile needs before it can be started. Returns what is missing rather than
 *  a boolean: a disabled button with no explanation is how a user concludes a feature
 *  is broken. */
export function missingFromAgent(profile: AgentProfile | null): string[] {
  if (!profile || !profile.bin.trim()) return ["the program to run"];
  // The prompt has to reach the agent somehow. An argument list with no `{prompt}`
  // and no standard input would start the agent with no instructions at all.
  if (profile.prompt_via === "arg" && !profile.args.some((a) => a.includes("{prompt}"))) {
    return ["{prompt} in the arguments"];
  }
  return [];
}
