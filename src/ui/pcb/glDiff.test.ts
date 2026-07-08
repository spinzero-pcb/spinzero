import { describe, expect, it } from "vitest";
import { computeDiffFlags } from "./glDiff";
import type { PcbGeometry } from "../../lib/pcbGeometry";

/** Minimal two-layer board with one segment, one via, one pad. */
function board(over?: Partial<PcbGeometry>): PcbGeometry {
  return {
    schema: "extract.pcb.geometry.a0",
    units: "mm",
    bbox: [0, 0, 100, 100],
    layers: [
      { name: "F.Cu", role: "copper", ord: 0 },
      { name: "B.Cu", role: "copper", ord: 31 },
    ],
    nets: ["", "/VBUS"],
    components: [{ ref: "R1", fp: "R_0402", layer: 0, x: 5, y: 5, angle: 0 }],
    tracks: {
      seg: { xy: [0, 0, 10, 0], w: [0.25], layer: [0], net: [1] },
      arc: { xy: [], w: [], layer: [], net: [] },
    },
    vias: [{ x: 10, y: 0, size: 0.6, drill: 0.3, net: 1, layers: [0, 1] }],
    pads: [
      { x: 5, y: 5, w: 1, h: 0.5, angle: 0, shape: 1, net: 1, comp: 0, num: "1", layers: [0] },
    ],
    zones: [],
    graphics: [],
    texts: [],
    ...over,
  };
}

describe("computeDiffFlags", () => {
  it("flags nothing for identical boards", () => {
    const f = computeDiffFlags(board(), board());
    expect([...f.a.seg, ...f.a.vias, ...f.a.pads]).toEqual([0, 0, 0]);
    expect([...f.b.seg, ...f.b.vias, ...f.b.pads]).toEqual([0, 0, 0]);
  });

  it("a moved via flags on both sides (remove + add), tracks/pads stay clean", () => {
    const b = board();
    b.vias = [{ x: 12, y: 0, size: 0.6, drill: 0.3, net: 1, layers: [0, 1] }];
    const f = computeDiffFlags(board(), b);
    expect([...f.a.vias]).toEqual([1]); // gone from its old spot → red on A
    expect([...f.b.vias]).toEqual([1]); // present at a new spot → green on B
    expect([...f.a.seg]).toEqual([0]);
    expect([...f.b.pads]).toEqual([0]);
  });

  it("identity dereferences net/layer indices to names across differing tables", () => {
    // B's net table has an extra entry BEFORE /VBUS, shifting its index — the same
    // copper must still compare equal because keys use names, not indices.
    const b = board({ nets: ["", "/3V3", "/VBUS"] });
    b.tracks.seg.net = [2];
    b.vias[0].net = 2;
    b.pads[0].net = 2;
    const f = computeDiffFlags(board(), b);
    expect([...f.a.seg, ...f.a.vias, ...f.a.pads]).toEqual([0, 0, 0]);
    expect([...f.b.seg, ...f.b.vias, ...f.b.pads]).toEqual([0, 0, 0]);
  });

  it("is multiset-aware: 3 identical stitching vias vs 2 flags exactly one", () => {
    const via = { x: 10, y: 0, size: 0.6, drill: 0.3, net: 1, layers: [0, 1] };
    const a = board();
    a.vias = [{ ...via }, { ...via }, { ...via }];
    const b = board();
    b.vias = [{ ...via }, { ...via }];
    const f = computeDiffFlags(a, b);
    expect([...f.a.vias].reduce((s, v) => s + v, 0)).toBe(1); // one removed
    expect([...f.b.vias]).toEqual([0, 0]);
  });

  it("a resized pad flags on both sides", () => {
    const b = board();
    b.pads = [{ ...b.pads[0], w: 1.2 }];
    const f = computeDiffFlags(board(), b);
    expect([...f.a.pads]).toEqual([1]);
    expect([...f.b.pads]).toEqual([1]);
  });
});
