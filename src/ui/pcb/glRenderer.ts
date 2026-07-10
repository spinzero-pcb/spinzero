// WebGL2 PCB renderer — uploads the geometry IR (see src/lib/pcbGeometry.ts) to GPU
// buffers once and draws it with a single camera uniform, so pan/zoom never re-rasters
// (the SVG-in-DOM view's core problem on high layer counts).
//
// Design for scale:
//  - Each primitive carries its layer/net/component as an attribute. Layer colour and
//    alpha are small uniform arrays indexed by layer id, so showing/hiding a layer or
//    emphasising the active one is a uniform update — no geometry rebuild, one draw call
//    per object class.
//  - Highlight compares each primitive's net/component id against R8 *mask textures*
//    (texelFetch in the fragment shader), so an arbitrary multi-net/-component selection
//    costs one texture upload, not a per-element restyle.
//  - Thick tracks/lines are instanced capsules (round caps via a segment-distance SDF);
//    pads are an instanced rounded-rect SDF (covers rect/roundrect/oval/circle); vias are
//    an instanced annulus; zones/fills are CPU-triangulated (earcut) triangle batches.
//
// Everything is in board coordinates (mm, Y-down); the camera maps world→clip.

import earcut from "earcut";
import type { PcbFrame, PcbGeometry, PcbLayerDef, PcbTextDef } from "../../lib/pcbGeometry";
import { PAD_SHAPE } from "../../lib/pcbGeometry";
import { layerColorVar } from "../../stores/pcbViewStore";
import { resolveCssColor } from "./glColor";

const MAX_LAYERS = 64;
/** Max segments used to flatten an arc track / circle outline. */
const ARC_SEG = 48;
/** Copper plating ring drawn at a plated through-hole's drill edge (mm). KiCad renders the
 *  barrel as a ~0.03 mm ring; we draw it just OUTSIDE the drill so the dark hole keeps the
 *  full extracted drill diameter (feedback: pin5 J1's 1.3 mm hole must not be eaten into). */
const PLATE_RING_MM = 0.03;

export interface Camera {
  /** World point (mm) at the viewport centre. */
  x: number;
  y: number;
  /** CSS pixels per mm. */
  scale: number;
}

export interface BBox {
  minx: number;
  miny: number;
  maxx: number;
  maxy: number;
}

/** Object-class visibility/opacity (mirrors pcbViewStore PCB_OBJECT_KEYS). */
export interface ObjectState {
  objects: Record<string, boolean>;
  opacity: Record<string, number>;
}

/** A net-name label anchored to a piece of net geometry (world mm). The overlay sizes
 *  the text to `w`×`h`, rotates it by `angle`, and gates it by `key`/`layer` visibility. */
export interface NetLabel {
  net: string;
  /** Anchor centre, world mm. */
  x: number;
  y: number;
  /** Extent along / across the text, world mm. */
  w: number;
  h: number;
  /** Text rotation, degrees, normalised to (-90, 90]. */
  angle: number;
  /** Layer indices the labelled object occupies; the label hides only when ALL of them
   *  are hidden — so a pad's net/number shows on its copper AND its mask layers. */
  layers: number[];
  /** Object class gating the label (mirrors PCB_OBJECT_KEYS). */
  key: "pads" | "vias" | "zones" | "tracks";
  /** Pad number, drawn stacked above the net name (pads only). */
  num?: string;
}

/** Board-space text angle for a pad net label: oriented along the placed pad's longer side
 *  and normalised to (-90, 90] so the name reads upright and stays inside the pad rather than
 *  running across it. `angle` is the absolute KiCad pad angle (footprint rotation already
 *  folded in, exactly as the pad shape is drawn); `w`/`h` are the pad's local size. */
export function padLabelAngle(angle: number, w: number, h: number): number {
  // Run the name along the pad's longer local side (cross to local-Y, +90°, when it is taller
  // than wide) — KiCad's rule: whichever pad dimension is larger sets the text direction. This
  // must agree with the overlay's budget (alongPx = max(w,h)); a hysteresis here would let a
  // mildly-tall pad keep horizontal text while the budget assumed vertical, overflowing the pad.
  let a = angle + (h > w ? 90 : 0);
  a = ((a % 180) + 180) % 180; // → [0, 180)
  return a > 90 ? a - 180 : a; // → (-90, 90]
}

/** One text row of a net label: the string, its font size (px) and its centre offset along
 *  the label's across-axis (px), before any width-fit shrink. */
export interface LabelRow {
  text: string;
  size: number;
  cy: number;
}

/** Lay out a net label's rows for the Canvas2D overlay (`acrossPx` = the body's short
 *  on-screen extent). A pad stacks a LARGE pad number over a SMALLER net name — each centred
 *  in its half of the pad, the number at ~1.3× the name, matching KiCad; every other class
 *  (and a pad missing one of the two) shows a single centred net name. Sizes are tuned to fill
 *  the copper (bigger than before, per feedback) while staying within the body's short side;
 *  only tracks stay capped so a wide power track never gets giant letters. Pad/via text is
 *  uncapped so it grows as you zoom in. The overlay then width-fits each row to the body's long
 *  side, so a label never exceeds the pad/track/via — it just shrinks (and drops below ~4px). */
export function netLabelRows(lab: Pick<NetLabel, "key" | "num" | "net">, acrossPx: number): LabelRow[] {
  const cap = lab.key === "tracks" ? 22 : Infinity;
  if (lab.key === "pads" && lab.num && lab.net) {
    return [
      { text: lab.num, size: Math.min(acrossPx * 0.34, cap), cy: -acrossPx * 0.24 },
      { text: lab.net, size: Math.min(acrossPx * 0.26, cap), cy: acrossPx * 0.27 },
    ];
  }
  const only = lab.key === "pads" && lab.num ? lab.num : lab.net;
  if (!only) return [];
  // Single centred label: fill most of the pad's short side, vias unchanged, tracks nearly the
  // full track width (capped). Bigger than the old 0.42/0.82 so pad & track names read clearly.
  const f = lab.key === "vias" ? 0.42 : lab.key === "pads" ? 0.6 : 0.92;
  return [{ text: only, size: Math.min(acrossPx * f, cap), cy: 0 }];
}

/** RGB components in 0..1. */
export type RGB = [number, number, number];

/** Per-primitive "changed" flags for diff mode (visual-diff plan §4), one value per IR
 *  primitive in IR order: 0 = unchanged (present on both sides); non-zero = this
 *  primitive is NOT present on the other side (removed when the geometry is the A side,
 *  added when it is B), and the value encodes which semantic change owns it (see
 *  glDiff.ts: 1 = orphan, k+2 = change k). Computed by `glDiff.ts` from a content-hash
 *  set-diff of the two revisions' geometry IRs and baked into the GPU batches as a
 *  per-instance attribute at build time; per-change show/hide is a cheap visibility-
 *  mask texture update (`setDiffVisibility`), never a rebuild. */
export interface DiffFlags {
  seg: Uint16Array;
  arc: Uint16Array;
  vias: Uint16Array;
  pads: Uint16Array;
  zones: Uint16Array;
  graphics: Uint16Array;
  /** Entries in the visibility mask (DIFF_OWNED_BASE + change count, min 2). */
  maskSize: number;
}

/** Options for a single `render` call in diff mode. Default = the normal single-
 *  revision draw (clear + full palette + holes). */
export interface RenderOpts {
  /** Clear the framebuffer first (default true). Compare modes composite several
   *  passes, so the later passes pass false. */
  clear?: boolean;
  /** 0 = normal; 1 = "base" pass: draw ONLY unchanged primitives, dimmed; 2 =
   *  "changed-only" pass: draw ONLY changed primitives recoloured in `diffColor`. */
  diffPass?: 0 | 1 | 2;
  /** Recolour for pass 2 (removed = red from A's buffers, added = green from B's). */
  diffColor?: RGB;
  /** Pass-2 tint mix: the fraction of the primitive's OWN layer colour kept in the
   *  changed-copper tint (0 = flat diffColor, ~0.4 keeps the layer identity readable
   *  while red/green still dominates). Default 0. */
  diffMix?: number;
  /** Draw drill holes (default true; the changed-only overlay passes skip them). */
  holes?: boolean;
}
/** A highlighted net/component: its IR index plus the colour. `emphasize` keeps the
 *  primitive's own layer colour (a selected net, undimmed) instead of recolouring it in
 *  `color` (a pinned "highlight in colour"). */
export interface HotId {
  id: number;
  color: RGB;
  emphasize?: boolean;
}

// ----------------------------------------------------------------- shaders

const CAMERA_UNIFORMS = `
uniform vec2 uResolution;   // device px
uniform float uScale;       // device px per mm
uniform vec2 uCenter;       // world mm at viewport centre
uniform float uAA;          // world mm per device px (AA feather)
vec4 toClip(vec2 world){
  vec2 screen = (world - uCenter) * uScale + uResolution * 0.5;
  vec2 clip = screen / uResolution * 2.0 - 1.0;
  return vec4(clip.x, -clip.y, 0.0, 1.0);
}
`;

