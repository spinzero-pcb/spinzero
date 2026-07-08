// KiCad stroke-font text engine — an exact port of how KiCad lays out and draws
// board text, so authored `(size h w)` / `(thickness)` reproduce pixel-for-pixel.
//
// KiCad draws text with its built-in Newstroke STROKE font: each glyph is a set of
// polylines, scaled by the authored glyph size and stroked at the authored pen
// width. A filled TTF can never match those metrics (advance widths, cap height,
// pen weight all differ), so this module ports the real thing:
//   - glyph decoding      → stroke_font.cpp  loadNewStrokeFont / GetTextAsGlyphs
//   - line layout/justify → font.cpp         getLinePositions
//   - pen width defaults  → gr_text.cpp      GetPenSizeForNormal/Bold + ClampTextPenSize
//   - knockout box        → pcb_text.cpp     TransformTextToPolySet + GetKnockoutTextMargin
//
// Everything here is in text-local mm, y-down, anchor at (0,0), UNrotated and
// UNmirrored — the caller applies translate/rotate/mirror via the canvas transform
// (matching KiCad, which rotates/mirrors the finished glyph strokes about the anchor).

import { NEWSTROKE_GLYPHS } from "./newstrokeData";

// stroke_font.cpp
const STROKE_FONT_SCALE = 1 / 21;
const FONT_OFFSET = -8; // moves the glyph origin to the baseline
const TAB_WIDTH = 4;
// font.h — italic shear on y (applied to x, glyph-local, about the baseline)
const ITALIC_TILT = 1 / 8;
// font_metrics.h m_InterlinePitch (1.68) × stroke_font.cpp LEGACY_FACTOR (0.9583)
const INTERLINE = 1.68 * 0.9583;
// font.cpp getLinePositions: "1.17 is a fudge to match 6.0 positioning"
const FIRST_LINE_HEIGHT = 1.17;

interface Glyph {
  /** Polylines in glyph units: x has the left bearing removed, y=0 is the baseline
   *  (caps span to ≈ -1, descenders positive — y-down like the board). */
  strokes: number[][];
  /** Cursor advance in glyph units (multiplied by the authored glyph width). */
  advance: number;
}

const glyphCache: (Glyph | undefined)[] = new Array(NEWSTROKE_GLYPHS.length);
const QUESTION = "?".charCodeAt(0) - 0x20;

function decodeGlyph(idx: number): Glyph {
  const def = NEWSTROKE_GLYPHS[idx];
  const startX = (def.charCodeAt(0) - 82) * STROKE_FONT_SCALE; // 82 = 'R'
  const endX = (def.charCodeAt(1) - 82) * STROKE_FONT_SCALE;
  const strokes: number[][] = [];
  let cur: number[] = [];
  for (let i = 2; i < def.length; i += 2) {
    // " R" pair = pen up
    if (def.charCodeAt(i) === 32 && def.charCodeAt(i + 1) === 82) {
      if (cur.length) strokes.push(cur);
      cur = [];
      continue;
    }
    cur.push(
      (def.charCodeAt(i) - 82) * STROKE_FONT_SCALE - startX,
      (def.charCodeAt(i + 1) - 82 + FONT_OFFSET) * STROKE_FONT_SCALE,
    );
  }
  if (cur.length) strokes.push(cur);
  return { strokes, advance: endX - startX };
}

function glyphFor(cp: number): Glyph {
  let idx = cp - 0x20;
  // Outside the table → '?', exactly like KiCad's fallback for unknown glyphs.
  if (idx < 0 || idx >= NEWSTROKE_GLYPHS.length) idx = QUESTION;
  return (glyphCache[idx] ??= decodeGlyph(idx));
}

const SPACE_ADVANCE = decodeGlyph(0).advance; // "JZ" → 16/21

export interface StrokeTextStyle {
  /** Glyph (cap) height in mm — KiCad `(size h w)` first value. */
  size: number;
  /** Glyph width in mm — second `(size h w)` value; defaults to `size`. */
  width?: number;
  /** Pen (stroke) width in mm — KiCad `(font (thickness t))`. */
  thickness?: number;
  bold?: boolean;
  italic?: boolean;
  /** [h, v]: -1 left/top, 0 centre, +1 right/bottom (KiCad semantics). */
  justify: readonly [number, number];
}

/** Effective pen width in mm — EDA_TEXT::GetEffectiveTextPenWidth: the authored
 *  thickness, else width/5 for bold / width/8 for normal (gr_text.cpp uses the glyph
 *  WIDTH), clamped so strokes never exceed 25% of the smaller glyph dimension. */
export function strokeTextPen(s: StrokeTextStyle): number {
  const w = s.width ?? s.size;
  let pen = s.thickness && s.thickness > 0 ? s.thickness : s.bold ? w / 5 : w / 8;
  return Math.min(pen, 0.25 * Math.min(Math.abs(w), Math.abs(s.size)));
}

/** Baseline-to-baseline distance in mm (STROKE_FONT::GetInterline, line spacing 1). */
export function strokeInterline(size: number): number {
  return INTERLINE * size;
}

