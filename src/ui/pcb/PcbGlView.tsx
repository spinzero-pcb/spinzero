import { useEffect, useRef, useState } from "react";
import { useDesignStore } from "../../stores/designStore";
import {
  COMP_COLOR,
  NET_COLOR,
  useSelectionStore,
  type Highlight,
} from "../../stores/selectionStore";
import { usePcbViewStore } from "../../stores/pcbViewStore";
import { useReviewStore } from "../../stores/reviewStore";
import { useMeasureStore } from "../../stores/measureStore";
import { useViewStore } from "../../stores/viewStore";
import { measureNav, nav, pcbNav, type ChipComment } from "../canvas/navigator";
import { ContextMenu, type MenuItem } from "../ContextMenu";
import { IconClose, IconComment, IconCopy, IconFit, IconRuler, IconSheet, IconTrash } from "../icons";
import type { CommentAnchor } from "../../lib/types";
import type { PcbGeometry, PcbTextDef } from "../../lib/pcbGeometry";
import { DIFF_HATCH_PERIOD_CSS, PcbGlRenderer, netLabelRows, type BBox, type Camera, type DiffFlags, type ObjectState } from "./glRenderer";
import { buildDiffVisibility, changeCode, changeExtent, computeDiffFlags, unionExtent } from "./glDiff";
import type { Change } from "../../lib/diff";
import { isTypingTarget } from "../../lib/keymap";
import { useDiffStore } from "../../stores/diffStore";
import { resolveCssColor, PCB_DIFF_BASE_FALLBACK } from "./glColor";
import { registerRenderProbe } from "../../lib/renderProbe";
import { parseMarkup, type MarkupRun } from "./textMarkup";
import { knockoutMargin, layoutStrokeText, traceStrokeText } from "./strokeFont";
import { buildEdgeIndex, buildSnapIndex, type EdgeIndex, type SnapIndex, type SnapKind } from "./measureSnap";
import { constrain45, drawMeasure, measureReadoutText, type MPoint } from "./measureOverlay";
import { PcbToolbar } from "./PcbToolbar";

// GPU PCB view: renders the geometry IR with WebGL (see glRenderer.ts), the fast
// replacement for the SVG-in-DOM islands. Drives the same stores as the SVG PcbView
// (pcbViewStore for layers/objects, selectionStore for highlight + cross-probe) so the
// Appearance panel and properties card work unchanged. Used when the bundle carries a
// geometry IR; the SVG PcbView remains the fallback.
//
// Ported: pan/zoom/fit, layer visibility + active emphasis, object class toggles +
// opacity, click→select (pad→pin, track/via/zone→net, courtyard→component) with
// cross-probe, highlight reflection, schematic→PCB camera landing, net-name labels,
// board/footprint text, object-anchored comment chips, and the right-click menu.

/** Camera scale clamp (px/mm). Min keeps a whole large board on-screen; max sets how
 *  far you can zoom into fine features (200 µm traces, drill hits) — 2000 px/mm ≈ 50 px
 *  per 25 µm, comfortably past what the geometry resolves. Both zoom paths (wheel +
 *  toolbar/keyboard) clamp to this range. */
const MIN_SCALE = 0.05;
const MAX_SCALE = 2000;

/** Diff overlay encoding: colour = LAYER, texture = old vs new. Changed copper keeps
 *  its TRUE layer colour (mix 1.0) so "which layer changed" reads instantly over the
 *  grey base; removed (A) copper is crosshatched, added (B) is solid (glRenderer
 *  diffHatch). Red/green tints were tried and dropped — per-layer tint blends were
 *  illegible; 45° single-direction stripes likewise (parallel lines on diagonal
 *  tracks); a checkerboard likewise (blotchy when zoomed out). */
const DIFF_LAYER_MIX = 1.0;
/** Added-copper alpha: slightly sheer so the removed crosshatch painted on top of a
 *  same-spot restyle (thickened track) stands out against it. */
const DIFF_ADDED_ALPHA = 0.85;


/** Prepare the reusable scratch canvas for the removed-text pass: sized to the view,
 *  cleared, with the same css-px transform as the overlay context. Returns null when
 *  a 2D context is unavailable (the pass is skipped — never crash the frame). */
function beginHatchScratch(
  ref: { current: HTMLCanvasElement | null },
  cssW: number,
  cssH: number,
  dpr: number,
): CanvasRenderingContext2D | null {
  ref.current ??= document.createElement("canvas");
  const cnv = ref.current;
  const W = Math.round(cssW * dpr);
  const H = Math.round(cssH * dpr);
  if (cnv.width !== W || cnv.height !== H) {
    cnv.width = W;
    cnv.height = H;
  }
  const sctx = cnv.getContext("2d");
  if (!sctx) return null;
  sctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  sctx.clearRect(0, 0, cssW, cssH);
  return sctx;
}

/** Punch the GL shader's ±45° crosshatch through everything drawn on the scratch
 *  canvas (destination-out, cuts keep a 0.25 ghost like the copper texture), then
 *  composite it onto the overlay. Both diagonal directions, screen-space period. */
function compositeHatched(ctx: CanvasRenderingContext2D, sctx: CanvasRenderingContext2D, dpr: number) {
  const { width: W, height: H } = sctx.canvas; // device px
  sctx.save();
  sctx.setTransform(1, 0, 0, 1, 0, 0);
  sctx.globalCompositeOperation = "destination-out";
  sctx.strokeStyle = "rgba(0,0,0,0.75)"; // cut to a faint ghost, not fully out
  const period = DIFF_HATCH_PERIOD_CSS * dpr; // spacing along an axis, like gl_FragCoord.x+y
  sctx.lineWidth = 0.22 * period * 0.7071; // the shader's band width, made perpendicular
  sctx.beginPath();
  for (let o = -H; o < W; o += period) {
    sctx.moveTo(o, 0);
    sctx.lineTo(o + H, H); // "\" set: x − y = o
  }
  for (let o = 0; o < W + H; o += period) {
    sctx.moveTo(o, 0);
    sctx.lineTo(o - H, H); // "/" set: x + y = o
  }
  sctx.stroke();
  sctx.restore();
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.drawImage(sctx.canvas, 0, 0);
  ctx.restore();
}

/** Draw one net-label row: the plain `text` centred at (0, cy) plus a KiCad-style
 *  overline over each `~{…}` run (feedback: nets like `~{project_rst}`). ctx.font /
 *  fillStyle / textAlign="center" / textBaseline="middle" are already set; `size` is
 *  the row's px font size, used to place and weight the line. */
function drawLabelRow(
  ctx: CanvasRenderingContext2D,
  runs: readonly MarkupRun[],
  text: string,
  cy: number,
  size: number,
) {
  ctx.fillText(text, 0, cy);
  if (!runs.some((r) => r.over)) return;
  // Centred text spans [-total/2, +total/2]; walk the runs and bar the `~{…}` spans.
  // Newstroke advances are additive (no kerning), so run widths sum to the total.
  let x = -ctx.measureText(text).width / 2;
  const y = cy - size * 0.65; // just above the caps at a middle baseline
  ctx.save();
  ctx.lineWidth = Math.max(size * 0.08, 1);
  ctx.strokeStyle = ctx.fillStyle;
  for (const run of runs) {
    const w = ctx.measureText(run.text).width;
    if (run.over && w > 0) {
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.lineTo(x + w, y);
      ctx.stroke();
    }
    x += w;
  }
  ctx.restore();
}

/** KiCad scales outline (TTF) fonts by 1.4 relative to the nominal `(size h)`: that
 *  `size` is the stroke font's CAP-height, but an outline font's `size` would set its
 *  FULL height (ascenders + descenders included), leaving it visibly smaller than the
 *  stroke font at the same value. KiCad compensates with this exact factor
 *  (`OUTLINE_FONT::m_outlineFontSizeCompensation` in outline_font.h); a Canvas font `px`
 *  is the em, so it needs the same multiplier to match KiCad's rendered size. */
const OUTLINE_FONT_SIZE_COMP = 1.4;

/** Draw a text item that overrides KiCad's stroke font with a real outline font (e.g.
 *  Calibri): KiCad plots such text with the actual TTF, so we FILL glyphs in the named
 *  family rather than stroking Newstroke. Metrics are the family's own — approximate vs
 *  KiCad, but the point is that the authored typeface shows. Handles justification,
 *  rotation, mirror, condensed/expanded width, bold/italic, multi-line and knockout.
 *  `sx,sy` = screen-px anchor, `a` = screen-CW angle (deg), `scale` = CSS px per mm. */
function drawOutlineText(
  ctx: CanvasRenderingContext2D,
  t: PcbTextDef,
  sx: number,
  sy: number,
  a: number,
  scale: number,
  color: string,
  fallback: string,
) {
  const face = t.font?.trim();
  if (!face || !t.text) return;
  // Quote a family with spaces; drop any embedded quotes so the CSS font shorthand is safe.
  // Fall back to the KiCad stroke-font stack so a font the machine doesn't have degrades to
  // KiCad's own look, not the webview's default serif.
  const family = `${/[\s"]/.test(face) ? `"${face.replace(/"/g, "")}"` : face}, ${fallback}`;
  const px = Math.max(t.size * scale * OUTLINE_FONT_SIZE_COMP, 1);
  const wRatio = t.width && t.size ? t.width / t.size : 1; // condensed/expanded text
  const lines = t.text.split("\n");
  const n = lines.length;
  const lineH = px * 1.16; // interline proportional to the em (≈ the stroke-font spacing)
  const vj = t.justify[1];
  // Stack lines around the anchor per the vertical justify (top→down, bottom→up, else centred).
  const lineY = (i: number) =>
    vj < 0 ? i * lineH : vj > 0 ? -(n - 1 - i) * lineH : (i - (n - 1) / 2) * lineH;

  ctx.save();
  ctx.translate(sx, sy);
  if (a) ctx.rotate((-a * Math.PI) / 180);
  if (t.mirror) ctx.scale(-1, 1);
  if (wRatio !== 1) ctx.scale(wRatio, 1);
  ctx.font = `${t.italic ? "italic " : ""}${t.bold ? "bold " : ""}${px}px ${family}`;
  ctx.textAlign = t.justify[0] < 0 ? "left" : t.justify[0] > 0 ? "right" : "center";
  ctx.textBaseline = vj < 0 ? "top" : vj > 0 ? "bottom" : "middle";

  if (t.knockout) {
    // KiCad inverted silk: fill a background box in the layer colour and punch the glyphs
    // out. Box from the measured block (approximate — the family's own metrics).
    let maxW = 0;
    for (const ln of lines) maxW = Math.max(maxW, ctx.measureText(ln).width);
    const ha = ctx.textAlign;
    const bx = ha === "left" ? 0 : ha === "right" ? -maxW : -maxW / 2;
    const top = lineY(0), bot = lineY(n - 1);
    const yTop = vj < 0 ? top : vj > 0 ? top - px : top - px / 2;
    const yBot = vj < 0 ? bot + px : vj > 0 ? bot : bot + px / 2;
    const m = px * 0.15;
    ctx.fillStyle = color;
    ctx.fillRect(bx - m, yTop - m, maxW + 2 * m, yBot - yTop + 2 * m);
    ctx.globalCompositeOperation = "destination-out";
    ctx.fillStyle = "#000"; // any opaque colour — only alpha matters when punching out
    for (let i = 0; i < n; i++) ctx.fillText(lines[i], 0, lineY(i));
    ctx.globalCompositeOperation = "source-over";
  } else {
    ctx.fillStyle = color;
    for (let i = 0; i < n; i++) ctx.fillText(lines[i], 0, lineY(i));
  }
  ctx.restore();
}