// Shared fragment epilogue: layer colour/alpha + selection dim/highlight.
const FRAG_COMMON = `
precision highp float;
uniform float uAA;          // world mm per device px (AA feather), shared with the VS
uniform vec3 uLayerColor[${MAX_LAYERS}];
uniform float uLayerAlpha[${MAX_LAYERS}];
uniform float uObjAlpha;
uniform float uSel;         // 1 when a selection is active
uniform float uDim;         // how far to fade unselected toward uDimColor
uniform float uDimAlpha;    // alpha multiplier for unselected
uniform vec3 uDimColor;
uniform sampler2D uNetMask;   // RGBA: rgb = highlight colour, a > 0.5 when highlighted
uniform sampler2D uCompMask;  // RGBA, same convention
uniform int uActiveLayer;   // active (selected) layer index, or -1
uniform int uLayerMode;     // 0 = all, 1 = skip active layer, 2 = only active layer
// Two-pass layer ordering: pass 1 paints every non-active layer, pass 2 repaints the
// active layer on top so it reads as fully on top (no bleed-through from other layers).
bool cullLayer(int layer){
  if (uLayerMode == 1) return layer == uActiveLayer;
  if (uLayerMode == 2) return layer != uActiveLayer;
  return false;
}
// A primitive's highlight texel: rgb = colour, a encodes the MODE — a≈1.0 recolour (a pinned
// "highlight in colour"), a≈0.6 emphasise (a selected net keeps its own layer colour, just
// undimmed), a≈0 not highlighted. Net wins over component.
vec4 hotColor(float net, float comp){
  if (net >= 0.5) { vec4 t = texelFetch(uNetMask, ivec2(int(net + 0.5), 0), 0); if (t.a > 0.25) return t; }
  if (comp >= 0.0) { vec4 t = texelFetch(uCompMask, ivec2(int(comp + 0.5), 0), 0); if (t.a > 0.25) return t; }
  return vec4(0.0);
}
// Diff compare passes (visual-diff §4): 0 = off; 1 = base (unchanged only, painted a
// flat neutral grey — fully greyed out, not hue-dimmed — so only the red/green changed
// copper carries colour); 2 = changed-only (tinted in uDiffColor, mixed with the
// primitive's own layer colour by uDiffMix so the layer identity stays readable).
// The flag attribute encodes WHICH change owns a changed primitive (glDiff.ts:
// 0 unchanged, 1 orphan, k+2 = change k); uDiffVis gates each code per frame, so
// hiding/soloing changes never rebuilds the batches. A hidden-changed primitive
// draws as unchanged grey in pass 1 (it exists on this side — just not spotlit).
uniform int uDiffPass;
uniform vec3 uDiffColor;
uniform vec3 uDiffBase;      // flat grey for the unchanged base (whitish/blackish)
uniform float uDiffMix;      // pass-2 fraction of the layer colour kept in the tint
uniform sampler2D uDiffVis;  // a > 0.5 at texel [flag] = this change is shown
vec4 shade(vec3 baseColor, float layerAlpha, float coverage, float net, float comp, float flag){
  bool changed = flag > 0.5 && texelFetch(uDiffVis, ivec2(int(flag + 0.5), 0), 0).a > 0.5;
  if (uDiffPass == 1 && changed) discard;      // base pass: changed prims are the overlay's job
  if (uDiffPass == 2) {
    if (!changed) discard;                     // changed-only pass: everything else skipped
    float ac = layerAlpha * uObjAlpha * coverage;
    if (ac < 0.003) discard;
    return vec4(mix(uDiffColor, baseColor, uDiffMix), ac);
  }
  vec3 col = baseColor;
  float a = layerAlpha * uObjAlpha;
  if (uDiffPass == 1) { col = uDiffBase; a *= 0.85; } // unchanged copper → flat grey
  if (uSel > 0.5) {
    vec4 hot = hotColor(net, comp);
    if (hot.a > 0.25) {
      // Highlighted, so NOT dimmed: recolour in the pinned colour (a≈1.0), or — for a
      // selected net (a≈0.6) — keep this primitive's own layer colour so the net reads in
      // its copper layers rather than one flat blue.
      if (hot.a > 0.75) col = hot.rgb;
    } else {
      col = mix(col, uDimColor, uDim); a *= uDimAlpha;  // everything else fades back
    }
  }
  a *= coverage;
  if (a < 0.003) discard;
  return vec4(col, a);
}
`;

// --- lines (instanced capsule) ---
const LINE_VS = `#version 300 es
layout(location=0) in vec2 aCorner;   // unit quad [-1,1]^2
layout(location=1) in vec2 aA;
layout(location=2) in vec2 aB;
layout(location=3) in float aHW;
layout(location=4) in float aLayer;
layout(location=5) in float aNet;
layout(location=6) in float aComp;
layout(location=7) in float aFlag;
${CAMERA_UNIFORMS}
out vec2 vWorld; out vec2 vA; out vec2 vB; out float vHW;
flat out int vLayer; out float vNet; out float vComp; out float vFlag;
void main(){
  vFlag = aFlag;
  vec2 d = aB - aA; float len = length(d);
  vec2 u = len > 1e-6 ? d / len : vec2(1.0, 0.0);
  vec2 v = vec2(-u.y, u.x);
  vec2 c = (aA + aB) * 0.5;
  float halfLen = len * 0.5 + aHW + uAA;
  float halfW = aHW + uAA;
  vec2 world = c + u * (aCorner.x * halfLen) + v * (aCorner.y * halfW);
  vWorld = world; vA = aA; vB = aB; vHW = aHW;
  vLayer = int(aLayer + 0.5); vNet = aNet; vComp = aComp;
  gl_Position = toClip(world);
}`;

const LINE_FS = `#version 300 es
${FRAG_COMMON}
in vec2 vWorld; in vec2 vA; in vec2 vB; in float vHW;
flat in int vLayer; in float vNet; in float vComp; in float vFlag;
out vec4 frag;
void main(){
  if (cullLayer(vLayer)) discard;
  vec2 pa = vWorld - vA; vec2 ba = vB - vA;
  float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-9), 0.0, 1.0);
  float dist = length(pa - ba * h);
  float cov = 1.0 - smoothstep(vHW - uAA, vHW + uAA, dist);
  if (cov <= 0.0) discard;
  frag = shade(uLayerColor[vLayer], uLayerAlpha[vLayer], cov, vNet, vComp, vFlag);
}`;

// --- pads (instanced rounded-rect SDF) ---
const PAD_VS = `#version 300 es
layout(location=0) in vec2 aCorner;
layout(location=1) in vec2 aCenter;
layout(location=2) in vec2 aHalf;
layout(location=3) in float aAngle;   // radians (board Y-down handedness)
layout(location=4) in float aRadius;
layout(location=5) in float aLayer;
layout(location=6) in float aNet;
layout(location=7) in float aComp;
layout(location=8) in float aFlag;
${CAMERA_UNIFORMS}
out vec2 vLocal; out vec2 vHalf; out float vRadius;
flat out int vLayer; out float vNet; out float vComp; out float vFlag;
void main(){
  vFlag = aFlag;
  float ca = cos(aAngle), sa = sin(aAngle);
  vec2 local = aCorner * (aHalf + uAA);
  // Board space is Y-down: x' = lx*cos + ly*sin, y' = -lx*sin + ly*cos (matches place_fp).
  vec2 world = aCenter + vec2(local.x * ca + local.y * sa, -local.x * sa + local.y * ca);
  vLocal = local; vHalf = aHalf; vRadius = aRadius;
  vLayer = int(aLayer + 0.5); vNet = aNet; vComp = aComp;
  gl_Position = toClip(world);
}`;

const PAD_FS = `#version 300 es
${FRAG_COMMON}
in vec2 vLocal; in vec2 vHalf; in float vRadius;
flat in int vLayer; in float vNet; in float vComp; in float vFlag;
out vec4 frag;
void main(){
  if (cullLayer(vLayer)) discard;
  vec2 q = abs(vLocal) - (vHalf - vRadius);
  float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - vRadius;
  float cov = 1.0 - smoothstep(-uAA, uAA, d);
  if (cov <= 0.0) discard;
  frag = shade(uLayerColor[vLayer], uLayerAlpha[vLayer], cov, vNet, vComp, vFlag);
}`;

// --- vias (instanced annulus, fixed metal colour) ---
const VIA_VS = `#version 300 es
layout(location=0) in vec2 aCorner;
layout(location=1) in vec2 aCenter;
layout(location=2) in float aOuter;
layout(location=3) in float aDrill;
layout(location=4) in float aLayer;
layout(location=5) in float aNet;
layout(location=6) in float aRing;    // 1 = full annular ring on this layer, 0 = barrel + wall only
layout(location=7) in float aFlag;
${CAMERA_UNIFORMS}
out vec2 vLocal; out float vOuter; out float vDrill;
flat out int vLayer; out float vNet; flat out int vRing; out float vFlag;
void main(){
  vFlag = aFlag;
  vec2 local = aCorner * (aOuter + uAA);
  vLocal = local; vOuter = aOuter; vDrill = aDrill;
  vLayer = int(aLayer + 0.5); vNet = aNet; vRing = int(aRing + 0.5);
  gl_Position = toClip(aCenter + local);
}`;