/** A line's horizontal extent in mm as used for justification: the full cursor
 *  advance. (font.cpp getLinePositions takes boundingBoxSingleLine's CURSOR return,
 *  not the INTER_CHAR-trimmed bbox — verified against kicad-cli plots to the µm.)
 *  Tabs snap to 4-column stops. */
function lineExtent(line: string, w: number): number {
  let cursor = 0;
  let charCount = 0;
  for (const ch of line) {
    const cp = ch.codePointAt(0)!;
    if (cp === 9) {
      charCount = Math.floor(charCount / TAB_WIDTH + 1) * TAB_WIDTH - 1;
      let next = w * charCount + w * SPACE_ADVANCE;
      while (next <= cursor) {
        charCount += TAB_WIDTH;
        next += w * TAB_WIDTH;
      }
      cursor = next;
    } else if (cp === 32) {
      cursor += w * SPACE_ADVANCE;
    } else {
      cursor += glyphFor(cp).advance * w;
    }
    charCount++;
  }
  return cursor;
}

export interface StrokeTextLayout {
  /** Every polyline of every glyph, positioned in text-local mm (anchor 0,0). */
  strokes: number[][];
  /** Ink bounding box including the pen radius on all sides (text-local mm).
   *  Null when the text has no drawable strokes (empty / whitespace only). */
  bbox: { minx: number; miny: number; maxx: number; maxy: number } | null;
  /** Effective pen width (mm). */
  pen: number;
}

/** Lay a (possibly multi-line) text out exactly as KiCad's FONT::Draw +
 *  getLinePositions do, returning positioned polylines ready to stroke. */
export function layoutStrokeText(text: string, s: StrokeTextStyle): StrokeTextLayout {
  const h = s.size;
  const w = s.width ?? s.size;
  const pen = strokeTextPen(s);
  const tilt = s.italic ? ITALIC_TILT : 0;
  const lines = text.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();

  const interline = strokeInterline(h);
  const extents = lines.map((ln) => lineExtent(ln, w));
  const blockHeight = FIRST_LINE_HEIGHT * h + (lines.length - 1) * interline;

  // font.cpp getLinePositions: baseline offset + stroke-font fudge factors.
  const offX = pen / 1.52;
  let offY = h - pen * 0.052;
  if (s.justify[1] === 0) offY -= blockHeight / 2;
  else if (s.justify[1] > 0) offY -= blockHeight;

  const strokes: number[][] = [];
  let minx = Infinity, miny = Infinity, maxx = -Infinity, maxy = -Infinity;

  for (let i = 0; i < lines.length; i++) {
    const lineX =
      s.justify[0] === 0 ? -extents[i] / 2 : s.justify[0] > 0 ? -(extents[i] + offX) : offX;
    const lineY = offY + i * interline;
    let cursor = 0;
    let charCount = 0;
    for (const ch of lines[i]) {
      const cp = ch.codePointAt(0)!;
      if (cp === 9) {
        charCount = Math.floor(charCount / TAB_WIDTH + 1) * TAB_WIDTH - 1;
        let next = w * charCount + w * SPACE_ADVANCE;
        while (next <= cursor) {
          charCount += TAB_WIDTH;
          next += w * TAB_WIDTH;
        }
        cursor = next;
      } else if (cp === 32) {
        cursor += w * SPACE_ADVANCE;
      } else {
        const g = glyphFor(cp);
        for (const src of g.strokes) {
          const out = new Array<number>(src.length);
          for (let p = 0; p < src.length; p += 2) {
            const gy = src[p + 1] * h;
            // STROKE_GLYPH::Transform: scale, then shear x by -y·tilt (italic).
            const gx = lineX + cursor + src[p] * w - gy * tilt;
            const py = lineY + gy;
            out[p] = gx;
            out[p + 1] = py;
            if (gx < minx) minx = gx;
            if (gx > maxx) maxx = gx;
            if (py < miny) miny = py;
            if (py > maxy) maxy = py;
          }
          strokes.push(out);
        }
        cursor += g.advance * w;
      }
      charCount++;
    }
  }

  const r = pen / 2;
  return {
    strokes,
    bbox:
      minx <= maxx
        ? { minx: minx - r, miny: miny - r, maxx: maxx + r, maxy: maxy + r }
        : null,
    pen,
  };
}

/** Knockout margin around the ink bbox — gr_text.h GetKnockoutTextMargin. */
export function knockoutMargin(size: number, pen: number): number {
  return Math.max(pen / 2, size / 9);
}

/** Append a layout's polylines to the current canvas path. The ctx transform must
 *  already map text-local mm to the screen (translate/rotate/mirror/scale). */
export function traceStrokeText(ctx: CanvasRenderingContext2D, layout: StrokeTextLayout): void {
  for (const line of layout.strokes) {
    ctx.moveTo(line[0], line[1]);
    for (let p = 2; p < line.length; p += 2) ctx.lineTo(line[p], line[p + 1]);
  }
}
