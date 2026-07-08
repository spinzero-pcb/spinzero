// Snap index for the measure tool (docs/measure-tool-plan.md §5). Two structures, both
// read straight from the geometry IR (exact mm floats — no DOM, no CTM math) and built
// once per geometry load:
//   • SnapIndex — a uniform grid-bucket index of discrete candidate points (pad centres
//     + corners, via centres, track/graphic endpoints & midpoints, poly vertices, arc &
//     circle centres carrying a radius for the R/Ø readout).
//   • EdgeIndex — the same buckets over line/arc/circle *primitives*, so the cursor can
//     snap onto the nearest point ON a track, board edge, courtyard or pad/via outline
//     (feature 13 + the center/edge helpers). Point snaps take priority over edge snaps.
// Pure module (no React / GL), unit-tested in measureSnap.test.ts.

import { PAD_SHAPE, type PcbGeometry, type PcbGraphicDef, type PcbPadDef } from "../../lib/pcbGeometry";

export type SnapKind = "pad" | "via" | "track" | "arc" | "graphic" | "component";

export interface SnapPoint {
  x: number;
  y: number;
  kind: SnapKind;
  /** Layer indices the source object occupies. The point is snappable only when at
   *  least one of them is visible; an empty list ⇒ layer-independent (always eligible). */
  layers: number[];
  /** Radius (mm) when this point is the centre of an arc/circle — drives the R/Ø readout. */
  radius?: number;
}

/** A resolved snap: a world point, its class (for marker/toggle purposes), whether it
 *  landed on an edge (vs a discrete vertex/centre), and an optional radius. */
export interface SnapHit {
  x: number;
  y: number;
  kind: SnapKind;
  edge: boolean;
  radius?: number;
}

/** Bucket size in mm. Queries scan the ring of cells covering the snap radius. */
const CELL = 5;
/** Cap the scanned ring so an extreme zoom-out (huge world radius) can't scan the
 *  whole board — snapping that far out isn't useful anyway. */
const MAX_RING = 24;

/** Pack two cell coords into one number key. Boards sit well within ±32k mm cells. */
function cellKey(cx: number, cy: number): number {
  return (cx + 32768) * 65536 + (cy + 32768);
}

export class SnapIndex {
  private cells = new Map<number, SnapPoint[]>();

  add(p: SnapPoint): void {
    const k = cellKey(Math.floor(p.x / CELL), Math.floor(p.y / CELL));
    const arr = this.cells.get(k);
    if (arr) arr.push(p);
    else this.cells.set(k, [p]);
  }

  /** Nearest snap point within `radius` (world mm) of (wx, wy) that passes the layer
   *  and object-class filters, or null. */
  query(
    wx: number,
    wy: number,
    radius: number,
    layerVisible: (idx: number) => boolean,
    objectOn: (kind: SnapKind) => boolean,
  ): SnapPoint | null {
    const rings = Math.min(Math.max(Math.ceil(radius / CELL), 1), MAX_RING);
    const cx = Math.floor(wx / CELL);
    const cy = Math.floor(wy / CELL);
    let best: SnapPoint | null = null;
    let bestD = radius * radius;
    for (let gx = cx - rings; gx <= cx + rings; gx++) {
      for (let gy = cy - rings; gy <= cy + rings; gy++) {
        const arr = this.cells.get(cellKey(gx, gy));
        if (!arr) continue;
        for (const p of arr) {
          if (!objectOn(p.kind)) continue;
          if (p.layers.length && !p.layers.some(layerVisible)) continue;
          const dx = p.x - wx;
          const dy = p.y - wy;
          const d = dx * dx + dy * dy;
          if (d < bestD) {
            bestD = d;
            best = p;
          }
        }
      }
    }
    return best;
  }
}

// ---- edge / on-segment projection ---------------------------------------

type Prim =
  | { t: "seg"; x1: number; y1: number; x2: number; y2: number; kind: SnapKind; layers: number[] }
  | { t: "arc"; cx: number; cy: number; r: number; a0: number; aMid: number; aEnd: number; kind: SnapKind; layers: number[]; radius?: number }
  | { t: "circle"; cx: number; cy: number; r: number; kind: SnapKind; layers: number[]; radius?: number };

