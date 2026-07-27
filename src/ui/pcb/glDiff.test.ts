import { describe, expect, it } from "vitest";
import {
  buildDiffVisibility,
  changeExtent,
  computeDiffFlags,
  DIFF_ORPHAN,
  DIFF_OWNED_BASE,
  unionExtent,
} from "./glDiff";
import type { PcbGeometry } from "../../lib/pcbGeometry";
import type { Change } from "../../lib/diff";

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

  it("a moved text flags on both sides; a restyled text flags too", () => {
    const text = { layer: 0, text: "TDO", x: 10, y: 10, angle: 0, size: 1, justify: [0, 0] as [number, number] };
    const a = board({ texts: [{ ...text }, { ...text, text: "V_phs", x: 20, thickness: 0.1 }] });
    const b = board({ texts: [{ ...text, x: 14 }, { ...text, text: "V_phs", x: 20, thickness: 0.15 }] });
    const f = computeDiffFlags(a, b);
    expect([...f.a.texts]).toEqual([1, 1]); // old spot + old pen → removed side
    expect([...f.b.texts]).toEqual([1, 1]); // new spot + new pen → added side
    // Identical texts stay unflagged.
    const same = computeDiffFlags(a, a);
    expect([...same.a.texts]).toEqual([0, 0]);
  });

  it("a resized pad flags on both sides", () => {
    const b = board();
    b.pads = [{ ...b.pads[0], w: 1.2 }];
    const f = computeDiffFlags(board(), b);
    expect([...f.a.pads]).toEqual([1]);
    expect([...f.b.pads]).toEqual([1]);
  });
});

// ------------------------------------------------- change ownership + zone gating

function change(over: Partial<Change> & Pick<Change, "id" | "group">): Change {
  return {
    kind: "modified",
    impact: "electrical",
    title: over.id,
    anchors: {},
    side: "both",
    ...over,
  };
}

