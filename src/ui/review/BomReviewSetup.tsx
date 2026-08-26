import { useEffect, useRef, useState } from "react";
import { isAgentRunning, useAgentReviewStore } from "../../stores/agentReviewStore";
import { useBomCheckStore } from "../../stores/bomCheckStore";
import { useBomMappingStore } from "../../stores/bomMappingStore";
import { DEFAULT_SERVICE_URL, isRunning, useDetailedReviewStore } from "../../stores/detailedReviewStore";
import { useProjectStore } from "../../stores/projectStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { useSettingsStore } from "../../stores/settingsStore";
import type { MappingView } from "../../lib/findings";
import { bomProfileForClass, isProjectClass, PROJECT_CLASSES } from "../../lib/projectClass";
import { ipc } from "../../lib/ipc";
import { bomFieldLabel, bomFieldRank } from "../BomMappingDialog";
import { IconInfo, IconPremium } from "../icons";

// The BOM review's setup sheet — the ONE place a BOM review is set up and started.
//
// It used to be two: this sheet chose the depth, and a second "detailed review"
// dialog then itemised every file about to be uploaded and offered a separate button.
// Two dialogs for one intent, and the second one's inventory was the wrong answer to
// the question it was trying to answer. The privacy promise is one sentence behind an
// info icon; the readiness checks it fronted now run when Run Review is pressed, and
// anything that goes wrong is an error right here rather than in a dialog the user has
// to correlate with the button they pressed.
//
// The end application is `project.class`, not a second setting — see lib/projectClass.

/** Short, non-verbose, and the whole promise. Deliberately says nothing about file
 *  names or sizes: the user is being asked to trust a boundary, not to audit one.
 *
 *  There are two promises now, because there are two places the review can run, and
 *  the difference is the entire point of offering the choice. */
const PRIVACY_SERVICE =
  "Only your BOM is sent for review — never your schematic or layout. " +
  "It is deleted as soon as the review finishes.";
const PRIVACY_AGENT =
  "The review runs on this machine. Only part numbers are looked up online — " +
  "your BOM, schematic and layout never leave the computer.";

