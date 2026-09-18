import { useEffect, useState } from "react";

import { ipc } from "../../lib/ipc";
import { jsonBlockFor, SERVER_NAME } from "../../lib/mcpConfig";
import type { AssistantClient, AssistantSetup } from "../../lib/types";
import { useToastStore } from "../../stores/toastStore";
import { IconCopy, IconSparkle } from "../icons";

// "Connect your AI assistant" — the setup screen for running SpinZero reviews through
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
// gone. What the user does on this screen is what the in-app review runs on, so the
// screen has to say so — a reader who thinks the app registers itself will not come
// here, and will then start a review against an agent that has no SpinZero tools.
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
          <p className="wizard-hint">
            This is the only place SpinZero is registered with an assistant. A review you
            start from <em>Run a review</em> uses the same connection, so if the steps below
            are not done, that button cannot work either.
          </p>

          {loadError && <p className="wizard-hint err">Could not read this machine's setup: {loadError}</p>}

          {setup?.server_problem && (
            <>
              <div className="wizard-label">The review server is missing</div>
              <p className="wizard-hint err">{setup.server_problem}</p>
            </>
          )}

          {setup && (
            <>
              <div className="wizard-label">Step 1 — your licence key</div>
              {setup.licence_present ? (
                <p className="wizard-hint">
                  A key is saved in <code>{setup.licence_file}</code>. Every assistant reads it
                  from there, so nothing below carries it. Paste a new one to replace it — the
                  old one is kept, commented out, in case you need it back.
                </p>
              ) : (
                <p className="wizard-hint">
                  Without a key a review has almost no evidence to work from: no distributor
                  data, no datasheets. It is saved to <code>{setup.licence_file}</code> and read
                  from there by every assistant you connect.
                </p>
              )}
              <label className="review-field">
                <span>Licence key</span>
                <input
                  className="wizard-input"
                  // A secret, on screen, during every screen share of somebody's first
                  // setup.
                  type="password"
                  value={key}
                  spellCheck={false}
                  placeholder={setup.licence_present ? "(a key is saved)" : "sz_…"}
                  onChange={(e) => setKey(e.target.value)}
                />
              </label>
              <button className="btn-ghost" disabled={savingKey || !key.trim()} onClick={() => void saveKey()}>
                {savingKey ? "Saving…" : setup.licence_present ? "Replace the key" : "Save the key"}
              </button>

              <div className="wizard-label">Step 2 — your assistant</div>
              <p className="wizard-hint">
                SpinZero never edits another program's settings file. Where your assistant has
                its own command, we run that and it edits its own config. Where it does not,
                copy the block into the file named beside it.
              </p>

              <ul className="connect-clients">
                {setup.clients.map((client) => (
                  <li key={client.id} className="connect-client">
                    <div className="connect-client-head">
                      <span className="connect-client-name">
                        {client.label}
                        {!client.installed && <span className="connect-absent"> · not found here</span>}
                      </span>
                      {client.how === "command" ? (
                        <span className="connect-actions">
                          <button
                            className="btn-ghost"
                            disabled={!ready || busy === client.id}
                            onClick={() => void register(client)}
                          >
                            {busy === client.id ? "Connecting…" : "Connect"}
                          </button>
                          <button
                            className="btn-ghost"
                            onClick={() => setShown(shown === client.id ? null : client.id)}
                          >
                            {shown === client.id ? "Hide command" : "Show command"}
                          </button>
                        </span>
                      ) : (
                        <button
                          className="btn-ghost"
                          disabled={!ready}
                          onClick={() => setShown(shown === client.id ? null : client.id)}
                        >
                          {shown === client.id ? "Hide block" : "Show block"}
                        </button>
                      )}
                    </div>

                    {shown === client.id && ready && setup && (
                      <>
                        <pre className="connect-block">
                          {client.how === "command"
                            ? client.command
                            : jsonBlockFor(client.config_key, setup.server_command)}
                        </pre>
                        {client.how === "config_file" && (
                          <p className="wizard-hint">
                            Goes in <code>{client.config_path}</code>. Restart {client.label}
                            {" "}afterwards — it reads that file only at startup.
                          </p>
                        )}
                        <button
                          className="btn-ghost"
                          onClick={() =>
                            void copy(
                              client.how === "command"
                                ? client.command
                                : jsonBlockFor(client.config_key, setup.server_command),
                              client.how === "command" ? "Command" : "Config",
                            )
                          }
                        >
                          <IconCopy size={13} /> Copy
                        </button>
                      </>
                    )}
                  </li>
                ))}
              </ul>

              <div className="wizard-label">Step 3 — ask for a review</div>
              <p className="wizard-hint">
                In your assistant, say <em>run a {SERVER_NAME} review of this board</em> and point
                it at your KiCad project folder. It will ask you two questions — what the board
                is for, and whether we read your BOM columns right — and then work through the
                review on its own.
              </p>
              <p className="wizard-hint">
                Or press <em>Run a review</em> in SpinZero. It starts the same assistant with
                both of those answers already filled in, and shows the progress here.
              </p>
            </>
          )}

          <div className="wizard-label">What leaves this machine</div>
          <p className="wizard-hint">
            Manufacturer part numbers, for distributor and datasheet lookups. Not your schematic,
            BOM or layout. Note what that does not say: the rows your assistant reasons over go to{" "}
            <em>your</em> model provider, because your assistant is the one doing the reasoning —
            that is your subscription and their terms, not ours.
          </p>
          <p className="wizard-hint">
            Improvement telemetry is on. It sends rule ids, severities and part numbers for
            findings and dismissed rule candidates, plus which datasheets we failed to fetch —
            never designators, titles, evidence, file paths, project names or your licence key.
            Set <code>SPINZERO_TELEMETRY=0</code> in the server's environment to switch it off.
          </p>
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
