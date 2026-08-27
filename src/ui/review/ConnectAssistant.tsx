import { useMemo, useState } from "react";

import {
  CLIENT_LABELS,
  configBlock,
  KNOWN_ENV,
  missingFrom,
  redactSecrets,
  type McpClient,
} from "../../lib/mcpConfig";
import type { AgentReviewSettings } from "../../lib/types";
import { useSettingsStore } from "../../stores/settingsStore";
import { useToastStore } from "../../stores/toastStore";
import { IconCopy, IconSparkle } from "../icons";

// "Connect your AI assistant" — the setup screen for running SpinZero reviews
// through Claude Code, Cursor, or anything else that speaks MCP.
//
// **What this exists to prevent.** A misconfigured MCP server does not fail loudly.
// The client starts it, the server exits or never registers, and the user sees an
// assistant that simply has no SpinZero tools — no error, no missing-file message,
// nothing to search for. So the app writes the block rather than documenting it: the
// four values that can be wrong (the command, its path, the licence key, the rule-pack
// binary) are collected in fields where they can be checked, and the shell quoting —
// the part that actually breaks, on every path containing a space — is done by code
// with tests behind it.
//
// **One saved config, two renderings.** The same settings drive the in-app run, where
// SpinZero spawns the assistant itself. If the copied block and the in-app path read
// different configuration the bug report is "it works in SpinZero but not in Cursor",
// with two places to look and no reason to prefer either.
//
// **The disclosure is part of the screen, not a link off it.** Telemetry is on by
// default (M4 item 5), and the price of that default is that the user is told what it
// sends before they paste anything — in the same view, in plain words, with the switch
// beside it.

export function ConnectAssistant({ onClose }: { onClose: () => void }) {
  const saved = useSettingsStore((s) => s.agentReview);
  const push = useToastStore((s) => s.push);

  const [client, setClient] = useState<McpClient>("claude-code");
  const [command, setCommand] = useState(saved?.server_command ?? "");
  const [args, setArgs] = useState((saved?.server_args ?? []).join(" "));
  const [env, setEnv] = useState<Record<string, string>>(saved?.server_env ?? {});
  const [saving, setSaving] = useState(false);

  const config: AgentReviewSettings = useMemo(
    () => ({
      claude_bin: saved?.claude_bin ?? "",
      server_command: command.trim(),
      server_args: args.trim().split(/\s+/).filter(Boolean),
      server_env: env,
    }),
    [saved?.claude_bin, command, args, env],
  );

  const missing = missingFrom(config);
  const block = missing.length ? "" : configBlock(client, config);

  async function copy() {
    try {
      // The FULL block, not the redacted one on screen: a copied config that is
      // missing its key is a config that silently does not work.
      await navigator.clipboard.writeText(block);
      push({ kind: "success", title: "Config copied", message: "Paste it into your assistant's setup." });
    } catch (e) {
      // Clipboard access can be refused by the webview. Say so rather than leaving a
      // button that appears to do nothing — the block is on screen and selectable.
      push({
        kind: "warning",
        title: "Could not copy",
        message: `Select the block and copy it by hand (${String(e)}).`,
      });
    }
  }

  async function save() {
    setSaving(true);
    try {
      await useSettingsStore.getState().setAgentReview(config.server_command ? config : null);
      push({ kind: "success", title: "Assistant setup saved" });
      onClose();
    } catch (e) {
      push({ kind: "error", title: "Could not save", message: String(e) });
    } finally {
      setSaving(false);
    }
  }

  const telemetryOff = env.SPINZERO_TELEMETRY?.trim() === "0";

  return (
    <div
      className="wizard-overlay"
      onPointerDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div className="wizard-card connect-card" role="dialog" aria-label="Connect your AI assistant">
        <div className="wizard-head">
          <span className="wizard-icon">
            <IconSparkle size={18} />
          </span>
          <div>
            <div className="wizard-title">Connect your AI assistant</div>
            <div className="wizard-step">Run SpinZero reviews on your own subscription</div>
          </div>
        </div>

        <div className="wizard-body">
          <p className="wizard-hint">
            SpinZero hands your assistant a real review, one step at a time — it does the
            reasoning, on your subscription, and your design never leaves this machine.
          </p>

          <div className="wizard-label">Where the review server is</div>
          <label className="review-field">
            <span>Command</span>
            <input
              className="wizard-input"
              value={command}
              spellCheck={false}
              onChange={(e) => setCommand(e.target.value)}
              placeholder="/path/to/spinzero-mcp"
            />
          </label>
          <label className="review-field">
            <span>Arguments</span>
            <input
              className="wizard-input"
              value={args}
              spellCheck={false}
              onChange={(e) => setArgs(e.target.value)}
              placeholder="(none — the shipped build takes no arguments)"
            />
          </label>

          <div className="wizard-label">Settings</div>
          {KNOWN_ENV.map(({ key, label, hint }) => (
            <label className="review-field" key={key} title={hint}>
              <span>{label}</span>
              <input
                className="wizard-input"
                value={env[key] ?? ""}
                spellCheck={false}
                // A licence key is a secret and this field is on screen during every
                // screen share of somebody's first setup.
                type={key === "SPINZERO_LICENCE_KEY" ? "password" : "text"}
                onChange={(e) => setEnv({ ...env, [key]: e.target.value })}
                placeholder={key === "SPINZERO_TELEMETRY" ? "on" : ""}
              />
            </label>
          ))}

          <div className="wizard-label">Config block</div>
          <div className="connect-tabs" role="tablist">
            {(Object.keys(CLIENT_LABELS) as McpClient[]).map((id) => (
              <button
                key={id}
                role="tab"
                aria-selected={client === id}
                className={`connect-tab ${client === id ? "on" : ""}`}
                onClick={() => setClient(id)}
              >
                {CLIENT_LABELS[id]}
              </button>
            ))}
          </div>

          {missing.length ? (
            <p className="wizard-hint">
              Fill in {missing.join(" and ")} and the block appears here, ready to paste.
            </p>
          ) : (
            <>
              {/* Redacted on screen, complete in the clipboard. */}
              <pre className="connect-block">{redactSecrets(block)}</pre>
              <button className="btn-ghost" onClick={() => void copy()}>
                <IconCopy size={13} /> Copy config
              </button>
            </>
          )}

          <div className="wizard-label">What leaves this machine</div>
          <p className="wizard-hint">
            Manufacturer part numbers, for distributor and datasheet lookups. Not your
            schematic, BOM or layout. Note what that does not say: the rows your assistant
            reasons over go to <em>your</em> model provider, because your assistant is the
            one doing the reasoning — that is your subscription and their terms, not ours.
          </p>
          <p className="wizard-hint">
            Improvement telemetry is {telemetryOff ? "off" : "on"}. It sends rule ids,
            severities and part numbers for findings and dismissed rule candidates, plus which
            datasheets we failed to fetch — never designators, titles, evidence, file paths,
            project names or your licence key. Set it to 0 above to switch it off.
          </p>
        </div>

        <div className="wizard-actions">
          <button className="btn-ghost" onClick={onClose}>
            Close
          </button>
          <button className="btn-primary" disabled={saving || !command.trim()} onClick={() => void save()}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