const VIA_FS = `#version 300 es
${FRAG_COMMON}
uniform vec3 uViaColor;   // plated barrel — exposed copper/gold (the via centre)
uniform vec3 uViaHole;    // hole-wall ring (KiCad via_hole_walls): a light ring at the drill edge
uniform float uForceBarrel; // 1 = a mask/paste/silk layer is active: drop the copper ring, show barrel + wall only
in vec2 vLocal; in float vOuter; in float vDrill;
flat in int vLayer; in float vNet; flat in int vRing; in float vFlag;
out vec4 frag;
void main(){
  if (cullLayer(vLayer)) discard;
  float r = length(vLocal);
  float t = max(uAA, vDrill * 0.08);                                  // hole-wall half-width
  float wall = 1.0 - smoothstep(t - uAA, t + uAA, abs(r - vDrill));   // 1 on the ring, 0 off it
  if (vRing == 0 || uForceBarrel > 0.5) {
    // Not connected on this layer: no copper annular ring — draw only the golden plated
    // barrel inside the drill and the light hole wall at the drill edge (KiCad
    // remove-unused-layers). Coverage stops at the wall so lower layers read through.
    float edge = vDrill + t;
    float cov = 1.0 - smoothstep(edge - uAA, edge + uAA, r);
    if (cov <= 0.0) discard;
    // The via barrel is physical — it's exposed on the solder mask/paste/silk. When one of
    // those layers is being viewed (uForceBarrel), show the barrel at full opacity even if
    // the copper layers this via instance rides are hidden — otherwise it vanishes on a
    // mask/paste/silk-only view (every via instance is keyed to a copper layer). shade()
    // multiplies this by uObjAlpha, so pass 1.0 (not uObjAlpha) here.
    float la = uForceBarrel > 0.5 ? 1.0 : uLayerAlpha[vLayer];
    frag = shade(mix(uViaColor, uViaHole, wall), la, cov, vNet, -1.0, vFlag);
    return;
  }
  // Solid disc — no centre hole; the plated barrel fills the middle (matches KiCad).
  float cov = 1.0 - smoothstep(vOuter - uAA, vOuter + uAA, r);
  if (cov <= 0.0) discard;
  // Three concentric bands (matches KiCad): a golden plated barrel inside the drill, a
  // light hole-wall ring straddling the drill edge, and the copper annular pad (this
  // layer's colour) outside it.
  vec3 col = mix(uViaColor, uLayerColor[vLayer], smoothstep(vDrill - uAA, vDrill + uAA, r));
  col = mix(col, uViaHole, wall);
  frag = shade(col, uLayerAlpha[vLayer], cov, vNet, -1.0, vFlag);
}`;

// --- drill holes (instanced rounded-rect SDF: a circle for a round drill, a stadium
//     for an oval/slot drill; NPTH blue vs plated drill colour) ---
const HOLE_VS = `#version 300 es
layout(location=0) in vec2 aCorner;
layout(location=1) in vec2 aCenter;
layout(location=2) in vec2 aHalf;     // half (w,h) — equal for a round drill
layout(location=3) in float aAngle;   // radians (board Y-down handedness, matches pads)
layout(location=4) in float aNpth;    // 1 = non-plated (blue), 0 = plated drill
${CAMERA_UNIFORMS}
out vec2 vLocal; out vec2 vHalf; out float vNpth;
void main(){
  float ca = cos(aAngle), sa = sin(aAngle);
  // Plated holes carry a thin gold plating ring drawn just OUTSIDE the drill edge (so the
  // dark hole keeps the full drill diameter); grow the quad by that ring width + AA feather.
  float ringW = max(${PLATE_RING_MM.toFixed(3)}, uAA);
  vec2 local = aCorner * (aHalf + ringW + uAA);
  vec2 world = aCenter + vec2(local.x * ca + local.y * sa, -local.x * sa + local.y * ca);
  vLocal = local; vHalf = aHalf; vNpth = aNpth;
  gl_Position = toClip(world);
}`;

const HOLE_FS = `#version 300 es
precision highp float;
uniform float uAA;
uniform float uObjAlpha;
uniform vec3 uDrillColor;   // plated through-hole drill (the dark, empty hole)
uniform vec3 uNpthColor;    // non-plated through hole (blue/teal)
uniform vec3 uViaColor;     // plated barrel — exposed copper/gold (the thin plating ring)
in vec2 vLocal; in vec2 vHalf; in float vNpth;
out vec4 frag;
void main(){
  float rad = min(vHalf.x, vHalf.y);          // full round caps on the short axis
  vec2 q = abs(vLocal) - (vHalf - rad);
  float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - rad;   // signed SDF: 0 at the drill edge, <0 inside
  // NPTH: a bare drilled hole, no plating — solid teal filling exactly the drill.
  if (vNpth > 0.5) {
    float covN = 1.0 - smoothstep(-uAA, uAA, d);
    if (covN * uObjAlpha < 0.003) discard;
    frag = vec4(uNpthColor, uObjAlpha * covN);
    return;
  }
  // Plated hole — identical on copper AND mask/paste/silk (KiCad shows a plated through
  // hole the same way on every layer): a dark, empty circle of the FULL drill diameter
  // wrapped by a thin gold plating ring drawn just OUTSIDE the drill edge, so the ring
  // never eats into the hole. Ring width is KiCad's ~0.03 mm, clamped to ≥1 px so it stays
  // visible when zoomed out.
  float ringW = max(${PLATE_RING_MM.toFixed(3)}, uAA);
  float cov = 1.0 - smoothstep(ringW - uAA, ringW + uAA, d);       // drill + the ring band
  if (cov * uObjAlpha < 0.003) discard;
  float wall = smoothstep(0.0, uAA, d);                            // 0 inside the drill, 1 in the ring
  vec3 col = mix(uDrillColor, uViaColor, wall);                    // black hole + thin gold ring
  frag = vec4(col, uObjAlpha * cov);
}`;

// --- triangles (zones / filled graphics) ---
const TRI_VS = `#version 300 es
layout(location=0) in vec2 aPos;
layout(location=1) in float aLayer;
layout(location=2) in float aNet;
layout(location=3) in float aComp;
layout(location=4) in float aFlag;
${CAMERA_UNIFORMS}
flat out int vLayer; out float vNet; out float vComp; out float vFlag;
void main(){
  vLayer = int(aLayer + 0.5); vNet = aNet; vComp = aComp; vFlag = aFlag;
  gl_Position = toClip(aPos);
}`;

const TRI_FS = `#version 300 es
${FRAG_COMMON}
flat in int vLayer; in float vNet; in float vComp; in float vFlag;
out vec4 frag;
void main(){
  if (cullLayer(vLayer)) discard;
  frag = shade(uLayerColor[vLayer], uLayerAlpha[vLayer], 1.0, vNet, vComp, vFlag);
}`;

// ----------------------------------------------------------------- GL helpers

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const sh = gl.createShader(type)!;
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(sh);
    gl.deleteShader(sh);
    throw new Error(`shader compile failed: ${log}`);
  }
  return sh;
}

function link(gl: WebGL2RenderingContext, vs: string, fs: string): WebGLProgram {
  const p = gl.createProgram()!;
  const v = compile(gl, gl.VERTEX_SHADER, vs);
  const f = compile(gl, gl.FRAGMENT_SHADER, fs);
  gl.attachShader(p, v);
  gl.attachShader(p, f);
  gl.linkProgram(p);
  // Once linked, the program keeps its own copy — detach + delete the shaders so they
  // don't leak for the (reused) context's lifetime across design switches. The info log
  // stays valid after deletion.
  gl.detachShader(p, v);
  gl.detachShader(p, f);
  gl.deleteShader(v);
  gl.deleteShader(f);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    const log = gl.getProgramInfoLog(p);
    gl.deleteProgram(p);
    throw new Error(`program link failed: ${log}`);
  }
  return p;
}

interface AttrSpec {
  loc: number;
  size: number;
  /** float offset within the per-item stride. */
  offset: number;
}

// ----------------------------------------------------------------- batches

/** A GPU batch: an interleaved float buffer + the attribute layout + item count. */
interface Batch {
  vao: WebGLVertexArrayObject;
  buffer: WebGLBuffer;
  /** instance count (instanced) or vertex count (tris). */
  count: number;
  instanced: boolean;
}

const LINE_STRIDE = 9; // ax,ay,bx,by,hw,layer,net,comp,flag
const PAD_STRIDE = 10; // cx,cy,hx,hy,angle,radius,layer,net,comp,flag
const VIA_STRIDE = 8; // cx,cy,outer,drill,layer,net,ring,flag
const TRI_STRIDE = 6; // x,y,layer,net,comp,flag
const HOLE_STRIDE = 6; // cx,cy,hx,hy,angle,npth

/** Mutable arrays accumulated while walking the IR, one per GPU batch. */
class Accum {
  trackLines: number[] = [];
  fpLines: number[] = [];
  boardLines: number[] = [];
  pads: number[] = [];
  vias: number[] = [];
  holes: number[] = [];
  zoneTris: number[] = [];
  fpFills: number[] = [];
  boardFills: number[] = [];
  bbox: BBox = { minx: Infinity, miny: Infinity, maxx: -Infinity, maxy: -Infinity };
  grow(x: number, y: number) {
    const b = this.bbox;
    if (x < b.minx) b.minx = x;
    if (y < b.miny) b.miny = y;
    if (x > b.maxx) b.maxx = x;
    if (y > b.maxy) b.maxy = y;
  }
  pushLine(
    arr: number[],
    ax: number,
    ay: number,
    bx: number,
    by: number,
    hw: number,
    layer: number,
    net: number,
    comp: number,
    flag = 0,
  ) {
    arr.push(ax, ay, bx, by, hw, layer, net, comp, flag);
    this.grow(ax, ay);
    this.grow(bx, by);
  }
}

/** Sample an arc through start/mid/end into a polyline (board Y-down space). */
function arcPolyline(
  sx: number,
  sy: number,
  mx: number,
  my: number,
  ex: number,
  ey: number,
): number[] {
  const det = 2 * (sx * (my - ey) + mx * (ey - sy) + ex * (sy - my));
  if (Math.abs(det) < 1e-9) return [sx, sy, mx, my, ex, ey]; // collinear → chord
  const s2 = sx * sx + sy * sy;
  const m2 = mx * mx + my * my;
  const e2 = ex * ex + ey * ey;
  const cx = (s2 * (my - ey) + m2 * (ey - sy) + e2 * (sy - my)) / det;
  const cy = (s2 * (ex - mx) + m2 * (sx - ex) + e2 * (mx - sx)) / det;
  const r = Math.hypot(sx - cx, sy - cy);
  const TAU = Math.PI * 2;
  const ang = (px: number, py: number) => Math.atan2(py - cy, px - cx);
  const norm = (a: number) => ((a % TAU) + TAU) % TAU;
  const a0 = ang(sx, sy);
  const span = norm(ang(ex, ey) - a0);
  const midRel = norm(ang(mx, my) - a0);
  // Direction the mid point implies (matches the Rust arc sweep choice).
  const ccw = midRel <= span;
  const sweep = ccw ? span : span - TAU;
  const steps = Math.max(2, Math.min(ARC_SEG, Math.ceil((Math.abs(sweep) / TAU) * ARC_SEG)));
  const out: number[] = [];
  for (let i = 0; i <= steps; i++) {
    const a = a0 + (sweep * i) / steps;
    out.push(cx + r * Math.cos(a), cy + r * Math.sin(a));
  }
  return out;
}

