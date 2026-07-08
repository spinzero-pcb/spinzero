// Measure-tool overlay drawing (docs/measure-tool-plan.md §7.6): the KiCad-style ruler
// (common/preview_items/ruler_item.cpp) rendered on the Canvas2D overlay in screen space.
// Pure module — no React, no WebGL, no stores — so it unit-tests under jsdom
// (measureRuler.test.ts) and renders in a plain <canvas> harness.

import type { MeasureUnit } from "../../stores/measureStore";
import type { Camera } from "./glRenderer";

/** A measurement point in world mm, plus whether it locked onto real geometry (drawn
 *  with a square lock marker) vs a free / constrained / grid point (a small crosshair).
 *  `edge` marks an on-line/on-outline projection (drawn as a circle); `radius` is set
 *  when the point is the centre of / a point on an arc or circle (drives the R/Ø line). */
export interface MPoint {
  x: number;
  y: number;
  snapped: boolean;
  edge?: boolean;
  radius?: number;
}

/** Colours + font for the overlay; sourced from the --measure-* CSS tokens. */
export interface MeasureStyle {
  line: string;
  shadow: string;
  text: string;
  /** Font family for the numeric readouts — monospace, like KiCad's stroke font. */
  font: string;
}

/** Per-unit display: multiplier from mm and the decimal precision for the readout. */
const UNIT_FMT: Record<MeasureUnit, { mul: number; prec: number }> = {
  mm: { mul: 1, prec: 3 },
  mil: { mul: 1000 / 25.4, prec: 1 },
  in: { mul: 1 / 25.4, prec: 4 },
};

const fmtLen = (mm: number, u: MeasureUnit): string => {
  const { mul, prec } = UNIT_FMT[u];
  return (mm * mul).toFixed(prec);
};

// KiCad ruler graduation (common/preview_items/ruler_item.cpp). Ticks are spaced on a
// 1/2/5-per-decade scale so majors land on round values; the format also decides how many
// minor ticks fall between mid/major ticks.
const TICK_MIN_PX = 10; // maxTickDensity — min screen px between minor ticks
const TICK_MINOR_PX = 5; // minorTickLen = 5 / worldScale
const MID_FACTOR = 1.5; // midTickLengthFactor
const MAJOR_FACTOR = 2.5; // majorTickLengthFactor
/** {divisionBase, majorStep, midStep} — cycled to grow the spacing 1→2→5→10… */
const TICK_FORMATS = [
  { div: 2, major: 10, mid: 5 }, // |....:....|
  { div: 2, major: 5, mid: 0 }, //  |....|
  { div: 2.5, major: 2, mid: 0 }, // |.|.|
];

// Text sizes (KiCad draws the readout + tick labels in its stroke font at a very
// readable size — see feedback/35.png; 12px unrotated labels were too small to read).
// Bumped again (batch2): the readout/tick labels were still hard to read at a glance.
const TICK_FONT_PX = 15;
const READOUT_FONT_PX = 18;
const READOUT_LINE_H = 23;

/** Pick the graduation spacing (mm) and major/mid steps for a given px-per-mm scale,
 *  mirroring KiCad's `getTickFormatForScale`. KiCad grows the spacing on the 1/2/5
 *  cycle from ONE INTERNAL UNIT (a nanometre; ×2.54 for imperial so ticks land on round
 *  mil/inch values) until minor ticks are ≥10 px apart — so at high zoom the spacing
 *  subdivides well below 1 mm (e.g. 0.02 mm ticks labelled every 0.100, feedback/35.png).
 *  An earlier port started at 1 mm and never subdivided, which is why short measurements
 *  showed a bare ruler with no values. */
export function tickFormatForScale(scale: number, imperial: boolean): { spaceMm: number; major: number; mid: number } {
  let spaceMm = imperial ? 2.54e-6 : 1e-6;
  let fmt = 0;
  for (let k = 0; k < 100 && spaceMm * scale < TICK_MIN_PX; k++) {
    fmt = (fmt + 1) % TICK_FORMATS.length;
    spaceMm *= TICK_FORMATS[fmt].div;
  }
  return { spaceMm, major: TICK_FORMATS[fmt].major, mid: TICK_FORMATS[fmt].mid };
}

