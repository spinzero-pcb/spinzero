import { describe, expect, it } from "vitest";

import {
  claudeCodeBlock,
  formatArgs,
  jsonBlock,
  missingFrom,
  parseArgs,
  redactSecrets,
  shellQuote,
} from "./mcpConfig";
import type { AgentReviewSettings } from "./types";

const CONFIG: AgentReviewSettings = {
  claude_bin: "",
  server_command: "/opt/SpinZero/spinzero-mcp",
  server_args: [],
  server_env: { SPINZERO_LICENCE_KEY: "sz_abcdef0123456789" },
};

describe("shellQuote", () => {
  it("leaves an ordinary path alone", () => {
    expect(shellQuote("/opt/SpinZero/spinzero-mcp")).toBe("/opt/SpinZero/spinzero-mcp");
  });

  it("quotes a path with spaces", () => {
    // The common case on Windows and macOS both, and the one whose failure is
    // silent: unquoted, `C:\Program Files\…` becomes two arguments and the client
    // reports a server that would not start, not a path that was wrong.
    expect(shellQuote("C:\\Program Files\\SpinZero\\spinzero-mcp.exe")).toBe(
      "'C:\\Program Files\\SpinZero\\spinzero-mcp.exe'",
    );
  });

  it("survives a value containing a single quote", () => {
    expect(shellQuote("/home/o'brien/mcp")).toBe(`'/home/o'\\''brien/mcp'`);
  });
});

describe("the Arguments field", () => {
  it("keeps a quoted path with spaces as ONE argument", () => {
    // The failure this exists to stop: `args.trim().split(/\s+/)` turned
    // `node "C:\Program Files\x\server.ts"` into three arguments, and the block that
    // came out was three correctly quoted pieces of nonsense — the exact silent
    // breakage shellQuote and its tests exist to prevent, reintroduced one field
    // earlier.
    expect(parseArgs('node "C:\\Program Files\\SpinZero\\server.ts"')).toEqual([
      "node",
      "C:\\Program Files\\SpinZero\\server.ts",
    ]);
  });

  it("handles single quotes, empty arguments and runs of whitespace", () => {
    expect(parseArgs("  a   b  ")).toEqual(["a", "b"]);
    expect(parseArgs(`'one two' three`)).toEqual(["one two", "three"]);
    expect(parseArgs('a "" b')).toEqual(["a", "", "b"]);
    expect(parseArgs("")).toEqual([]);
  });

  it("leaves a backslash alone outside quotes — it is a path separator, not an escape", () => {
    expect(parseArgs("C:\\tools\\mcp.exe")).toEqual(["C:\\tools\\mcp.exe"]);
  });

  it("escapes a quote inside a double-quoted argument", () => {
    expect(parseArgs('"say \\"hi\\""')).toEqual(['say "hi"']);
  });

  it("round-trips through formatArgs, so re-opening a saved setup does not resplit it", () => {
    // The parse was only half of it: the field was seeded with `join(" ")`, so a saved
    // list re-opened as text that split differently from the list it came from.
    for (const args of [
      ["node", "C:\\Program Files\\SpinZero\\server.ts"],
      ["--flag", "value with spaces", ""],
      ["plain"],
      [],
    ]) {
      expect(parseArgs(formatArgs(args))).toEqual(args);
    }
  });

  it("reaches the config block with the path intact", () => {
    const block = claudeCodeBlock({
      ...CONFIG,
      server_command: "node",
      server_args: parseArgs('"C:\\Program Files\\SpinZero\\server.ts"'),
    });
    expect(block).toContain("'C:\\Program Files\\SpinZero\\server.ts'");
  });
});

describe("the Claude Code block", () => {
  it("names the server, carries the environment, and separates the command", () => {
    const block = claudeCodeBlock(CONFIG);
    expect(block).toContain("claude mcp add spinzero");
    expect(block).toContain("-e SPINZERO_LICENCE_KEY=sz_abcdef0123456789");
    // The `--` matters: without it the client parses our path as its own flag.
    expect(block).toContain("--");
    expect(block.indexOf("--")).toBeLessThan(block.indexOf("/opt/SpinZero/spinzero-mcp"));
  });

  it("omits an environment variable that was left blank", () => {
    // An empty value is not the same as an absent one — `SPINZERO_TELEMETRY=""` reads
    // as ON — so writing it out would tell the reader they had chosen something.
    const block = claudeCodeBlock({
      ...CONFIG,
      server_env: { ...CONFIG.server_env, SPINZERO_BOM_RULES_BIN: "  " },
    });
    expect(block).not.toContain("SPINZERO_BOM_RULES_BIN");
  });
});

describe("the JSON block", () => {
  it("is valid JSON with the server under mcpServers", () => {
    const parsed = JSON.parse(jsonBlock({ ...CONFIG, server_args: ["--stdio"] })) as {
      mcpServers: Record<string, { command: string; args: string[]; env?: Record<string, string> }>;
    };
    expect(parsed.mcpServers.spinzero).toEqual({
      command: "/opt/SpinZero/spinzero-mcp",
      args: ["--stdio"],
      env: { SPINZERO_LICENCE_KEY: "sz_abcdef0123456789" },
    });
  });

  it("leaves env out entirely when there is none", () => {
    const parsed = JSON.parse(jsonBlock({ ...CONFIG, server_env: {} })) as {
      mcpServers: Record<string, Record<string, unknown>>;
    };
    expect(parsed.mcpServers.spinzero).not.toHaveProperty("env");
  });
});

describe("missingFrom", () => {
  it("names what is missing rather than answering yes or no", () => {
    // A disabled Copy button with no explanation is how a user concludes the feature
    // is broken.
    expect(missingFrom(null)).toContain("the licence key");
    expect(missingFrom({ ...CONFIG, server_env: {} })).toEqual(["the licence key"]);
    expect(missingFrom({ ...CONFIG, server_command: "  " })).toContain("the server command");
  });

  it("is satisfied by a development build, which needs no key", () => {
    expect(missingFrom({ ...CONFIG, server_env: { SPINZERO_MCP_DEV: "1" } })).toEqual([]);
  });

  it("is satisfied by a complete setup", () => {
    expect(missingFrom(CONFIG)).toEqual([]);
  });
});

describe("redactSecrets", () => {
  it("hides most of a licence key in both block formats", () => {
    // Applied only to what is SHOWN — the copied block stays complete. A key on screen
    // is a key in every screenshot of somebody's first setup.
    const shell = redactSecrets(claudeCodeBlock(CONFIG));
    expect(shell).not.toContain("sz_abcdef0123456789");
    expect(shell).toContain("sz_abc");
    const json = redactSecrets(jsonBlock(CONFIG));
    expect(json).not.toContain("sz_abcdef0123456789");
  });

  it("leaves everything else intact", () => {
    const block = claudeCodeBlock({ ...CONFIG, server_env: { SPINZERO_TELEMETRY: "0" } });
    expect(redactSecrets(block)).toBe(block);
  });
});
