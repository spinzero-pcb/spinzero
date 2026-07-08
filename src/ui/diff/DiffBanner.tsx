import { useDiffStore, type PcbDiffMode } from "../../stores/diffStore";
import { useViewStore } from "../../stores/viewStore";
import { IconRefresh } from "../icons";

/** The PCB compare modes (plan §4), in toolbar order. Side-by-side is deferred. */
const PCB_MODES: { id: PcbDiffMode; label: string; title: string }[] = [
  { id: "onion", label: "Overlay", title: "Isolated layer: removed copper red, added green, over the greyed-out base" },
  { id: "flicker", label: "Flicker", title: "Blink between the two revisions (~2 Hz) — hold Space to pause" },
  { id: "wipe", label: "Wipe", title: "Drag a divider: older revision left, newer right" },
];

/** Short, git-style revision id for the banner's trailing ref. */
const shortId = (id: string) => id.slice(0, 8);

/** Diff-mode banner (visual-diff §3): a strip at the top of the canvas area showing
 *  `A → B` with labels, a swap button, and × to exit back to normal viewing. View-global
 *  — it renders above whichever canvas (schematic / PCB) is up. */
export function DiffBanner() {
  const active = useDiffStore((s) => s.active);
  const a = useDiffStore((s) => s.a);
  const b = useDiffStore((s) => s.b);
  const preparing = useDiffStore((s) => s.preparing);
  const swap = useDiffStore((s) => s.swap);
  const exitDiff = useDiffStore((s) => s.exitDiff);
  const pcbMode = useDiffStore((s) => s.pcbMode);
  const setPcbMode = useDiffStore((s) => s.setPcbMode);
  const view = useViewStore((s) => s.view);

  if (!active || !a || !b) return null;

  return (
    <div className="diff-banner" role="region" aria-label="Comparing revisions">
      <span className="diff-banner-label">Comparing</span>
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
      {/* PCB compare-mode toggle (plan §4) — only meaningful on the PCB canvas. */}
      {view === "pcb" && (
        <span className="diff-mode-toggle" role="group" aria-label="PCB compare mode">
          {PCB_MODES.map((m) => (
            <button
              key={m.id}
              className={`btn-ghost diff-banner-btn ${pcbMode === m.id ? "is-active" : ""}`}
              title={m.title}
              onClick={() => setPcbMode(m.id)}
            >
              {m.label}
            </button>
          ))}
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