/** Circumcircle of three points, or null when they are colinear. */
export function circleFrom3(
  sx: number, sy: number, mx: number, my: number, ex: number, ey: number,
): { cx: number; cy: number; r: number } | null {
  const d = 2 * (sx * (my - ey) + mx * (ey - sy) + ex * (sy - my));
  if (Math.abs(d) < 1e-9) return null;
  const s2 = sx * sx + sy * sy;
  const m2 = mx * mx + my * my;
  const e2 = ex * ex + ey * ey;
  const cx = (s2 * (my - ey) + m2 * (ey - sy) + e2 * (sy - my)) / d;
  const cy = (s2 * (ex - mx) + m2 * (sx - ex) + e2 * (mx - sx)) / d;
  return { cx, cy, r: Math.hypot(sx - cx, sy - cy) };
}

/** Is `theta` on the arc that sweeps from `a0` to `aEnd` passing through `aMid`?
 *  All angles in radians; the sweep direction is inferred from the mid angle. */
function arcContains(a0: number, aMid: number, aEnd: number, theta: number): boolean {
  const TAU = Math.PI * 2;
  const norm = (x: number) => {
    let v = (x - a0) % TAU;
    if (v < 0) v += TAU;
    return v;
  };
  const em = norm(aEnd);
  const mm = norm(aMid);
  const tm = norm(theta);
  // mid before end (in the CCW sense) ⇒ CCW sweep [0, em]; otherwise CW ⇒ [em, TAU].
  return mm <= em ? tm <= em : tm >= em;
}

/** Nearest point on segment (x1,y1)-(x2,y2) to (px,py). */
function projSeg(px: number, py: number, x1: number, y1: number, x2: number, y2: number): { x: number; y: number } {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const len2 = dx * dx + dy * dy;
  if (len2 < 1e-12) return { x: x1, y: y1 };
  let t = ((px - x1) * dx + (py - y1) * dy) / len2;
  t = t < 0 ? 0 : t > 1 ? 1 : t;
  return { x: x1 + t * dx, y: y1 + t * dy };
}

export class EdgeIndex {
  private prims: Prim[] = [];
  private cells = new Map<number, number[]>();

  private addToCells(idx: number, minx: number, miny: number, maxx: number, maxy: number): void {
    const gx0 = Math.floor(minx / CELL);
    const gy0 = Math.floor(miny / CELL);
    const gx1 = Math.floor(maxx / CELL);
    const gy1 = Math.floor(maxy / CELL);
    // Guard against a degenerate/huge primitive flooding the map.
    if ((gx1 - gx0 + 1) * (gy1 - gy0 + 1) > 4096) return;
    for (let gx = gx0; gx <= gx1; gx++) {
      for (let gy = gy0; gy <= gy1; gy++) {
        const k = cellKey(gx, gy);
        const arr = this.cells.get(k);
        if (arr) arr.push(idx);
        else this.cells.set(k, [idx]);
      }
    }
  }

  addSeg(x1: number, y1: number, x2: number, y2: number, kind: SnapKind, layers: number[]): void {
    const i = this.prims.push({ t: "seg", x1, y1, x2, y2, kind, layers }) - 1;
    this.addToCells(i, Math.min(x1, x2), Math.min(y1, y2), Math.max(x1, x2), Math.max(y1, y2));
  }

  addCircle(cx: number, cy: number, r: number, kind: SnapKind, layers: number[], radius?: number): void {
    if (r <= 0) return;
    const i = this.prims.push({ t: "circle", cx, cy, r, kind, layers, radius }) - 1;
    this.addToCells(i, cx - r, cy - r, cx + r, cy + r);
  }

  /** Arc from start/mid/end (the IR's `[sx,sy,mx,my,ex,ey]`). No-op if colinear. */
  addArc3(
    sx: number, sy: number, mx: number, my: number, ex: number, ey: number,
    kind: SnapKind, layers: number[], radius = true,
  ): void {
    const c = circleFrom3(sx, sy, mx, my, ex, ey);
    if (!c) return;
    const a0 = Math.atan2(sy - c.cy, sx - c.cx);
    const aMid = Math.atan2(my - c.cy, mx - c.cx);
    const aEnd = Math.atan2(ey - c.cy, ex - c.cx);
    const i = this.prims.push({ t: "arc", cx: c.cx, cy: c.cy, r: c.r, a0, aMid, aEnd, kind, layers, radius: radius ? c.r : undefined }) - 1;
    this.addToCells(i, c.cx - c.r, c.cy - c.r, c.cx + c.r, c.cy + c.r);
  }

