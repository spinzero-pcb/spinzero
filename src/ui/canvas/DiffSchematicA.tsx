import { useEffect, useRef } from "react";
import { useDesignStore } from "../../stores/designStore";
import { useDiffStore } from "../../stores/diffStore";
import { camBridge, diffPaint, emphasizeDiffText } from "./navigator";
import { tintsA } from "../../lib/diff";

const SVG_NS = "http://www.w3.org/2000/svg";

/** The read-only A (older) schematic island for the side-by-side diff (§4). It renders
 *  the A-side SVG of whatever sheet the primary (B) Canvas currently shows, follows B's
 *  shared camera every frame, forwards its own pan/zoom back to B (so panning either side
 *  moves both), and paints the focused change's *removed/modified* objects in red/amber.
 *
 *  Deliberately NOT a fork of Canvas: no selection, no comments, no history, no hit-test
 *  — selection/cross-probe/comments stay anchored to B, the active revision. It reuses
 *  the same SVG-clone tint idiom and the same world-transform camera, nothing more.
 *
 *  Deviation (logged): the A sheet is resolved by reusing B's sheet→svg path (paired by
 *  the design's own numbering, which is stable for name-matched sheets across a
 *  deterministic re-extraction), read from the A cache via read_artifact_from. */
export function DiffSchematicA() {
  const active = useDiffStore((s) => s.active);
  const indexes = useDesignStore((s) => s.indexes); // B-side (active) indexes
  const getSheetSvgA = useDiffStore((s) => s.getSheetSvgA);

  const stageRef = useRef<HTMLDivElement>(null);
  const worldRef = useRef<HTMLDivElement>(null);
  const islandRef = useRef<HTMLDivElement>(null);

  // Which B-sheet the mounted SVG corresponds to (avoid reloading every frame).
  const loadedEpoch = useRef<number>(-1);
  const curSvg = useRef<SVGSVGElement | null>(null);

  useEffect(() => {
    if (!active || !indexes) return;
    const world = worldRef.current!;
    const island = islandRef.current!;
    const stage = stageRef.current!;
    const esc = (s: string) =>
      window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/["\\]/g, "\\$&");

    let raf = 0;
    let disposed = false;

    /** Clone the focused change's A-side uuids into a tinted overlay + scrim, mirroring
     *  Canvas.paintDiff. Only removed/modified/renamed/moved changes exist on A. */
    function paintFocused() {
      const svg = curSvg.current;
      if (!svg) return;
      svg.querySelectorAll(".hl-diff, .hl-diff-scrim").forEach((n) => n.remove());
      const change = diffPaint.focused;
      // Prefer the A-side anchor when the engine carried one (the object's uuids
      // differ between revisions — a re-annotated symbol, a renamed net's old wires).
      const sch = change?.anchors.schematicA ?? change?.anchors.schematic;
      if (!change || !sch || !tintsA(change)) return;
      const uuids = sch.uuids;
      const vb = camBridge.vb;
      const scrim = document.createElementNS(SVG_NS, "rect");
      scrim.setAttribute("class", "hl-diff-scrim");
      scrim.setAttribute("x", String(vb[0]));
      scrim.setAttribute("y", String(vb[1]));
      scrim.setAttribute("width", String(vb[2]));
      scrim.setAttribute("height", String(vb[3]));
      svg.appendChild(scrim);
      if (uuids.length === 0) return;
      // A side shows the OLD state: removed → red, modified/moved/renamed → amber.
      // (Never green — added objects don't exist on the older revision.)
      const role = change.kind === "removed" ? "err" : "warn";
      const ov = document.createElementNS(SVG_NS, "g");
      ov.setAttribute("class", `hl-diff hl-diff-${role} hl-diff-pulse`);
      // Accumulate the tinted objects' world (mm) extent while cloning, so an A-only
      // change can land the shared camera below.
      let minX = Infinity,
        minY = Infinity,
        maxX = -Infinity,
        maxY = -Infinity;
      for (const u of uuids) {
        const src = svg.querySelector(`g[data-uuid="${esc(u)}"]`) as SVGGraphicsElement | null;
        if (!src) continue;
        ov.appendChild(src.cloneNode(true));
        try {
          const b = src.getBBox();
          if (b.width || b.height) {
            minX = Math.min(minX, b.x);
            minY = Math.min(minY, b.y);
            maxX = Math.max(maxX, b.x + b.width);
            maxY = Math.max(maxY, b.y + b.height);
          }
        } catch {
          /* detached/hidden */
        }
      }
      // A side shows the OLD state: colour the pre-edit text (e.g. the old value
      // string) red inside the cloned overlay so the exact edit stands out.
      emphasizeDiffText(ov, change.emphA, "hl-diff-emph-err");
      svg.appendChild(ov);
      // An A-only change (a removed object) has no B-side geometry, so B deliberately
      // skips the camera landing (revealDiff `aOnly`). Land it here instead, from the A
      // extent — the shared camera works in the same world units. Objects present on B
      // (modified/moved) are left to B, which owns that landing (avoid a two-sided fight).
      if (change.side === "a" && isFinite(minX)) {
        camBridge.centerWorld({ x: minX, y: minY, width: maxX - minX, height: maxY - minY });
      }
    }

    /** Load the A-side SVG for whatever sheet B currently shows (paired by number →
     *  B's svg path, read from the A cache). Placeholder when the sheet has no A-side. */
    async function syncSheet() {
      const bSheetNum = camBridge.sheet;
      const bSheet = bSheetNum == null ? null : indexes!.sheets.find((s) => s.num === bSheetNum);
      loadedEpoch.current = camBridge.epoch;
      if (!bSheet?.svg) {
        island.innerHTML = `<div class="diff-a-placeholder">No matching sheet on ${
          useDiffStore.getState().doc?.a.label ?? "the older revision"
        }</div>`;
        curSvg.current = null;
        return;
      }
      try {
        const txt = await getSheetSvgA(bSheet.num, bSheet.svg);
        if (disposed || loadedEpoch.current !== camBridge.epoch) return;
        island.innerHTML = txt;
        const svg = island.querySelector("svg") as SVGSVGElement | null;
        curSvg.current = svg;
        if (svg) {
          // The extractor SVG carries only a viewBox, so give it an intrinsic size from
          // that viewBox (1px = 1mm world units) — exactly as the B Canvas does in
          // loadSheet. The .diff-a-world/.diff-a-island wrappers have no size of their
          // own, so a sized SVG is the only thing giving the pane dimensions; without it
          // the whole A island collapsed to 0×0 (an invisible left pane).
          const raw = (svg.getAttribute("viewBox") ?? "0 0 297 210").split(/\s+/).map(Number);
          svg.setAttribute("width", String(raw[2] || 297));
          svg.setAttribute("height", String(raw[3] || 210));
          svg.style.display = "block";
        }
        paintFocused();
      } catch {
        if (disposed) return;
        // Sheet absent on A (added sheet) or read failed → placeholder, not a crash.
        island.innerHTML = `<div class="diff-a-placeholder">Sheet added on ${
          useDiffStore.getState().doc?.b.label ?? "the newer revision"
        }</div>`;
        curSvg.current = null;
      }
    }

    // Follow B's camera every frame; reload when B's sheet changes. Only WRITE the
    // transform when it actually moved — an idle review session would otherwise keep the
    // compositor busy rewriting the same string 60×/s over a full schematic SVG.
    let lastX = NaN,
      lastY = NaN,
      lastS = NaN;
    const tick = () => {
      if (camBridge.epoch !== loadedEpoch.current) void syncSheet();
      const c = camBridge.cam;
      if (c.x !== lastX || c.y !== lastY || c.s !== lastS) {
        world.style.transform = `translate(${c.x}px,${c.y}px) scale(${c.s})`;
        lastX = c.x;
        lastY = c.y;
        lastS = c.s;
      }
      raf = requestAnimationFrame(tick);
    };

    // Repaint the tint whenever the focused change changes.
    const unsub = diffPaint.subscribe(() => paintFocused());

    // Forward pan/zoom to B so both sides move together.
    let drag: { px: number; py: number } | null = null;
    const onPointerDown = (e: PointerEvent) => {
      if (e.button !== 0 && e.button !== 1) return;
      drag = { px: e.clientX, py: e.clientY };
      stage.setPointerCapture(e.pointerId);
    };
    const onPointerMove = (e: PointerEvent) => {
      if (!drag) return;
      camBridge.drive(e.clientX - drag.px, e.clientY - drag.py, 1, 0, 0);
      drag = { px: e.clientX, py: e.clientY };
    };
    const onPointerUp = (e: PointerEvent) => {
      drag = null;
      try {
        stage.releasePointerCapture(e.pointerId);
      } catch {
        /* pointer already released */
      }
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      // Anchor the zoom at the cursor, translated into B's screen space (both stages are
      // the same size + layout, so the local cursor position maps 1:1).
      const r = stage.getBoundingClientRect();
      camBridge.drive(0, 0, e.deltaY < 0 ? 1.1 : 1 / 1.1, e.clientX - r.left, e.clientY - r.top);
    };

    stage.addEventListener("pointerdown", onPointerDown);
    stage.addEventListener("pointermove", onPointerMove);
    stage.addEventListener("pointerup", onPointerUp);
    stage.addEventListener("wheel", onWheel, { passive: false });

    loadedEpoch.current = -1; // force an initial sheet sync
    raf = requestAnimationFrame(tick);

    return () => {
      disposed = true;
      cancelAnimationFrame(raf);
      unsub();
      stage.removeEventListener("pointerdown", onPointerDown);
      stage.removeEventListener("pointermove", onPointerMove);
      stage.removeEventListener("pointerup", onPointerUp);
      stage.removeEventListener("wheel", onWheel);
    };
  }, [active, indexes, getSheetSvgA]);

  if (!active) return null;
  return (
    <div className="diff-a-stage" ref={stageRef}>
      <div className="diff-a-world" ref={worldRef}>
        {/* `canvas-island` carries the whole KiCad schematic colour theme — those rules
            are scoped to that class, so without it the A (older) SVG renders unthemed
            (labels/pin numbers fill:none, symbol strokes black). Reusing the class themes
            A identically to B and gives it the same paper card. `.diff-a-island` keeps the
            read-only-side layout hooks. */}
        <div className="diff-a-island canvas-island" ref={islandRef} />
      </div>
    </div>
  );
}
