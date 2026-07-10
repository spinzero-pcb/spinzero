// Per-primitive changed-flag computation for the PCB visual diff (plan §4).
//
// Identity mirrors the Rust diff engine (src-tauri/src/diff.rs `hash_prim`): a
// primitive is "the same" across two revisions when its content tuple matches with
// layer/net indices dereferenced to NAMES (the two boards' index tables can differ)
// and coordinates compared at the extractor's own 1e-4 mm rounding. Because both
// sides are extracted at the same EXTRACTOR_CACHE_EPOCH, identical source geometry
// serializes identically — so string-key equality is exact, not fuzzy.
//
// The output marks, per IR primitive, whether it is ABSENT on the other side
// (flagged on A = removed copper, on B = added — a moved primitive flags on both,
// the classic gerber-diff read) AND which semantic change OWNS it, so the view can
// show/solo individual changes without rebuilding the GPU batches:
//
//   0            unchanged (present on both sides)
//   ORPHAN (1)   changed but below every semantic threshold (no owning change row)
//   k + 2        changed, owned by `changes[k]` in the diff doc
//
// The renderer bakes these values into the batches once and gates them per frame
// through a small visibility-mask texture (see PcbGlRenderer.setDiffVisibility) —
// index 1 is the orphan bucket, index k+2 is change k.
//
// Ownership matching mirrors how diff.rs keys its groups: routing & zones by
// (layer-name, net-name), placement/component by refdes, silk/outline by layer.
//
// Zones additionally GATE on the semantic row: a pour whose (layer, net) has no
// zone change in the doc is refill jitter (the Rust engine's ~1 mm² area threshold
// said so) and stays UNFLAGGED — without this, any edit that re-flows a GND pour
// tints the whole zone across every layer and buries the real change.

import type { PcbGeometry } from "../../lib/pcbGeometry";
import type { Change } from "../../lib/diff";
import type { DiffFlags } from "./glRenderer";

/** Flag value for a changed primitive no semantic change row owns. */
export const DIFF_ORPHAN = 1;
/** First flag value that encodes an owning change (`changes[flag - DIFF_OWNED_BASE]`). */
export const DIFF_OWNED_BASE = 2;

/** Fixed-point encode of an mm coordinate (extractor rounds to 1e-4). */
const q = (v: number) => Math.round(v * 1e4);

/** Build the per-class identity keys of one side's geometry. */
function buildKeys(g: PcbGeometry) {
  const layer = (i: number) => g.layers[i]?.name ?? "";
  const net = (i: number) => g.nets[i] ?? "";
  const comp = (i: number | undefined) =>
    i != null && i >= 0 ? (g.components[i]?.ref ?? "") : "";

  const seg: string[] = [];
  for (let i = 0; i < g.tracks.seg.w.length; i++) {
    const o = i * 4;
    const xy = g.tracks.seg.xy;
    seg.push(
      `s|${layer(g.tracks.seg.layer[i])}|${net(g.tracks.seg.net[i])}|${q(xy[o])},${q(xy[o + 1])},${q(xy[o + 2])},${q(xy[o + 3])}|${q(g.tracks.seg.w[i])}`,
    );
  }
  const arc: string[] = [];
  for (let i = 0; i < g.tracks.arc.w.length; i++) {
    const o = i * 6;
    const xy = g.tracks.arc.xy;
    const c = [xy[o], xy[o + 1], xy[o + 2], xy[o + 3], xy[o + 4], xy[o + 5]].map(q).join(",");
    arc.push(`a|${layer(g.tracks.arc.layer[i])}|${net(g.tracks.arc.net[i])}|${c}|${q(g.tracks.arc.w[i])}`);
  }
  const vias = g.vias.map(
    (v) =>
      `v|${v.layers.map(layer).join("+")}|${net(v.net)}|${q(v.x)},${q(v.y)},${q(v.size)},${q(v.drill)}`,
  );
  const pads = g.pads.map(
    (p) =>
      `p|${comp(p.comp)}|${net(p.net)}|${p.num}|${q(p.x)},${q(p.y)},${q(p.w)},${q(p.h)},${q(p.angle)}|${p.shape},${q(p.rratio ?? 0)},${q(p.drill ?? 0)}|${p.layers.map(layer).join("+")}`,
  );
  // Zones compare by their fill outline. KiCad refill jitter can differ, but both
  // sides are re-extracted from the checked-in fills at the same epoch, so identical
  // pours stay identical; a genuinely re-poured zone flags whole — the honest read.
  const zones = g.zones.map(
    (z) => `z|${layer(z.layer)}|${net(z.net)}|${z.filled ? 1 : 0}|${z.pts.map(q).join(",")}`,
  );
  const graphics = g.graphics.map(
    (gr) =>
      `g|${gr.kind}|${layer(gr.layer)}|${comp(gr.comp)}|${q(gr.width)}|${gr.filled ? 1 : 0}|${gr.data.map(q).join(",")}`,
  );
  return { seg, arc, vias, pads, zones, graphics };
}