  private project(prim: Prim, px: number, py: number): { x: number; y: number; radius?: number } | null {
    if (prim.t === "seg") return projSeg(px, py, prim.x1, prim.y1, prim.x2, prim.y2);
    // circle / arc: closest point on the circle, clamped to the arc span for arcs.
    const dx = px - prim.cx;
    const dy = py - prim.cy;
    const len = Math.hypot(dx, dy);
    if (len < 1e-9) return null; // dead centre: no meaningful nearest edge point
    if (prim.t === "arc" && !arcContains(prim.a0, prim.aMid, prim.aEnd, Math.atan2(dy, dx))) return null;
    return { x: prim.cx + (dx / len) * prim.r, y: prim.cy + (dy / len) * prim.r, radius: prim.radius };
  }

  /** Nearest point on any primitive within `radius` (world mm), passing the filters. */
  query(
    wx: number,
    wy: number,
    radius: number,
    layerVisible: (idx: number) => boolean,
    objectOn: (kind: SnapKind) => boolean,
  ): SnapHit | null {
    const rings = Math.min(Math.max(Math.ceil(radius / CELL), 1), MAX_RING);
    const cx = Math.floor(wx / CELL);
    const cy = Math.floor(wy / CELL);
    let best: SnapHit | null = null;
    let bestD = radius * radius;
    const seen = new Set<number>();
    for (let gx = cx - rings; gx <= cx + rings; gx++) {
      for (let gy = cy - rings; gy <= cy + rings; gy++) {
        const arr = this.cells.get(cellKey(gx, gy));
        if (!arr) continue;
        for (const pi of arr) {
          if (seen.has(pi)) continue;
          seen.add(pi);
          const prim = this.prims[pi];
          if (!objectOn(prim.kind)) continue;
          if (prim.layers.length && !prim.layers.some(layerVisible)) continue;
          const q = this.project(prim, wx, wy);
          if (!q) continue;
          const dx = q.x - wx;
          const dy = q.y - wy;
          const d = dx * dx + dy * dy;
          if (d < bestD) {
            bestD = d;
            best = { x: q.x, y: q.y, kind: prim.kind, edge: true, radius: q.radius };
          }
        }
      }
    }
    return best;
  }
}

// ---- corner geometry ----------------------------------------------------

/** The four world-space corners of a rectangular pad. Local→world uses the same
 *  convention as `PcbGlRenderer.hitTest`: x' = p.x + ca·lx + sa·ly, y' = p.y − sa·lx + ca·ly. */
export function padCorners(p: PcbPadDef): { x: number; y: number }[] {
  const hw = p.w / 2;
  const hh = p.h / 2;
  const a = (p.angle * Math.PI) / 180;
  const ca = Math.cos(a);
  const sa = Math.sin(a);
  const locals: [number, number][] = [
    [-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh],
  ];
  return locals.map(([lx, ly]) => ({ x: p.x + ca * lx + sa * ly, y: p.y - sa * lx + ca * ly }));
}

/** Shapes with straight rectangular sides (corners + edge segments are meaningful). */
function isRectangular(shape: number): boolean {
  return shape === PAD_SHAPE.rect || shape === PAD_SHAPE.roundrect || shape === PAD_SHAPE.trapezoid || shape === PAD_SHAPE.custom;
}

// ---- index builders -----------------------------------------------------

