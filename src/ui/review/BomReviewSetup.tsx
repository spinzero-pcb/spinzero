import { useEffect, useState, type ReactNode } from "react";
import { isAgentRunning, useAgentReviewStore } from "../../stores/agentReviewStore";
import { useBomCheckStore, type BomDepth } from "../../stores/bomCheckStore";
import { effectiveColumn, useBomMappingStore } from "../../stores/bomMappingStore";
import { useProjectStore } from "../../stores/projectStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { useSettingsStore } from "../../stores/settingsStore";
import type { AgentProfile } from "../../lib/types";
import { DEFAULT_AGENT_PROFILE, missingFromAgent } from "../../lib/agentProfiles";
import { bomFieldLabel, bomFieldRank } from "../../lib/bomFields";
import { bomProfileForClass, isProjectClass, PROJECT_CLASSES } from "../../lib/projectClass";
import { ipc } from "../../lib/ipc";
import { IconCheck, IconChevron, IconInfo, IconPremium, IconRefresh } from "../icons";

// The BOM review window — the ONE place a BOM review is set up and started.
//
// Everything is on this one sheet, in the order it is decided: end application, the
// column mapping (edited in place — there is no second dialog), depth, and for a
// detailed review which assistant runs it. Pressing Run saves the mapping (that IS
// the approval) and starts the review. Explanations are tooltips, not paragraphs.
//
// **Which AI agent.** A dropdown of every agent the app can start. A connected one
// (its own config lists SpinZero) gets a green tick beside the dropdown; one that is
// not gets Connect… there instead, and Run waits. The command SpinZero starts for
// each agent is not shown: the shipped one works, and editing it helps nobody.
//
// The end application is `project.class`, not a second setting — see lib/projectClass.

const PRIVACY_AGENT =
  "Runs on this machine with your own AI agent, on your subscription. Only part " +
  "numbers are looked up online. The agent runs with its own permissions.";

const DEPTH_HINT: Record<BomDepth, string> = {
  quick: "Rule checks · a few seconds · runs locally",
  detailed: "Your AI agent follows the SpinZero review workflow to check every part against its datasheet." +
    "\nMuch deeper than Instant · about 10 minutes",
};

/** Which assistant (a Connect screen client) drives which agent profile. Only these
 *  can be started by the app: the others (Claude Desktop, VS Code, Windsurf) have no
 *  command line to start. */
