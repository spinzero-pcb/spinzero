// Mirrors the Rust geometry IR (`extract/src/ir.rs`, schema `extract.pcb.geometry.a0`).
// The extractor emits this as `pcb/geometry.json`; the GPU renderer uploads it to
// buffers once instead of mounting one SVG-per-layer DOM island. See
// `docs/geometry-ir.md`. All coordinates are board millimetres, Y-down, one shared
// space; primitives reference the layer/net/component tables by integer index.

export interface PcbFrame {
  title: string;
  company: string;
  rev: string;
  date: string;
  version: string;
  comments: string[];
  paper: string;
  /** Board file name for the title block's "File" cell (absent on older bundles). */
  file?: string;
}

export interface PcbLayerDef {
  name: string;
  /** copper | silkscreen | mask | fab | courtyard | paste | edge | user */
  role: string;
  /** front | back | inner (copper); absent for non-sided layers. */
  side?: string;
  /** KiCad stackup ordinal. */
  ord: number;
  /** Resolved #RRGGBB for a user layer; standard layers omit it (theme via CSS vars). */
  color?: string;
}

export interface PcbCompDef {
  ref: string;
  fp: string;
  /** Layer index of the mount side (F.Cu/B.Cu); -1 if unknown. */
  layer: number;
  x: number;
  y: number;
  angle: number;
  dnp?: boolean;
  /** Placed courtyard/graphic extent [x, y, w, h]. */
  bbox?: [number, number, number, number];
  /** KiCad footprint uuid — stable per-instance identity (EPOCH ≥ 19; absent on
   *  legacy caches / Altium). The diff engine pairs instances across revisions by it. */
  uuid?: string;
}

/** Straight track segments: `xy` is [x1,y1,x2,y2] per segment; `w`/`layer`/`net`
 *  carry one entry per segment. */
export interface PcbSegCol {
  xy: number[];
  w: number[];
  layer: number[];
  net: number[];
}

/** Arc tracks: `xy` is [sx,sy,mx,my,ex,ey] (start, mid, end) per arc. */
export interface PcbArcCol {
  xy: number[];
  w: number[];
  layer: number[];
  net: number[];
}

export interface PcbViaDef {
  x: number;
  y: number;
  size: number;
  drill: number;
  net: number;
  /** Copper layer indices the via spans (barrel + hole wall paint on all of them). */
  layers: number[];
  /** Subset of `layers` that keep a full copper annular ring (the via connects there).
   *  On the spanned layers not listed, only the barrel + hole wall are drawn. Absent when
   *  every spanned layer keeps its ring (the common through via). */
  ring?: number[];
}

/** Pad shape codes (mirror of `ir::shape_code`). */
export const PAD_SHAPE = {
  circle: 0,
  rect: 1,
  roundrect: 2,
  oval: 3,
  trapezoid: 4,
  custom: 5,
} as const;

export interface PcbPadDef {
  x: number;
  y: number;
  w: number;
  h: number;
  angle: number;
  /** See {@link PAD_SHAPE}. */
  shape: number;
  rratio?: number;
  drill?: number;
  /** Oval/slot drill: the second dimension H (mm); `drill` holds W. Absent for a round
   *  drill. The hole renders as a stadium of `drill`×`drillh` at the pad `angle`. */
  drillh?: number;
  net: number;
  comp: number;
  num: string;
  /** Layer indices the pad occupies (copper + mask + paste). */
  layers: number[];
  /** Per-pad solder-mask expansion (mm) when overriding the board default. */
  mask?: number;
  /** Non-plated through hole (KiCad `np_thru_hole`): a bare drilled hole, no copper. */
  npth?: boolean;
}

export interface PcbZoneDef {
  layer: number;
  net: number;
  filled: boolean;
  keepout?: boolean;
  /** Polygon ring [x, y, …]. */
  pts: number[];
}

export type PcbGraphicKind = "seg" | "arc" | "circle" | "poly";

/** Board (`gr_*`) and footprint (`fp_*`) graphics, unified and placed to board space.
 *  `data`: seg [x1,y1,x2,y2], arc [sx,sy,mx,my,ex,ey], circle [cx,cy,r], poly [x,y,…].
 *  A rectangle is emitted as a 4-corner poly. */
export interface PcbGraphicDef {
  layer: number;
  width: number;
  kind: PcbGraphicKind;
  data: number[];
  filled?: boolean;
  comp?: number;
}

export interface PcbTextDef {
  layer: number;
  text: string;
  x: number;
  y: number;
  angle: number;
  /** Glyph height (mm). */
  size: number;
  /** Glyph width (mm) for condensed/expanded text; absent ⇒ square glyph cell. */
  width?: number;
  /** KiCad stroke (pen) thickness in mm; absent ⇒ the font's default weight. */
  thickness?: number;
  /** [h, v]: -1 left/top, 0 centre, +1 right/bottom. */
  justify: [number, number];
  mirror?: boolean;
  /** KiCad bold text — drawn with a heavier pen (width/5 instead of width/8). */
  bold?: boolean;
  /** KiCad italic text — glyphs sheared by KiCad's 1/8 tilt. */
  italic?: boolean;
  /** KiCad knockout (inverted) text — filled layer-colour background, glyphs cut out. */
  knockout?: boolean;
  /** Footprint reference/value text is kept upright; board text uses its literal angle. */
  upright?: boolean;
  /** Custom outline-font family (e.g. "Calibri") from KiCad `(font (face …))`; absent ⇒
   *  the KiCad stroke font. When set, the overlay fills glyphs with this family instead
   *  of stroking Newstroke, so the board shows the authored typeface. */
  font?: string;
  comp?: number;
  /** reference | value | user for footprint text; empty/absent for board text. */
  role?: string;
}

export interface PcbGeometry {
  schema: string;
  units: string;
  /** Content extent [x, y, w, h] (board + off-board notes + page), board mm. */
  bbox: [number, number, number, number];
  /** Paper size [w, h] (origin 0,0) when the board declares one. */
  page?: [number, number];
  frame?: PcbFrame;
  layers: PcbLayerDef[];
  /** Net table; index 0 is the empty/no-net sentinel. */
  nets: string[];
  components: PcbCompDef[];
  tracks: { seg: PcbSegCol; arc: PcbArcCol };
  vias: PcbViaDef[];
  pads: PcbPadDef[];
  zones: PcbZoneDef[];
  graphics: PcbGraphicDef[];
  texts: PcbTextDef[];
}

export const GEOMETRY_SCHEMA = "extract.pcb.geometry.a0";

/** Parse the raw IR JSON, validating the schema tag. Throws on a wrong/absent schema
 *  so a stale or mismatched artifact fails loudly rather than rendering garbage. */
export function parsePcbGeometry(text: string): PcbGeometry {
  const g = JSON.parse(text) as PcbGeometry;
  if (g?.schema !== GEOMETRY_SCHEMA) {
    throw new Error(`unexpected PCB geometry schema: ${g?.schema ?? "(none)"}`);
  }
  return g;
}

/** Is this a copper layer (by role, with a name fallback for safety)? */
export function isCopperRole(l: PcbLayerDef): boolean {
  return l.role === "copper" || l.name.endsWith(".Cu");
}