function addGraphicPoints(idx: SnapIndex, gr: PcbGraphicDef): void {
  const layers = [gr.layer];
  const d = gr.data;
  const mid = (ax: number, ay: number, bx: number, by: number) =>
    idx.add({ x: (ax + bx) / 2, y: (ay + by) / 2, kind: "graphic", layers });
  if (gr.kind === "seg") {
    idx.add({ x: d[0], y: d[1], kind: "graphic", layers });
    idx.add({ x: d[2], y: d[3], kind: "graphic", layers });
    mid(d[0], d[1], d[2], d[3]); // centre of a line (e.g. a courtyard edge)
  } else if (gr.kind === "arc") {
    // [sx, sy, mx, my, ex, ey] — endpoints + the arc's own mid point, plus the centre (R/Ø).
    idx.add({ x: d[0], y: d[1], kind: "graphic", layers });
    idx.add({ x: d[2], y: d[3], kind: "graphic", layers });
    idx.add({ x: d[4], y: d[5], kind: "graphic", layers });
    const c = circleFrom3(d[0], d[1], d[2], d[3], d[4], d[5]);
    if (c) idx.add({ x: c.cx, y: c.cy, kind: "graphic", layers, radius: c.r });
  } else if (gr.kind === "circle") {
    // [cx, cy, r] — the centre carries the radius for the R/Ø readout.
    idx.add({ x: d[0], y: d[1], kind: "graphic", layers, radius: d[2] });
  } else if (gr.kind === "poly") {
    const n = d.length;
    for (let i = 0; i + 1 < n; i += 2) {
      idx.add({ x: d[i], y: d[i + 1], kind: "graphic", layers });
      // edge midpoint to the next vertex (closing edge wraps to vertex 0).
      const j = i + 2 < n ? i + 2 : 0;
      mid(d[i], d[i + 1], d[j], d[j + 1]);
    }
  }
}

function addGraphicEdges(idx: EdgeIndex, gr: PcbGraphicDef): void {
  const layers = [gr.layer];
  const d = gr.data;
  if (gr.kind === "seg") {
    idx.addSeg(d[0], d[1], d[2], d[3], "graphic", layers);
  } else if (gr.kind === "arc") {
    idx.addArc3(d[0], d[1], d[2], d[3], d[4], d[5], "graphic", layers);
  } else if (gr.kind === "circle") {
    idx.addCircle(d[0], d[1], d[2], "graphic", layers, d[2]);
  } else if (gr.kind === "poly") {
    const n = d.length;
    for (let i = 0; i + 1 < n; i += 2) {
      const j = i + 2 < n ? i + 2 : 0;
      idx.addSeg(d[i], d[i + 1], d[j], d[j + 1], "graphic", layers);
    }
  }
}

/** Build the discrete-point snap index from the loaded geometry IR (§5 sources). */
export function buildSnapIndex(g: PcbGeometry): SnapIndex {
  const idx = new SnapIndex();

  // Pad centres + corners (drilled pads: hole centre = pad centre).
  for (const p of g.pads) {
    idx.add({ x: p.x, y: p.y, kind: "pad", layers: p.layers });
    if (isRectangular(p.shape)) {
      for (const c of padCorners(p)) idx.add({ x: c.x, y: c.y, kind: "pad", layers: p.layers });
    }
  }

  // Via / hole centres.
  for (const v of g.vias) idx.add({ x: v.x, y: v.y, kind: "via", layers: v.layers });

  // Straight track endpoints + midpoint ([x1,y1,x2,y2] per segment).
  const seg = g.tracks.seg;
  for (let i = 0; i < seg.w.length; i++) {
    const layers = [seg.layer[i]];
    const x1 = seg.xy[i * 4], y1 = seg.xy[i * 4 + 1], x2 = seg.xy[i * 4 + 2], y2 = seg.xy[i * 4 + 3];
    idx.add({ x: x1, y: y1, kind: "track", layers });
    idx.add({ x: x2, y: y2, kind: "track", layers });
    idx.add({ x: (x1 + x2) / 2, y: (y1 + y2) / 2, kind: "track", layers });
  }

  // Arc track endpoints + mid + centre ([sx,sy,mx,my,ex,ey] per arc).
  const arc = g.tracks.arc;
  for (let i = 0; i < arc.w.length; i++) {
    const layers = [arc.layer[i]];
    const sx = arc.xy[i * 6], sy = arc.xy[i * 6 + 1];
    const mx = arc.xy[i * 6 + 2], my = arc.xy[i * 6 + 3];
    const ex = arc.xy[i * 6 + 4], ey = arc.xy[i * 6 + 5];
    idx.add({ x: sx, y: sy, kind: "arc", layers });
    idx.add({ x: ex, y: ey, kind: "arc", layers });
    const c = circleFrom3(sx, sy, mx, my, ex, ey);
    if (c) idx.add({ x: c.cx, y: c.cy, kind: "arc", layers, radius: c.r });
  }

  // Board outline + graphics endpoints/vertices/midpoints (Edge.Cuts, silk, courtyards).
  for (const gr of g.graphics) addGraphicPoints(idx, gr);

  // Footprint origins.
  for (const c of g.components) {
    idx.add({ x: c.x, y: c.y, kind: "component", layers: c.layer >= 0 ? [c.layer] : [] });
  }

  return idx;
}

