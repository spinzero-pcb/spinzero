import { useEffect, useRef, useState } from "react";
import { useCrunchStore } from "../../stores/crunchStore";
import { ipc } from "../../lib/ipc";

const MIN_H = 64;
const STORE_KEY = "bottomPanelH";

export function BottomPanel() {
  const { phase, lines, artifacts, error, skipReason } = useCrunchStore();
  const bodyRef = useRef<HTMLDivElement>(null);
  // Drag-resizable height (item 10), persisted across sessions.
  const [height, setHeight] = useState(() => {
    const h = Number(localStorage.getItem(STORE_KEY));
    return Number.isFinite(h) && h >= MIN_H ? h : 200;
  });

  useEffect(() => {
    bodyRef.current?.scrollTo({ top: bodyRef.current.scrollHeight });
  }, [lines.length, artifacts.length]);

  function startResize(e: React.PointerEvent<HTMLDivElement>) {
    e.preventDefault();
    const startY = e.clientY;
    const startH = height;
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const clamp = (h: number) =>
      Math.min(Math.round(window.innerHeight * 0.7), Math.max(MIN_H, h));
    const onMove = (ev: PointerEvent) => setHeight(clamp(startH + (startY - ev.clientY)));
    const onUp = (ev: PointerEvent) => {
      handle.releasePointerCapture(ev.pointerId);
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      setHeight((h) => {
        localStorage.setItem(STORE_KEY, String(h));
        return h;
      });
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
  }

  return (
    <div className="bottom-panel" style={{ height }}>
      <div className="panel-resize" onPointerDown={startResize} title="Drag to resize" />
      <div className="panel-tabs">
        <button className="panel-tab active">Output</button>
      </div>
      <div className="panel-body" ref={bodyRef}>
        {error && (
          <div className="error-card">
            <div className="title">Refresh failed · step: {error.stage}</div>
            <div className="line">{error.stderrTail}</div>
            <button className="btn-ghost" onClick={() => void ipc.crunchNow().catch(() => {})}>
              Retry
            </button>
          </div>
        )}
        {phase === "skipped" && skipReason === "hashes_unchanged" && (
          <div className="line">Already up to date — no design changes since last refresh.</div>
        )}
        {lines.map((l, i) => (
          <div className="line" key={i}>
            {l}
          </div>
        ))}
        {artifacts.length > 0 && (
          <div className="line">
            {artifacts.length} artifact{artifacts.length === 1 ? "" : "s"} written
          </div>
        )}
        {phase === "idle" && lines.length === 0 && !error && (
          <div className="line">
            Output appears here when the design is refreshed — automatically on
            save, or via the Refresh button.
          </div>
        )}
      </div>
    </div>
  );
}