type Keys = ReturnType<typeof buildKeys>;

/** Flag every key not present on the other side. Presence is multiset-aware: N
 *  identical primitives on A vs M on B flag |N−M| of them, not all or none. */
function flagAbsent(mine: string[], other: string[]): Uint16Array {
  const counts = new Map<string, number>();
  for (const k of other) counts.set(k, (counts.get(k) ?? 0) + 1);
  const flags = new Uint16Array(mine.length);
  for (let i = 0; i < mine.length; i++) {
    const c = counts.get(mine[i]) ?? 0;
    if (c > 0) counts.set(mine[i], c - 1);
    else flags[i] = DIFF_ORPHAN;
  }
  return flags;
}

// ------------------------------------------------------- change-ownership index

/** Lookup tables from the semantic change list, keyed the same way diff.rs keys its
 *  groups. Values are the change's flag code (index + DIFF_OWNED_BASE). */
interface OwnerIndex {
  /** (layer\0net) → routing change. */
  routing: Map<string, number>;
  /** (layer\0net) → zone change. */
  zone: Map<string, number>;
  /** refdes → placement/component change (placement wins — a moved footprint's pads
   *  flag because of the move, not the value edit). */
  comp: Map<string, number>;
  /** layer name → silk/outline change. */
  silk: Map<string, number>;
}

function buildOwnerIndex(changes: Change[]): OwnerIndex {
  const idx: OwnerIndex = { routing: new Map(), zone: new Map(), comp: new Map(), silk: new Map() };
  const key = (layer: string, net: string) => `${layer}\u{0}${net}`;
  changes.forEach((c, i) => {
    const code = i + DIFF_OWNED_BASE;
    const pcb = c.anchors.pcb;
    if (c.group === "routing" && pcb?.layers?.[0] != null) {
      idx.routing.set(key(pcb.layers[0], pcb.net ?? ""), code);
    } else if (c.group === "zone" && pcb?.layers?.[0] != null) {
      idx.zone.set(key(pcb.layers[0], pcb.net ?? ""), code);
    } else if (c.group === "component" && pcb?.comp) {
      if (!idx.comp.has(pcb.comp)) idx.comp.set(pcb.comp, code);
    } else if ((c.group === "silk" || c.group === "outline") && pcb?.layers?.[0] != null) {
      idx.silk.set(pcb.layers[0], code);
    }
  });
  // Placement rows overwrite component rows for the same refdes (see OwnerIndex.comp).
  changes.forEach((c, i) => {
    const ref = c.anchors.pcb?.comp;
    if (c.group === "placement" && ref) idx.comp.set(ref, i + DIFF_OWNED_BASE);
  });
  return idx;
}

/** Rewrite one side's 0/1 flags into owner codes (and gate zones on their semantic
 *  row). Mutates `flags` in place. */