/** White text over a dark outline — KiCad's two-pass shadow, but crisp. The caller is
 *  responsible for font/align; this only swaps stroke width + colour around the text. */
function paintText(ctx: CanvasRenderingContext2D, s: string, x: number, y: number, shadow: string) {
  const lw = ctx.lineWidth;
  const ss = ctx.strokeStyle;
  ctx.lineWidth = 3;
  ctx.strokeStyle = shadow;
  ctx.lineJoin = "round";
  ctx.strokeText(s, x, y);
  ctx.fillText(s, x, y);
  ctx.lineWidth = lw;
  ctx.strokeStyle = ss;
}

/** Draw KiCad's graduated ruler along A→end: perpendicular minor / mid / major tick marks
 *  (majors + mids labelled with the round distance value, rotated to run along the tick
 *  like KiCad's), plus the backside 2-division ticks and the origin back-crosshair.
 *  Screen space; `ax..ey` in CSS px, `distMm` world length, `scale` px per mm. The caller
 *  has set stroke/fill + the black drop-shadow. */
function drawGraduations(
  ctx: CanvasRenderingContext2D,
  ax: number, ay: number, ex: number, ey: number,
  distMm: number, scale: number,
  units: MeasureUnit,
  style: MeasureStyle,
) {
  const dxs = ex - ax;
  const dys = ey - ay;
  const lenPx = Math.hypot(dxs, dys);
  if (lenPx < 8 || distMm <= 0) return; // too short to graduate usefully
  const ux = dxs / lenPx;
  const uy = dys / lenPx;
  const nx = -uy; // unit perpendicular — front ticks + labels hang off this side
  const ny = ux;
  const { mul, prec } = UNIT_FMT[units];
  const { spaceMm, major, mid } = tickFormatForScale(scale, units !== "mm");
  const stepPx = spaceMm * scale;
  if (stepPx < 4) return;
  const nTicks = Math.floor(distMm / spaceMm + 1e-6);
  if (nTicks > 2000) return; // safety at extreme partial-offscreen zoom
  const minorLen = TICK_MINOR_PX;
  const midLen = TICK_MINOR_PX * MID_FACTOR;
  const majorLen = TICK_MINOR_PX * MAJOR_FACTOR;

  // Labels run along the tick direction (perpendicular to the ruler), KiCad-style —
  // flipped when that would put them upside-down, so they always read top-to-bottom.
  const tickAngle = Math.atan2(ny, nx);
  const flip = tickAngle > Math.PI / 2 || tickAngle < -Math.PI / 2;

  ctx.strokeStyle = style.line;
  ctx.fillStyle = style.text;
  ctx.lineWidth = 1;
  ctx.font = `bold ${TICK_FONT_PX}px ${style.font}`;
  ctx.textBaseline = "middle";
  // Front ticks + labels.
  for (let i = 0; i <= nTicks; i++) {
    const d = i * stepPx;
    const bx = ax + ux * d;
    const by = ay + uy * d;
    const isMajor = i % major === 0;
    const isMid = mid > 0 && i % mid === 0;
    const len = isMajor ? majorLen : isMid ? midLen : minorLen;
    ctx.beginPath();
    ctx.moveTo(bx, by);
    ctx.lineTo(bx + nx * len, by + ny * len);
    ctx.stroke();
    if (isMajor || isMid) {
      ctx.save();
      ctx.translate(bx + nx * (majorLen + 4), by + ny * (majorLen + 4));
      ctx.rotate(flip ? tickAngle + Math.PI : tickAngle);
      ctx.textAlign = flip ? "right" : "left";
      paintText(ctx, (i * spaceMm * mul).toFixed(prec), 0, 0, style.shadow);
      ctx.restore();
    }
  }
  // Backside ticks: divide the ruler into 2, ticks on the opposite side (KiCad default).
  for (let i = 0; i <= 2; i++) {
    const d = (lenPx * i) / 2;
    const bx = ax + ux * d;
    const by = ay + uy * d;
    ctx.beginPath();
    ctx.moveTo(bx, by);
    ctx.lineTo(bx - nx * majorLen, by - ny * majorLen);
    ctx.stroke();
  }
  // Origin back-crosshair (showEndArrowHead=false): a short stub behind the start point.
  ctx.beginPath();
  ctx.moveTo(ax, ay);
  ctx.lineTo(ax - ux * midLen, ay - uy * midLen);
  ctx.stroke();
}

