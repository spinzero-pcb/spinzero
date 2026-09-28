// Why a detailed review failed, in words the user can act on.
//
// The backend hands over the agent's own last line ("Failed to authenticate. API
// Error: 401 OAuth access token has expired…"). That line is accurate and useless to
// most readers, so it is matched against the few causes we know and turned into a
// title, one line of what to do, and the button that does it. Anything unknown keeps
// the agent's own words: they are still the best explanation there is.

export interface FailureAdvice {
  /** What went wrong, short enough for a heading. */
  title: string;
  /** What to do about it, in one sentence. */
  fix: string;
  /** The one button that helps: run again, or open the Connect screen. */
  action: "retry" | "connect";
}

export function explainFailure(detail: string, agent: string): FailureAdvice {
  const d = detail.toLowerCase();
  if (/401|authenticat|oauth|access token|log ?in|sign ?in|not logged/.test(d)) {
    return {
      title: `${agent} is signed out`,
      fix: `Open a terminal, run ${commandFor(agent)}, and sign in. Then run the review again.`,
      action: "retry",
    };
  }
  if (/usage limit|rate limit|quota|credit|billing|429|overloaded/.test(d)) {
    return {
      title: `${agent} hit its usage limit`,
      fix: "Wait for the limit to reset, or check your plan. Then run the review again.",
      action: "retry",
    };
  }
  if (/could not start|not recognized|no such file|cannot find|enoent/.test(d)) {
    return {
      title: `${agent} is not installed`,
      fix: `Install ${agent}, or pick another AI agent in the BOM Review window.`,
      action: "retry",
    };
  }
  // Before the "cannot reach" rule: the server was reached, and it refused the licence.
  // The agent's line names SpinZero, so the rule below would call it unreachable.
  if (/licen[cs]e/.test(d)) {
    return {
      title: "SpinZero refused the licence",
      fix: `The SpinZero server that ${agent} runs could not confirm your licence key. Check the key on the Connect screen, then run the review again.`,
      action: "connect",
    };
  }
  // Before the "cannot reach" rule: a denied call names the SpinZero tool too, but
  // the server was reached. The agent's own permissions blocked it.
  if (/requested permissions|haven't granted|not granted|permission.*denied|denied.*permission/.test(d)) {
    return {
      title: `${agent} blocked the SpinZero tools`,
      fix:
        agent === "Claude Code"
          ? `SpinZero runs Claude Code with nobody there to approve a tool. Add "mcp__spinzero" to "permissions.allow" in ~/.claude/settings.json, then run the review again.`
          : `SpinZero runs ${agent} with nobody there to approve a tool. Allow the SpinZero tools in ${agent}'s settings, then run the review again.`,
      action: "retry",
    };
  }
  if (/spinzero|mcp|no such tool|unknown tool/.test(d)) {
    return {
      title: `${agent} cannot reach SpinZero`,
      fix: `Connect SpinZero to ${agent}, restart it, and run the review again.`,
      action: "connect",
    };
  }
  if (/network|enotfound|econnrefused|timed? ?out|offline/.test(d)) {
    return {
      title: `${agent} could not reach the internet`,
      fix: "Check your connection, then run the review again.",
      action: "retry",
    };
  }
  return {
    title: `${agent} stopped before the review finished`,
    fix: "Run the review again. If it fails again, the reason below is what the agent said.",
    action: "retry",
  };
}

/** The command a user types to open this agent in a terminal. */
function commandFor(agent: string): string {
  const known: Record<string, string> = {
    "Claude Code": "claude",
    "Codex CLI": "codex",
    "Gemini CLI": "gemini",
    "Cursor CLI": "cursor-agent",
  };
  return known[agent] ?? agent;
}