/** Append connected segments of a polyline as line instances. */
function polylineToLines(
  acc: Accum,
  arr: number[],
  pts: number[],
  hw: number,
  layer: number,
  net: number,
  comp: number,
  close: boolean,
  flag = 0,
) {
  const n = pts.length / 2;
  for (let i = 0; i + 1 < n; i++) {
    acc.pushLine(arr, pts[i * 2], pts[i * 2 + 1], pts[i * 2 + 2], pts[i * 2 + 3], hw, layer, net, comp, flag);
  }
  if (close && n > 2) {
    acc.pushLine(arr, pts[(n - 1) * 2], pts[(n - 1) * 2 + 1], pts[0], pts[1], hw, layer, net, comp, flag);
  }
}

/** Triangulate a polygon ring (flat [x,y,…]) and push triangles into `arr`. */
function fillPolygon(
  acc: Accum,
  arr: number[],
  pts: number[],
  layer: number,
  net: number,
  comp: number,
  flag = 0,
) {
  if (pts.length < 6) return;
  const tris = earcut(pts);
  for (const idx of tris) {
    const x = pts[idx * 2];
    const y = pts[idx * 2 + 1];
    arr.push(x, y, layer, net, comp, flag);
    acc.grow(x, y);
  }
}

/** A filled circle as a triangle fan. */
function fillCircle(
  acc: Accum,
  arr: number[],
  cx: number,
  cy: number,
  r: number,
  layer: number,
  net: number,
  comp: number,
  flag = 0,
) {
  const steps = 32;
  for (let i = 0; i < steps; i++) {
    const a0 = (i / steps) * Math.PI * 2;
    const a1 = ((i + 1) / steps) * Math.PI * 2;
    arr.push(cx, cy, layer, net, comp, flag);
    arr.push(cx + r * Math.cos(a0), cy + r * Math.sin(a0), layer, net, comp, flag);
    arr.push(cx + r * Math.cos(a1), cy + r * Math.sin(a1), layer, net, comp, flag);
  }
  acc.grow(cx - r, cy - r);
  acc.grow(cx + r, cy + r);
}

/** Squared distance from (px,py) to segment (ax,ay)-(bx,by). */
function segDist2(px: number, py: number, ax: number, ay: number, bx: number, by: number): number {
  const dx = bx - ax;
  const dy = by - ay;
  const l2 = dx * dx + dy * dy;
  const t = l2 > 1e-12 ? Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / l2)) : 0;
  const cx = ax + t * dx;
  const cy = ay + t * dy;
  return (px - cx) ** 2 + (py - cy) ** 2;
}