/** Constrain the point (px,py) onto the nearest 45° ray from the origin (ox,oy),
 *  preserving its distance — the Ctrl/orthogonal constraint (feature 10). */
export function constrain45(ox: number, oy: number, px: number, py: number): { x: number; y: number } {
  const dx = px - ox;
  const dy = py - oy;
  const len = Math.hypot(dx, dy);
  if (len < 1e-9) return { x: px, y: py };
  const step = Math.PI / 4;
  const ang = Math.round(Math.atan2(dy, dx) / step) * step;
  return { x: ox + Math.cos(ang) * len, y: oy + Math.sin(ang) * len };
}

/** The x/y/r/θ readout strings KiCad shows next to the cursor (GetDimensionStrings),
 *  plus the R/Ø of an arc/circle either endpoint sits on (our extension, feature 12). */
function dimensionStrings(A: MPoint, end: MPoint, units: MeasureUnit): string[] {
  const dx = end.x - A.x;
  const dy = end.y - A.y;
  const dist = Math.hypot(dx, dy);
  // KiCad reports angle CCW-positive in its Y-up display convention (board Y is down),
  // so negate dy: rightward = 0°, upward = +90°. atan2(−0, −x) yields −180°; KiCad
  // shows that as +180.0°.
  let angle = (Math.atan2(-dy, dx) * 180) / Math.PI;
  if (angle < -179.95) angle += 360; // would print "-180.0" — show it as +180.0

  const lines = [
    `x: ${fmtLen(dx, units)} ${units}`,
    `y: ${fmtLen(dy, units)} ${units}`,
    `r: ${fmtLen(dist, units)} ${units}`,
    `θ: ${angle.toFixed(1)}°`,
  ];
  const rp = end.radius != null ? end : A.radius != null ? A : null;
  if (rp?.radius != null) {
    lines.push(`R: ${fmtLen(rp.radius, units)} ${units}`, `Ø: ${fmtLen(rp.radius * 2, units)} ${units}`);
  }
  return lines;
}

/** Draw the ruler + readout on the Canvas2D overlay in screen space, matching KiCad's
 *  ruler (white line + graduations, black drop-shadow, x/y/r/θ text next to the cursor;
 *  common/preview_items/ruler_item.cpp). `A`→`end`; `hover` drives the live snap marker. */