export function BomReviewSetup() {
  const setupFor = useRunLauncherStore((s) => s.setupFor);
  const closeSetup = useRunLauncherStore((s) => s.closeSetup);

  const project = useProjectStore((s) => s.project);
  const setClass = useProjectStore((s) => s.setClass);
  const cls = project?.class ?? "general";

  const depth = useBomCheckStore((s) => s.depth);
  const setDepth = useBomCheckStore((s) => s.setDepth);
  const running = useBomCheckStore((s) => s.running);
  const run = useBomCheckStore((s) => s.run);

  const openMapping = useBomMappingStore((s) => s.openDialog);

  // Which surface a detailed review runs on. Absent means the hosted service, so an
  // existing install's button keeps doing exactly what it did yesterday.
  const driver = useSettingsStore((s) => s.reviewDriver) ?? "service";
  const setDriver = useSettingsStore((s) => s.setReviewDriver);
  const agentConfig = useSettingsStore((s) => s.agentReview);
  const agentPhase = useAgentReviewStore((s) => s.phase);
  const agentError = useAgentReviewStore((s) => s.error);
  const startAgent = useAgentReviewStore((s) => s.start);
  const startDetailed = useDetailedReviewStore((s) => s.start);
  const detailedPhase = useDetailedReviewStore((s) => s.phase);
  const detailedError = useDetailedReviewStore((s) => s.error);
  const clearError = useDetailedReviewStore((s) => s.clearError);
  // Subscribed, not read once: saving the address below has to make these fields go away.
  const service = useSettingsStore((s) => s.reviewService);

  const open = setupFor === "bom";

  // The mapping itself, read-only — the sheet SHOWS what the review will read instead
  // of hiding it behind a button that opens somewhere else.
  const [mapping, setMapping] = useState<MappingView | null>(null);
  const [mapErr, setMapErr] = useState(false);
  const profile = bomProfileForClass(cls);

  useEffect(() => {
    if (!open) return;
    let alive = true;
    setMapping(null);
    setMapErr(false);
    ipc
      .getBomMapping(profile)
      .then((v) => alive && setMapping(v))
      .catch(() => alive && setMapErr(true));
    return () => {
      alive = false;
    };
  }, [open, profile]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        closeSetup();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, closeSetup]);

  // A detailed run that got past its readiness checks belongs to the footer and the
  // BOM tab, not to a dialog sitting on top of the board — so the sheet steps aside
  // the moment the job THIS SHEET started becomes real. A failure keeps it open,
  // with the error attached. `preparing` is excluded because that IS the readiness
  // checks, and a finished run (`done`) because the sheet must still open afterwards
  // to run a second one.
  //
  // `startedHere` is what makes this a transition rather than a state. Without it the
  // effect fired on every render while any detailed run was in flight, so opening BOM
  // Review from the launcher during a run slammed the sheet shut before it painted —
  // the review became unreachable for the several minutes it takes to run, which is
  // exactly when someone wants to look at what it is reviewing.
  const startedHere = useRef(false);
  useEffect(() => {
    if (!open) {
      startedHere.current = false;
      return;
    }
    if (startedHere.current && isRunning(detailedPhase) && detailedPhase !== "preparing") {
      startedHere.current = false;
      closeSetup();
    }
  }, [open, detailedPhase, closeSetup]);

  // A verdict from a previous press is not a verdict on this one.
  useEffect(() => {
    if (open) clearError();
  }, [open, clearError]);

  if (!open) return null;

  const detailedBusy = isRunning(detailedPhase) || isAgentRunning(agentPhase);
  const busy = running || detailedBusy;
  const fields = mapping
    ? [...mapping.fields].sort(
        (a, b) => bomFieldRank(a.logical) - bomFieldRank(b.logical) || a.logical.localeCompare(b.logical),
      )
    : [];

  function start() {
    clearError();
    if (depth !== "detailed") {
      closeSetup();
      void run();
      return;
    }
    if (driver === "agent") {
      // The assistant runs in its own process and reports through `agent-event`, so
      // there is no in-sheet phase to wait on: close and let the status bar carry it.
      closeSetup();
      void startAgent();
      return;
    }
    startedHere.current = true;
    void startDetailed();
  }

  return (
    <div
      className="wizard-overlay"
      onPointerDown={(e) => e.target === e.currentTarget && closeSetup()}
    >
      <div className="wizard-card review-setup" role="dialog" aria-label="BOM review">
        <div className="wizard-head">
          <div>
            <div className="wizard-title">BOM Review</div>
            <div className="wizard-step">
              {mapping
                ? `${mapping.row_count} lines · ${mapping.columns.length} columns`
                : mapErr
                  ? "No BOM extracted yet"
                  : "Reading the BOM…"}
            </div>
          </div>
        </div>

        <div className="wizard-body">
          <div className="setup-app">
            <span className="setup-app-label">End application</span>
            <select
              className="rv-select setup-app-select"
              value={cls}
              disabled={busy}
              title="Decides which rules apply and how severe a gap is"
              onChange={(e) => isProjectClass(e.target.value) && void setClass(e.target.value)}
            >
              {PROJECT_CLASSES.map((c) => (
                <option key={c.value} value={c.value}>
                  {c.label}
                </option>
              ))}
            </select>
          </div>

          <div className="setup-section">
            Column mapping
            <button
              className="btn-ghost setup-section-act"
              disabled={!mapping || busy}
              onClick={() => void openMapping(profile)}
            >
              Edit…
            </button>
          </div>
          {mapErr ? (
            <p className="wizard-hint">Can’t read the BOM yet — extract the design first.</p>
          ) : !mapping ? (
            <p className="wizard-hint">…</p>
          ) : (
            <ul className="setup-map">
              {fields.map((f) => (
                <li key={f.logical} className={`setup-map-row ${f.column ? "" : "unmapped"}`}>
                  <span className="setup-map-field">{bomFieldLabel(f.logical)}</span>
                  <span className="setup-map-col">{f.column || "not in this BOM"}</span>
                </li>
              ))}
            </ul>
          )}

          <div className="setup-section">Depth</div>
          <label className={`setup-depth ${depth === "quick" ? "on" : ""}`}>
            <input
              type="radio"
              name="bom-depth"
              checked={depth === "quick"}
              disabled={busy}
              onChange={() => setDepth("quick")}
            />
            <span className="setup-depth-name">Instant Check</span>
          </label>
          <label className={`setup-depth ${depth === "detailed" ? "on" : ""}`}>
            <input
              type="radio"
              name="bom-depth"
              checked={depth === "detailed"}
              disabled={busy}
              onChange={() => setDepth("detailed")}
            />
            <span className="setup-depth-name">
              Detailed Review
              <span className="badge-premium" title="Premium review" aria-label="Premium review">
                <IconPremium size={12} />
              </span>
            </span>
            <span className="setup-depth-what">
              Find the deepest errors in your BOM, backed by the datasheets.
              <button
                type="button"
                className="setup-info"
                title={driver === "agent" ? PRIVACY_AGENT : PRIVACY_SERVICE}
                aria-label={driver === "agent" ? PRIVACY_AGENT : PRIVACY_SERVICE}
                onClick={(e) => e.preventDefault()}
              >
                <IconInfo size={13} />
              </button>
            </span>
          </label>

          {/* WHERE it runs, which is a different question from how deep it goes. Both
              produce the same findings document through the same ingestion path; what
              differs is whose tokens pay for it and whether the BOM leaves the
              machine. */}
          {depth === "detailed" && (
            <>
              <div className="setup-section">Run it</div>
              <label className={`setup-depth ${driver === "service" ? "on" : ""}`}>
                <input
                  type="radio"
                  name="bom-driver"
                  checked={driver === "service"}
                  disabled={busy}
                  onChange={() => void setDriver("service")}
                />
                <span className="setup-depth-name">On SpinZero&rsquo;s service</span>
                <span className="setup-depth-what">
                  We run it. Your BOM is uploaded and deleted when the review finishes.
                </span>
              </label>
              <label className={`setup-depth ${driver === "agent" ? "on" : ""}`}>
                <input
                  type="radio"
                  name="bom-driver"
                  checked={driver === "agent"}
                  disabled={busy}
                  onChange={() => void setDriver("agent")}
                />
                <span className="setup-depth-name">With my AI assistant</span>
                <span className="setup-depth-what">
                  Your assistant does the reading, on your own subscription. Nothing about the
                  design leaves this computer.
                </span>
              </label>
            </>
          )}

          {/* Readiness is checked on the button press, so this is where its verdict
              belongs — beside the control the user just used, not in a toast. */}
          {depth === "detailed" && detailedError && (
            <p className="wizard-hint setup-error">Couldn’t start: {detailedError}</p>
          )}
          {/* The sheet is reachable while a review runs — you can read the mapping and
              the depth it is running at. It just cannot start a second one, and saying
              so beats a disabled button with no explanation. */}
          {detailedBusy && (
            <p className="wizard-hint">
              A detailed review is running. Its progress is in the status bar; you can start
              another when it finishes.
            </p>
          )}
          {depth === "detailed" && driver === "agent" && agentError && (
            <p className="wizard-hint setup-error">Couldn’t start: {agentError}</p>
          )}
          {depth === "detailed" && driver === "service" && !service?.base_url && <ServiceFields />}
          {depth === "detailed" && driver === "agent" && !agentConfig && <AgentFields />}

          <div className="wizard-actions">
            <button className="btn-ghost" onClick={closeSetup}>
              Cancel
            </button>
            {/* Two different disabled states, and conflating them was a small lie: a
                run someone else already started is not this button "starting". */}
            <button className="btn-primary" disabled={busy} onClick={start}>
              {detailedBusy ? "Review running…" : running ? "Starting…" : "Run Review"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/** Where the review service lives. Shown only when there is nothing configured — the
 *  one thing the retired pre-flight dialog owned that still has to be reachable. */
function ServiceFields() {
  const saved = useSettingsStore((s) => s.reviewService);
  const [baseUrl, setBaseUrl] = useState(saved?.base_url ?? DEFAULT_SERVICE_URL);
  const [token, setToken] = useState(saved?.token ?? "");

  async function save() {
    const url = baseUrl.trim().replace(/\/+$/, "");
    if (!/^https?:\/\//i.test(url)) return;
    await useSettingsStore.getState().setReviewService({ base_url: url, token: token.trim() });
    void useDetailedReviewStore.getState().checkService();
  }

  return (
    <div className="review-service-config">
      <label className="review-field">
        <span>Service URL</span>
        <input
          className="wizard-input"
          value={baseUrl}
          spellCheck={false}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder={DEFAULT_SERVICE_URL}
        />
      </label>
      <label className="review-field">
        <span>Token</span>
        <input
          className="wizard-input"
          type="password"
          value={token}
          spellCheck={false}
          onChange={(e) => setToken(e.target.value)}
          placeholder="SPINZERO_DEV_TOKEN"
        />
      </label>
      <button className="btn-ghost" onClick={() => void save()}>
        Save service
      </button>
    </div>
  );
}

/**
 * How to start the assistant, shown only when nothing is configured.
 *
 * Two fields and no more. The MCP server's location is the one thing the app cannot
 * guess before it ships as a bundled binary (M2), and the environment box exists
 * because that server needs distributor credentials and the path to the rule pack.
 * Everything else — the config file, `--strict-mcp-config`, the tool allowlist — is
 * written by the app, so a user who has never configured an MCP server still gets a
 * working review.
 */
function AgentFields() {
  const saved = useSettingsStore((s) => s.agentReview);
  const [command, setCommand] = useState(saved?.server_command ?? "node");
  const [args, setArgs] = useState((saved?.server_args ?? []).join(" "));
  const [env, setEnv] = useState(
    Object.entries(saved?.server_env ?? {})
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
  );

  async function save() {
    const parsedArgs = args.trim().split(/\s+/).filter(Boolean);
    if (!command.trim() || !parsedArgs.length) return;
    // `KEY=value` per line, and a value may itself contain `=` (paths and tokens do).
    const parsedEnv: Record<string, string> = {};
    for (const line of env.split(/\r?\n/)) {
      const eq = line.indexOf("=");
      if (eq <= 0) continue;
      parsedEnv[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
    }
    await useSettingsStore.getState().setAgentReview({
      claude_bin: "",
      server_command: command.trim(),
      server_args: parsedArgs,
      server_env: parsedEnv,
    });
  }

  return (
    <div className="review-service-config">
      <p className="wizard-hint">
        SpinZero starts its own review server and hands it to your assistant. Tell it how.
      </p>
      <label className="review-field">
        <span>Server command</span>
        <input
          className="wizard-input"
          value={command}
          spellCheck={false}
          onChange={(e) => setCommand(e.target.value)}
          placeholder="node"
        />
      </label>
      <label className="review-field">
        <span>Arguments</span>
        <input
          className="wizard-input"
          value={args}
          spellCheck={false}
          onChange={(e) => setArgs(e.target.value)}
          placeholder="/path/to/spinzero-mcp/src/server.ts"
        />
      </label>
      <label className="review-field">
        <span>Environment</span>
        <textarea
          className="wizard-input"
          rows={3}
          value={env}
          spellCheck={false}
          onChange={(e) => setEnv(e.target.value)}
          placeholder={"SPINZERO_MCP_DEV=1\nDIGIKEY_CLIENT_ID=…\nDIGIKEY_CLIENT_SECRET=…"}
        />
      </label>
      <button className="btn-ghost" onClick={() => void save()}>
        Save
      </button>
    </div>
  );
}
