import { describe, expect, it } from "vitest";
import { buildDiffVisibility, computeDiffFlags, DIFF_ORPHAN, DIFF_OWNED_BASE } from "./glDiff";
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
