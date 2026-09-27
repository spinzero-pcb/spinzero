import { useEffect, useState } from "react";

import { ipc } from "../../lib/ipc";
import { jsonBlockFor } from "../../lib/mcpConfig";
import type { AssistantClient, AssistantSetup } from "../../lib/types";
import { useToastStore } from "../../stores/toastStore";
import { IconCheck, IconCopy, IconInfo, IconSparkle } from "../icons";

// "Connect your AI agent" — the setup screen for running SpinZero reviews through
// Claude Code, Cursor, Codex, or anything else that speaks MCP.
//
// **What this exists to prevent.** A misconfigured MCP server does not fail loudly. The
// client starts it, the server exits or never registers, and the user sees an assistant
// that simply has no SpinZero tools — no error, no missing-file message, nothing to
// search for. That is the worst class of setup failure, because nothing tells you it
// happened.
//
// **Three things used to be typeable and are now facts.** The server path is resolved
// (it is beside this app), the licence key lives in one file rather than in every
// client's config, and the registration line is generated. What is left for the user to
// get wrong is: nothing, on a client with a CLI; one paste, on a client without.
//
// **We never write another product's config file.** `~/.claude.json`, `~/.cursor/mcp.json`
// and VS Code's `mcp.json` belong to their products. They are hand-edited, some tolerate
// comments a strict writer would destroy, and a file we corrupt is a support incident
// with no undo. So: run the client's own `mcp add` where there is one, and show the
// block and the path where there is not.
//
// **This is the ONLY registration path now, including for a review the app starts.**
// SpinZero used to write a private MCP config and force it on the one agent it knew
// how to spawn. That worked for one command line program and nothing else, and it is
// gone. What the user does on this screen is what the in-app review runs on.
//
// **The screen is two numbered steps and nothing else.** There is no "now ask for a
// review" step: Run a review starts the assistant itself. Every explanation is behind
// an info icon (hover to read). If a sentence has to be visible for the step to make
// sense, the step is badly designed; fix the step, not the text.
//
// **The command carries no secret,** which is what makes it safe to show at all. The
// key is in `~/.spinzero/licence.key` and the server reads it for itself.