describe("computeDiffFlags ownership", () => {
  it("assigns routing primitives to their (layer, net) change; unmatched → orphan", () => {
    const b = board();
    b.tracks.seg = { xy: [0, 0, 12, 0], w: [0.25], layer: [0], net: [1] }; // rerouted
    const changes = [
      change({
        id: "ch_0000",
        group: "routing",
        anchors: { pcb: { layers: ["F.Cu"], net: "/VBUS" } },
      }),
    ];
    const f = computeDiffFlags(board(), b, changes);
    expect([...f.a.seg]).toEqual([DIFF_OWNED_BASE]); // owned by change 0
    expect([...f.b.seg]).toEqual([DIFF_OWNED_BASE]);
    expect(f.a.maskSize).toBe(DIFF_OWNED_BASE + 1);
  });

  it("a changed via belongs to the per-net via row, not the layer's track row", () => {
    const b = board();
    b.tracks.seg = { xy: [0, 0, 12, 0], w: [0.25], layer: [0], net: [1] }; // rerouted
    b.vias = [{ x: 14, y: 0, size: 0.6, drill: 0.3, net: 1, layers: [0, 1] }]; // moved
    const changes = [
      change({
        id: "ch_0000",
        group: "routing",
        anchors: { pcb: { layers: ["F.Cu"], net: "/VBUS" } },
      }),
      change({
        id: "ch_0001",
        group: "routing",
        anchors: { pcb: { layers: ["F.Cu", "B.Cu"], net: "/VBUS", vias: true } },
      }),
    ];
    const f = computeDiffFlags(board(), b, changes);
    expect([...f.a.seg]).toEqual([DIFF_OWNED_BASE]); // track row keeps the segment
    expect([...f.a.vias]).toEqual([DIFF_OWNED_BASE + 1]); // via row owns the via
    expect([...f.b.vias]).toEqual([DIFF_OWNED_BASE + 1]);
  });

  it("falls back to per-layer track rows for vias in older docs (no via row)", () => {
    const b = board();
    b.vias = [{ x: 14, y: 0, size: 0.6, drill: 0.3, net: 1, layers: [0, 1] }];
    const changes = [
      change({
        id: "ch_0000",
        group: "routing",
        anchors: { pcb: { layers: ["B.Cu"], net: "/VBUS" } },
      }),
    ];
    const f = computeDiffFlags(board(), b, changes);
    expect([...f.a.vias]).toEqual([DIFF_OWNED_BASE]); // matched via its B.Cu span
    expect([...f.b.vias]).toEqual([DIFF_OWNED_BASE]);
  });

  it("gates zone tinting on a semantic zone row (refill jitter stays unflagged)", () => {
    const zoneA = { layer: 0, net: 1, filled: true, pts: [0, 0, 20, 0, 20, 20, 0, 20] };
    const zoneB = { layer: 0, net: 1, filled: true, pts: [0, 0, 20.001, 0, 20, 20, 0, 20] };
    const a = board({ zones: [zoneA] });
    const b = board({ zones: [zoneB] });
    // Geometrically different, but NO zone change row (area delta under threshold):
    const gated = computeDiffFlags(a, b, []);
    expect([...gated.a.zones]).toEqual([0]);
    expect([...gated.b.zones]).toEqual([0]);
    // With a semantic row, the pour tints and is owned by it:
    const changes = [
      change({ id: "ch_0000", group: "zone", anchors: { pcb: { layers: ["F.Cu"], net: "/VBUS" } } }),
    ];
    const owned = computeDiffFlags(a, b, changes);
    expect([...owned.a.zones]).toEqual([DIFF_OWNED_BASE]);
    expect([...owned.b.zones]).toEqual([DIFF_OWNED_BASE]);
    // Without the change list at all (legacy/geometric mode), zones stay ungated:
    const raw = computeDiffFlags(a, b);
    expect([...raw.a.zones]).toEqual([DIFF_ORPHAN]);
  });

  it("texts match their text row by layer + bbox (a moved row's bbox covers both spots)", () => {
    const text = { layer: 0, text: "TDO", x: 10, y: 10, angle: 0, size: 1, justify: [0, 0] as [number, number] };
    const a = board({ texts: [{ ...text }] });
    const b = board({ texts: [{ ...text, x: 14 }] });
    const changes = [
      change({
        id: "ch_0000",
        group: "text",
        kind: "moved",
        // The Rust moved-row anchor: a box covering the old AND new positions.
        anchors: { pcb: { layers: ["F.Cu"], bbox: [5, 5, 14, 10] } },
      }),
    ];
    const f = computeDiffFlags(a, b, changes);
    expect([...f.a.texts]).toEqual([DIFF_OWNED_BASE]); // old copy owned by the row
    expect([...f.b.texts]).toEqual([DIFF_OWNED_BASE]); // new copy too
    // A flagged text outside every text row's bbox stays an orphan.
    const far = computeDiffFlags(
      board({ texts: [{ ...text, x: 90, y: 90 }] }),
      board({ texts: [{ ...text, x: 94, y: 90 }] }),
      changes,
    );
    expect([...far.a.texts]).toEqual([DIFF_ORPHAN]);
  });

  it("changeExtent covers an owned text's glyph run", () => {
    const text = { layer: 0, text: "TDO", x: 10, y: 10, angle: 0, size: 1, justify: [0, 0] as [number, number] };
    const b = board({ texts: [{ ...text, x: 14 }] });
    const changes = [
      change({
        id: "ch_0000",
        group: "text",
        kind: "moved",
        anchors: { pcb: { layers: ["F.Cu"], bbox: [5, 5, 14, 10] } },
      }),
    ];
    const f = computeDiffFlags(board({ texts: [{ ...text }] }), b, changes);
    const ext = changeExtent(b, f.b, DIFF_OWNED_BASE);
    expect(ext).not.toBeNull();
    expect(ext!.minx).toBeLessThan(14);
    expect(ext!.maxx).toBeGreaterThan(14);
  });

  it("pads follow their component's placement change over its field change", () => {
    const b = board();
    b.components = [{ ...b.components[0], x: 8 }];
    b.pads = [{ ...b.pads[0], x: 8 }]; // moved with the footprint
    const changes = [
      change({ id: "ch_0000", group: "component", anchors: { pcb: { comp: "R1" } } }),
      change({ id: "ch_0001", group: "placement", kind: "moved", anchors: { pcb: { comp: "R1" } } }),
    ];
    const f = computeDiffFlags(board(), b, changes);
    expect([...f.a.pads]).toEqual([DIFF_OWNED_BASE + 1]); // the placement row wins
    expect([...f.b.pads]).toEqual([DIFF_OWNED_BASE + 1]);
  });
});

