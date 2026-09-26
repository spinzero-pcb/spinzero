import { useEffect, useRef, useState, type ReactNode } from "react";
import { isAgentRunning, useAgentReviewStore } from "../../stores/agentReviewStore";
import { useBomCheckStore, type BomDepth } from "../../stores/bomCheckStore";
import { effectiveColumn, useBomMappingStore } from "../../stores/bomMappingStore";
import { DEFAULT_SERVICE_URL, isRunning, useDetailedReviewStore } from "../../stores/detailedReviewStore";
import { useProjectStore } from "../../stores/projectStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { bomFieldLabel, bomFieldRank } from "../../lib/bomFields";
import { bomProfileForClass, isProjectClass, PROJECT_CLASSES } from "../../lib/projectClass";
import { IconChevron, IconInfo, IconPremium, IconRefresh } from "../icons";

// The BOM review window — the ONE place a BOM review is set up and started.
//
// Everything is on this one sheet, in the order it is decided: end application, the
// column mapping (edited in place — there is no second dialog), depth, and for a
// detailed review where it runs. Pressing Run saves the mapping (that IS the approval)
// and starts the review. Explanations are tooltips, not paragraphs.
//
// The end application is `project.class`, not a second setting — see lib/projectClass.

const PRIVACY_SERVICE =
  "Only your BOM is sent — never the schematic or layout. It is deleted when the review finishes.";
const PRIVACY_AGENT =
  "Runs on this machine with your own AI assistant. Only part numbers are looked up online.";

