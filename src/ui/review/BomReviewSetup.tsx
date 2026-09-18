import { useEffect, useState } from "react";
import { isAgentRunning, useAgentReviewStore } from "../../stores/agentReviewStore";
import { useBomCheckStore } from "../../stores/bomCheckStore";
import { useBomMappingStore } from "../../stores/bomMappingStore";
import { useProjectStore } from "../../stores/projectStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { useSettingsStore } from "../../stores/settingsStore";
import type { MappingView } from "../../lib/findings";
import type { AgentProfile } from "../../lib/types";
import { DEFAULT_AGENT_PROFILE, missingFromAgent } from "../../lib/agentProfiles";
import { formatArgs, parseArgs } from "../../lib/mcpConfig";
import { bomProfileForClass, isProjectClass, PROJECT_CLASSES } from "../../lib/projectClass";
import { ipc } from "../../lib/ipc";
import { bomFieldLabel, bomFieldRank } from "../BomMappingDialog";
import { IconInfo, IconPremium } from "../icons";

// The BOM review's setup sheet — the ONE place a BOM review is set up and started.
//
// It used to ask WHERE the review runs, because there were two answers: a hosted
// service, or the user's own agent. The hosted tier is gone, so the question is gone
// with it, and what is left is which agent — a different question, and one the sheet
// only has to answer once.
//
// The end application is `project.class`, not a second setting — see lib/projectClass.

/** Short, non-verbose, and the whole promise. Deliberately says nothing about file
 *  names or sizes: the user is being asked to trust a boundary, not to audit one. */
const PRIVACY_AGENT =
  "The review runs on this machine. Only part numbers are looked up online — " +
  "your BOM, schematic and layout never leave the computer.";