/** KiCad drawing sheet: the page border, the row/column reference band, and the
 *  bottom-right title block. Page context, drawn on the Canvas2D overlay regardless of the
 *  active layer (feedback 31.PNG) — the frame reads even with no layer selected. World mm;
 *  the page spans (0,0)..(pw,ph) and the camera maps it to overlay CSS px like the GL view. */
function drawWorksheet(
  r: PcbGlRenderer,
  ctx: CanvasRenderingContext2D,
  camera: Camera,
  cssW: number,
  cssH: number,
  strokeFont: string,
  color: string,
) {
  const page = r.page;
  if (!page) return;
  const [pw, ph] = page;
  const scale = camera.scale; // CSS px per mm
  const sx = (x: number) => (x - camera.x) * scale + cssW / 2;
  const sy = (y: number) => (y - camera.y) * scale + cssH / 2;
  // Cull when the whole page is off-screen.
  if (sx(pw) < 0 || sx(0) > cssW || sy(ph) < 0 || sy(0) > cssH) return;

  ctx.save();
  ctx.strokeStyle = color;
  ctx.fillStyle = color;
  ctx.lineWidth = Math.max(0.15 * scale, 0.5); // ~0.15 mm, min one thin px

  const M_OUT = 10; // page border (mm)
  const M_IN = 12; // content border; the 2 mm band between holds the ref markers
  const rect = (x0: number, y0: number, x1: number, y1: number) =>
    ctx.strokeRect(sx(x0), sy(y0), (x1 - x0) * scale, (y1 - y0) * scale);
  const line = (x0: number, y0: number, x1: number, y1: number) => {
    ctx.beginPath();
    ctx.moveTo(sx(x0), sy(y0));
    ctx.lineTo(sx(x1), sy(y1));
    ctx.stroke();
  };
  rect(M_OUT, M_OUT, pw - M_OUT, ph - M_OUT);
  rect(M_IN, M_IN, pw - M_IN, ph - M_IN);

  // Reference band: numbers 1..cols along top+bottom, letters A.. down left+right, ~50 mm per
  // division (KiCad's scheme). Draw the glyphs only when they're legible at this zoom.
  const innerW = pw - 2 * M_IN;
  const innerH = ph - 2 * M_IN;
  const cols = Math.max(1, Math.round(innerW / 50));
  const rowsN = Math.max(1, Math.round(innerH / 50));
  const band = (M_OUT + M_IN) / 2;
  const legibleBand = 1.8 * scale >= 4;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  if (legibleBand) ctx.font = `${1.8 * scale}px ${strokeFont}`;
  for (let i = 0; i < cols; i++) {
    const x0 = M_IN + (innerW * i) / cols;
    if (legibleBand) {
      const num = String(i + 1);
      const xc = x0 + innerW / cols / 2;
      ctx.fillText(num, sx(xc), sy(band));
      ctx.fillText(num, sx(xc), sy(ph - band));
    }
    if (i > 0) {
      line(x0, M_OUT, x0, M_IN);
      line(x0, ph - M_IN, x0, ph - M_OUT);
    }
  }
  for (let j = 0; j < rowsN; j++) {
    const y0 = M_IN + (innerH * j) / rowsN;
    if (legibleBand) {
      const letter = String.fromCharCode(65 + (j % 26)); // A..Z, wrapping on absurd pages
      const yc = y0 + innerH / rowsN / 2;
      ctx.fillText(letter, sx(band), sy(yc));
      ctx.fillText(letter, sx(pw - band), sy(yc));
    }
    if (j > 0) {
      line(M_OUT, y0, M_IN, y0);
      line(pw - M_IN, y0, pw - M_OUT, y0);
    }
  }

  // ---- title block (bottom-right, anchored to the inner border corner) ----
  const right = pw - M_IN;
  const bottom = ph - M_IN;
  const left = right - 108; // 108 mm wide
  const top = bottom - 32; // 32 mm tall
  rect(left, top, right, bottom);
  const yUp = (up: number) => bottom - up;
  const [yr1, yr2, yr3, yr4] = [yUp(3.5), yUp(6.5), yUp(10.5), yUp(16.5)];
  for (const yl of [yr1, yr2, yr3, yr4]) line(left, yl, right, yl);
  const vRev = right - 23.9; // Rev/Id column (bottom two rows)
  const vDate = right - 88; // Size | Date split
  line(vRev, yr2, vRev, bottom);
  line(vDate, yr1, vDate, yr2);

  const f = r.frame;
  // Cell text: left-aligned, small inset. Skip when a 1.5 mm cell would be sub-pixel — the
  // frame lines still read, the text just isn't legible that far out.
  if (!f || 1.5 * scale < 3.5) {
    ctx.restore();
    return;
  }
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  const pad = 1.3;
  const cell = (x: number, baseline: number, sizeMm: number, bold: boolean, italic: boolean, t: string) => {
    if (!t) return;
    ctx.font = `${italic ? "italic " : ""}${bold ? "bold " : ""}${sizeMm * scale}px ${strokeFont}`;
    ctx.fillText(t, sx(x), sy(baseline));
  };
  cell(left + pad, yr4 + 2.4, 1.5, false, false, "Sheet:");
  cell(left + pad, yr4 + 5.1, 1.5, false, false, f.file ? `File: ${f.file}` : "File:");
  cell(left + pad, yr2 - 1.1, 2.6, true, true, `Title: ${f.title}`);
  cell(left + pad, yr1 - 0.9, 1.5, false, false, `Size: ${f.paper}`);
  cell(vDate + pad, yr1 - 0.9, 1.5, false, false, `Date: ${f.date}`);
  cell(vRev + pad, yr1 - 0.9, 1.5, true, false, `Rev: ${f.rev}`);
  if (f.version) cell(left + pad, bottom - 0.9, 1.5, false, false, `KiCad E.D.A. ${f.version}`);
  cell(vRev + pad, bottom - 0.9, 1.5, false, false, "Id: 1/1"); // a PCB is one sheet
  // Company then the comment lines, stacked upward above the Sheet/File row (KiCad order),
  // bounded so a long comment list can't climb past the box top.
  let up = yr4 - 2.6;
  for (const t of [f.company, ...f.comments]) {
    if (up < top + 1) break;
    if (t) {
      cell(left + pad, up, 1.5, false, false, t);
      up -= 2.7;
    }
  }
  ctx.restore();
}