describe("buildDiffVisibility", () => {
  const changes = [
    change({ id: "ch_0000", group: "routing" }),
    change({ id: "ch_0001", group: "zone" }),
  ];

  it("shows everything (orphans included) when nothing is hidden", () => {
    const vis = buildDiffVisibility(changes, new Set());
    expect(vis[DIFF_ORPHAN]).toBe(1);
    expect(vis[DIFF_OWNED_BASE]).toBe(1);
    expect(vis[DIFF_OWNED_BASE + 1]).toBe(1);
  });

  it("hides listed changes AND the orphan bucket while a subset is active", () => {
    const vis = buildDiffVisibility(changes, new Set(["ch_0001"]));
    expect(vis[DIFF_ORPHAN]).toBe(0); // soloing must also drop sub-threshold noise
    expect(vis[DIFF_OWNED_BASE]).toBe(1);
    expect(vis[DIFF_OWNED_BASE + 1]).toBe(0);
  });
});

describe("changeExtent", () => {
  it("covers ONLY the change's owned primitives, width-inflated — not the whole net", () => {
    const b = board();
    // Rerouted stretch: the F.Cu /VBUS segment moved; the via + pad on the same net
    // are untouched and must not stretch the extent.
    b.tracks.seg = { xy: [2, 1, 6, 1], w: [0.5], layer: [0], net: [1] };
    const changes = [
      change({ id: "ch_0000", group: "routing", anchors: { pcb: { layers: ["F.Cu"], net: "/VBUS" } } }),
    ];
    const f = computeDiffFlags(board(), b, changes);
    const ext = changeExtent(b, f.b, DIFF_OWNED_BASE);
    expect(ext).toEqual({ minx: 1.75, miny: 0.75, maxx: 6.25, maxy: 1.25 });
  });

  it("returns null when the change owns nothing on this side", () => {
    const f = computeDiffFlags(board(), board(), [change({ id: "ch_0000", group: "routing" })]);
    expect(changeExtent(board(), f.a, DIFF_OWNED_BASE)).toBeNull();
  });

  it("pads inflate by the rotation-safe half-diagonal; unionExtent merges sides", () => {
    const b = board();
    b.components = [{ ...b.components[0], x: 8 }];
    b.pads = [{ ...b.pads[0], x: 8 }];
    const changes = [
      change({ id: "ch_0000", group: "placement", kind: "moved", anchors: { pcb: { comp: "R1" } } }),
    ];
    const f = computeDiffFlags(board(), b, changes);
    const extA = changeExtent(board(), f.a, DIFF_OWNED_BASE); // pad at old x=5
    const extB = changeExtent(b, f.b, DIFF_OWNED_BASE); // pad at new x=8
    const half = Math.hypot(1, 0.5) / 2;
    expect(extA?.minx).toBeCloseTo(5 - half);
    expect(extB?.maxx).toBeCloseTo(8 + half);
    const both = unionExtent(extA, extB);
    expect(both?.minx).toBeCloseTo(5 - half); // old + new spot both framed
    expect(both?.maxx).toBeCloseTo(8 + half);
    expect(unionExtent(extA, null)).toEqual(extA);
    expect(unionExtent(null, null)).toBeNull();
  });
});