const DEPTH_HINT: Record<BomDepth, string> = {
  quick: "Rule checks · a few seconds · runs locally",
  detailed: "Every part checked against its datasheet · about 10 minutes",
};

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

  const loadMapping = useBomMappingStore((s) => s.load);
  const saveMapping = useBomMappingStore((s) => s.save);
  const resetMapping = useBomMappingStore((s) => s.reset);
  const mapping = useBomMappingStore((s) => s.view);
  const mapError = useBomMappingStore((s) => s.error);
  const saving = useBomMappingStore((s) => s.saving);

  // Which surface a detailed review runs on. Absent means the hosted service.
  const driver = useSettingsStore((s) => s.reviewDriver) ?? "service";
  const setDriver = useSettingsStore((s) => s.setReviewDriver);
  const agentConfig = useSettingsStore((s) => s.agentReview);
  const agentPhase = useAgentReviewStore((s) => s.phase);
  const agentError = useAgentReviewStore((s) => s.error);
  const startAgent = useAgentReviewStore((s) => s.start);
  const clearAgentError = useAgentReviewStore((s) => s.clearError);
  const startDetailed = useDetailedReviewStore((s) => s.start);
  const detailedPhase = useDetailedReviewStore((s) => s.phase);
  const detailedError = useDetailedReviewStore((s) => s.error);
  const clearError = useDetailedReviewStore((s) => s.clearError);
  // Subscribed, not read once: saving the address below has to make these fields go away.
  const service = useSettingsStore((s) => s.reviewService);

  const open = setupFor === "bom";
  const profile = bomProfileForClass(cls);

  // Read on open and whenever the end application changes. Edits survive the latter
  // (they are keyed by field) and are dropped on close.
  useEffect(() => {
    if (open) void loadMapping(profile);
  }, [open, profile, loadMapping]);
  useEffect(() => {
    if (!open) resetMapping();
  }, [open, resetMapping]);

  // A detailed run that got past its readiness checks belongs to the footer, so the
  // sheet steps aside the moment the job THIS SHEET started becomes real. A failure
  // keeps it open with the error. `startedHere` makes this a transition rather than a
  // state, so the sheet can still be opened while a run is in flight.
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
    if (open) {
      clearError();
      clearAgentError();
    }
  }, [open, clearError, clearAgentError]);

  const detailedBusy = isRunning(detailedPhase) || isAgentRunning(agentPhase);
  const busy = running || detailedBusy || saving;

  async function start() {
    if (busy) return;
    clearError();
    clearAgentError();
    // Run is the approval: persist the mapping the user is looking at, edits and all.
    if (!(await saveMapping())) return;
    if (depth !== "detailed") {
      closeSetup();
      void run();
      return;
    }
    if (driver === "agent") {
      // The assistant reports through `agent-event`; the footer carries it from here.
      closeSetup();
      void startAgent();
      return;
    }
    startedHere.current = true;
    void startDetailed();
  }

  // Re-bound every render so Ctrl+Enter always runs with the current choices.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        closeSetup();
      } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        void start();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  if (!open) return null;

  // One error line, for whichever step the last press failed at.
  const error =
    mapError && mapping
      ? mapError
      : depth === "detailed"
        ? driver === "agent"
          ? agentError
          : detailedError
        : null;

  return (
    <div className="wizard-overlay" onPointerDown={(e) => e.target === e.currentTarget && closeSetup()}>
      <div className="wizard-card review-setup" role="dialog" aria-label="BOM review">
        <div className="wizard-head">
          <div className="wizard-title">BOM Review</div>
          {mapping && (
            <div className="wizard-step rs-meta">
              {mapping.row_count} lines · {mapping.columns.length} columns
            </div>
          )}
        </div>

        <div className="wizard-body">
          <div className="rs-row">
            <span className="rs-label">Application</span>
            <select
              className="rv-select"
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

          <MappingSection disabled={busy} />

          <div className="rs-row">
            <span className="rs-label">Depth</span>
            <div className="rs-seg" role="radiogroup" aria-label="Depth">
              <SegButton on={depth === "quick"} disabled={busy} onClick={() => setDepth("quick")}>
                Instant
              </SegButton>
              <SegButton on={depth === "detailed"} disabled={busy} onClick={() => setDepth("detailed")}>
                Detailed
                <span className="badge-premium" title="Premium review" aria-label="Premium review">
                  <IconPremium size={12} />
                </span>
              </SegButton>
            </div>
          </div>
          <div className="rs-row rs-row-sub">
            <span />
            <span className="rs-hint">{DEPTH_HINT[depth]}</span>
          </div>

          {depth === "detailed" && (
            <div className="rs-row">
              <span className="rs-label">Runs on</span>
              <div className="rs-seg-line">
                <div className="rs-seg" role="radiogroup" aria-label="Runs on">
                  <SegButton
                    on={driver === "service"}
                    disabled={busy}
                    onClick={() => void setDriver("service")}
                  >
                    SpinZero
                  </SegButton>
                  <SegButton on={driver === "agent"} disabled={busy} onClick={() => void setDriver("agent")}>
                    My AI assistant
                  </SegButton>
                </div>
                <span
                  className="setup-info"
                  title={driver === "agent" ? PRIVACY_AGENT : PRIVACY_SERVICE}
                  aria-label={driver === "agent" ? PRIVACY_AGENT : PRIVACY_SERVICE}
                >
                  <IconInfo size={14} />
                </span>
              </div>
            </div>
          )}

          {depth === "detailed" && driver === "service" && !service?.base_url && <ServiceFields />}
          {depth === "detailed" && driver === "agent" && !agentConfig && <AgentFields />}

          {error && <p className="wizard-hint setup-error">{error}</p>}

          <div className="wizard-actions">
            <button className="btn-ghost" onClick={closeSetup}>
              Cancel
            </button>
            <button
              className="btn-primary"
              disabled={busy}
              title={detailedBusy ? "A detailed review is running — see the status bar" : "Ctrl+Enter"}
              onClick={() => void start()}
            >
              {detailedBusy ? "Review running…" : running || saving ? "Starting…" : "Run review"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function SegButton({
  on,
  disabled,
  onClick,
  children,
}: {
  on: boolean;
  disabled: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={on}
      className={`rs-seg-btn ${on ? "on" : ""}`}
      disabled={disabled}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

/**
 * The column mapping, edited in place. A one-line summary once approved; expanded the
 * first time, so the name-matching guess is seen before a review reads it. A field
 * nothing feeds reads downstream as "this data is missing", so those rows are coloured.
 */
function MappingSection({ disabled }: { disabled: boolean }) {
  const view = useBomMappingStore((s) => s.view);
  const loading = useBomMappingStore((s) => s.loading);
  const error = useBomMappingStore((s) => s.error);
  const draft = useBomMappingStore((s) => s.draft);
  const expanded = useBomMappingStore((s) => s.expanded);
  const setExpanded = useBomMappingStore((s) => s.setExpanded);
  const setField = useBomMappingStore((s) => s.setField);
  const resetField = useBomMappingStore((s) => s.resetField);

  if (!view) {
    return (
      <div className="rs-row">
        <span className="rs-label">Columns</span>
        <span className="rs-hint">
          {loading ? "Reading the BOM…" : error ? "No BOM yet — the design has not been extracted." : ""}
        </span>
      </div>
    );
  }

  const fields = [...view.fields].sort(
    (a, b) => bomFieldRank(a.logical) - bomFieldRank(b.logical) || a.logical.localeCompare(b.logical),
  );
  const samples = new Map(view.columns.map((c) => [c.name, c.sample]));
  const missing = fields.filter((f) => !effectiveColumn(view, draft, f.logical)).length;
  const edits = fields.filter((f) => effectiveColumn(view, draft, f.logical) !== f.column).length;
  // Recomputed against the draft: assigning a column must stop it being listed as
  // unused in the same breath.
  const claimed = new Set(fields.map((f) => effectiveColumn(view, draft, f.logical)));
  const unused = view.unmapped_columns.map((u) => u.column).filter((c) => !claimed.has(c));

  return (
    <div className="rs-map">
      <button
        type="button"
        className="rs-row rs-map-toggle"
        aria-expanded={expanded}
        onClick={() => setExpanded(!expanded)}
      >
        <span className="rs-label">Columns</span>
        <span className="rs-map-summary">
          <span>
            {fields.length - missing} of {fields.length} matched
          </span>
          {missing > 0 && <span className="rs-map-missing">{missing} not found</span>}
          {edits > 0 && <span className="rs-map-edited">{edits} edited</span>}
          <span className={`rs-map-chev ${expanded ? "open" : ""}`}>
            <IconChevron size={12} />
          </span>
        </span>
      </button>

      {expanded && (
        <>
          <div className="rs-map-table" role="table" aria-label="Column mapping">
            {fields.map((f) => {
              const col = effectiveColumn(view, draft, f.logical);
              // Diverging from the alias guess — whether edited now or approved long ago.
              const changed = col !== f.auto;
              const sample = col ? (samples.get(col) ?? "") : "";
              const label = bomFieldLabel(f.logical);
              return (
                <div className={`rs-map-row ${col ? "" : "missing"}`} key={f.logical} role="row">
                  <span className="rs-map-field" title={label}>
                    {label}
                  </span>
                  <select
                    className="bom-select rs-map-pick"
                    value={col}
                    disabled={disabled}
                    aria-label={`Column for ${label}`}
                    onChange={(e) => setField(f.logical, e.target.value)}
                  >
                    <option value="">Not in BOM</option>
                    {view.columns.map((c) => (
                      <option key={c.name} value={c.name}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                  <span className="rs-map-sample" title={sample}>
                    {sample}
                  </span>
                  {changed ? (
                    <button
                      type="button"
                      className="btn-ghost rs-map-reset"
                      disabled={disabled}
                      title={f.auto ? `Reset to ${f.auto}` : "Reset to not in BOM"}
                      aria-label={`Reset ${label}`}
                      onClick={() => resetField(f.logical)}
                    >
                      <IconRefresh size={12} />
                    </button>
                  ) : (
                    <span />
                  )}
                </div>
              );
            })}
          </div>
          {unused.length > 0 && (
            <p className="rs-hint rs-map-unused" title={unused.join(", ")}>
              Unused columns: {unused.join(", ")}
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** Where the review service lives. Shown only when nothing is configured. */
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
        Save
      </button>
    </div>
  );
}

/**
 * How to start the assistant's review server, shown only when nothing is configured.
 *
 * The MCP server's location is the one thing the app cannot guess before it ships as a
 * bundled binary (M2), and the environment box exists because that server needs
 * distributor credentials and the path to the rule pack. Everything else — the config
 * file, `--strict-mcp-config`, the tool allowlist — is written by the app.
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