/** Even-odd point-in-polygon test for a flat [x,y,…] ring. */
function pointInPoly(px: number, py: number, pts: number[]): boolean {
  let inside = false;
  const n = pts.length / 2;
  for (let i = 0, j = n - 1; i < n; j = i++) {
    const xi = pts[i * 2];
    const yi = pts[i * 2 + 1];
    const xj = pts[j * 2];
    const yj = pts[j * 2 + 1];
    if (yi > py !== yj > py && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

function padRadius(shape: number, w: number, h: number, rratio: number): number {
  switch (shape) {
    case PAD_SHAPE.circle:
      return Math.min(w, h) / 2;
    case PAD_SHAPE.oval:
      return Math.min(w, h) / 2;
    case PAD_SHAPE.roundrect:
      return rratio * Math.min(w, h);
    default:
      return 0; // rect / trapezoid / custom → sharp (custom approximated as rect)
  }
}

// ----------------------------------------------------------------- renderer

export class PcbGlRenderer {
  private gl: WebGL2RenderingContext;
  private geom: PcbGeometry;
  private progLine: WebGLProgram;
  private progPad: WebGLProgram;
  private progVia: WebGLProgram;
  private progTri: WebGLProgram;
  private progHole: WebGLProgram;
  private quad: WebGLBuffer;
  private batches: Record<string, Batch> = {};
  private netMask: WebGLTexture;
  private compMask: WebGLTexture;
  private contentBbox: BBox;

  // Index maps for selection / hit-test.
  readonly netIndexByName = new Map<string, number>();
  readonly compIndexByRef = new Map<string, number>();
  /** Layer names by index (the renderer's layer table). */
  readonly layerNames: string[] = [];
  /** Copper layer ids (for "which layers a net is on" defaulting). */
  readonly copperLayers: number[] = [];

  // Live state.
  private layerColors = new Float32Array(MAX_LAYERS * 3);
  private layerAlpha = new Float32Array(MAX_LAYERS);
  private viaCopper: [number, number, number] = [0.79, 0.66, 0.24];
  private viaHole: [number, number, number] = [0.93, 0.93, 0.93];
  private drillColor: [number, number, number] = [0.08, 0.09, 0.11];
  private npthColor: [number, number, number] = [0.102, 0.769, 0.824]; // --pcb-npth fallback (#1ac4d2)
  /** Flat grey for the unchanged base in diff mode (--pcb-diff-base). */
  private diffBase: [number, number, number] = [0.545, 0.561, 0.596]; // #8b8f98 fallback
  private dpr = 1;
  private selActive = false;
  /** Count of highlighted nets + components in the current mask (the "marked" probe
   *  field): how many distinct objects setSelection last lit, not primitive count. */
  private selNetCount = 0;
  private selCompCount = 0;
  /** Active layer index (painted on top) and the current draw pass. See cullLayer in
   *  the shaders: -1 / mode 0 means "no active layer, draw everything in one pass". */
  private activeLayerIdx = -1;
  private layerMode = 0;
  /** Net-name labels, computed once from the IR (the overlay culls/sizes per frame). */
  private labels: NetLabel[] | null = null;

  /** Per-primitive changed flags for diff mode (baked into the batches); null = all 0. */
  private diffFlags: DiffFlags | null = null;
  /** Per-change visibility mask texture (see DiffFlags/setDiffVisibility). */
  private diffVis: WebGLTexture;
  private diffVisSize: number;
  /** Diff-mode copper stack fade (top layers more translucent); reapplied by setLayerState. */
  private diffFade = false;

  constructor(gl: WebGL2RenderingContext, geom: PcbGeometry, diffFlags?: DiffFlags) {
    this.gl = gl;
    this.geom = geom;
    this.diffFlags = diffFlags ?? null;
    this.progLine = link(gl, LINE_VS, LINE_FS);
    this.progPad = link(gl, PAD_VS, PAD_FS);
    this.progVia = link(gl, VIA_VS, VIA_FS);
    this.progTri = link(gl, TRI_VS, TRI_FS);
    this.progHole = link(gl, HOLE_VS, HOLE_FS);

    this.quad = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.quad);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);

    geom.nets.forEach((n, i) => this.netIndexByName.set(n, i));
    geom.components.forEach((c, i) => this.compIndexByRef.set(c.ref, i));
    geom.layers.forEach((l, i) => {
      this.layerNames.push(l.name);
      if (l.role === "copper" || l.name.endsWith(".Cu")) this.copperLayers.push(i);
    });

    const acc = this.build(geom);
    this.contentBbox = acc.bbox.minx <= acc.bbox.maxx ? acc.bbox : { minx: 0, miny: 0, maxx: 100, maxy: 100 };
    // Include the drawing sheet (page rect at 0,0..pw,ph) so fit frames the whole worksheet
    // — the frame + title block read on open, not just the board (feedback 31.PNG).
    if (geom.page) {
      const [pw, ph] = geom.page;
      const b = this.contentBbox;
      this.contentBbox = {
        minx: Math.min(b.minx, 0), miny: Math.min(b.miny, 0),
        maxx: Math.max(b.maxx, pw), maxy: Math.max(b.maxy, ph),
      };
    }
    this.uploadBatches(acc);

    this.netMask = this.makeMask(Math.max(1, geom.nets.length));
    this.compMask = this.makeMask(Math.max(1, geom.components.length));
    // Diff visibility mask: one texel per flag code, default everything shown. A plain
    // (non-diff) renderer keeps the tiny default texture — flags are all 0, never sampled.
    this.diffVisSize = Math.max(2, diffFlags?.maskSize ?? 2);
    this.diffVis = this.makeMask(this.diffVisSize);
    this.setDiffVisibility(null);

    this.resolveColors();

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.disable(gl.DEPTH_TEST);
  }

  /** Walk the IR into per-batch interleaved arrays. */
  private build(geom: PcbGeometry): Accum {
    const acc = new Accum();
    const clampLayer = (l: number) => (l >= 0 && l < MAX_LAYERS ? l : 0);
    const df = this.diffFlags;
    // The flag is the owner CODE (glDiff encoding), not a boolean — pass it through.
    const flagOf = (arr: Uint16Array | undefined, i: number) => (arr ? arr[i] : 0);

    // tracks: straight segments
    const seg = geom.tracks.seg;
    for (let i = 0; i < seg.w.length; i++) {
      acc.pushLine(
        acc.trackLines,
        seg.xy[i * 4], seg.xy[i * 4 + 1], seg.xy[i * 4 + 2], seg.xy[i * 4 + 3],
        seg.w[i] / 2, clampLayer(seg.layer[i]), seg.net[i], -1,
        flagOf(df?.seg, i),
      );
    }
    // tracks: arcs → flattened polylines
    const arc = geom.tracks.arc;
    for (let i = 0; i < arc.w.length; i++) {
      const poly = arcPolyline(
        arc.xy[i * 6], arc.xy[i * 6 + 1], arc.xy[i * 6 + 2], arc.xy[i * 6 + 3], arc.xy[i * 6 + 4], arc.xy[i * 6 + 5],
      );
      polylineToLines(acc, acc.trackLines, poly, arc.w[i] / 2, clampLayer(arc.layer[i]), arc.net[i], -1, false, flagOf(df?.arc, i));
    }

    // graphics: board (no comp) vs footprint (comp set)
    for (let gi = 0; gi < geom.graphics.length; gi++) {
      const g = geom.graphics[gi];
      const flag = flagOf(df?.graphics, gi);
      const layer = clampLayer(g.layer);
      const comp = g.comp ?? -1;
      const lineArr = comp >= 0 ? acc.fpLines : acc.boardLines;
      const fillArr = comp >= 0 ? acc.fpFills : acc.boardFills;
      const hw = Math.max(g.width, 0.01) / 2;
      const d = g.data;
      if (g.kind === "seg") {
        acc.pushLine(lineArr, d[0], d[1], d[2], d[3], hw, layer, 0, comp, flag);
      } else if (g.kind === "arc") {
        const poly = arcPolyline(d[0], d[1], d[2], d[3], d[4], d[5]);
        polylineToLines(acc, lineArr, poly, hw, layer, 0, comp, false, flag);
      } else if (g.kind === "circle") {
        if (g.filled) fillCircle(acc, fillArr, d[0], d[1], d[2], layer, 0, comp, flag);
        else {
          const poly: number[] = [];
          for (let i = 0; i <= ARC_SEG; i++) {
            const a = (i / ARC_SEG) * Math.PI * 2;
            poly.push(d[0] + d[2] * Math.cos(a), d[1] + d[2] * Math.sin(a));
          }
          polylineToLines(acc, lineArr, poly, hw, layer, 0, comp, false, flag);
        }
      } else if (g.kind === "poly") {
        if (g.filled) fillPolygon(acc, fillArr, d, layer, 0, comp, flag);
        else polylineToLines(acc, lineArr, d, hw, layer, 0, comp, true, flag);
      }
    }

    // pads (expanded across the copper layers each pad sits on). A non-plated through
    // hole has no copper — it paints as a bare hole only (no pad disc).
    for (let pi = 0; pi < geom.pads.length; pi++) {
      const p = geom.pads[pi];
      const pFlag = flagOf(df?.pads, pi);
      const ang = (p.angle * Math.PI) / 180;
      if (!p.npth) {
        const baseR = padRadius(p.shape, p.w, p.h, p.rratio ?? 0);
        const m = p.mask ?? 0; // per-pad solder-mask expansion (board default is 0)
        // A through-hole pad (has a drill) is shown on the mask/paste/silk layers as JUST its
        // drilled hole — the dark hole + thin gold plating ring drawn by the hole batch, so it
        // matches its F.Cu appearance — with no pad aperture (feedback). SMD pads (no drill)
        // keep their mask/paste apertures.
        const isTht = (p.drill ?? 0) > 0;
        // A pad shows on its copper layers (copper size) AND its mask layers, where the
        // aperture is the copper grown by the mask margin on each side — a mask opening
        // is NOT the pad, it's usually larger (mirrors KiCad / the SVG view's emit_pad).
        for (const layer of p.layers.length ? p.layers : [0]) {
          const role = geom.layers[layer]?.role;
          if (role === "mask") {
            if (isTht) continue;
            const mw = Math.max(p.w + 2 * m, 0.05), mh = Math.max(p.h + 2 * m, 0.05);
            const mr = baseR > 0 ? Math.max(baseR + m, 0) : 0;
            acc.pads.push(p.x, p.y, mw / 2, mh / 2, ang, mr, clampLayer(layer), p.net, p.comp, pFlag);
          } else if (role === "paste") {
            if (isTht) continue;
            // Solder-paste aperture: the pad copper footprint (KiCad's default paste margin is
            // 0; a per-pad paste margin isn't carried in the IR). Same shape/size as the copper.
            acc.pads.push(p.x, p.y, p.w / 2, p.h / 2, ang, baseR, clampLayer(layer), p.net, p.comp, pFlag);
          } else if (this.isCopperLayerIdx(geom, layer)) {
            acc.pads.push(p.x, p.y, p.w / 2, p.h / 2, ang, baseR, clampLayer(layer), p.net, p.comp, pFlag);
          }
        }
      }
      // Drill hole: plated pads show a dark drilled hole, NPTH a blue one. An oval/slot
      // drill (drillh set) paints as a stadium of drill×drillh at the pad angle; a round
      // drill is the degenerate equal-half case (angle ignored).
      if (p.drill && p.drill > 0) {
        const npth = p.npth ? 1 : 0;
        const hh = p.drillh && p.drillh > 0 ? p.drillh : p.drill;
        acc.holes.push(p.x, p.y, p.drill / 2, hh / 2, p.drillh && p.drillh > 0 ? ang : 0, npth);
      }
      acc.grow(p.x - p.w, p.y - p.h);
      acc.grow(p.x + p.w, p.y + p.h);
    }

    // vias: one instance PER spanned copper layer (a through via lists every copper
    // layer), so it shows on every layer it passes through — cullLayer keys each
    // instance to its own layer — not just the top. Each instance flags whether it keeps
    // a full annular ring on that layer: on the layers a via doesn't connect to (KiCad
    // "remove unused layers") the shader draws only the barrel + hole wall, no ring.
    // Identical concentric discs otherwise, so stacking visible layers is harmless.
    for (let vi = 0; vi < geom.vias.length; vi++) {
      const v = geom.vias[vi];
      const vFlag = flagOf(df?.vias, vi);
      const layers = v.layers.length ? v.layers : [0];
      const ring = v.ring ? new Set(v.ring) : null; // null → every spanned layer keeps its ring
      for (const layer of layers) {
        const hasRing = !ring || ring.has(layer) ? 1 : 0;
        acc.vias.push(v.x, v.y, v.size / 2, v.drill / 2, clampLayer(layer), v.net, hasRing, vFlag);
      }
      acc.grow(v.x - v.size, v.y - v.size);
      acc.grow(v.x + v.size, v.y + v.size);
    }

    // zones → triangles
    for (let zi = 0; zi < geom.zones.length; zi++) {
      const z = geom.zones[zi];
      const zFlag = flagOf(df?.zones, zi);
      if (!z.filled) {
        // keepout / unfilled: outline only
        polylineToLines(acc, acc.boardLines, z.pts, 0.06, clampLayer(z.layer), z.net, -1, true, zFlag);
      } else {
        fillPolygon(acc, acc.zoneTris, z.pts, clampLayer(z.layer), z.net, -1, zFlag);
      }
    }

    return acc;
  }

  private isCopperLayerIdx(geom: PcbGeometry, idx: number): boolean {
    const l = geom.layers[idx];
    return !!l && (l.role === "copper" || l.name.endsWith(".Cu"));
  }

  // -------------------------------------------------------------- buffer setup

  private makeBatch(data: number[], stride: number, attrs: AttrSpec[], instanced: boolean): Batch {
    const gl = this.gl;
    const vao = gl.createVertexArray()!;
    gl.bindVertexArray(vao);
    if (instanced) {
      // location 0 = unit quad (per-vertex)
      gl.bindBuffer(gl.ARRAY_BUFFER, this.quad);
      gl.enableVertexAttribArray(0);
      gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
      gl.vertexAttribDivisor(0, 0);
    }
    const buffer = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(data), gl.STATIC_DRAW);
    const strideBytes = stride * 4;
    for (const a of attrs) {
      gl.enableVertexAttribArray(a.loc);
      gl.vertexAttribPointer(a.loc, a.size, gl.FLOAT, false, strideBytes, a.offset * 4);
      if (instanced) gl.vertexAttribDivisor(a.loc, 1);
    }
    gl.bindVertexArray(null);
    const count = instanced ? data.length / stride : data.length / stride;
    return { vao, buffer, count, instanced };
  }

  private uploadBatches(acc: Accum) {
    const lineAttrs: AttrSpec[] = [
      { loc: 1, size: 2, offset: 0 },
      { loc: 2, size: 2, offset: 2 },
      { loc: 3, size: 1, offset: 4 },
      { loc: 4, size: 1, offset: 5 },
      { loc: 5, size: 1, offset: 6 },
      { loc: 6, size: 1, offset: 7 },
      { loc: 7, size: 1, offset: 8 }, // aFlag
    ];
    const padAttrs: AttrSpec[] = [
      { loc: 1, size: 2, offset: 0 },
      { loc: 2, size: 2, offset: 2 },
      { loc: 3, size: 1, offset: 4 },
      { loc: 4, size: 1, offset: 5 },
      { loc: 5, size: 1, offset: 6 },
      { loc: 6, size: 1, offset: 7 },
      { loc: 7, size: 1, offset: 8 },
      { loc: 8, size: 1, offset: 9 }, // aFlag
    ];
    const viaAttrs: AttrSpec[] = [
      { loc: 1, size: 2, offset: 0 },
      { loc: 2, size: 1, offset: 2 },
      { loc: 3, size: 1, offset: 3 },
      { loc: 4, size: 1, offset: 4 },
      { loc: 5, size: 1, offset: 5 },
      { loc: 6, size: 1, offset: 6 },
      { loc: 7, size: 1, offset: 7 }, // aFlag
    ];
    const triAttrs: AttrSpec[] = [
      { loc: 0, size: 2, offset: 0 },
      { loc: 1, size: 1, offset: 2 },
      { loc: 2, size: 1, offset: 3 },
      { loc: 3, size: 1, offset: 4 },
      { loc: 4, size: 1, offset: 5 }, // aFlag
    ];
    const holeAttrs: AttrSpec[] = [
      { loc: 1, size: 2, offset: 0 }, // aCenter
      { loc: 2, size: 2, offset: 2 }, // aHalf
      { loc: 3, size: 1, offset: 4 }, // aAngle
      { loc: 4, size: 1, offset: 5 }, // aNpth
    ];
    this.batches.trackLines = this.makeBatch(acc.trackLines, LINE_STRIDE, lineAttrs, true);
    this.batches.fpLines = this.makeBatch(acc.fpLines, LINE_STRIDE, lineAttrs, true);
    this.batches.boardLines = this.makeBatch(acc.boardLines, LINE_STRIDE, lineAttrs, true);
    this.batches.pads = this.makeBatch(acc.pads, PAD_STRIDE, padAttrs, true);
    this.batches.vias = this.makeBatch(acc.vias, VIA_STRIDE, viaAttrs, true);
    this.batches.holes = this.makeBatch(acc.holes, HOLE_STRIDE, holeAttrs, true);
    this.batches.zoneTris = this.makeBatch(acc.zoneTris, TRI_STRIDE, triAttrs, false);
    this.batches.fpFills = this.makeBatch(acc.fpFills, TRI_STRIDE, triAttrs, false);
    this.batches.boardFills = this.makeBatch(acc.boardFills, TRI_STRIDE, triAttrs, false);
  }

  private makeMask(width: number): WebGLTexture {
    const gl = this.gl;
    const tex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, width, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(width * 4));
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    return tex;
  }

  // -------------------------------------------------------------- public state

  get bbox(): BBox {
    return this.contentBbox;
  }

  /** (Re)resolve every layer colour from the live CSS theme (call on theme change). */
  resolveColors() {
    const { geom } = this;
    for (let i = 0; i < geom.layers.length && i < MAX_LAYERS; i++) {
      const l: PcbLayerDef = geom.layers[i];
      const [r, g, b] = resolveCssColor(layerColorVar(l.name, l.color));
      this.layerColors[i * 3] = r;
      this.layerColors[i * 3 + 1] = g;
      this.layerColors[i * 3 + 2] = b;
    }
    // Via barrel is a theme colour (the golden plated centre); the annular pad carries
    // the layer colour in the shader.
    this.viaCopper = resolveCssColor("var(--pcb-via-copper)");
    this.viaHole = resolveCssColor("var(--pcb-hole)");
    this.drillColor = resolveCssColor("var(--pcb-drill)");
    this.npthColor = resolveCssColor("var(--pcb-npth)");
    this.diffBase = resolveCssColor("var(--pcb-diff-base)");
  }

  /** Recompute per-layer alpha + the active-layer index from the hidden set + active
   *  layer. The active layer is painted on top (two-pass; see render) at full opacity;
   *  other visible layers are NOT dimmed — they read through where the active layer has
   *  no copper. With no active layer, copper sits at 75% for a natural board wash.
   *  `diffFade` (diff mode, multi-layer overlay) fades the visible copper stack top →
   *  bottom so upper layers read as translucent sheets and lower copper shows through. */
  setLayerState(hidden: Set<string>, active: string | null, diffFade = this.diffFade) {
    const { geom } = this;
    this.diffFade = diffFade;
    this.activeLayerIdx = -1;
    for (let i = 0; i < MAX_LAYERS; i++) this.layerAlpha[i] = 0;
    const visCopper: number[] = []; // visible copper layers, table (front → back) order
    for (let i = 0; i < geom.layers.length && i < MAX_LAYERS; i++) {
      const l = geom.layers[i];
      if (l.name === active) this.activeLayerIdx = i;
      if (hidden.has(l.name)) continue;
      const copper = l.role === "copper" || l.name.endsWith(".Cu");
      this.layerAlpha[i] = !active && copper ? 0.75 : 1;
      if (copper) visCopper.push(i);
    }
    if (diffFade && visCopper.length > 1) {
      // Top (front) copper most translucent, bottom opaque: 0.45 → 0.95 linear ramp.
      for (let k = 0; k < visCopper.length; k++) {
        this.layerAlpha[visCopper[k]] = 0.45 + (0.5 * k) / (visCopper.length - 1);
      }
    }
  }

  /** Upload the per-change visibility mask (glDiff.buildDiffVisibility): one 0/1 entry
   *  per flag code. `null` = everything shown. Cheap (a tiny texSubImage2D) — this is
   *  how show/hide/solo of individual changes works without touching the batches. */
  setDiffVisibility(vis: Uint8Array | null) {
    const gl = this.gl;
    const buf = new Uint8Array(this.diffVisSize * 4);
    for (let i = 0; i < this.diffVisSize; i++) {
      const on = vis ? (vis[i] ?? 0) !== 0 : true;
      buf[i * 4 + 3] = on ? 255 : 0; // the shader reads .a only
    }
    gl.bindTexture(gl.TEXTURE_2D, this.diffVis);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, this.diffVisSize, 1, gl.RGBA, gl.UNSIGNED_BYTE, buf);
  }

  /** Set the highlighted nets/components and the colour to repaint each in (by IR index).
   *  The shader paints these in their colour and fades everything else back. Empty → none. */
  setSelection(nets: Iterable<HotId>, comps: Iterable<HotId>) {
    const gl = this.gl;
    const nbuf = new Uint8Array(Math.max(1, this.geom.nets.length) * 4);
    const cbuf = new Uint8Array(Math.max(1, this.geom.components.length) * 4);
    let any = false;
    let netCount = 0;
    let compCount = 0;
    const put = (buf: Uint8Array, id: number, color: RGB, emphasize: boolean) => {
      const o = id * 4;
      buf[o] = (color[0] * 255) | 0;
      buf[o + 1] = (color[1] * 255) | 0;
      buf[o + 2] = (color[2] * 255) | 0;
      // Alpha carries the highlight MODE: 255 = recolour in `color`; 153 (≈0.6) = emphasise,
      // i.e. keep the primitive's own layer colour but don't dim it (a selected net).
      buf[o + 3] = emphasize ? 153 : 255;
      any = true;
    };
    for (const { id, color, emphasize } of nets) if (id > 0 && id * 4 < nbuf.length) { put(nbuf, id, color, emphasize ?? false); netCount++; }
    for (const { id, color, emphasize } of comps) if (id >= 0 && id * 4 < cbuf.length) { put(cbuf, id, color, emphasize ?? false); compCount++; }
    gl.bindTexture(gl.TEXTURE_2D, this.netMask);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, nbuf.length / 4, 1, gl.RGBA, gl.UNSIGNED_BYTE, nbuf);
    gl.bindTexture(gl.TEXTURE_2D, this.compMask);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, cbuf.length / 4, 1, gl.RGBA, gl.UNSIGNED_BYTE, cbuf);
    this.selActive = any;
    this.selNetCount = netCount;
    this.selCompCount = compCount;
  }

  /** Whether a selection is active — i.e. the shader dims unselected primitives (the
   *  `dimsel` probe field / the SVG view's scrim). */
  get dimActive(): boolean {
    return this.selActive;
  }

  /** How many nets + components the current highlight mask lights (the `marked` probe
   *  field). Mask-derived object count, not a primitive count. */
  get markedCount(): number {
    return this.selNetCount + this.selCompCount;
  }

  setDpr(dpr: number) {
    this.dpr = dpr;
  }

  /** Resolve a board-space point to what the user clicked, honouring layer
   *  visibility. Type priority (feedback): track/via (→ net) > pad (→ pin) >
   *  component (→ courtyard) > zone (→ net). Hidden layers are ignored entirely; among
   *  the VISIBLE layers, when a tier has candidates on several of them the one on the
   *  ACTIVE layer wins ("find the best way to select when multiple layers are active"),
   *  else the first visible hit. Returns names, not indices. */
  hitTest(
    wx: number,
    wy: number,
    layerVisible: (idx: number) => boolean,
  ): { net?: string; comp?: string; pad?: string } | null {
    const g = this.geom;
    const active = this.activeLayerIdx; // -1 when no active layer → nothing prefers it
    const netName = (i: number) => (i > 0 && i < g.nets.length ? g.nets[i] : undefined);
    const compRef = (i: number) => (i >= 0 && i < g.components.length ? g.components[i].ref : undefined);

    // --- 1) net conductors: tracks (segments + arcs) and vias → net. Thin, hard-to-hit
    //     targets, so they win over the larger pad / component / zone bodies. A hit on the
    //     active layer returns immediately; otherwise the first visible hit is the fallback. ---
    let netHit: { net: string } | null = null;
    const seg = g.tracks.seg;
    for (let i = 0; i < seg.w.length; i++) {
      if (!layerVisible(seg.layer[i])) continue;
      const hw = seg.w[i] / 2;
      if (
        segDist2(wx, wy, seg.xy[i * 4], seg.xy[i * 4 + 1], seg.xy[i * 4 + 2], seg.xy[i * 4 + 3]) <=
        hw * hw
      ) {
        const n = netName(seg.net[i]);
        if (n) {
          if (seg.layer[i] === active) return { net: n };
          netHit ??= { net: n };
        }
      }
    }
    // Tracks (arcs) — flatten to the same polyline the renderer draws and distance-test
    // each chord segment, so a curved trace (common on RF/motor boards) is clickable too.
    const tarc = g.tracks.arc;
    for (let i = 0; i < tarc.w.length; i++) {
      if (!layerVisible(tarc.layer[i])) continue;
      const hw = tarc.w[i] / 2;
      const poly = arcPolyline(
        tarc.xy[i * 6], tarc.xy[i * 6 + 1], tarc.xy[i * 6 + 2],
        tarc.xy[i * 6 + 3], tarc.xy[i * 6 + 4], tarc.xy[i * 6 + 5],
      );
      for (let j = 0; j + 3 < poly.length; j += 2) {
        if (segDist2(wx, wy, poly[j], poly[j + 1], poly[j + 2], poly[j + 3]) <= hw * hw) {
          const n = netName(tarc.net[i]);
          if (n) {
            if (tarc.layer[i] === active) return { net: n };
            netHit ??= { net: n };
          }
          break;
        }
      }
    }
    // Vias (net conductors like tracks — a PCB_VIA is a PCB_TRACK in KiCad).
    for (const v of g.vias) {
      if (!v.layers.some((l) => layerVisible(l))) continue;
      const r = v.size / 2;
      if ((wx - v.x) ** 2 + (wy - v.y) ** 2 <= r * r) {
        const n = netName(v.net);
        if (n) {
          if (v.layers.some((l) => l === active)) return { net: n };
          netHit ??= { net: n };
        }
      }
    }
    if (netHit) return netHit;

    // --- 2) pads → pin (cross-probe). Hittable only where one of the pad's COPPER layers
    //     is visible; an active-copper-layer pad wins over one on another visible layer. ---
    let padHit: { net?: string; comp?: string; pad?: string } | null = null;
    for (const p of g.pads) {
      if (!p.layers.some((l) => this.isCopperLayerIdx(g, l) && layerVisible(l))) continue;
      const a = (p.angle * Math.PI) / 180;
      const ca = Math.cos(a);
      const sa = Math.sin(a);
      const dx = wx - p.x;
      const dy = wy - p.y;
      const lx = ca * dx - sa * dy;
      const ly = sa * dx + ca * dy;
      if (Math.abs(lx) <= p.w / 2 && Math.abs(ly) <= p.h / 2) {
        const hit = { comp: compRef(p.comp), pad: p.num, net: netName(p.net) };
        if (p.layers.some((l) => l === active && this.isCopperLayerIdx(g, l))) return hit;
        padHit ??= hit;
      }
    }
    if (padHit) return padHit;

    // --- 3) footprint courtyard / bbox → the component. The SMALLEST enclosing bbox wins,
    //     so a small part sitting inside a large connector's courtyard stays selectable. ---
    let comp: { ref: string; area: number } | null = null;
    for (const c of g.components) {
      if (!c.bbox) continue;
      const [bx, by, bw, bh] = c.bbox;
      if (wx >= bx && wx <= bx + bw && wy >= by && wy <= by + bh) {
        const area = bw * bh;
        if (!comp || area < comp.area) comp = { ref: c.ref, area };
      }
    }
    if (comp) return { comp: comp.ref };

    // --- 4) zones (filled pours) → net, last: they blanket large areas of the board. ---
    let zoneHit: { net: string } | null = null;
    for (const z of g.zones) {
      if (!z.filled || !layerVisible(z.layer)) continue;
      if (pointInPoly(wx, wy, z.pts)) {
        const n = netName(z.net);
        if (n) {
          if (z.layer === active) return { net: n };
          zoneHit ??= { net: n };
        }
      }
    }
    return zoneHit;
  }

  /** Board-space bounding box of every primitive on a net, for camera landing on a
   *  schematic→PCB cross-probe. Null when the net carries no geometry. */
  netBBox(name: string): BBox | null {
    const ni = this.netIndexByName.get(name);
    if (ni == null || ni <= 0) return null;
    const g = this.geom;
    let minx = Infinity, miny = Infinity, maxx = -Infinity, maxy = -Infinity;
    const add = (x: number, y: number) => {
      if (x < minx) minx = x;
      if (y < miny) miny = y;
      if (x > maxx) maxx = x;
      if (y > maxy) maxy = y;
    };
    const seg = g.tracks.seg;
    for (let i = 0; i < seg.w.length; i++) {
      if (seg.net[i] !== ni) continue;
      add(seg.xy[i * 4], seg.xy[i * 4 + 1]);
      add(seg.xy[i * 4 + 2], seg.xy[i * 4 + 3]);
    }
    const arc = g.tracks.arc;
    for (let i = 0; i < arc.w.length; i++) {
      if (arc.net[i] !== ni) continue;
      add(arc.xy[i * 6], arc.xy[i * 6 + 1]);
      add(arc.xy[i * 6 + 4], arc.xy[i * 6 + 5]);
    }
    for (const p of g.pads) if (p.net === ni) add(p.x, p.y);
    for (const v of g.vias) if (v.net === ni) add(v.x, v.y);
    for (const z of g.zones) {
      if (z.net !== ni) continue;
      for (let i = 0; i < z.pts.length; i += 2) add(z.pts[i], z.pts[i + 1]);
    }
    return minx <= maxx ? { minx, miny, maxx, maxy } : null;
  }

  /** Placed bounding box of a component (courtyard/graphic extent), for landing a
   *  camera or anchoring a comment chip. Null when the ref is unknown or has no bbox. */
  compBBox(ref: string): BBox | null {
    const i = this.compIndexByRef.get(ref);
    if (i == null) return null;
    const b = this.geom.components[i]?.bbox;
    if (!b) return null;
    return { minx: b[0], miny: b[1], maxx: b[0] + b[2], maxy: b[1] + b[3] };
  }

  /** Net-name labels for every net-bearing pad, via, filled zone, and longer track
   *  segment (computed once; the geometry is static). The overlay decides which are
   *  legible at the current zoom and draws them. */
  netLabels(): NetLabel[] {
    if (this.labels) return this.labels;
    const g = this.geom;
    const out: NetLabel[] = [];
    // KiCad net names carry a leading sheet-path slash (e.g. "/DRIVER_LO_V"); strip it for
    // display (matches the SVG view), keeping the raw name only if it is nothing but slashes.
    const netName = (i: number) => {
      if (i <= 0 || i >= g.nets.length) return "";
      return g.nets[i].replace(/^\/+/, "") || g.nets[i];
    };

    for (const p of g.pads) {
      if (p.npth) continue;
      const n = netName(p.net);
      if (!n && !p.num) continue;
      // Orient along the placed pad's longer side so the name (and number) sit inside the pad
      // rather than spilling past it — e.g. J5.9 (0.75×2.05 @90°) reads horizontally, not down.
      // The label is gated by all the pad's layers, so it shows on its copper AND mask layers.
      out.push({ net: n, x: p.x, y: p.y, w: p.w, h: p.h, angle: padLabelAngle(p.angle, p.w, p.h), layers: p.layers.length ? p.layers : [0], key: "pads", num: p.num });
    }
    for (const v of g.vias) {
      const n = netName(v.net);
      if (!n) continue;
      out.push({ net: n, x: v.x, y: v.y, w: v.size, h: v.size, angle: 0, layers: v.layers.length ? v.layers : [0], key: "vias" });
    }
    // Zones are intentionally NOT labelled: a net name over a big GND/ISO pour reads as
    // clutter, not information (user feedback). Pads/vias/tracks carry the net instead.
    const seg = g.tracks.seg;
    for (let i = 0; i < seg.w.length; i++) {
      const n = netName(seg.net[i]);
      if (!n) continue;
      const ax = seg.xy[i * 4], ay = seg.xy[i * 4 + 1], bx = seg.xy[i * 4 + 2], by = seg.xy[i * 4 + 3];
      const len = Math.hypot(bx - ax, by - ay);
      if (len < 1.5) continue; // skip stubs — too short to letter
      let ang = (Math.atan2(by - ay, bx - ax) * 180) / Math.PI;
      if (ang > 90) ang -= 180;
      if (ang <= -90) ang += 180;
      out.push({ net: n, x: (ax + bx) / 2, y: (ay + by) / 2, w: len, h: seg.w[i], angle: ang, layers: [seg.layer[i]], key: "tracks" });
    }
    this.labels = out;
    return out;
  }

  /** Board + footprint text (silk, fab, board notes, dimension labels) for the
   *  Canvas2D overlay — the GL pipeline draws no glyphs. */
  get texts(): readonly PcbTextDef[] {
    return this.geom.texts;
  }

  /** Drawing-sheet page size `[w, h]` (mm) when the board declares one — the worksheet
   *  overlay draws the frame + title block over the page rect at (0,0)..(w,h). */
  get page(): readonly [number, number] | undefined {
    return this.geom.page;
  }

  /** Title-block fields for the worksheet overlay (undefined when the board has no page). */
  get frame(): PcbFrame | undefined {
    return this.geom.frame;
  }

  /** A layer's resolved colour as a CSS `rgb()` string (for Canvas2D text). */
  layerColorCss(idx: number): string {
    const i = idx >= 0 && idx < MAX_LAYERS ? idx : 0;
    const r = Math.round(this.layerColors[i * 3] * 255);
    const g = Math.round(this.layerColors[i * 3 + 1] * 255);
    const b = Math.round(this.layerColors[i * 3 + 2] * 255);
    return `rgb(${r}, ${g}, ${b})`;
  }

  // -------------------------------------------------------------- draw

  private setCameraUniforms(prog: WebGLProgram, cam: Camera, w: number, h: number) {
    const gl = this.gl;
    const scale = cam.scale * this.dpr;
    gl.uniform2f(gl.getUniformLocation(prog, "uResolution"), w, h);
    gl.uniform1f(gl.getUniformLocation(prog, "uScale"), scale);
    gl.uniform2f(gl.getUniformLocation(prog, "uCenter"), cam.x, cam.y);
    gl.uniform1f(gl.getUniformLocation(prog, "uAA"), 1 / Math.max(scale, 1e-6));
  }

  /** Live diff-pass state for the current render call (set by render(opts)). */
  private diffPass: 0 | 1 | 2 = 0;
  private diffColor: RGB = [1, 0, 0];
  private diffMix = 0;

  private setCommonUniforms(prog: WebGLProgram, objAlpha: number) {
    const gl = this.gl;
    gl.uniform1i(gl.getUniformLocation(prog, "uDiffPass"), this.diffPass);
    gl.uniform3f(gl.getUniformLocation(prog, "uDiffColor"), this.diffColor[0], this.diffColor[1], this.diffColor[2]);
    gl.uniform3f(gl.getUniformLocation(prog, "uDiffBase"), this.diffBase[0], this.diffBase[1], this.diffBase[2]);
    gl.uniform1f(gl.getUniformLocation(prog, "uDiffMix"), this.diffMix);
    gl.uniform1i(gl.getUniformLocation(prog, "uDiffVis"), 2);
    gl.uniform3fv(gl.getUniformLocation(prog, "uLayerColor"), this.layerColors);
    gl.uniform1fv(gl.getUniformLocation(prog, "uLayerAlpha"), this.layerAlpha);
    gl.uniform1f(gl.getUniformLocation(prog, "uObjAlpha"), objAlpha);
    gl.uniform1f(gl.getUniformLocation(prog, "uSel"), this.selActive ? 1 : 0);
    gl.uniform1f(gl.getUniformLocation(prog, "uDim"), 0.82);
    gl.uniform1f(gl.getUniformLocation(prog, "uDimAlpha"), 0.4);
    gl.uniform3f(gl.getUniformLocation(prog, "uDimColor"), 0.07, 0.08, 0.1);
    gl.uniform1i(gl.getUniformLocation(prog, "uNetMask"), 0);
    gl.uniform1i(gl.getUniformLocation(prog, "uCompMask"), 1);
    gl.uniform1i(gl.getUniformLocation(prog, "uActiveLayer"), this.activeLayerIdx);
    gl.uniform1i(gl.getUniformLocation(prog, "uLayerMode"), this.layerMode);
  }

  private drawInstanced(prog: WebGLProgram, batch: Batch, cam: Camera, w: number, h: number, objAlpha: number) {
    if (batch.count === 0 || objAlpha <= 0) return;
    const gl = this.gl;
    gl.useProgram(prog);
    this.setCameraUniforms(prog, cam, w, h);
    this.setCommonUniforms(prog, objAlpha);
    gl.bindVertexArray(batch.vao);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, batch.count);
  }

  private drawTris(batch: Batch, cam: Camera, w: number, h: number, objAlpha: number) {
    if (batch.count === 0 || objAlpha <= 0) return;
    const gl = this.gl;
    gl.useProgram(this.progTri);
    this.setCameraUniforms(this.progTri, cam, w, h);
    this.setCommonUniforms(this.progTri, objAlpha);
    gl.bindVertexArray(batch.vao);
    gl.drawArrays(gl.TRIANGLES, 0, batch.count);
  }

  /** True when the active layer is a solder-mask / paste / silk layer. On those a via drops
   *  its copper annular ring and shows just the plated barrel (gold fill + light wall),
   *  matching KiCad's non-copper layer views. Plated holes render the same on every layer
   *  (dark hole + thin gold ring), so they no longer key off this. */
  private barrelLayerActive(): boolean {
    if (this.activeLayerIdx < 0) return false;
    const role = this.geom.layers[this.activeLayerIdx]?.role;
    return role === "mask" || role === "paste" || role === "silkscreen";
  }

  /** Whether through-board features (drill holes AND vias) should be drawn at all: only
   *  when at least one layer they're exposed on — copper / mask / paste / silkscreen — is
   *  visible. They span the board, so any one such visible layer shows them; when every one
   *  is hidden they must vanish too (feedback: PTH/NPTH holes and via barrels floated over a
   *  blank board with all layers off — the via barrel is force-drawn on a mask/paste/silk
   *  active layer even while hidden). */
  private exposedLayerVisible(): boolean {
    const { geom } = this;
    for (let i = 0; i < geom.layers.length && i < MAX_LAYERS; i++) {
      if (this.layerAlpha[i] <= 0) continue; // hidden layer
      const l = geom.layers[i];
      const role = l.role;
      if (
        role === "copper" || role === "mask" || role === "paste" || role === "silkscreen" ||
        l.name.endsWith(".Cu") || /\.(Mask|Paste|SilkS|Silkscreen)$/i.test(l.name)
      ) {
        return true;
      }
    }
    return false;
  }

  /** Drill holes are physical (span every layer), so they paint once on top of the
   *  pads regardless of the active layer — no layer cull, no per-layer alpha. */
  private drawHoles(cam: Camera, w: number, h: number, objAlpha: number) {
    const batch = this.batches.holes;
    if (batch.count === 0 || objAlpha <= 0) return;
    const gl = this.gl;
    const prog = this.progHole;
    gl.useProgram(prog);
    this.setCameraUniforms(prog, cam, w, h);
    gl.uniform1f(gl.getUniformLocation(prog, "uObjAlpha"), objAlpha);
    gl.uniform3f(gl.getUniformLocation(prog, "uDrillColor"), this.drillColor[0], this.drillColor[1], this.drillColor[2]);
    gl.uniform3f(gl.getUniformLocation(prog, "uNpthColor"), this.npthColor[0], this.npthColor[1], this.npthColor[2]);
    // The thin gold plating ring drawn at the drill edge — the same on every layer, so a PTH
    // reads like its F.Cu appearance (gold ring + dark hole) on mask/paste/silk too.
    gl.uniform3f(gl.getUniformLocation(prog, "uViaColor"), this.viaCopper[0], this.viaCopper[1], this.viaCopper[2]);
    gl.bindVertexArray(batch.vao);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, batch.count);
  }

  /** Render one frame. `objState` carries the per-class visibility + opacity.
   *  `opts` drives the diff compare passes (see RenderOpts); omitted = normal draw. */
  render(cam: Camera, w: number, h: number, objState: ObjectState, opts?: RenderOpts) {
    const gl = this.gl;
    this.diffPass = opts?.diffPass ?? 0;
    this.diffMix = opts?.diffMix ?? 0;
    if (opts?.diffColor) this.diffColor = opts.diffColor;
    gl.viewport(0, 0, w, h);
    if (opts?.clear !== false) {
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
    }

    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.netMask);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.compMask);
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, this.diffVis);

    if (this.activeLayerIdx >= 0) {
      // Pass 1: every other layer underneath; pass 2: the active layer on top.
      this.layerMode = 1;
      this.drawScene(cam, w, h, objState);
      this.layerMode = 2;
      this.drawScene(cam, w, h, objState);
    } else {
      this.layerMode = 0;
      this.drawScene(cam, w, h, objState);
    }

    // Drill holes ride the "pads" class and paint last so the hole reads on top — but only
    // when a layer they're exposed on (copper/mask/paste/silk) is visible, so they don't
    // float over a board with every layer turned off. Holes carry no changed-flag, so the
    // changed-only overlay pass (2) skips them; the base/normal passes draw them.
    const wantHoles = (opts?.holes ?? true) && this.diffPass !== 2;
    if (wantHoles && objState.objects["pads"] !== false && this.exposedLayerVisible()) {
      this.drawHoles(cam, w, h, objState.opacity["pads"] ?? 1);
    }
    this.diffPass = 0; // never leak a compare pass into the next plain render call
  }

  /** Draw every object class once for the current `layerMode` (the shaders cull by
   *  layer per pass). Painter's order: zones → tracks → pads → vias → fp art → board art. */
  private drawScene(cam: Camera, w: number, h: number, objState: ObjectState) {
    const gl = this.gl;
    const on = (k: string) => objState.objects[k] !== false;
    const op = (k: string) => objState.opacity[k] ?? 1;

    if (on("zones")) this.drawTris(this.batches.zoneTris, cam, w, h, op("zones"));
    if (on("tracks")) this.drawInstanced(this.progLine, this.batches.trackLines, cam, w, h, op("tracks"));
    if (on("pads")) this.drawInstanced(this.progPad, this.batches.pads, cam, w, h, op("pads"));
    // Vias are through-board like drill holes: only draw them when a layer they're exposed
    // on (copper/mask/paste/silk) is visible, so a via barrel doesn't float over a board
    // with every layer off (its barrel is force-drawn on a mask/paste/silk active layer).
    if (on("vias") && this.exposedLayerVisible()) {
      gl.useProgram(this.progVia);
      gl.uniform3f(gl.getUniformLocation(this.progVia, "uViaColor"), this.viaCopper[0], this.viaCopper[1], this.viaCopper[2]);
      gl.uniform3f(gl.getUniformLocation(this.progVia, "uViaHole"), this.viaHole[0], this.viaHole[1], this.viaHole[2]);
      // On a mask/paste/silk layer, drop the copper ring — show barrel + wall only (item 4).
      gl.uniform1f(gl.getUniformLocation(this.progVia, "uForceBarrel"), this.barrelLayerActive() ? 1 : 0);
      this.drawInstanced(this.progVia, this.batches.vias, cam, w, h, op("vias"));
    }
    if (on("footprints")) {
      this.drawTris(this.batches.fpFills, cam, w, h, op("footprints"));
      this.drawInstanced(this.progLine, this.batches.fpLines, cam, w, h, op("footprints"));
    }
    // Board graphics (edge cuts, dimensions, board notes) ride the "text" class toggle.
    if (on("text")) {
      this.drawTris(this.batches.boardFills, cam, w, h, op("text"));
      this.drawInstanced(this.progLine, this.batches.boardLines, cam, w, h, op("text"));
    }
  }

  dispose() {
    const gl = this.gl;
    for (const b of Object.values(this.batches)) {
      gl.deleteVertexArray(b.vao);
      gl.deleteBuffer(b.buffer);
    }
    gl.deleteBuffer(this.quad);
    gl.deleteTexture(this.netMask);
    gl.deleteTexture(this.compMask);
    gl.deleteTexture(this.diffVis);
    gl.deleteProgram(this.progLine);
    gl.deleteProgram(this.progPad);
    gl.deleteProgram(this.progVia);
    gl.deleteProgram(this.progTri);
    gl.deleteProgram(this.progHole);
  }
}