export function PcbGlView({ visible }: { visible: boolean }) {
  const indexes = useDesignStore((s) => s.indexes);
  const getPcbGeometry = useDesignStore((s) => s.getPcbGeometry);
  const active = usePcbViewStore((s) => s.active);
  const hidden = usePcbViewStore((s) => s.hidden);
  const objects = usePcbViewStore((s) => s.objects);
  const opacity = usePcbViewStore((s) => s.opacity);
  const resetForLayers = usePcbViewStore((s) => s.resetForLayers);
  const highlights = useSelectionStore((s) => s.highlights);
  const pinned = useSelectionStore((s) => s.pinned);
  // Comment mode (C): a crosshair signals you can click an object to anchor a comment.
  const armed = useReviewStore((s) => s.armed);
  // Measure mode (Ctrl+Shift+M): crosshair + click-A/click-B ruler on the overlay.
  const measureActive = useMeasureStore((s) => s.active);
  const measureUnits = useMeasureStore((s) => s.units);

  // Visual diff (plan §4): while a comparison is active and BOTH sides carry a PCB
  // geometry IR, the view renders the compare overlay instead of the plain board.
  const diffActive = useDiffStore((s) => s.active);
  const diffBlink = useDiffStore((s) => s.blink);
  const diffHideZones = useDiffStore((s) => s.hideZones);
  const diffHiddenIds = useDiffStore((s) => s.hiddenChangeIds);
  const diffFocusedId = useDiffStore((s) => s.focusedChangeId);
  // Identity of the current comparison — the diff-inputs effect must re-bake geomA + the
  // owner flags when a NEW doc lands even if diffActive/indexes are unchanged (else the
  // GPU codes go stale against the new changeset).
  const diffDoc = useDiffStore((s) => s.doc);
  const diffCacheKeyA = useDiffStore((s) => s.cacheKeyA);

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const labelCanvasRef = useRef<HTMLCanvasElement>(null);
  /** Reusable offscreen canvas for the removed-text crosshatch pass (diff mode). */
  const hatchScratchRef = useRef<HTMLCanvasElement | null>(null);
  const commentLayerRef = useRef<HTMLDivElement>(null);
  const rendererRef = useRef<PcbGlRenderer | null>(null);
  /** The A (older) side's renderer, sharing the same GL context; null outside diff mode. */
  const rendererARef = useRef<PcbGlRenderer | null>(null);
  /** Prepared diff inputs (A geometry + both sides' changed flags); renderers rebuild
   *  with the flags baked in when this lands (diffEpoch bump re-runs the create effect). */
  const diffDataRef = useRef<{
    geomA: PcbGeometry;
    geomB: PcbGeometry;
    flagsA: DiffFlags;
    flagsB: DiffFlags;
  } | null>(null);
  /** Memoised per-change extents (world mm), keyed by change id; dropped whole when
   *  the diff-data object identity changes (flags rebuilt). */
  const extentMemo = useRef<{ data: object; boxes: Map<string, BBox | null> } | null>(null);
  /** The GPU visibility mask, cached from the visibility effect so drawOverlay's
   *  per-frame text pass reads it instead of rebuilding it every frame (blink/pulse keep
   *  the loop hot). Rebuilt only when hiddenChangeIds / the doc change. */
  const diffVisRef = useRef<Uint8Array | null>(null);
  const [diffEpoch, setDiffEpoch] = useState(0);
  const diffOnRef = useRef(false);
  diffOnRef.current = diffActive;
  const blinkRef = useRef(false);
  blinkRef.current = diffActive && diffBlink;
  const hideZonesRef = useRef(false);
  hideZonesRef.current = diffHideZones;
  /** True while the last overlay frame drew the focused-change pulse frame — keeps the
   *  dirty-driven render loop animating (the pulse is the only continuous animation). */
  const diffPulse = useRef(false);
  /** The indexes the current renderer was built for — a rebuild for the SAME design
   *  (diff enter/exit) keeps the camera; a new design triggers the first fit. */
  const lastIndexesRef = useRef<typeof indexes | null>(null);
  /** Blink phase (true = the removed/A overlay is showing) + Space-held pause. */
  const blinkA = useRef(true);
  const blinkHold = useRef(false);
  const cam = useRef<Camera>({ x: 0, y: 0, scale: 1 });
  const dirty = useRef(true);
  const needsFit = useRef(false);
  // Whether the camera has framed the board at least once (the `fitted` probe field);
  // cleared when a new geometry loads (needsFit set), set true once fit() runs.
  const fitted = useRef(false);
  // True while the camera still sits at a plain whole-board fit and nothing else has
  // moved it. While true, a canvas resize re-runs the fit — the first fit can land on
  // a not-yet-settled layout (window still restoring/maximizing, panels mounting) and
  // under-zoom the board (feedback 2.PNG). Any pan/zoom/reveal clears it, so a resize
  // never overrides a camera the user has placed.
  const atFit = useRef(false);
  // Net-name strings actually drawn on the last overlay frame (the `netLabels` probe
  // field). drawOverlay culls by zoom + collision, so this is the placed set, not all
  // candidates from renderer.netLabels().
  const placedLabels = useRef<string[]>([]);
  // A cross-probe reveal fired while the PCB tab is still hidden (canvas clientWidth 0) can't
  // compute a fit scale yet; stash the target bbox and land it on the first sized frame.
  const pendingReveal = useRef<BBox | null>(null);
  const objRef = useRef<ObjectState>({ objects, opacity });
  objRef.current = { objects, opacity };
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  // Comment chips: world anchor (mm) + the chip element, positioned each frame.
  const commentChips = useRef<{ el: HTMLDivElement; x: number; y: number }[]>([]);
  const commentList = useRef<ChipComment[]>([]);
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  // WebGL context-loss recovery: a GPU reset / driver update / sleep-resume kills the
  // context. `contextLost` drives an inline retry overlay; bumping `restoreNonce` re-runs
  // the create effect to rebuild the renderer once the context is back.
  const [contextLost, setContextLost] = useState(false);
  const [restoreNonce, setRestoreNonce] = useState(0);
  // Measure tool: the ephemeral measurement (A, B) + live hover live in refs so the rAF
  // loop reads them without re-rendering; the snap index is built once per geometry load.
  const measureActiveRef = useRef(measureActive);
  measureActiveRef.current = measureActive;
  const mA = useRef<MPoint | null>(null);
  const mB = useRef<MPoint | null>(null);
  const mHover = useRef<MPoint | null>(null);
  const snapIndex = useRef<SnapIndex | null>(null);
  const edgeIndex = useRef<EdgeIndex | null>(null);

  // New design may change the layer set; reset hides/active (shared with SVG view).
  useEffect(() => {
    resetForLayers(indexes?.layers.map((l) => l.name) ?? []);
  }, [indexes, resetForLayers]);

  // ---- selection → renderer mask -----------------------------------------
  const syncSelection = (r: PcbGlRenderer) => {
    const sel = useSelectionStore.getState();
    const isSel = (h: Highlight) => sel.highlights.some((x) => x.kind === h.kind && x.ref === h.ref);
    const combined = [...sel.pinned.filter((p) => !isSel(p)), ...sel.highlights];
    const nets: { id: number; color: [number, number, number]; emphasize: boolean }[] = [];
    const comps: { id: number; color: [number, number, number] }[] = [];
    for (const h of combined) {
      // resolveCssColor handles hex, rgb() and var(--x) so any future colour source is
      // safe (the old hex-only parser fell back to white on anything else).
      const color = resolveCssColor(h.color);
      if (h.kind === "net") {
        const i = r.netIndexByName.get(h.ref);
        // A transient net selection (single or shift-multi) emphasises the net in its own
        // layer colours; a pinned "highlight in colour" (right-click) recolours it.
        if (i != null) nets.push({ id: i, color, emphasize: isSel(h) });
      } else {
        const i = r.compIndexByRef.get(h.ref);
        if (i != null) comps.push({ id: i, color });
      }
    }
    r.setSelection(nets, comps);
  };

  // ---- camera helpers -----------------------------------------------------
  const fit = () => {
    const r = rendererRef.current;
    const canvas = canvasRef.current;
    if (!r || !canvas) return;
    const b = r.bbox;
    const bw = Math.max(b.maxx - b.minx, 1);
    const bh = Math.max(b.maxy - b.miny, 1);
    const cw = canvas.clientWidth || 1;
    const ch = canvas.clientHeight || 1;
    cam.current = {
      x: (b.minx + b.maxx) / 2,
      y: (b.miny + b.maxy) / 2,
      scale: Math.min(cw / bw, ch / bh) * 0.9,
    };
    fitted.current = true;
    atFit.current = true;
    dirty.current = true;
  };

  /** Zoom about the viewport centre by a factor (toolbar ± / PgUp-PgDn). cam.{x,y} is
   *  the world point AT the centre, so scaling alone keeps it fixed. Clamped to the
   *  same MIN_SCALE–MAX_SCALE px/mm range as the wheel. */
  const zoomBy = (factor: number) => {
    atFit.current = false;
    cam.current.scale = Math.max(MIN_SCALE, Math.min(cam.current.scale * factor, MAX_SCALE));
    dirty.current = true;
  };

  /** Frame `b` (world mm) in the viewport, deferring until the canvas has on-screen size.
   *  A schematic→PCB cross-probe fires the reveal BEFORE the PCB tab is shown, when the
   *  canvas is still display:none (clientWidth 0); computing the scale then collapsed the
   *  board to a dot ("zoomed-out state"). Instead we stash the target and land it on the
   *  first sized frame (see the rAF loop). Caps at 60 px/mm so a tiny feature isn't over-zoomed. */
  const applyReveal = (b: BBox) => {
    const canvas = canvasRef.current;
    const cw = canvas?.clientWidth ?? 0;
    const ch = canvas?.clientHeight ?? 0;
    if (cw <= 0 || ch <= 0) {
      pendingReveal.current = b; // defer until the tab is visible and the canvas is sized
      return;
    }
    const bw = Math.max(b.maxx - b.minx, 2);
    const bh = Math.max(b.maxy - b.miny, 2);
    const scale = Math.min(Math.min(cw / bw, ch / bh) * 0.6, 60);
    atFit.current = false;
    cam.current = { x: (b.minx + b.maxx) / 2, y: (b.miny + b.maxy) / 2, scale };
    pendingReveal.current = null;
    needsFit.current = false; // an explicit reveal supersedes the first-reveal fit
    dirty.current = true;
  };

  /** Land the camera on a net's geometry (schematic→PCB cross-probe). */
  const landOnNet = (name: string) => {
    const b = rendererRef.current?.netBBox(name);
    if (b) applyReveal(b);
  };

  /** Center on a comment's anchored object/region WITHOUT selecting it (item 8). */
  const revealAnchor = (anchor: CommentAnchor) => {
    const r = rendererRef.current;
    if (!r) return;
    let b: BBox | null;
    if (anchor.type === "region" && anchor.rect) {
      const rc = anchor.rect;
      b = { minx: rc.x, miny: rc.y, maxx: rc.x + rc.w, maxy: rc.y + rc.h };
    } else if (anchor.type === "net") {
      // Make the net's copper layers visible first so its geometry is framable.
      const pcb = useDesignStore.getState().pcbIndex;
      for (const l of pcb?.nets[anchor.ref]?.layers ?? []) usePcbViewStore.getState().showLayer(l);
      b = r.netBBox(anchor.ref);
    } else {
      b = r.compBBox(anchor.ref);
    }
    if (b) applyReveal(b);
  };

  /** The focused change's TRUE extent: the union bbox of the primitives that change
   *  owns on BOTH revisions (glDiff flags), so the camera/pulse-frame covers exactly
   *  the rerouted stretch / moved footprint's old+new spot — not the whole net.
   *  Null until the async diff flags land (callers fall back to the anchor). */
  const extentForChange = (id: string): BBox | null => {
    const data = diffDataRef.current;
    if (!data) return null;
    let m = extentMemo.current;
    if (!m || m.data !== data) {
      m = { data, boxes: new Map() };
      extentMemo.current = m;
    }
    const cached = m.boxes.get(id);
    if (cached !== undefined) return cached;
    const doc = useDiffStore.getState().doc;
    const k = doc?.changes.findIndex((c) => c.id === id) ?? -1;
    let box: BBox | null = null;
    if (k >= 0) {
      const code = changeCode(k);
      box = unionExtent(
        changeExtent(data.geomA, data.flagsA, code),
        changeExtent(data.geomB, data.flagsB, code),
      );
    }
    m.boxes.set(id, box);
    return box;
  };

  /** A change's frame rect (world mm): its own changed-primitive extent, else — until the
   *  flags land — the anchor's bbox, else the comp/net extent on the renderer. Shared by
   *  revealChange's camera landing and the accent-frame draw so the camera and the pulsing
   *  highlight always frame the same rectangle. */
  const boxForChange = (change: Change, r: PcbGlRenderer): BBox | null => {
    const b = extentForChange(change.id);
    if (b) return b;
    const pcb = change.anchors.pcb;
    if (!pcb) return null;
    if (pcb.bbox) {
      const [x, y, w, h] = pcb.bbox;
      return { minx: x, miny: y, maxx: x + w, maxy: y + h };
    }
    if (pcb.comp) return r.compBBox(pcb.comp);
    if (pcb.net) return r.netBBox(pcb.net);
    return null;
  };

  /** Land the camera on a focused change (diff mode). Prefers the change's own extent;
   *  falls back to the anchor's bbox/comp/net until the flags land. Deliberately does
   *  NOT un-hide layers (unlike revealAnchor's net path) — focusChange just isolated
   *  the changed layer, and this must not undo that. */
  const revealChange = (change: Change) => {
    const r = rendererRef.current;
    if (!r) return;
    const b = boxForChange(change, r);
    if (b) applyReveal(b);
  };

  // ---- right-click menu + comment anchoring -------------------------------
  type PcbTarget =
    | { kind: "pin"; designator: string; pin: string }
    | { kind: "net" | "comp"; ref: string }
    | null;
  /** What's under a screen point (no selection mutation) — for the context menu. */
  const resolveAtPoint = (clientX: number, clientY: number): PcbTarget => {
    const r = rendererRef.current;
    if (!r) return null;
    const { x, y } = toWorld(clientX, clientY);
    const hiddenSet = usePcbViewStore.getState().hidden;
    const hit = r.hitTest(x, y, (idx) => !hiddenSet.has(r.layerNames[idx] ?? ""));
    if (!hit) return null;
    if (hit.pad && hit.comp) return { kind: "pin", designator: hit.comp, pin: hit.pad };
    if (hit.net) return { kind: "net", ref: hit.net };
    if (hit.comp) return { kind: "comp", ref: hit.comp };
    return null;
  };

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    const at = toWorld(e.clientX, e.clientY);

    // Measure mode owns the right-click menu: cancel the in-progress measurement, copy a
    // completed one, re-anchor here, or exit the tool (task: right-click cancel option).
    if (measureActiveRef.current) {
      const inProgress = mA.current != null;
      const complete = mA.current != null && mB.current != null;
      const mItems: MenuItem[] = [];
      if (inProgress)
        mItems.push({ label: "Cancel measurement", icon: <IconTrash size={14} />, onClick: () => measureNav.clear() });
      if (complete)
        mItems.push({ label: "Copy measurement", icon: <IconCopy size={14} />, onClick: () => void measureNav.copy() });
      mItems.push({ label: "Measure from here", icon: <IconRuler size={14} />, onClick: () => measureNav.from(at.x, at.y) });
      mItems.push({ separator: true });
      mItems.push({ label: "Fit to screen", icon: <IconFit size={14} />, onClick: () => fit() });
      mItems.push({ label: "Exit measure tool", icon: <IconClose size={14} />, onClick: () => useMeasureStore.getState().setActive(false) });
      setCtxMenu({ x: e.clientX, y: e.clientY, items: mItems });
      return;
    }

    const t = resolveAtPoint(e.clientX, e.clientY);
    const pos = { x: e.clientX + 12, y: e.clientY };
    const sel = useSelectionStore.getState();
    const items: MenuItem[] = [];
    if (t && t.kind !== "pin") {
      const { kind, ref } = t;
      const isPinned = sel.pinned.some((p) => p.kind === kind && p.ref === ref);
      items.push({ label: `Highlight ${ref}`, colorPicker: { onPick: (c) => void sel.pinHighlight({ kind, ref, color: c }) } });
      if (isPinned) items.push({ label: "Remove highlight", icon: <IconTrash size={14} />, onClick: () => void sel.unpinHighlight(kind, ref) });
      items.push({ label: kind === "net" ? "Copy net name" : "Copy designator", icon: <IconCopy size={14} />, onClick: () => void navigator.clipboard?.writeText(ref) });
      if (kind === "comp") {
        const val = indexes?.components[ref]?.value;
        if (val) items.push({ label: "Copy value", icon: <IconCopy size={14} />, onClick: () => void navigator.clipboard?.writeText(val) });
      }
      items.push({
        label: "Show on schematic",
        icon: <IconSheet size={14} />,
        onClick: () => {
          useViewStore.getState().setView("schematic");
          if (kind === "net") nav.goNet(ref);
          else nav.goComp(ref);
        },
      });
      items.push({
        label: "Add comment",
        icon: <IconComment size={14} />,
        onClick: () =>
          useReviewStore.getState().beginCompose({
            anchor: kind === "net" ? { type: "net", ref, at } : { type: "component", ref, at },
            pos,
          }),
      });
      items.push({ separator: true });
    } else if (t && t.kind === "pin") {
      const label = `${t.designator}.${t.pin}`;
      const pin = t;
      items.push({ label: `Copy ${label}`, icon: <IconCopy size={14} />, onClick: () => void navigator.clipboard?.writeText(label) });
      items.push({
        label: "Show pin on schematic",
        icon: <IconSheet size={14} />,
        onClick: () => {
          useViewStore.getState().setView("schematic");
          nav.goPin(pin.designator, pin.pin);
        },
      });
      items.push({
        label: "Add comment",
        icon: <IconComment size={14} />,
        onClick: () => useReviewStore.getState().beginCompose({ anchor: { type: "component", ref: pin.designator, at }, pos }),
      });
      items.push({ separator: true });
    }
    items.push({
      label: "Measure from here",
      icon: <IconRuler size={14} />,
      onClick: () => {
        useReviewStore.getState().arm(false); // measure ⇄ comment are exclusive
        useMeasureStore.getState().setActive(true);
        measureNav.from(at.x, at.y);
      },
    });
    items.push({ label: "Fit to screen", icon: <IconFit size={14} />, onClick: () => fit() });
    items.push({
      label: "Clear highlights",
      icon: <IconTrash size={14} />,
      disabled: sel.highlights.length === 0 && sel.pinned.length === 0,
      onClick: () => {
        void sel.clearPinned();
        sel.setHighlights([], "pcb");
        sel.setSelection(null, "pcb");
      },
    });
    // In comparison mode, expose the same exit the banner's × does — a right-click on
    // the board is where users reach for it (batch1).
    if (useDiffStore.getState().active) {
      items.push({ separator: true });
      items.push({ label: "Exit comparison", icon: <IconClose size={14} />, onClick: () => useDiffStore.getState().exitDiff() });
    }
    setCtxMenu({ x: e.clientX, y: e.clientY, items });
  };

  /** Object-anchored comment chips: build the chip elements (re-anchored on list or
   *  design change); the render loop rides them on their world anchors each frame. */
  const renderPcbCommentChips = () => {
    const layer = commentLayerRef.current;
    if (!layer) return;
    layer.innerHTML = "";
    commentChips.current = [];
    const r = rendererRef.current;
    for (const c of commentList.current) {
      let ax: number, ay: number;
      if (c.anchor.type === "region") {
        if (!c.anchor.rect) continue;
        ax = c.anchor.rect.x + c.anchor.rect.w; // chip rides the top-right corner
        ay = c.anchor.rect.y;
      } else if (c.anchor.at) {
        ax = c.anchor.at.x; // pin where the user clicked (24.PNG)
        ay = c.anchor.at.y;
      } else if (r) {
        const b = c.anchor.type === "net" ? r.netBBox(c.anchor.ref) : r.compBBox(c.anchor.ref);
        if (!b) continue;
        ax = b.maxx;
        ay = b.miny;
      } else continue;
      const div = document.createElement("div");
      div.className = `cmt-chip st-${c.status}${c.severity ? ` sev-${c.severity}` : ""}`;
      div.textContent = c.status === "resolved" ? "✓" : c.status === "recheck" ? "⟳" : String(c.number);
      div.title = `Comment ${c.number} on ${c.anchor.ref} — ${c.status}`;
      div.onpointerdown = (e) => e.stopPropagation();
      div.onclick = (e) => {
        e.stopPropagation(); // item 8: a chip click just opens its thread
        const rb = div.getBoundingClientRect();
        useReviewStore.getState().openThread(c.id, { x: rb.right + 8, y: rb.top });
      };
      layer.appendChild(div);
      commentChips.current.push({ el: div, x: ax, y: ay });
    }
    dirty.current = true;
  };

  // ---- Canvas2D overlay above the GL canvas: silk/board text + net labels ----
  const drawOverlay = (r: PcbGlRenderer, cssW: number, cssH: number, dpr: number) => {
    const lc = labelCanvasRef.current;
    if (!lc) return;
    const W = Math.round(cssW * dpr);
    const H = Math.round(cssH * dpr);
    if (lc.width !== W || lc.height !== H) {
      lc.width = W;
      lc.height = H;
    }
    const ctx = lc.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    const pv = usePcbViewStore.getState();
    const c = cam.current;
    const scale = c.scale; // CSS px per mm
    const rootStyle = getComputedStyle(document.documentElement);
    const strokeFont = rootStyle.getPropertyValue("--font-stroke").trim() || "sans-serif";

    // --- drawing sheet: frame + title block (page context, drawn behind everything and
    //     independent of the active layer, so it reads even with no layer selected) ---
    const worksheetColor = rootStyle.getPropertyValue("--pcb-worksheet").trim() || "#c86464";
    drawWorksheet(r, ctx, c, cssW, cssH, strokeFont, worksheetColor);

    // --- board / footprint / dimension text ---
    // Default text uses the KiCad stroke-font engine (strokeFont.ts): real Newstroke
    // polylines scaled by the authored `(size h w)` and stroked at the authored
    // `(thickness)`, so glyph metrics, pen weight and knockout boxes match the .kicad_pcb
    // exactly. Text that names a real outline font (`(font (face …))`, e.g. Calibri) is
    // the exception — KiCad plots those with the actual TTF, so drawOutlineText fills them
    // in that family (the stroke font can't reproduce an arbitrary typeface).
    const drawBoardText = (
      tctx: CanvasRenderingContext2D,
      rr: PcbGlRenderer,
      t: PcbTextDef,
      colorOverride?: string,
    ) => {
      const key = t.comp != null ? "footprints" : "text"; // footprint text vs board text
      if (pv.objects[key] === false) return;
      if (pv.hidden.has(rr.layerNames[t.layer] ?? "")) return;
      if (t.size * scale < 3.2) return; // unreadable at this zoom
      const sx = (t.x - c.x) * scale + cssW / 2;
      const sy = (t.y - c.y) * scale + cssH / 2;
      if (sx < -250 || sx > cssW + 250 || sy < -250 || sy > cssH + 250) return;
      // KiCad CCW angle → screen CW = -a; footprint text is kept upright [0,180), board
      // text uses its literal angle normalised to (-180, 180] (mirrors the SVG view).
      let a = t.angle % 360;
      if (t.upright) {
        if (a < 0) a += 360;
        if (a >= 180) a -= 180;
      } else if (a > 180) a -= 360;
      else if (a <= -180) a += 360;
      const textColor = colorOverride ?? rr.layerColorCss(t.layer);
      // A text that names a real outline font (e.g. Calibri) is rendered in that font —
      // KiCad plots such text with the actual TTF, not the stroke font. Everything else
      // keeps the exact Newstroke stroke-font pipeline below.
      if (t.font) {
        drawOutlineText(tctx, t, sx, sy, a, scale, textColor, strokeFont);
        return;
      }
      const layout = layoutStrokeText(t.text, {
        size: t.size,
        width: t.width,
        thickness: t.thickness,
        bold: t.bold,
        italic: t.italic,
        justify: t.justify,
      });
      if (layout.strokes.length === 0) return;
      tctx.save();
      tctx.translate(sx, sy);
      if (a) tctx.rotate((-a * Math.PI) / 180);
      if (t.mirror) tctx.scale(-1, 1);
      tctx.scale(scale, scale); // draw in text-local mm; lineWidth is the pen in mm
      tctx.lineWidth = layout.pen;
      tctx.lineCap = "round";
      tctx.lineJoin = "round";
      if (t.knockout && layout.bbox) {
        // KiCad knockout: the ink bbox (pen included) inflated by max(pen/2, height/9),
        // filled in the layer colour, with the glyph strokes punched back out
        // (pcb_text.cpp TransformTextToPolySet + buildBoundingHull).
        const b = layout.bbox;
        const m = knockoutMargin(t.size, layout.pen);
        tctx.fillStyle = textColor;
        tctx.fillRect(b.minx - m, b.miny - m, b.maxx - b.minx + 2 * m, b.maxy - b.miny + 2 * m);
        tctx.globalCompositeOperation = "destination-out";
        tctx.strokeStyle = "#000"; // any opaque colour — only alpha matters here
        tctx.beginPath();
        traceStrokeText(tctx, layout);
        tctx.stroke();
      } else {
        tctx.strokeStyle = textColor;
        tctx.beginPath();
        traceStrokeText(tctx, layout);
        tctx.stroke();
      }
      tctx.restore();
    };

    // In diff mode the text passes mirror the GL copper passes: unchanged texts (and
    // a HIDDEN change's texts) draw in the flat base grey so only the spotlit change
    // carries colour; a visible changed text draws solid in its layer colour and
    // follows the added blink phase; then A's removed/old texts draw crosshatched ON
    // TOP for visible rows only.
    const diff = diffOnRef.current ? diffDataRef.current : null;
    const rA = rendererARef.current;
    const diffDoc = diff && rA ? useDiffStore.getState() : null;
    // The mask is maintained by the visibility effect (keyed on hiddenChangeIds/doc), so
    // read the cached ref rather than rebuilding a Uint8Array + walking every change on
    // each frame — the pulse/blink loop can run this dozens of times a second.
    const vis = diffDoc?.doc ? diffVisRef.current : null;
    const blinkOn = blinkRef.current;
    const diffGrey = rootStyle.getPropertyValue("--pcb-diff-base").trim() || PCB_DIFF_BASE_FALLBACK;
    for (let i = 0; i < r.texts.length; i++) {
      const spotlit = diff && vis ? diff.flagsB.texts[i] > 0 && vis[diff.flagsB.texts[i]] > 0 : false;
      if (spotlit && blinkOn && blinkA.current) continue; // added: B phase only
      drawBoardText(ctx, r, r.texts[i], diff && vis && !spotlit ? diffGrey : undefined);
    }
    if (diff && vis && rA && (!blinkOn || blinkA.current)) {
      let sctx: CanvasRenderingContext2D | null = null;
      for (let i = 0; i < diff.geomA.texts.length; i++) {
        const code = diff.flagsA.texts[i];
        if (!code || !vis[code]) continue;
        sctx ??= beginHatchScratch(hatchScratchRef, cssW, cssH, dpr);
        if (!sctx) break; // scratch context unavailable — skip the removed-text pass
        drawBoardText(sctx, rA, diff.geomA.texts[i]);
      }
      if (sctx) compositeHatched(ctx, sctx, dpr);
    }

    // --- net-name labels (+ pad numbers) ---
    // netLabels() yields pads & vias before tracks; we draw in that order and skip any label
    // whose screen box overlaps one already drawn. So a track name never displaces a pad/via
    // name (the requested pad/via-over-track priority) and no two labels overlap.
    const fill = rootStyle.getPropertyValue("--pcb-netlabel").trim() || "#f2f2f2";
    const viaFill = rootStyle.getPropertyValue("--pcb-netlabel-via").trim() || "#111111";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    // Net labels use KiCad's stroke font (Newstroke), matching the schematic — no
    // faux-bold weight. A soft shadow (not a heavy outline) keeps them legible over
    // copper while staying thin, mirroring the SVG view's `.pcb-netlabel` text-shadow.
    ctx.shadowBlur = 3;
    const font = (px: number) => `${px}px ${strokeFont}`;
    const claimed: { x0: number; y0: number; x1: number; y1: number }[] = [];
    const placedNames: string[] = []; // net names actually drawn this frame (probe: netLabels)
    let drawn = 0;
    for (const lab of r.netLabels()) {
      if (drawn >= 500) break; // safety cap so a dense view never stalls on text
      if (pv.objects[lab.key] === false) continue;
      // Hide only when every layer the object occupies is hidden — so a pad's net/number
      // still shows when its mask layer is on but its copper layer is off.
      if (lab.layers.every((L) => pv.hidden.has(r.layerNames[L] ?? ""))) continue;
      const sx = (lab.x - c.x) * scale + cssW / 2;
      const sy = (lab.y - c.y) * scale + cssH / 2;
      if (sx < -48 || sx > cssW + 48 || sy < -48 || sy > cssH + 48) continue;
      // On-screen box: text runs along the longer axis (the track / pad length).
      const alongPx = (lab.key === "tracks" ? lab.w : Math.max(lab.w, lab.h)) * scale;
      const acrossPx = (lab.key === "tracks" ? lab.h : Math.min(lab.w, lab.h)) * scale;
      // Per-row font size + centre offset (px, along the across-axis, pre-rotation): a pad
      // stacks a large number over a smaller net name, every other class shows one net name.
      const rows = netLabelRows(lab, acrossPx);
      if (rows.length === 0) continue;
      // Resolve KiCad markup up front: measure & draw the plain text (braces stripped,
      // `{slash}`→`/`), keeping the runs so `~{…}` spans can be overlined.
      const runs = rows.map((row) => parseMarkup(row.text));
      const plains = runs.map((rs) => rs.map((r) => r.text).join(""));
      // Fit each row's width to the body, shrinking the row's own size so its line stays inside.
      // The budget is the body's long on-screen axis for every class (pads, vias, tracks, zones),
      // so a name never exceeds the pad/via at any zoom — it just shrinks (and drops out below 4px).
      const budget = alongPx;
      let maxSize = 0, blockW = 0, top = Infinity, bottom = -Infinity;
      for (let ri = 0; ri < rows.length; ri++) {
        const row = rows[ri];
        ctx.font = font(row.size);
        const w = ctx.measureText(plains[ri]).width;
        if (w > budget && w > 0) row.size *= budget / w;
        if (Math.min(w, budget) > blockW) blockW = Math.min(w, budget);
        if (row.size > maxSize) maxSize = row.size;
        if (row.cy - row.size / 2 < top) top = row.cy - row.size / 2;
        if (row.cy + row.size / 2 > bottom) bottom = row.cy + row.size / 2;
      }
      if (maxSize < 4) continue; // nothing legible at this zoom
      // Skip if this label's (rotated) screen box collides with an already-drawn one.
      const blockH = bottom - top;
      const rad = (lab.angle * Math.PI) / 180;
      const cos = Math.abs(Math.cos(rad)), sin = Math.abs(Math.sin(rad));
      const halfW = (blockW * cos + blockH * sin) / 2;
      const halfH = (blockW * sin + blockH * cos) / 2;
      const box = { x0: sx - halfW, y0: sy - halfH, x1: sx + halfW, y1: sy + halfH };
      if (claimed.some((o) => box.x0 < o.x1 && box.x1 > o.x0 && box.y0 < o.y1 && box.y1 > o.y0)) continue;
      claimed.push(box);
      if (lab.net) placedNames.push(lab.net); // record for the render probe (netLabels)
      // A via carries dark text on its gold barrel (with a light halo so it reads over the
      // copper annulus too); copper/track labels are light text with a dark halo.
      const onVia = lab.key === "vias";
      ctx.fillStyle = onVia ? viaFill : fill;
      ctx.shadowColor = onVia ? "rgba(255,255,255,0.6)" : "rgba(0,0,0,0.85)";
      ctx.save();
      ctx.translate(sx, sy);
      // World and the overlay are both Y-down, so the board-space angle maps directly.
      if (lab.angle) ctx.rotate(rad);
      for (let ri = 0; ri < rows.length; ri++) {
        const row = rows[ri];
        if (row.size < 3.5) continue; // a row width-fit below legibility is dropped, not smeared
        ctx.font = font(row.size);
        drawLabelRow(ctx, runs[ri], plains[ri], row.cy, row.size);
      }
      ctx.restore();
      drawn++;
    }
    placedLabels.current = placedNames; // expose the placed set to the render probe
    ctx.shadowBlur = 0;
    ctx.shadowColor = "transparent";

    // --- comment region outlines (accent rect + faint fill) ---
    const accent = rootStyle.getPropertyValue("--accent").trim() || "#5b9dff";
    for (const cm of commentList.current) {
      if (cm.anchor.type !== "region" || !cm.anchor.rect) continue;
      const rc = cm.anchor.rect;
      const sx = (rc.x - c.x) * scale + cssW / 2;
      const sy = (rc.y - c.y) * scale + cssH / 2;
      ctx.save();
      ctx.globalAlpha = 0.08;
      ctx.fillStyle = accent;
      ctx.fillRect(sx, sy, rc.w * scale, rc.h * scale);
      ctx.globalAlpha = 1;
      ctx.strokeStyle = accent;
      ctx.lineWidth = 1.2;
      ctx.strokeRect(sx, sy, rc.w * scale, rc.h * scale);
      ctx.restore();
    }

    // --- visual diff: placement move vectors + focused-change pulse frame ---
    // A vector shows WHERE a moved footprint came from (old centroid, red dot) and
    // landed (new centroid, green dot) — drawn for EVERY visible placement move, so
    // the overview reads at a glance (a solo naturally leaves just one). The pulsing
    // accent frame marks the focused change's extent so a stepper landing is
    // unmistakable against the compare tint.
    diffPulse.current = false;
    if (diffOnRef.current) {
      const d = useDiffStore.getState();
      const errCol = rootStyle.getPropertyValue("--err").trim() || "#e05252";
      const okCol = rootStyle.getPropertyValue("--ok").trim() || "#4caf7d";
      const sxOf = (x: number) => (x - c.x) * scale + cssW / 2;
      const syOf = (y: number) => (y - c.y) * scale + cssH / 2;
      const rA = rendererARef.current;
      if (rA && d.doc) {
        let vectors = 0;
        for (const chg of d.doc.changes) {
          if (vectors >= 100) break; // cap: a pathological changeset can't stall the frame
          if (chg.kind !== "moved" || !chg.anchors.pcb?.comp) continue;
          if (d.hiddenChangeIds.has(chg.id)) continue;
          const bA = rA.compBBox(chg.anchors.pcb.comp);
          const bB = r.compBBox(chg.anchors.pcb.comp);
          if (!bA || !bB) continue;
          const x0 = sxOf((bA.minx + bA.maxx) / 2);
          const y0 = syOf((bA.miny + bA.maxy) / 2);
          const x1 = sxOf((bB.minx + bB.maxx) / 2);
          const y1 = syOf((bB.miny + bB.maxy) / 2);
          // Off-screen or sub-4-px (rotation-only / tiny nudge at this zoom): skip —
          // two smeared dots read worse than nothing.
          if (Math.hypot(x1 - x0, y1 - y0) <= 4) continue;
          if (Math.max(x0, x1) < 0 || Math.min(x0, x1) > cssW || Math.max(y0, y1) < 0 || Math.min(y0, y1) > cssH) continue;
          ctx.save();
          ctx.strokeStyle = accent;
          ctx.lineWidth = 1.5;
          ctx.setLineDash([5, 4]);
          ctx.beginPath();
          ctx.moveTo(x0, y0);
          ctx.lineTo(x1, y1);
          ctx.stroke();
          ctx.setLineDash([]);
          ctx.fillStyle = errCol;
          ctx.beginPath();
          ctx.arc(x0, y0, 3.5, 0, Math.PI * 2);
          ctx.fill();
          ctx.fillStyle = okCol;
          ctx.beginPath();
          ctx.arc(x1, y1, 3.5, 0, Math.PI * 2);
          ctx.fill();
          ctx.restore();
          vectors++;
        }
      }
      // Accent frames: the focused change pulses; while a shift-click subset is active,
      // every OTHER visible change gets a steady frame too, so everything selected is
      // findable at a glance (feedback: "whatever is selected needs the blue box").
      if (d.doc) {
        const subsetActive = d.hiddenChangeIds.size > 0;
        let framed = 0;
        for (const chg of d.doc.changes) {
          if (framed >= 40) break; // cap: a huge subset can't stall the frame
          const pcbA = chg.anchors.pcb;
          if (!pcbA || d.hiddenChangeIds.has(chg.id)) continue;
          const isFocused = chg.id === d.focusedChangeId;
          if (!isFocused && !subsetActive) continue; // overview: only the focused frames
          // Frame rect: the change's OWN extent (just the changed primitives — a
          // rerouted stretch, not the whole net); until the flags land, the anchor's
          // bbox, else the comp/net extent. Same cascade revealChange lands the camera
          // on, so the frame and the camera always agree.
          const fb = boxForChange(chg, r);
          if (!fb) continue;
          const pad = 6; // screen-px breathing room so the frame never sits on the copper
          const fx = sxOf(fb.minx) - pad;
          const fy = syOf(fb.miny) - pad;
          const fw = (fb.maxx - fb.minx) * scale + 2 * pad;
          const fh = (fb.maxy - fb.miny) * scale + 2 * pad;
          ctx.save();
          // Faint fill so the whole selected region reads highlighted, not just an edge.
          ctx.globalAlpha = 0.08;
          ctx.fillStyle = accent;
          ctx.fillRect(fx, fy, fw, fh);
          if (isFocused) {
            ctx.globalAlpha = 0.45 + 0.35 * Math.sin(performance.now() / 250);
            ctx.lineWidth = 2;
            diffPulse.current = true; // keep the rAF loop ticking for the pulse
          } else {
            ctx.globalAlpha = 0.55;
            ctx.lineWidth = 1.5;
          }
          ctx.strokeStyle = accent;
          ctx.strokeRect(fx, fy, fw, fh);
          ctx.restore();
          framed++;
        }
      }
    }

    // --- measure tool: ruler + readout, on top of everything (§7.6) ---
    if (measureActiveRef.current) {
      const cssVar = (name: string, fallback: string) =>
        rootStyle.getPropertyValue(name).trim() || fallback;
      const end = mB.current ?? mHover.current;
      drawMeasure(
        ctx,
        mA.current,
        end,
        mHover.current,
        useMeasureStore.getState().units,
        c,
        cssW,
        cssH,
        {
          line: cssVar("--measure-line", "#ffffff"),
          shadow: cssVar("--measure-shadow", "#000000"),
          text: cssVar("--measure-text", "#ffffff"),
          font: rootStyle.getPropertyValue("--font-mono").trim() || "monospace",
        },
      );
    }
  };

  // ---- diff-mode inputs: A geometry + both sides' changed flags ------------
  // Prepared asynchronously; landing bumps diffEpoch so the create effect below
  // rebuilds both renderers with the flags baked into their GPU batches.
  useEffect(() => {
    if (!diffActive) {
      if (diffDataRef.current) {
        diffDataRef.current = null;
        setDiffEpoch((e) => e + 1); // rebuild back to the plain single-board renderer
      }
      return;
    }
    let cancelled = false;
    void (async () => {
      const geomB = await getPcbGeometry();
      const geomA = await useDiffStore.getState().getPcbGeometryA(indexes?.pcb_geometry);
      if (cancelled || !geomA || !geomB) return; // schematic-only side → plain render
      // The semantic change list drives per-primitive OWNERSHIP (per-change show/solo)
      // and gates zone tinting on a real zone row (refill jitter stays grey).
      const changes = useDiffStore.getState().doc?.changes ?? [];
      const flags = computeDiffFlags(geomA, geomB, changes);
      diffDataRef.current = { geomA, geomB, flagsA: flags.a, flagsB: flags.b };
      setDiffEpoch((e) => e + 1);
      // A change focused before the flags landed was framed on its coarse anchor
      // (whole net / comp): re-land the camera on the true extent now known.
      const d = useDiffStore.getState();
      const focused = d.doc?.changes.find((c) => c.id === d.focusedChangeId);
      if (focused?.anchors.pcb) revealChange(focused);
    })();
    return () => {
      cancelled = true;
    };
    // diffDoc + diffCacheKeyA key the effect to the diff SESSION: a re-prepared doc (or a
    // new A side) re-bakes the flags/geomA even when diffActive and indexes don't change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [diffActive, indexes, getPcbGeometry, diffDoc, diffCacheKeyA]);

  // ---- create / dispose renderer -----------------------------------------
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !indexes?.pcb_geometry) {
      rendererRef.current?.dispose();
      rendererRef.current = null;
      rendererARef.current?.dispose();
      rendererARef.current = null;
      return;
    }
    // premultipliedAlpha: true — the SRC_ALPHA blend leaves a premultiplied framebuffer;
    // telling the compositor the truth avoids a double-darkened AA fringe (a dark border
    // that reads worst on thin tracks).
    const gl = canvas.getContext("webgl2", { antialias: true, premultipliedAlpha: true });
    if (!gl || gl.isContextLost()) return; // a lost context can't build GL resources yet
    let cancelled = false;
    let created: PcbGlRenderer | null = null;
    let createdA: PcbGlRenderer | null = null;
    void getPcbGeometry().then((geom) => {
      if (cancelled || !geom) return;
      const diff = diffDataRef.current;
      try {
        created = new PcbGlRenderer(gl, geom, diff?.flagsB);
        // The A side shares the GL context (two compact geometries resident is fine);
        // its own layer table maps the same hidden/active names.
        if (diff) createdA = new PcbGlRenderer(gl, diff.geomA, diff.flagsA);
      } catch (e) {
        console.error("PcbGlRenderer init failed", e);
        return;
      }
      rendererRef.current = created;
      rendererARef.current = createdA;
      snapIndex.current = buildSnapIndex(geom); // measure-tool point-snap targets (§5)
      edgeIndex.current = buildEdgeIndex(geom); // on-edge / edge-to-edge projection (feature 13)
      // Drop any measurement carried over from the previous board.
      mA.current = mB.current = mHover.current = null;
      const pv = usePcbViewStore.getState();
      // Diff mode fades the visible copper stack top→bottom so a multi-layer compare
      // reads in depth; a plain rebuild keeps the normal opaque alphas.
      created.setLayerState(pv.hidden, pv.active, !!diff);
      createdA?.setLayerState(pv.hidden, pv.active, true);
      // Fresh renderers start with everything visible — re-apply the current
      // per-change visibility (a solo may already be active when we rebuild).
      if (diff) {
        const d = useDiffStore.getState();
        const vis = d.doc ? buildDiffVisibility(d.doc.changes, d.hiddenChangeIds) : null;
        diffVisRef.current = vis; // keep the drawOverlay cache in step with the rebuild
        created.setDiffVisibility(vis);
        createdA?.setDiffVisibility(vis);
      }
      syncSelection(created);
      renderPcbCommentChips(); // re-anchor chips to the fresh geometry
      // A NEW design refits; a diff-mode rebuild of the SAME board keeps the camera
      // (entering/leaving a comparison must not yank the user's viewpoint).
      if (lastIndexesRef.current !== indexes) {
        lastIndexesRef.current = indexes;
        fitted.current = false;
        needsFit.current = true;
      }
      dirty.current = true;
    });
    return () => {
      cancelled = true;
      created?.dispose();
      createdA?.dispose();
      if (rendererRef.current === created) rendererRef.current = null;
      if (rendererARef.current === createdA) rendererARef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [indexes, getPcbGeometry, restoreNonce, diffEpoch]);

  // Context-loss recovery (see contextLost above). Prevent the default so the browser can
  // restore the context, drop the now-invalid renderer, and rebuild on restore. Listeners
  // live on the canvas across renderer recreations, so this effect runs once.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onLost = (e: Event) => {
      e.preventDefault();
      rendererRef.current = null; // GL resources are gone; the loop no-ops until rebuilt
      setContextLost(true);
    };
    const onRestored = () => {
      setContextLost(false);
      setRestoreNonce((n) => n + 1);
    };
    canvas.addEventListener("webglcontextlost", onLost);
    canvas.addEventListener("webglcontextrestored", onRestored);
    return () => {
      canvas.removeEventListener("webglcontextlost", onLost);
      canvas.removeEventListener("webglcontextrestored", onRestored);
    };
  }, []);

  // Re-resolve GL layer colours when the KiCad theme changes (the --pcb-* vars are applied
  // asynchronously by kicadTheme.ts). The Canvas2D overlay re-reads colours per frame, so
  // without this the GL copper could disagree with the overlay text after a theme change.
  useEffect(() => {
    const r = rendererRef.current;
    if (!r) return;
    r.resolveColors();
    rendererARef.current?.resolveColors();
    dirty.current = true;
  }, [indexes?.theme]);

  // Blink: pulse the changed copper — removed (A) and added (B) overlays alternate
  // phases every 500 ms over the stable grey base, so what pulses IN is new copper and
  // what pulses OUT is old. Holding Space freezes the current phase.
  useEffect(() => {
    if (!diffActive || !diffBlink) {
      blinkA.current = true;
      return;
    }
    const t = setInterval(() => {
      if (blinkHold.current) return;
      blinkA.current = !blinkA.current;
      dirty.current = true;
    }, 500);
    const down = (e: KeyboardEvent) => {
      // Only while the PCB canvas is up — the blink pause is a PCB-compare affordance,
      // and this window-level preventDefault must not swallow Space on the schematic.
      if (e.code === "Space" && !isTypingTarget(e) && useViewStore.getState().view === "pcb") {
        e.preventDefault();
        blinkHold.current = true;
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.code === "Space") blinkHold.current = false;
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      clearInterval(t);
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, [diffActive, diffBlink]);

  // Per-change visibility (show-all / solo / eye toggles) → the renderers' GPU mask.
  // A texture update, not a rebuild — stepping J/K through changes stays instant.
  useEffect(() => {
    if (!diffActive) return;
    const doc = useDiffStore.getState().doc;
    if (!doc) return;
    const vis = buildDiffVisibility(doc.changes, diffHiddenIds);
    diffVisRef.current = vis;
    rendererRef.current?.setDiffVisibility(vis);
    rendererARef.current?.setDiffVisibility(vis);
    dirty.current = true;
  }, [diffActive, diffHiddenIds, diffEpoch]);

  // Mode/focus switches need a redraw (the loop is dirty-driven).
  useEffect(() => {
    dirty.current = true;
  }, [diffActive, diffBlink, diffHideZones, diffFocusedId]);

  // ---- store → renderer sync ---------------------------------------------
  useEffect(() => {
    const r = rendererRef.current;
    if (!r) return;
    r.setLayerState(hidden, active);
    rendererARef.current?.setLayerState(hidden, active);
    dirty.current = true;
  }, [hidden, active]);

  useEffect(() => {
    dirty.current = true;
  }, [objects, opacity]);

  useEffect(() => {
    const r = rendererRef.current;
    if (!r) return;
    syncSelection(r);
    dirty.current = true;
    // Land the camera when the cross-probe came from the schematic (not a PCB click).
    const sel = useSelectionStore.getState();
    if (visibleRef.current && sel.source !== "pcb") {
      const net = highlights.find((h) => h.kind === "net");
      if (net) landOnNet(net.ref);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [highlights, pinned]);

  // First reveal fits; later reveals keep the camera.
  useEffect(() => {
    if (visible) dirty.current = true;
  }, [visible]);

  // Canvas text falls back silently if the webfont isn't ready, so preload KiCad's
  // stroke font and force one redraw once it resolves.
  useEffect(() => {
    if (!document.fonts?.load) return;
    Promise.all([document.fonts.load("16px Newstroke"), document.fonts.load("16px osifont")])
      .then(() => {
        dirty.current = true;
      })
      .catch(() => {});
  }, []);

  // PCB nav bridge: CommentBridge pushes chips here, comment rows reveal anchors, and
  // the app-level Fit keybinding reaches the board. Restored to no-ops on unmount.
  useEffect(() => {
    pcbNav.fit = () => fit();
    pcbNav.zoomBy = (f) => zoomBy(f);
    pcbNav.setComments = (cs) => {
      commentList.current = cs;
      renderPcbCommentChips();
    };
    pcbNav.reveal = (anchor) => revealAnchor(anchor);
    pcbNav.revealChange = (change) => revealChange(change);
    // Esc while measuring: clear an in-progress measurement (report whether we did, so
    // the app keymap only exits the mode when there was nothing to clear).
    measureNav.escape = () => {
      // Only a placed point (A/B) counts as an in-progress measurement worth clearing;
      // a bare hover shouldn't swallow the Esc that exits the mode.
      if (!mA.current && !mB.current) return false;
      mA.current = mB.current = mHover.current = null;
      dirty.current = true;
      return true;
    };
    measureNav.clear = () => {
      mA.current = mB.current = null;
      dirty.current = true;
    };
    measureNav.copy = () => {
      const txt = measureReadoutText(mA.current, mB.current, useMeasureStore.getState().units);
      if (!txt) return false;
      void navigator.clipboard?.writeText(txt);
      return true;
    };
    measureNav.from = (x, y) => {
      const at = x != null && y != null ? { x, y, snapped: false } : mHover.current;
      if (!at) return;
      mA.current = { ...at };
      mB.current = null;
      mHover.current = { ...at };
      dirty.current = true;
    };
    return () => {
      pcbNav.fit = () => {};
      pcbNav.zoomBy = () => {};
      pcbNav.setComments = () => {};
      pcbNav.reveal = () => {};
      pcbNav.revealChange = () => {};
      measureNav.escape = () => false;
      measureNav.clear = () => {};
      measureNav.copy = () => false;
      measureNav.from = () => {};
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Measure mode toggled: reset any in-progress measurement when leaving, and force a
  // redraw either way (cursor + toolbar reflect it). Unit changes also need a redraw.
  useEffect(() => {
    if (!measureActive) mA.current = mB.current = mHover.current = null;
    dirty.current = true;
  }, [measureActive]);
  useEffect(() => {
    dirty.current = true;
  }, [measureUnits]);

  // ---- render loop (dirty-driven) ----------------------------------------
  useEffect(() => {
    let raf = 0;
    const loop = () => {
      const r = rendererRef.current;
      const canvas = canvasRef.current;
      if (r && canvas && visibleRef.current) {
        const dpr = Math.min(window.devicePixelRatio || 1, 2);
        const w = Math.round(canvas.clientWidth * dpr);
        const h = Math.round(canvas.clientHeight * dpr);
        if (canvas.clientWidth > 0) {
          // A deferred cross-probe reveal (fired while the tab was hidden) lands first and
          // supersedes the first-reveal fit; otherwise the first sized frame fits the board.
          if (pendingReveal.current) applyReveal(pendingReveal.current);
          else if (needsFit.current) {
            fit();
            needsFit.current = false;
          }
        }
        if (canvas.width !== w || canvas.height !== h) {
          canvas.width = w;
          canvas.height = h;
          // A camera still at a plain whole-board fit tracks the resize (the first
          // fit may have measured a not-yet-settled layout); a placed camera doesn't.
          if (atFit.current) fit();
          dirty.current = true;
        }
        if (dirty.current && w > 0 && h > 0) {
          r.setDpr(dpr);
          const rA = rendererARef.current;
          if (rA && diffOnRef.current) {
            rA.setDpr(dpr);
            const base = objRef.current;
            // The banner's Zones toggle drops pours from the WHOLE compare (they
            // re-flow around edits; sometimes even the gated tint is unwanted).
            const obj = hideZonesRef.current
              ? { objects: { ...base.objects, zones: false }, opacity: base.opacity }
              : base;
            // Overlay: the unchanged common base as flat grey (from B), then the changed
            // copper in its TRUE layer colour — B's added copper solid-but-sheer FIRST,
            // A's removed copper crosshatched ON TOP (colour = layer, texture = old/new;
            // DIFF_LAYER_MIX). Removed paints last because a same-spot restyle (e.g. a
            // thickened track) otherwise buries the old copper under the new; the sheer
            // added pass + the crosshatch cuts keep both readable where they overlap.
            // With blink on, removed and added alternate phases instead.
            r.render(cam.current, w, h, obj, { diffPass: 1 });
            const blinkOn = blinkRef.current;
            if (!blinkOn || !blinkA.current)
              r.render(cam.current, w, h, obj, { clear: false, diffPass: 2, diffMix: DIFF_LAYER_MIX, diffAlpha: DIFF_ADDED_ALPHA });
            if (!blinkOn || blinkA.current)
              rA.render(cam.current, w, h, obj, { clear: false, diffPass: 2, diffMix: DIFF_LAYER_MIX, diffHatch: true });
          } else {
            r.render(cam.current, w, h, objRef.current);
          }
          drawOverlay(r, canvas.clientWidth, canvas.clientHeight, dpr);
          // Comment chips ride their world anchors at fixed screen size.
          const cw = canvas.clientWidth;
          const ch = canvas.clientHeight;
          for (const cc of commentChips.current) {
            const sx = (cc.x - cam.current.x) * cam.current.scale + cw / 2;
            const sy = (cc.y - cam.current.y) * cam.current.scale + ch / 2;
            cc.el.style.transform = `translate(${sx + 2}px,${sy}px) translateY(-65%)`;
          }
          // The focused-change pulse frame is the one continuous animation; keep the
          // dirty-driven loop hot only while it's actually on screen.
          dirty.current = diffPulse.current;
        }
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Debug-only render probe (Layer-2 E2E): tauri-pilot can't read the WebGL board, so
  // expose the live render state for it to assert on via `eval`. Restores the "pcb" probe
  // the deleted SVG PcbView carried. The getter reads live refs/stores at call time (never
  // closes over a captured value); no-op outside dev builds. See lib/renderProbe.ts +
  // docs/e2e-tauri-pilot.md → "The render probe".
  useEffect(() => {
    return registerRenderProbe("pcb", () => {
      const r = rendererRef.current;
      const pv = usePcbViewStore.getState();
      const sel = useSelectionStore.getState();
      const c = cam.current;
      // GL Camera is {x,y,scale}; expose it as the schematic probe's {x,y,s} shape so the
      // catalog's `cam.s` assertions read the same field on both views.
      const camS = { x: c.x, y: c.y, s: c.scale };
      return {
        layers: r ? [...r.layerNames] : (useDesignStore.getState().indexes?.layers.map((l) => l.name) ?? []),
        active: pv.active,
        hidden: [...pv.hidden],
        bbox: r ? { ...r.bbox } : null,
        cam: camS,
        tgt: { ...camS }, // GL applies the camera directly — no separate eased target
        fitted: fitted.current,
        visible: visibleRef.current,
        highlights: sel.highlights.map((h) => ({ kind: h.kind, ref: h.ref, color: h.color })),
        selection: sel.selection,
        marked: r?.markedCount ?? 0,
        dimsel: r?.dimActive ?? false,
        objects: { ...pv.objects },
        netLabels: [...placedLabels.current],
        commentChips: commentChips.current.length,
      };
    });
  }, []);

  // ---- pointer interaction -----------------------------------------------
  const drag = useRef<{ x: number; y: number; moved: boolean } | null>(null);

  const toWorld = (clientX: number, clientY: number) => {
    const canvas = canvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    const mx = clientX - rect.left - rect.width / 2;
    const my = clientY - rect.top - rect.height / 2;
    return { x: cam.current.x + mx / cam.current.scale, y: cam.current.y + my / cam.current.scale };
  };

  /** Resolve a screen point to a measurement point. Priority: geometry vertex/centre
   *  snap, then on-edge projection, then the Ctrl 45° constraint from A, then grid, then
   *  the free world point. Holding **Shift** disables all snapping (free placement — the
   *  KiCad "magnetic off" modifier, replacing the snap-settings UI). Holding **Ctrl**
   *  constrains the A→cursor segment to the nearest 45°. */
  const snapAt = (clientX: number, clientY: number, mods: { noSnap: boolean; ortho: boolean }): MPoint => {
    const raw = toWorld(clientX, clientY);
    const r = rendererRef.current;
    const pv = usePcbViewStore.getState();
    // Shift → free cursor (no geometry / edge / grid snap).
    if (r && !mods.noSnap) {
      const radius = 10 / cam.current.scale; // ~10 screen px in world mm
      const layerVisible = (idx: number) => !pv.hidden.has(r.layerNames[idx] ?? "");
      const objectOn = (kind: SnapKind) => {
        const o = pv.objects;
        if (kind === "pad") return o.pads !== false;
        if (kind === "via") return o.vias !== false;
        if (kind === "track" || kind === "arc") return o.tracks !== false;
        if (kind === "component") return o.footprints !== false;
        return true; // board graphics / outline: gated by layer visibility only
      };
      // Discrete points (corners, centres, endpoints, midpoints) take priority…
      const pt = snapIndex.current?.query(raw.x, raw.y, radius, layerVisible, objectOn);
      if (pt) return { x: pt.x, y: pt.y, snapped: true, radius: pt.radius };
      // …then projection onto the nearest track / outline / pad-or-via edge.
      const eg = edgeIndex.current?.query(raw.x, raw.y, radius, layerVisible, objectOn);
      if (eg) return { x: eg.x, y: eg.y, snapped: true, edge: true, radius: eg.radius };
    }
    // 45° constraint applies while placing/previewing the second point (anchor = A).
    const a = mA.current;
    if (mods.ortho && a && !mB.current) {
      const c = constrain45(a.x, a.y, raw.x, raw.y);
      return { x: c.x, y: c.y, snapped: false };
    }
    const m = useMeasureStore.getState();
    if (!mods.noSnap && m.grid && m.gridStep > 0) {
      return {
        x: Math.round(raw.x / m.gridStep) * m.gridStep,
        y: Math.round(raw.y / m.gridStep) * m.gridStep,
        snapped: false,
      };
    }
    return { x: raw.x, y: raw.y, snapped: false };
  };

  /** A tap while measuring: place A, then B, then a third tap starts a new measurement. */
  const measureTap = (clientX: number, clientY: number, mods: { noSnap: boolean; ortho: boolean }) => {
    const p = snapAt(clientX, clientY, mods);
    if (mA.current && !mB.current) {
      mB.current = p; // complete the measurement
    } else {
      mA.current = p; // start a fresh one (first tap, or replace a completed one)
      mB.current = null;
    }
    mHover.current = p;
    dirty.current = true;
  };

  const handleClick = (clientX: number, clientY: number, shift: boolean) => {
    const r = rendererRef.current;
    if (!r) return;
    const { x, y } = toWorld(clientX, clientY);
    const hiddenSet = usePcbViewStore.getState().hidden;
    const hit = r.hitTest(x, y, (idx) => !hiddenSet.has(r.layerNames[idx] ?? ""));

    // Comment mode (C armed): anchor a new comment to the object under the click
    // instead of selecting it.
    const review = useReviewStore.getState();
    if (review.armed) {
      const at = { x, y };
      let anchor: CommentAnchor | null = null;
      if (hit?.pad && hit.comp) anchor = { type: "component", ref: hit.comp, at };
      else if (hit?.net) anchor = { type: "net", ref: hit.net, at };
      else if (hit?.comp) anchor = { type: "component", ref: hit.comp, at };
      if (anchor) review.beginCompose({ anchor, pos: { x: clientX + 12, y: clientY } });
      return;
    }

    const sel = useSelectionStore.getState();
    sel.setAnchor({ x: clientX, y: clientY });
    if (!hit) {
      if (!shift) {
        sel.setSelection(null, "pcb");
        sel.setHighlights([], "pcb");
      }
      return;
    }
    const addHl = (h: Highlight) => {
      if (!shift) {
        sel.setHighlights([h], "pcb");
        return;
      }
      // Shift toggles membership in the compare set — matches the schematic's toggleAdd, so
      // shift-clicking an already-highlighted net/comp removes it instead of re-adding it.
      const present = sel.highlights.some((x) => x.kind === h.kind && x.ref === h.ref);
      const base = sel.highlights.filter((x) => !(x.kind === h.kind && x.ref === h.ref));
      sel.setHighlights(present ? base : [...base, h], "pcb");
    };
    if (hit.pad && hit.comp) {
      // Focus the pin (the info card shows it) but give it the SAME visual styling as a
      // component selection — highlight its footprint in COMP_COLOR with the rest of the
      // board dimmed — rather than a bare outline (feedback). This also matches KiCad,
      // where clicking a pad selects its footprint. The net is NOT lit (that was too loud).
      sel.setSelection({ kind: "pin", ref: { designator: hit.comp, pin: hit.pad } }, "pcb");
      addHl({ kind: "comp", ref: hit.comp, color: COMP_COLOR });
    } else if (hit.net) {
      sel.setSelection({ kind: "net", ref: hit.net }, "pcb");
      addHl({ kind: "net", ref: hit.net, color: NET_COLOR });
    } else if (hit.comp) {
      sel.setSelection({ kind: "comp", ref: hit.comp }, "pcb");
      addHl({ kind: "comp", ref: hit.comp, color: COMP_COLOR });
    }
  };

  const onPointerDown = (e: React.PointerEvent) => {
    // Right-click opens the context menu (handleContextMenu) — starting a pan here made a
    // right-drag move the board and then pop the menu. Left/middle still pan.
    if (e.button === 2) return;
    drag.current = { x: e.clientX, y: e.clientY, moved: false };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) {
      // No button held: while measuring, track the live cursor (feature 5) so the rAF
      // loop can draw the A→cursor preview. Costs nothing when measure mode is off.
      if (measureActiveRef.current) {
        mHover.current = snapAt(e.clientX, e.clientY, { noSnap: e.shiftKey, ortho: e.ctrlKey });
        dirty.current = true;
      }
      return;
    }
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) d.moved = true;
    atFit.current = false;
    cam.current.x -= dx / cam.current.scale;
    cam.current.y -= dy / cam.current.scale;
    d.x = e.clientX;
    d.y = e.clientY;
    dirty.current = true;
  };
  const onPointerUp = (e: React.PointerEvent) => {
    const d = drag.current;
    drag.current = null;
    // Left button only: right-click opens the context menu (handleContextMenu) and must
    // not select/highlight a net (the documented item-22 behaviour the SVG view has).
    if (d && !d.moved && e.button === 0) {
      // A tap while measuring places a ruler point instead of selecting (drags still pan,
      // so mid-measurement navigation works like KiCad).
      if (measureActiveRef.current) measureTap(e.clientX, e.clientY, { noSnap: e.shiftKey, ortho: e.ctrlKey });
      else handleClick(e.clientX, e.clientY, e.shiftKey);
    }
  };

  // Wheel must be a non-passive native listener to allow preventDefault.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left - rect.width / 2;
      const my = e.clientY - rect.top - rect.height / 2;
      const wx = cam.current.x + mx / cam.current.scale;
      const wy = cam.current.y + my / cam.current.scale;
      atFit.current = false;
      cam.current.scale *= Math.exp(-e.deltaY * 0.0015);
      cam.current.scale = Math.max(MIN_SCALE, Math.min(cam.current.scale, MAX_SCALE));
      cam.current.x = wx - mx / cam.current.scale;
      cam.current.y = wy - my / cam.current.scale;
      dirty.current = true;
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, []);

  return (
    <>
      <div
        style={{ position: "relative", width: "100%", height: "100%", cursor: armed || measureActive ? "crosshair" : undefined }}
        onContextMenu={handleContextMenu}
      >
        <canvas
          ref={canvasRef}
          className="pcb-gl-canvas"
          style={{ position: "absolute", inset: 0, width: "100%", height: "100%", display: "block", touchAction: "none" }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={() => (drag.current = null)}
        />
        {/* Net labels + board text ride a Canvas2D layer above the GL canvas. */}
        <canvas
          ref={labelCanvasRef}
          style={{ position: "absolute", inset: 0, width: "100%", height: "100%", pointerEvents: "none" }}
        />
        {/* Object-anchored comment chips (chips are pointer-interactive; layer is not). */}
        <div ref={commentLayerRef} className="pcb-comments" />
        {/* Floating tool bar (fit / zoom / measure / comment) — PCB-only, top-centre. */}
        <PcbToolbar onFit={fit} onZoomIn={() => zoomBy(1.4)} onZoomOut={() => zoomBy(1 / 1.4)} />
        {/* GL context loss: show an inline retry instead of a silently-blank board (bug 1). */}
        {contextLost && (
          <div className="pcb-context-lost">
            <div className="pcb-context-lost-title">Renderer interrupted</div>
            <div className="pcb-context-lost-sub">
              The graphics context was lost (a GPU reset or driver update). It usually
              restores itself in a moment.
            </div>
            <button
              className="btn-primary"
              onClick={() => {
                setContextLost(false);
                setRestoreNonce((n) => n + 1);
              }}
            >
              Retry
            </button>
          </div>
        )}
      </div>
      {ctxMenu && <ContextMenu {...ctxMenu} onClose={() => setCtxMenu(null)} />}
    </>
  );
}