/** Build the edge/outline projection index (feature 13 + center/edge helpers). */
export function buildEdgeIndex(g: PcbGeometry): EdgeIndex {
  const idx = new EdgeIndex();

  // Track segments: the centreline PLUS the two copper edges (centreline offset by
  // ±½·width), so the cursor can snap to a track's centre OR either edge — measuring
  // clearances/widths needs the copper boundary, not just the routed midline (batch2).
  const seg = g.tracks.seg;
  for (let i = 0; i < seg.w.length; i++) {
    const x1 = seg.xy[i * 4], y1 = seg.xy[i * 4 + 1], x2 = seg.xy[i * 4 + 2], y2 = seg.xy[i * 4 + 3];
    const layers = [seg.layer[i]];
    idx.addSeg(x1, y1, x2, y2, "track", layers);
    const hw = seg.w[i] / 2;
    const dx = x2 - x1, dy = y2 - y1, len = Math.hypot(dx, dy);
    if (hw > 0 && len > 1e-9) {
      const px = (-dy / len) * hw, py = (dx / len) * hw; // perpendicular offset
      idx.addSeg(x1 + px, y1 + py, x2 + px, y2 + py, "track", layers);
      idx.addSeg(x1 - px, y1 - py, x2 - px, y2 - py, "track", layers);
    }
  }
  // Track arcs: centreline + the two concentric copper edges at r ± ½·width.
  const arc = g.tracks.arc;
  for (let i = 0; i < arc.w.length; i++) {
    const sx = arc.xy[i * 6], sy = arc.xy[i * 6 + 1];
    const mx = arc.xy[i * 6 + 2], my = arc.xy[i * 6 + 3];
    const ex = arc.xy[i * 6 + 4], ey = arc.xy[i * 6 + 5];
    const layers = [arc.layer[i]];
    idx.addArc3(sx, sy, mx, my, ex, ey, "arc", layers);
    const hw = arc.w[i] / 2;
    const c = hw > 0 ? circleFrom3(sx, sy, mx, my, ex, ey) : null;
    if (c) {
      const off = (x: number, y: number, delta: number): [number, number] => {
        const ox = x - c.cx, oy = y - c.cy, l = Math.hypot(ox, oy) || 1;
        return [x + (ox / l) * delta, y + (oy / l) * delta];
      };
      for (const delta of [hw, -hw]) {
        if (c.r + delta <= 0) continue;
        const [osx, osy] = off(sx, sy, delta), [omx, omy] = off(mx, my, delta), [oex, oey] = off(ex, ey, delta);
        idx.addArc3(osx, osy, omx, omy, oex, oey, "arc", layers, false); // edge, no R/Ø readout
      }
    }
  }

  // Board / footprint graphics (outline, courtyards, silk).
  for (const gr of g.graphics) addGraphicEdges(idx, gr);

  // Pad outlines: rectangular pads → four edges (edge-to-edge); round pads → a circle.
  for (const p of g.pads) {
    if (isRectangular(p.shape)) {
      const c = padCorners(p);
      for (let i = 0; i < 4; i++) {
        const j = (i + 1) % 4;
        idx.addSeg(c[i].x, c[i].y, c[j].x, c[j].y, "pad", p.layers);
      }
    } else if (p.shape === PAD_SHAPE.circle) {
      idx.addCircle(p.x, p.y, p.w / 2, "pad", p.layers);
    }
  }

  // Via outlines (edge-to-edge clearance).
  for (const v of g.vias) idx.addCircle(v.x, v.y, v.size / 2, "via", v.layers);

  return idx;
}
