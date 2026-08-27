// The MCP client config block, generated from one saved setup.
//
// **Why the app writes this rather than a docs page.** The block is four values —
// a command, its arguments, a licence key, and the path to the rule pack — and every
// one of them is something the user would otherwise have to find, quote correctly for
// their shell, and keep in step across two clients. Getting one wrong produces an
// assistant that silently has no SpinZero tools: no error, no missing-file message,
// just a capability that is not there. That is the worst class of setup failure,
// because nothing tells you it happened.
//
// **Why it shares its source with the in-app run.** SpinZero can also start the
// review itself, spawning the assistant against the same server (`agentReview` in
// settings). If the copied block and the in-app run read different configuration, the
// inevitable bug report is "it works in SpinZero but not in Cursor" — with two places
// to look and no reason to prefer either. So there is one saved config, and this
// module renders it for each client.
//
// Pure string-building on purpose: no store, no IPC, no clipboard. That is what makes
// the escaping — which is the part that actually breaks — testable.

import type { AgentReviewSettings } from "./types";

/** Clients we can write a block for. Both speak the same MCP; they differ only in
 *  how they want to be told. */
export type McpClient = "claude-code" | "json";

export const CLIENT_LABELS: Record<McpClient, string> = {
  "claude-code": "Claude Code",
  json: "Cursor, and other JSON configs",
};

/** The server name the block registers. Fixed rather than configurable: the user
 *  types it into a conversation ("run a SpinZero review"), and a server whose name
 *  varies per install is a server nobody can be told how to use. */
export const SERVER_NAME = "spinzero";

/**
 * Environment variables worth surfacing, in the order a person needs them.
 *
 * `SPINZERO_LICENCE_KEY` first because it is the one thing a customer must supply and
 * the one whose absence produces the confusing failure — a review that runs, finds no
 * distributor data and no datasheets, and stops at the coverage gate for what looks
 * like a reason about their board.
 */
export const KNOWN_ENV: { key: string; label: string; hint: string }[] = [
  {
    key: "SPINZERO_LICENCE_KEY",
    label: "Licence key",
    hint: "Enables the parts service and the datasheet corpus. Without it a review has almost no evidence to work from.",
  },
  {
    key: "SPINZERO_BOM_RULES_BIN",
    label: "Rule pack binary",
    hint: "Path to bom-rules. It is a separate program by design; leave this empty if it sits beside the server.",
  },
  {
    key: "SPINZERO_TELEMETRY",
    label: "Improvement telemetry",
    hint: "On by default. Set to 0 to switch it off.",
  },
];

/** Quote one argument for a POSIX-ish shell, which is what `claude mcp add` is pasted
 *  into. Paths with spaces are the common case on Windows and macOS alike, and an
 *  unquoted one silently becomes two arguments. */
export function shellQuote(value: string): string {
  if (value === "") return "''";
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) return value;
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** Drop empty values before they reach a config block. An env var set to "" is not
 *  the same as one that is absent — `SPINZERO_TELEMETRY=""` reads as ON, and writing
 *  it out would suggest to the reader that they had chosen something. */
function definedEnv(env: Record<string, string>): [string, string][] {
  return Object.entries(env).filter(([k, v]) => k.trim() !== "" && v.trim() !== "");
}

/** The `claude mcp add` line. */
export function claudeCodeBlock(config: AgentReviewSettings): string {
  const parts = [`claude mcp add ${SERVER_NAME}`];
  for (const [k, v] of definedEnv(config.server_env)) parts.push(`-e ${shellQuote(`${k}=${v}`)}`);
  parts.push("--");
  parts.push(shellQuote(config.server_command));
  for (const arg of config.server_args) parts.push(shellQuote(arg));
  // One flag per line with a continuation, because these lines get long and a wrapped
  // one is where a paste loses a character.
  return parts.join(" \\\n  ");
}

/** The `mcpServers` object Cursor and friends expect. */
export function jsonBlock(config: AgentReviewSettings): string {
  const env = Object.fromEntries(definedEnv(config.server_env));
  return `${JSON.stringify(
    {
      mcpServers: {
        [SERVER_NAME]: {
          command: config.server_command,
          args: config.server_args,
          ...(Object.keys(env).length ? { env } : {}),
        },
      },
    },
    null,
    2,
  )}\n`;
}

export function configBlock(client: McpClient, config: AgentReviewSettings): string {
  return client === "claude-code" ? claudeCodeBlock(config) : jsonBlock(config);
}

/**
 * Is this setup complete enough to hand to a client?
 *
 * Returns what is missing rather than a boolean, because "incomplete" has two very
 * different remedies here and a disabled Copy button with no explanation is how a
 * user concludes the feature is broken.
 */
export function missingFrom(config: AgentReviewSettings | null): string[] {
  if (!config) return ["the server command", "the licence key"];
  const missing: string[] = [];
  if (!config.server_command.trim()) missing.push("the server command");
  // Arguments are deliberately NOT required. The shipped build is a single executable
  // that takes none; only a source checkout needs `node …/server.ts`. Demanding a path
  // here would reject the setup every customer actually has.
  const licence = config.server_env.SPINZERO_LICENCE_KEY?.trim();
  const dev = config.server_env.SPINZERO_MCP_DEV?.trim();
  // A development build is a legitimate way to run this and needs no key; saying
  // otherwise would nag the one person who definitely knows what they are doing.
  if (!licence && dev !== "1") missing.push("the licence key");
  return missing;
}

/**
 * Redact secrets for display.
 *
 * The block is meant to be copied, and a copied block must be complete — so this is
 * only ever applied to what is SHOWN. A licence key on screen is a licence key in
 * every screenshot and screen share of somebody's first setup, and support threads
 * are full of them.
 */
export function redactSecrets(text: string): string {
  return text.replace(/(SPINZERO_LICENCE_KEY[=":\s]+)([^\s"',\\]+)/g, (_m, prefix: string, value: string) =>
    value.length <= 8 ? `${prefix}${"•".repeat(value.length)}` : `${prefix}${value.slice(0, 6)}${"•".repeat(8)}`,
  );
}
