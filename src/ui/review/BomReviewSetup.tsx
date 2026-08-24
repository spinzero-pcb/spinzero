import { useEffect, useState } from "react";
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
 *  names or sizes: the user is being asked to trust a boundary, not to audit one. */
const PRIVACY =
  "Only your BOM is sent for review — never your schematic or layout. " +
  "It is deleted as soon as the review finishes.";

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
  // the moment the job is real. A failure keeps it open, with the error attached.
  // `preparing` is excluded because that IS the readiness checks, and a finished run
  // (`done`) because the sheet must still open afterwards to run a second one.
  useEffect(() => {
    if (open && isRunning(detailedPhase) && detailedPhase !== "preparing") closeSetup();
  }, [open, detailedPhase, closeSetup]);

  // A verdict from a previous press is not a verdict on this one.
  useEffect(() => {
    if (open) clearError();
  }, [open, clearError]);

  if (!open) return null;

  const detailedBusy = isRunning(detailedPhase);
  const busy = running || detailedBusy;
  const fields = mapping
    ? [...mapping.fields].sort(
        (a, b) => bomFieldRank(a.logical) - bomFieldRank(b.logical) || a.logical.localeCompare(b.logical),
      )
    : [];

  function start() {
    clearError();
    if (depth === "detailed") void startDetailed();
    else {
      closeSetup();
      void run();
    }
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
                title={PRIVACY}
                aria-label={PRIVACY}
                onClick={(e) => e.preventDefault()}
              >
                <IconInfo size={13} />
              </button>
            </span>
          </label>

          {/* Readiness is checked on the button press, so this is where its verdict
              belongs — beside the control the user just used, not in a toast. */}
          {depth === "detailed" && detailedError && (
            <p className="wizard-hint setup-error">Couldn’t start: {detailedError}</p>
          )}
          {depth === "detailed" && !service?.base_url && <ServiceFields />}

          <div className="wizard-actions">
            <button className="btn-ghost" onClick={closeSetup}>
              Cancel
            </button>
            <button className="btn-primary" disabled={busy} onClick={start}>
              {busy ? "Starting…" : "Run Review"}
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