export function drawMeasure(
  ctx: CanvasRenderingContext2D,
  A: MPoint | null,
  end: MPoint | null,
  hover: MPoint | null,
  units: MeasureUnit,
  cam: Camera,
  cssW: number,
  cssH: number,
  style: MeasureStyle,
) {
  const scale = cam.scale;
  const sx = (x: number) => (x - cam.x) * scale + cssW / 2;
  const sy = (y: number) => (y - cam.y) * scale + cssH / 2;

  // Small snap indicator at the live cursor point (KiCad shows a magnetic-point marker
  // when snapping engages; the shape also tells you the snap kind). Only at the moving
  // point — placed endpoints are marked by the ruler geometry itself.
  const snapMarker = (p: MPoint) => {
    const mx = sx(p.x);
    const my = sy(p.y);
    ctx.beginPath();
    if (p.edge) ctx.arc(mx, my, 4, 0, Math.PI * 2); // on-edge projection
    else if (p.snapped) ctx.rect(mx - 4, my - 4, 8, 8); // vertex / centre lock
    else {
      ctx.moveTo(mx - 5, my);
      ctx.lineTo(mx + 5, my);
      ctx.moveTo(mx, my - 5);
      ctx.lineTo(mx, my + 5);
    }
    ctx.stroke();
  };

  // KiCad's cursor text: white over a dark outline, left/right-aligned in the quadrant
  // away from the origin, stacked top-to-bottom, offset ~15px from the cursor. Clamped
  // so it stays on-screen.
  const cursorText = (px: number, py: number, lines: string[], quadX: number, quadY: number) => {
    const lh = READOUT_LINE_H;
    const left = quadX < 0;
    ctx.font = `bold ${READOUT_FONT_PX}px ${style.font}`;
    ctx.textAlign = left ? "left" : "right";
    ctx.textBaseline = "alphabetic";
    let tx = px + (left ? 15 : -15);
    // clamp horizontally by measuring the widest line
    let tw = 0;
    for (const l of lines) tw = Math.max(tw, ctx.measureText(l).width);
    if (left && tx + tw > cssW - 4) tx = cssW - 4 - tw;
    if (!left && tx - tw < 4) tx = 4 + tw;
    let ty = quadY > 0 ? py - lh * (lines.length + 1) : py + lh;
    if (ty + lh * lines.length > cssH - 4) ty = cssH - 4 - lh * lines.length;
    if (ty < lh) ty = lh;
    for (const l of lines) {
      paintText(ctx, l, tx, ty, style.shadow);
      ty += lh;
    }
  };

  ctx.save();
  ctx.strokeStyle = style.line;
  ctx.fillStyle = style.text;
  ctx.lineWidth = 1.4;
  ctx.font = `bold ${READOUT_FONT_PX}px ${style.font}`;
  // The black drop-shadow that makes the white ruler read over any copper (KiCad draws the
  // whole item twice — once thick in the shadow colour; a blurred shadow approximates it
  // for the line/ticks; text gets a crisp strokeText outline in paintText instead).
  ctx.shadowColor = style.shadow;
  ctx.shadowBlur = 2;

  // Before the first point: just the snap marker (+ R/Ø if hovering an arc/circle).
  if (!A) {
    if (hover) {
      snapMarker(hover);
      if (hover.radius != null) {
        cursorText(sx(hover.x), sy(hover.y),
          [`R: ${fmtLen(hover.radius, units)} ${units}`, `Ø: ${fmtLen(hover.radius * 2, units)} ${units}`], -1, -1);
      }
    }
    ctx.restore();
    return;
  }
  const ax = sx(A.x);
  const ay = sy(A.y);
  if (!end) {
    snapMarker(A);
    ctx.restore();
    return;
  }
  const ex = sx(end.x);
  const ey = sy(end.y);
  const distMm = Math.hypot(end.x - A.x, end.y - A.y);

  // Main line + graduations.
  ctx.beginPath();
  ctx.moveTo(ax, ay);
  ctx.lineTo(ex, ey);
  ctx.stroke();
  drawGraduations(ctx, ax, ay, ex, ey, distMm, scale, units, style);

  // x/y/r/θ text next to the cursor, in the quadrant away from the origin (KiCad's rule).
  const sdx = ex - ax;
  const sdy = ey - ay;
  let quadX = sdy < 0 ? -1 : 1;
  let quadY = sdx < 0 ? 1 : -1;
  // For near-horizontal / near-vertical rulers the KiCad quadrant lands the readout on
  // the SAME side as the tick labels (which hang toward the (−uy, ux) perpendicular),
  // so the two stack and overlap. Push the readout to the opposite side of the ruler.
  const rlen = Math.hypot(sdx, sdy) || 1;
  const rux = sdx / rlen;
  const ruy = sdy / rlen;
  const nx = -ruy; // tick-label side (screen x)
  const ny = rux; //  ″            (screen y)
  if (Math.abs(ruy) < 0.26) quadY = ny > 0 ? 1 : -1; // horizontal → opposite vertically
  if (Math.abs(rux) < 0.26) quadX = nx > 0 ? 1 : -1; // vertical → opposite horizontally
  cursorText(ex, ey, dimensionStrings(A, end, units), quadX, quadY);

  // Live snap marker at the moving end (matches the ongoing preview to the cursor).
  if (end === hover) snapMarker(end);
  ctx.restore();
}

/** Plain-text form of the current measurement's readout, for copy-to-clipboard
 *  (feature 19). Returns null when there is no completed measurement. */
export function measureReadoutText(A: MPoint | null, B: MPoint | null, units: MeasureUnit): string | null {
  if (!A || !B) return null;
  return dimensionStrings(A, B, units).join("   ");
}