function assignOwners(g: PcbGeometry, flags: Omit<DiffFlags, "maskSize">, owners: OwnerIndex) {
  const layer = (i: number) => g.layers[i]?.name ?? "";
  const net = (i: number) => g.nets[i] ?? "";
  const compRef = (i: number | undefined) =>
    i != null && i >= 0 ? (g.components[i]?.ref ?? "") : "";
  const key = (l: string, n: string) => `${l}\u{0}${n}`;

  const seg = g.tracks.seg;
  for (let i = 0; i < flags.seg.length; i++) {
    if (!flags.seg[i]) continue;
    flags.seg[i] = owners.routing.get(key(layer(seg.layer[i]), net(seg.net[i]))) ?? DIFF_ORPHAN;
  }
  const arc = g.tracks.arc;
  for (let i = 0; i < flags.arc.length; i++) {
    if (!flags.arc[i]) continue;
    flags.arc[i] = owners.routing.get(key(layer(arc.layer[i]), net(arc.net[i]))) ?? DIFF_ORPHAN;
  }
  for (let i = 0; i < flags.vias.length; i++) {
    if (!flags.vias[i]) continue;
    // A via spans several copper layers; the Rust engine emits one routing row per
    // (layer, net) it appears under — any of them owns the via.
    const v = g.vias[i];
    let code = DIFF_ORPHAN;
    for (const li of v.layers) {
      const hit = owners.routing.get(key(layer(li), net(v.net)));
      if (hit != null) {
        code = hit;
        break;
      }
    }
    flags.vias[i] = code;
  }
  for (let i = 0; i < flags.pads.length; i++) {
    if (!flags.pads[i]) continue;
    flags.pads[i] = owners.comp.get(compRef(g.pads[i].comp)) ?? DIFF_ORPHAN;
  }
  for (let i = 0; i < flags.zones.length; i++) {
    if (!flags.zones[i]) continue;
    const z = g.zones[i];
    // GATE: no semantic zone row for this (layer, net) ⇒ refill jitter under the
    // ~1 mm² area threshold — treat as unchanged so it can't wash the compare.
    flags.zones[i] = owners.zone.get(key(layer(z.layer), net(z.net))) ?? 0;
  }
  for (let i = 0; i < flags.graphics.length; i++) {
    if (!flags.graphics[i]) continue;
    const gr = g.graphics[i];
    const ref = compRef(gr.comp);
    // Footprint art moves with its component (placement/component row); loose board
    // art belongs to the layer's silk/outline row.
    flags.graphics[i] =
      (ref ? owners.comp.get(ref) : undefined) ??
      owners.silk.get(layer(gr.layer)) ??
      DIFF_ORPHAN;
  }
}

function sideFlags(mine: Keys, other: Keys): Omit<DiffFlags, "maskSize"> {
  return {
    seg: flagAbsent(mine.seg, other.seg),
    arc: flagAbsent(mine.arc, other.arc),
    vias: flagAbsent(mine.vias, other.vias),
    pads: flagAbsent(mine.pads, other.pads),
    zones: flagAbsent(mine.zones, other.zones),
    graphics: flagAbsent(mine.graphics, other.graphics),
  };
}

/** Compute both sides' changed flags for a (base A, target B) revision pair. When the
 *  semantic change list is given, flags carry owner codes (and zones gate on their
 *  semantic row); without it every changed primitive is the ORPHAN bucket — the
 *  ungated, purely geometric compare. */
export function computeDiffFlags(
  a: PcbGeometry,
  b: PcbGeometry,
  changes?: Change[],
): { a: DiffFlags; b: DiffFlags } {
  const ka = buildKeys(a);
  const kb = buildKeys(b);
  const fa = sideFlags(ka, kb);
  const fb = sideFlags(kb, ka);
  const maskSize = DIFF_OWNED_BASE + (changes?.length ?? 0);
  if (changes) {
    const owners = buildOwnerIndex(changes);
    assignOwners(a, fa, owners);
    assignOwners(b, fb, owners);
  }
  return { a: { ...fa, maskSize }, b: { ...fb, maskSize } };
}

/** The visibility mask for `setDiffVisibility`: entry per flag code. Index 0 is the
 *  "unchanged" slot (never sampled), index 1 the orphan bucket — visible only when
 *  nothing is hidden (soloing a change must also hide sub-threshold noise), then one
 *  entry per change. `hidden` holds change ids ("ch_0007"); ids are ordinals by
 *  construction but we match by list position, not by parsing the id. */
export function buildDiffVisibility(changes: Change[], hidden: ReadonlySet<string>): Uint8Array {
  const vis = new Uint8Array(DIFF_OWNED_BASE + changes.length);
  vis[DIFF_ORPHAN] = hidden.size === 0 ? 1 : 0;
  changes.forEach((c, i) => {
    vis[i + DIFF_OWNED_BASE] = hidden.has(c.id) ? 0 : 1;
  });
  return vis;
}
