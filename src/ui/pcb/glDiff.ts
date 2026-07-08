// Per-primitive changed-flag computation for the PCB visual diff (plan §4).
//
// Identity mirrors the Rust diff engine (src-tauri/src/diff.rs `hash_prim`): a
// primitive is "the same" across two revisions when its content tuple matches with
// layer/net indices dereferenced to NAMES (the two boards' index tables can differ)
// and coordinates compared at the extractor's own 1e-4 mm rounding. Because both
// sides are extracted at the same EXTRACTOR_CACHE_EPOCH, identical source geometry
// serializes identically — so string-key equality is exact, not fuzzy.
//
// The output marks, per IR primitive, whether it is ABSENT on the other side:
// flagged on A = removed copper (painted red), flagged on B = added (green).
// A moved primitive is correctly flagged on both sides (remove + add), the
// classic gerber-diff read.

import type { PcbGeometry } from "../../lib/pcbGeometry";
import type { DiffFlags } from "./glRenderer";

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
function flagAbsent(mine: string[], other: string[]): Uint8Array {
  const counts = new Map<string, number>();
  for (const k of other) counts.set(k, (counts.get(k) ?? 0) + 1);
  const flags = new Uint8Array(mine.length);
  for (let i = 0; i < mine.length; i++) {
    const c = counts.get(mine[i]) ?? 0;
    if (c > 0) counts.set(mine[i], c - 1);
    else flags[i] = 1;
  }
  return flags;
}

function sideFlags(mine: Keys, other: Keys): DiffFlags {
  return {
    seg: flagAbsent(mine.seg, other.seg),
    arc: flagAbsent(mine.arc, other.arc),
    vias: flagAbsent(mine.vias, other.vias),
    pads: flagAbsent(mine.pads, other.pads),
    zones: flagAbsent(mine.zones, other.zones),
    graphics: flagAbsent(mine.graphics, other.graphics),
  };
}

/** Compute both sides' changed flags for a (base A, target B) revision pair. */
export function computeDiffFlags(a: PcbGeometry, b: PcbGeometry): { a: DiffFlags; b: DiffFlags } {
  const ka = buildKeys(a);
  const kb = buildKeys(b);
  return { a: sideFlags(ka, kb), b: sideFlags(kb, ka) };
}