export function ConnectAssistant({ onClose }: { onClose: () => void }) {
  const push = useToastStore((s) => s.push);

  const [setup, setSetup] = useState<AssistantSetup | null>(null);
  const [loadError, setLoadError] = useState("");
  const [key, setKey] = useState("");
  const [savingKey, setSavingKey] = useState(false);
  const [busy, setBusy] = useState("");
  const [shown, setShown] = useState<string | null>(null);
  // Clients connected in this session. Their config says so too, but we only read
  // three of those files; this covers the rest.
  const [joined, setJoined] = useState<Set<string>>(new Set());

  async function load() {
    try {
      setSetup(await ipc.assistantSetup());
      setLoadError("");
    } catch (e) {
      setLoadError(String(e));
    }
  }

  useEffect(() => {
    void load();
  }, []);

  async function saveKey() {
    setSavingKey(true);
    try {
      const path = await ipc.setLicenceKey(key);
      setKey("");
      await load();
      push({ kind: "success", title: "Licence key saved", message: path });
    } catch (e) {
      push({ kind: "error", title: "Could not save the key", message: String(e) });
    } finally {
      setSavingKey(false);
    }
  }

  async function register(client: AssistantClient) {
    setBusy(client.id);
    try {
      const out = await ipc.registerAssistant(client.id);
      if (out.ok) {
        setJoined((s) => new Set(s).add(client.id));
        setShown(null);
        void load();
        push({
          kind: "success",
          title: `${client.label} is connected`,
          message: "Restart it, then ask it to run a SpinZero review.",
        });
      } else {
        // Not an error dialog. The likeliest causes — the CLI is not on the PATH this
        // app inherited, or it wants an interactive login — are both fixed in a
        // terminal, so show the exact line to run there.
        setShown(client.id);
        push({
          kind: "warning",
          title: `${client.label} did not take it`,
          message: `${out.detail} — run the command below in a terminal instead.`,
        });
      }
    } catch (e) {
      push({ kind: "error", title: "Could not register", message: String(e) });
    } finally {
      setBusy("");
    }
  }

  async function copy(text: string, what: string) {
    try {
      await navigator.clipboard.writeText(text);
      push({ kind: "success", title: `${what} copied` });
    } catch (e) {
      // The webview can refuse clipboard access. Say so rather than leaving a button
      // that appears to do nothing — the text is on screen and selectable.
      push({ kind: "warning", title: "Could not copy", message: `Select it and copy by hand (${e}).` });
    }
  }

  const ready = Boolean(setup && !setup.server_problem);

  return (
    <div className="wizard-overlay" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="wizard-card connect-card" role="dialog" aria-label="Connect your AI agent">
        <div className="wizard-head">
          <span className="wizard-icon">
            <IconSparkle size={18} />
          </span>
          <div className="wizard-title">Connect your AI agent</div>
          <Info text={PRIVACY} />
        </div>

        <div className="wizard-body">
          {loadError && <p className="wizard-hint err">Could not read this machine's setup: {loadError}</p>}
          {setup?.server_problem && <p className="wizard-hint err">{setup.server_problem}</p>}

          {setup && (
            <>
              <div className="wizard-label connect-step">
                <span className="connect-num">1</span>
                Licence key
                {setup.licence_present && (
                  <span className="connect-ok" title="A key is saved" aria-label="A key is saved">
                    <IconCheck size={13} />
                  </span>
                )}
                <Info text={`Saved in ${setup.licence_file}. Every assistant reads it from there.`} />
              </div>
              <div className="connect-row">
                <input
                  className="wizard-input"
                  // A secret, on screen, during every screen share of somebody's first
                  // setup.
                  type="password"
                  aria-label="Licence key"
                  value={key}
                  spellCheck={false}
                  placeholder={setup.licence_present ? "Saved. Paste a new key to replace it." : "sz_…"}
                  onChange={(e) => setKey(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && key.trim() && !savingKey) void saveKey();
                  }}
                />
                <button className="btn-ghost" disabled={savingKey || !key.trim()} onClick={() => void saveKey()}>
                  {savingKey ? "Saving…" : "Save"}
                </button>
              </div>

              <div className="wizard-label connect-step">
                <span className="connect-num">2</span>
                AI agent
                <Info text={CLIENTS} />
              </div>
              <ul className="connect-clients">
                {setup.clients.map((client) => {
                  const text =
                    client.how === "command"
                      ? client.command
                      : jsonBlockFor(client.config_key, setup.server_command);
                  const open = shown === client.id && ready;
                  const toggle = client.how === "command" ? "Show the command" : "Show the config";
                  const connected = client.connected || joined.has(client.id);
                  const connecting = busy === client.id;
                  return (
                    <li key={client.id} className="connect-client">
                      <div className="connect-client-head">
                        <span
                          className={`connect-client-name${client.installed ? "" : " absent"}`}
                          title={client.installed ? undefined : "Not found on this machine"}
                        >
                          {client.label}
                          {connected && (
                            <span className="connect-ok" title="Connected" aria-label="Connected">
                              <IconCheck size={13} />
                            </span>
                          )}
                        </span>
                        <span className="connect-actions">
                          {client.how === "command" && !connected && (
                            <button
                              className="btn-ghost connect-go"
                              disabled={!ready || busy !== ""}
                              aria-busy={connecting}
                              onClick={() => void register(client)}
                            >
                              {connecting && <span className="connect-spin" aria-hidden />}
                              {connecting ? "Connecting…" : "Connect"}
                            </button>
                          )}
                          <button
                            className={`btn-ghost connect-toggle${open ? " on" : ""}`}
                            disabled={!ready}
                            title={toggle}
                            aria-label={toggle}
                            aria-expanded={open}
                            onClick={() => setShown(shown === client.id ? null : client.id)}
                          >
                            {"</>"}
                          </button>
                        </span>
                      </div>

                      {open && (
                        <>
                          <CopyBlock text={text} onCopy={() => void copy(text, client.how === "command" ? "Command" : "Config")} />
                          {client.how === "config_file" && (
                            <div className="connect-path">
                              <code>{client.config_path}</code>
                              <Info text={`Paste the block into this file, then restart ${client.label}.`} />
                            </div>
                          )}
                        </>
                      )}
                    </li>
                  );
                })}
              </ul>

            </>
          )}
        </div>

        <div className="wizard-actions">
          <button className="btn-primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}

const PRIVACY =
  "Only part numbers leave this machine, for distributor and datasheet lookups. " +
  "Your AI agent's model provider sees what the agent reads.";

const CLIENTS =
  "SpinZero never edits another program's settings. Connect runs the agent's own " +
  "command. Where there is no command, copy the config into the file it names.";

/** An info icon. Its text shows on hover. */
function Info({ text }: { text: string }) {
  return (
    <span className="setup-info" title={text} aria-label={text} role="img">
      <IconInfo size={13} />
    </span>
  );
}

/** Text the user pastes somewhere else, with a copy button in its corner. */
function CopyBlock({ text, onCopy }: { text: string; onCopy: () => void }) {
  return (
    <div className="connect-block-wrap">
      <pre className="connect-block">{text}</pre>
      <button className="connect-copy" title="Copy" aria-label="Copy" onClick={onCopy}>
        <IconCopy size={13} />
      </button>
    </div>
  );
}