export function BomReviewSetup() {
  const setupFor = useRunLauncherStore((s) => s.setupFor);
  const closeSetup = useRunLauncherStore((s) => s.closeSetup);
  const openConnect = useRunLauncherStore((s) => s.openConnect);

  const project = useProjectStore((s) => s.project);
  const setClass = useProjectStore((s) => s.setClass);
  const cls = project?.class ?? "general";

  const depth = useBomCheckStore((s) => s.depth);
  const setDepth = useBomCheckStore((s) => s.setDepth);
  const running = useBomCheckStore((s) => s.running);
  const run = useBomCheckStore((s) => s.run);

  const openMapping = useBomMappingStore((s) => s.openDialog);

  const agent = useSettingsStore((s) => s.agentProfile) ?? DEFAULT_AGENT_PROFILE;
  const serverConfigured = useSettingsStore((s) => s.agentReview !== null);
  const agentPhase = useAgentReviewStore((s) => s.phase);
  const agentError = useAgentReviewStore((s) => s.error);
  const startAgent = useAgentReviewStore((s) => s.start);
  const clearError = useAgentReviewStore((s) => s.clearError);

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

  // A verdict from a previous press is not a verdict on this one.
  useEffect(() => {
    if (open) clearError();
  }, [open, clearError]);

  if (!open) return null;

  const detailedBusy = isAgentRunning(agentPhase);
  const busy = running || detailedBusy;
  const fields = mapping
    ? [...mapping.fields].sort(
        (a, b) => bomFieldRank(a.logical) - bomFieldRank(b.logical) || a.logical.localeCompare(b.logical),
      )
    : [];
  const missing = missingFromAgent(agent);

  function start() {
    clearError();
    if (depth !== "detailed") {
      closeSetup();
      void run();
      return;
    }
    // The agent runs in its own process and reports through `agent-event`, so there is
    // no in-sheet phase to wait on: close and let the status bar carry it.
    closeSetup();
    void startAgent();
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
                title={PRIVACY_AGENT}
                aria-label={PRIVACY_AGENT}
                onClick={(e) => e.preventDefault()}
              >
                <IconInfo size={13} />
              </button>
            </span>
          </label>

          {depth === "detailed" && <AgentPicker busy={busy} />}

          {/* Two things the user has to have done, and only one of them is ours to
              check. Said plainly rather than left implied — see the setup screen. */}
          {depth === "detailed" && !serverConfigured && (
            <p className="wizard-hint">
              Your agent has to know about SpinZero’s review server first.{" "}
              <button className="btn-ghost setup-section-act" onClick={() => openConnect()}>
                Connect your AI assistant
              </button>
            </p>
          )}
          {depth === "detailed" && (
            <p className="wizard-hint">
              SpinZero starts your agent and adds nothing to it. It runs with your own
              permissions and can reach your other tools during the review, exactly as it
              does when you use it yourself.
            </p>
          )}

          {/* Readiness is checked on the button press, so this is where its verdict
              belongs — beside the control the user just used, not in a toast. */}
          {depth === "detailed" && agentError && (
            <p className="wizard-hint setup-error">Couldn’t start: {agentError}</p>
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

          <div className="wizard-actions">
            <button className="btn-ghost" onClick={closeSetup}>
              Cancel
            </button>
            {/* Two different disabled states, and conflating them was a small lie: a
                run someone else already started is not this button "starting". */}
            <button
              className="btn-primary"
              disabled={busy || (depth === "detailed" && missing.length > 0)}
              title={missing.length ? `Fill in ${missing.join(", ")} first` : undefined}
              onClick={start}
            >
              {detailedBusy ? "Review running…" : running ? "Starting…" : "Run Review"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * Which agent runs the review.
 *
 * An agent is a command line program that drives a model and speaks MCP. SpinZero
 * starts it and adds no flags of its own, so the whole of the setting is: which
 * program, which arguments, and how it takes its prompt.
 *
 * A profile SpinZero has not run end to end is still offered, and says so. Leaving it
 * out would mean a user with Codex cannot start a review at all; presenting it as
 * tested would be a claim we have not earned. The fields are editable either way, so
 * a wrong guess is a line to correct rather than a dead end.
 */
function AgentPicker({ busy }: { busy: boolean }) {
  const saved = useSettingsStore((s) => s.agentProfile);
  const setAgentProfile = useSettingsStore((s) => s.setAgentProfile);
  const current = saved ?? DEFAULT_AGENT_PROFILE;

  const [shipped, setShipped] = useState<AgentProfile[]>([]);
  const [edit, setEdit] = useState(false);
  const [bin, setBin] = useState(current.bin);
  const [args, setArgs] = useState(formatArgs(current.args));

  useEffect(() => {
    let alive = true;
    ipc
      .agentProfiles()
      .then((list) => alive && setShipped(list))
      .catch(() => {
        // The backend not answering leaves the saved profile in place, which is the
        // one that matters. Nothing to tell the user.
      });
    return () => {
      alive = false;
    };
  }, []);

  // The saved profile may not be in the shipped list (a hand-written one), so it is
  // offered alongside rather than silently replaced by the nearest match.
  const options = shipped.length ? shipped : [DEFAULT_AGENT_PROFILE];
  const known = options.some((p) => p.id === current.id);

  function pick(id: string) {
    const chosen = options.find((p) => p.id === id);
    if (!chosen) return;
    setBin(chosen.bin);
    setArgs(formatArgs(chosen.args));
    // A profile with nothing to run is not saved as the choice — it is the prompt to
    // fill the fields in, so the editor opens instead.
    setEdit(!chosen.bin.trim());
    void setAgentProfile(chosen.bin.trim() ? chosen : { ...chosen, bin: "" });
  }

  function saveEdits() {
    void setAgentProfile({ ...current, bin: bin.trim(), args: parseArgs(args) });
    setEdit(false);
  }

  return (
    <>
      <div className="setup-section">
        Your agent
        <button className="btn-ghost setup-section-act" disabled={busy} onClick={() => setEdit((v) => !v)}>
          {edit ? "Done" : "Edit…"}
        </button>
      </div>
      <select
        className="rv-select setup-app-select"
        value={known ? current.id : "custom"}
        disabled={busy}
        title="Which program SpinZero starts to run the review"
        onChange={(e) => pick(e.target.value)}
      >
        {options.map((p) => (
          <option key={p.id} value={p.id}>
            {p.label}
          </option>
        ))}
      </select>
      {!current.verified && current.bin.trim() && (
        <p className="wizard-hint">
          SpinZero has not tested this one. Check the command below before you run it; if
          the review never starts, the agent’s own output says why.
        </p>
      )}
      {edit && (
        <div className="review-service-config">
          <label className="review-field">
            <span>Program</span>
            <input
              className="wizard-input"
              value={bin}
              spellCheck={false}
              placeholder="claude"
              onChange={(e) => setBin(e.target.value)}
            />
          </label>
          <label className="review-field">
            <span>Arguments</span>
            <input
              className="wizard-input"
              value={args}
              spellCheck={false}
              placeholder="-p {prompt}"
              onChange={(e) => setArgs(e.target.value)}
            />
          </label>
          <p className="wizard-hint">
            <code>{"{prompt}"}</code> is where the review instructions go and{" "}
            <code>{"{project_dir}"}</code> is this board’s folder. Each argument is passed
            whole, so a path with a space needs no quoting of yours.
          </p>
          <button className="btn-ghost" disabled={!bin.trim()} onClick={saveEdits}>
            Save
          </button>
        </div>
      )}
    </>
  );
}
