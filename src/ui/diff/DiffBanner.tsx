import { useDiffStore } from "../../stores/diffStore";
import { useViewStore } from "../../stores/viewStore";
import { IconRefresh } from "../icons";

/** Short, git-style revision id for the banner's trailing ref. */
const shortId = (id: string) => id.slice(0, 8);

/** Diff-mode banner (visual-diff §3): a strip at the top of the canvas area showing
 *  `A → B` with labels, a swap button, and × to exit back to normal viewing. View-global
 *  — it renders above whichever canvas (schematic / PCB) is up. On the PCB it also
 *  carries the compare-overlay toggles: Blink (pulse the changed copper; remembered
 *  across sessions) and Zones (drop copper pours from the compare). */
export function DiffBanner() {
  const active = useDiffStore((s) => s.active);
  const a = useDiffStore((s) => s.a);
  const b = useDiffStore((s) => s.b);
  const preparing = useDiffStore((s) => s.preparing);
  const swap = useDiffStore((s) => s.swap);
  const exitDiff = useDiffStore((s) => s.exitDiff);
  const blink = useDiffStore((s) => s.blink);
  const setBlink = useDiffStore((s) => s.setBlink);
  const hideZones = useDiffStore((s) => s.hideZones);
  const setHideZones = useDiffStore((s) => s.setHideZones);
  const view = useViewStore((s) => s.view);

  if (!active || !a || !b) return null;

  return (
    <div className="diff-banner" role="region" aria-label="Comparing revisions">
      <span className="diff-banner-label">Comparing</span>
      <span className="diff-banner-beta" title="Visual diff is a beta feature">Beta</span>
      <span className="diff-side diff-side-a" title={`${a.label} (${a.rev})`}>
        <span className="diff-side-name">{a.label}</span>
        <span className="mono dim">{shortId(a.rev)}</span>
      </span>
      <span className="diff-arrow">→</span>
      <span className="diff-side diff-side-b" title={`${b.label} (${b.rev})`}>
        <span className="diff-side-name">{b.label}</span>
        <span className="mono dim">{shortId(b.rev)}</span>
      </span>
      {preparing && <span className="diff-preparing">Preparing comparison…</span>}
      <span style={{ flex: 1 }} />
      {/* PCB compare toggles — only meaningful on the PCB canvas. */}
      {view === "pcb" && (
        <span className="diff-mode-toggle" role="group" aria-label="PCB compare options">
          <button
            className={`btn-ghost diff-banner-btn ${blink ? "is-active" : ""}`}
            title="Pulse the changed copper: added and removed blink in opposite phases (hold Space to pause). Remembered."
            onClick={() => setBlink(!blink)}
          >
            Blink
          </button>
          <button
            className={`btn-ghost diff-banner-btn ${!hideZones ? "is-active" : ""}`}
            title="Show/hide copper pours in the compare — pours re-flow around edits and can wash the view"
            onClick={() => setHideZones(!hideZones)}
          >
            Zones
          </button>
        </span>
      )}
      <button
        className="btn-ghost diff-banner-btn"
        title="Swap direction (compare the other way round)"
        disabled={preparing}
        onClick={() => void swap()}
      >
        <IconRefresh size={13} /> Swap
      </button>
      <button
        className="btn-ghost diff-banner-btn diff-banner-close"
        title="Exit comparison"
        onClick={exitDiff}
      >
        ✕
      </button>
    </div>
  );
}