const CLIENT_FOR_PROFILE: Record<string, string> = {
  "claude-code": "claude-code",
  "codex-cli": "codex",
  "gemini-cli": "gemini",
  "cursor-cli": "cursor",
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

  const agent = useSettingsStore((s) => s.agentProfile) ?? DEFAULT_AGENT_PROFILE;
  const agentPhase = useAgentReviewStore((s) => s.phase);
  const agentError = useAgentReviewStore((s) => s.error);
  const startAgent = useAgentReviewStore((s) => s.start);
  const clearAgentError = useAgentReviewStore((s) => s.clearError);

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

  // A verdict from a previous press is not a verdict on this one.
  useEffect(() => {
    if (open) clearAgentError();
  }, [open, clearAgentError]);

  const detailedBusy = isAgentRunning(agentPhase);
  const busy = running || detailedBusy || saving;
  // Set by the Runs on row: is the chosen agent connected to SpinZero? An agent that
  // is not would start with no SpinZero tools and fail minutes later.
  const [agentConnected, setAgentConnected] = useState(true);
  const missing = missingFromAgent(agent);
  const blocked = depth === "detailed" && (missing.length > 0 || !agentConnected);

  async function start() {
    if (busy || blocked) return;
    clearAgentError();
    // Run is the approval: persist the mapping the user is looking at, edits and all.
    if (!(await saveMapping())) return;
    closeSetup();
    // Either way the status bar carries the run from here.
    void (depth === "detailed" ? startAgent() : run());
  }

  // Re-bound every render so Ctrl+Enter always runs with the current choices.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      // The Connect screen sits on top of this window and handles its own keys.
      if (useRunLauncherStore.getState().connectOpen) return;
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
  const error = mapError && mapping ? mapError : depth === "detailed" ? agentError : null;

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

          {depth === "detailed" && <AgentRow busy={busy} onConnected={setAgentConnected} />}

          {error && <p className="wizard-hint setup-error">{error}</p>}

          <div className="wizard-actions">
            <button className="btn-ghost" onClick={closeSetup}>
              Cancel
            </button>
            <button
              className="btn-primary"
              disabled={busy || blocked}
              title={
                detailedBusy
                  ? "A detailed review is running — see the status bar"
                  : blocked
                    ? missing.length
                      ? `Fill in ${missing.join(", ")} first`
                      : `Connect ${agent.label} first`
                    : "Ctrl+Enter"
              }
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
  title,
  children,
}: {
  on: boolean;
  disabled: boolean;
  onClick: () => void;
  title?: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={on}
      className={`rs-seg-btn ${on ? "on" : ""}`}
      disabled={disabled}
      title={title}
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

/**
 * Which AI agent runs the detailed review: a dropdown of every agent the app can start.
 */
function AgentRow({ busy, onConnected }: { busy: boolean; onConnected: (yes: boolean) => void }) {
  const saved = useSettingsStore((s) => s.agentProfile);
  const setAgentProfile = useSettingsStore((s) => s.setAgentProfile);
  const openConnect = useRunLauncherStore((s) => s.openConnect);
  const connectOpen = useRunLauncherStore((s) => s.connectOpen);
  const current = saved ?? DEFAULT_AGENT_PROFILE;

  const [shipped, setShipped] = useState<AgentProfile[]>([]);
  const [connected, setConnected] = useState<Set<string> | null>(null);

  useEffect(() => {
    let alive = true;
    ipc
      .agentProfiles()
      .then((list) => alive && setShipped(list))
      .catch(() => {
        // The backend not answering leaves the saved profile in place, which is the
        // one that matters.
      });
    return () => {
      alive = false;
    };
  }, []);

  // Re-read when the Connect screen closes: the user may just have connected one.
  useEffect(() => {
    if (connectOpen) return;
    let alive = true;
    ipc
      .assistantConnected()
      .then((list) => alive && setConnected(new Set(list.map((c) => c.id))))
      .catch(() => alive && setConnected(new Set()));
    return () => {
      alive = false;
    };
  }, [connectOpen]);

  const isConnected = (p: AgentProfile) => {
    const client = CLIENT_FOR_PROFILE[p.id];
    return Boolean(client && connected?.has(client));
  };
  const options = (shipped.length ? shipped : [DEFAULT_AGENT_PROFILE]).filter(
    (p) => p.id in CLIENT_FOR_PROFILE || p.id === current.id,
  );
  const currentConnected = isConnected(current);
  // Unknown until the first read; a custom command cannot be checked, so it counts.
  const ready = connected === null || currentConnected || !(current.id in CLIENT_FOR_PROFILE);
  useEffect(() => onConnected(ready), [ready, onConnected]);

  function pick(p: AgentProfile) {
    void setAgentProfile(p);
  }

  if (connected === null) return null;

  return (
    <>
      <div className="rs-row">
        <span className="rs-label">Runs on</span>
        <div className="rs-seg-line">
          <select
            className="rv-select rs-agent"
            aria-label="Runs on"
            value={current.id}
            disabled={busy}
            onChange={(e) => {
              const p = options.find((o) => o.id === e.target.value);
              if (p) pick(p);
            }}
          >
            {options.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
          {currentConnected && (
            <span className="rs-ok" title="Connected" aria-label="Connected">
              <IconCheck size={14} />
            </span>
          )}
          {!ready && (
            <button
              type="button"
              className="btn-ghost rs-connect"
              title={`Register SpinZero with ${current.label}`}
              onClick={() => openConnect()}
            >
              Connect…
            </button>
          )}
          <span className="setup-info" title={PRIVACY_AGENT} aria-label={PRIVACY_AGENT} role="img">
            <IconInfo size={14} />
          </span>
        </div>
      </div>
    </>
  );
}
