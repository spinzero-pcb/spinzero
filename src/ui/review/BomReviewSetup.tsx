import { useEffect, useState } from "react";
import { useBomCheckStore } from "../../stores/bomCheckStore";
import { useBomMappingStore } from "../../stores/bomMappingStore";
import { useDetailedReviewStore } from "../../stores/detailedReviewStore";
import { useReviewRunsStore } from "../../stores/reviewRunsStore";
import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { BOM_PROFILES, isBomProfile, type MappingView } from "../../lib/findings";
import { reviewKind } from "../../lib/reviewCatalog";
import { ipc } from "../../lib/ipc";
import { formatRelative } from "../../lib/time";

// The BOM review's setup sheet — beat two of a run: pick, set up, run.
//
// Every review type gets one of these and they all share the frame (what it reads →
// its own scope → what it will cost → Cancel/Run); only the scope section differs.
// For the BOM the scope is the end application and the column mapping, because those
// are the two things that decide what the rules actually see. Depth lives here rather
// than as a second entry in the picker: two tiers per review across six reviews is
// twelve tiles, and one radio is not.

export function BomReviewSetup() {
  const setupFor = useRunLauncherStore((s) => s.setupFor);
  const closeSetup = useRunLauncherStore((s) => s.closeSetup);

  const profile = useBomCheckStore((s) => s.profile);
  const setProfile = useBomCheckStore((s) => s.setProfile);
  const depth = useBomCheckStore((s) => s.depth);
  const setDepth = useBomCheckStore((s) => s.setDepth);
  const running = useBomCheckStore((s) => s.running);
  const run = useBomCheckStore((s) => s.run);

  const openMapping = useBomMappingStore((s) => s.openDialog);
  const openPreflight = useDetailedReviewStore((s) => s.openPreflight);
  const detailedPhase = useDetailedReviewStore((s) => s.phase);

  const lastRun = useReviewRunsStore((s) => s.runs.bom);

  const open = setupFor === "bom";
  const kind = reviewKind("bom");

  // Read-only view of the mapping, so the sheet can SAY what the review will read
  // instead of only offering a button that opens somewhere else.
  const [mapping, setMapping] = useState<MappingView | null>(null);
  const [mapErr, setMapErr] = useState(false);

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

  if (!open || !kind) return null;

  const readCount = mapping?.fields.filter((f) => f.column).length ?? 0;
  const fieldCount = mapping?.fields.length ?? 0;
  const unmapped = mapping?.unmapped_columns.length ?? 0;
  const busy = running || detailedPhase === "preflight";

  function start() {
    closeSetup();
    if (depth === "detailed") void openPreflight();
    else void run();
  }

  return (
    <div
      className="wizard-overlay"
      onPointerDown={(e) => e.target === e.currentTarget && closeSetup()}
    >
      <div className="wizard-card review-setup" role="dialog" aria-label="Review the bill of materials">
        <div className="wizard-head">
          <div>
            <div className="wizard-title">Review the bill of materials</div>
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
          <p className="wizard-hint">{kind.blurb}</p>

          <div className="setup-section">End application</div>
          <div className="setup-row">
            <select
              className="rv-select"
              value={profile}
              disabled={busy}
              title="Decides which rules apply and how severe a gap is"
              onChange={(e) => isBomProfile(e.target.value) && setProfile(e.target.value)}
            >
              {BOM_PROFILES.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </select>
          </div>

          <div className="setup-section">Column mapping</div>
          <div className="setup-row">
            <span className="setup-fact">
              {mapping
                ? `${readCount} of ${fieldCount} rule inputs read${
                    unmapped ? ` · ${unmapped} column${unmapped === 1 ? "" : "s"} unread` : ""
                  }`
                : mapErr
                  ? "Can’t read the BOM yet — extract the design first."
                  : "…"}
            </span>
            <button
              className="btn-ghost"
              disabled={!mapping}
              onClick={() => void openMapping(profile)}
            >
              Review mapping…
            </button>
          </div>

          <div className="setup-section">Depth</div>
          <label className={`setup-depth ${depth === "quick" ? "on" : ""}`}>
            <input
              type="radio"
              name="bom-depth"
              checked={depth === "quick"}
              disabled={busy}
              onChange={() => setDepth("quick")}
            />
            <span className="setup-depth-name">Quick checks</span>
            <span className="setup-depth-meta">included · instant</span>
            <span className="setup-depth-what">
              Deterministic rules over the columns — missing part numbers, DNP conflicts,
              spec mismatches against the value and footprint.
            </span>
          </label>
          <label className={`setup-depth ${depth === "detailed" ? "on" : ""}`}>
            <input
              type="radio"
              name="bom-depth"
              checked={depth === "detailed"}
              disabled={busy}
              onChange={() => setDepth("detailed")}
            />
            <span className="setup-depth-name">Detailed review</span>
            <span className="setup-depth-meta tag-premium">premium · minutes</span>
            <span className="setup-depth-what">
              Reads the datasheets behind the part numbers and judges what a column cannot
              state: whether a part is really qualified, really in production, really the
              one the design needs. You will see the exact file list before anything is sent.
            </span>
          </label>

          {/* Say that the scope persists. A remembered scope that stays quiet is how
              someone reviews less than they believe they reviewed. */}
          <p className="wizard-hint setup-memory">
            {lastRun
              ? `These settings are remembered for this project — last run ${formatRelative(lastRun.ts)}.`
              : "These settings are remembered for this project."}
          </p>

          <div className="wizard-actions">
            <button className="btn-ghost" onClick={closeSetup}>
              Cancel
            </button>
            <button className="btn-primary" disabled={busy} onClick={start}>
              {busy ? "Starting…" : depth === "detailed" ? "Continue" : "Run checks"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
