import { useMeasureStore } from "../../stores/measureStore";
import { useReviewStore } from "../../stores/reviewStore";
import { IconComment, IconFit, IconRuler, IconZoomIn, IconZoomOut } from "../icons";

// Floating PCB tool bar (docs/measure-tool-plan.md §6): top-centre of the PCB
// viewport, mounted inside PcbGlView so it exists only on the PCB tab (per-view tool).
// Fit / zoom are one-shot commands; Measure and Comment are mutually-exclusive mode
// toggles. Not a pan/zoom tool toggle (the banned ✋/I buttons, 2026-06-14) — pan/zoom
// stay gestures; these are commands + review/measure modes.

export function PcbToolbar({
  onFit,
  onZoomIn,
  onZoomOut,
}: {
  onFit: () => void;
  onZoomIn: () => void;
  onZoomOut: () => void;
}) {
  const active = useMeasureStore((s) => s.active);
  const units = useMeasureStore((s) => s.units);
  const armed = useReviewStore((s) => s.armed);

  const toggleMeasure = () => {
    const m = useMeasureStore.getState();
    if (!m.active) useReviewStore.getState().arm(false); // mutual exclusion
    m.toggle();
  };
  const toggleComment = () => {
    const r = useReviewStore.getState();
    if (!r.armed) useMeasureStore.getState().setActive(false); // mutual exclusion
    r.arm(!r.armed);
  };

  return (
    // Swallow pointer-downs so a click on the bar never starts a pan/measure on the canvas.
    <div className="pcb-toolbar" onPointerDown={(e) => e.stopPropagation()}>
      <button className="pcb-tbtn" title="Fit to screen (Home)" onClick={onFit}>
        <IconFit />
      </button>
      <button className="pcb-tbtn" title="Zoom out (PgDn)" onClick={onZoomOut}>
        <IconZoomOut />
      </button>
      <button className="pcb-tbtn" title="Zoom in (PgUp)" onClick={onZoomIn}>
        <IconZoomIn />
      </button>
      <span className="pcb-tsep" />
      <button
        className={`pcb-tbtn ${active ? "on" : ""}`}
        title="Measure (Ctrl+Shift+M)"
        aria-pressed={active}
        onClick={toggleMeasure}
      >
        <IconRuler />
      </button>
      <button
        className={`pcb-tbtn ${armed ? "on" : ""}`}
        title="Comment (C)"
        aria-pressed={armed}
        onClick={toggleComment}
      >
        <IconComment />
      </button>
      {active && (
        <button
          className="pcb-tunits"
          title="Cycle units (mm / mil / in)"
          onClick={() => useMeasureStore.getState().cycleUnits()}
        >
          {units}
        </button>
      )}
    </div>
  );
}
