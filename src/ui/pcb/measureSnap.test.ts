import { describe, expect, it } from "vitest";
import { buildEdgeIndex, buildSnapIndex, circleFrom3, padCorners, SnapIndex } from "./measureSnap";
import type { PcbGeometry, PcbPadDef } from "../../lib/pcbGeometry";

const GEOMETRY_SCHEMA = "extract.pcb.geometry.a0";

/** Minimal geometry with one pad (F.Cu, layer 0), one via, one straight track on
 *  B.Cu (layer 1), one Edge.Cuts graphic seg, and one component origin. */
function fixture(): PcbGeometry {
  return {
    schema: GEOMETRY_SCHEMA,
    units: "mm",
    bbox: [0, 0, 100, 100],
    layers: [
      { name: "F.Cu", role: "copper", side: "front", ord: 0 },
      { name: "B.Cu", role: "copper", side: "back", ord: 1 },
      { name: "Edge.Cuts", role: "edge", ord: 2 },
    ],
    nets: ["", "GND"],
    components: [{ ref: "U1", fp: "QFN", layer: 0, x: 50, y: 50, angle: 0 }],
    tracks: {
      seg: { xy: [10, 10, 20, 10], w: [0.2], layer: [1], net: [1] },
      arc: { xy: [], w: [], layer: [], net: [] },
    },
    vias: [{ x: 30, y: 30, size: 0.6, drill: 0.3, net: 1, layers: [0, 1] }],
    pads: [
      { x: 5, y: 5, w: 1, h: 1, angle: 0, shape: 1, net: 1, comp: 0, num: "1", layers: [0] },
    ],
    zones: [],
    graphics: [{ layer: 2, width: 0.15, kind: "seg", data: [0, 0, 100, 0] }],
    texts: [],
  };
}

const allVisible = () => true;
const allObjects = () => true;

describe("SnapIndex", () => {
  it("snaps to the nearest pad centre within the radius", () => {
    const idx = buildSnapIndex(fixture());
    const hit = idx.query(5.2, 5.1, 1, allVisible, allObjects);
    expect(hit).not.toBeNull();
    expect(hit!.kind).toBe("pad");
    expect(hit!.x).toBeCloseTo(5);
    expect(hit!.y).toBeCloseTo(5);
  });

  it("returns null when no candidate is within the radius", () => {
    const idx = buildSnapIndex(fixture());
    expect(idx.query(60, 60, 1, allVisible, allObjects)).toBeNull();
  });

  it("indexes both endpoints of a straight track", () => {
    const idx = buildSnapIndex(fixture());
    const a = idx.query(10, 10, 0.5, allVisible, allObjects);
    const b = idx.query(20, 10, 0.5, allVisible, allObjects);
    expect(a!.kind).toBe("track");
    expect(b!.kind).toBe("track");
    expect(b!.x).toBeCloseTo(20);
  });

  it("skips a candidate whose only layer is hidden", () => {
    const idx = buildSnapIndex(fixture());
    // The track sits on layer 1 (B.Cu); hide it → no snap there.
    const visible = (layer: number) => layer !== 1;
    expect(idx.query(10, 10, 0.5, visible, allObjects)).toBeNull();
    // The pad on layer 0 is unaffected.
    expect(idx.query(5, 5, 0.5, visible, allObjects)!.kind).toBe("pad");
  });

  it("honours the per-class object toggle", () => {
    const idx = buildSnapIndex(fixture());
    const noVias = (kind: string) => kind !== "via";
    expect(idx.query(30, 30, 0.5, allVisible, noVias)).toBeNull();
  });

  it("picks the closest of several candidates", () => {
    const idx = new SnapIndex();
    idx.add({ x: 0, y: 0, kind: "pad", layers: [] });
    idx.add({ x: 1, y: 0, kind: "via", layers: [] });
    const hit = idx.query(0.9, 0, 5, allVisible, allObjects);
    expect(hit!.kind).toBe("via");
  });

  it("snaps to a board-outline graphic endpoint (layer-visibility only)", () => {
    const idx = buildSnapIndex(fixture());
    const hit = idx.query(100, 0, 0.5, allVisible, allObjects);
    expect(hit!.kind).toBe("graphic");
  });

  it("snaps to a pad corner, not just the centre", () => {
    // The fixture pad is 1×1 at (5,5), angle 0 → a corner at (5.5, 5.5).
    const idx = buildSnapIndex(fixture());
    const hit = idx.query(5.4, 5.4, 0.5, allVisible, allObjects);
    expect(hit!.kind).toBe("pad");
    expect(hit!.x).toBeCloseTo(5.5);
    expect(hit!.y).toBeCloseTo(5.5);
  });

  it("snaps to the midpoint of a track segment", () => {
    // Track runs (10,10)-(20,10); midpoint is (15,10).
    const idx = buildSnapIndex(fixture());
    const hit = idx.query(15, 10, 0.4, allVisible, allObjects);
    expect(hit!.kind).toBe("track");
    expect(hit!.x).toBeCloseTo(15);
  });

  it("snaps to the midpoint of a board-outline segment (centre of a line)", () => {
    // Edge.Cuts seg (0,0)-(100,0); midpoint (50,0).
    const idx = buildSnapIndex(fixture());
    const hit = idx.query(50, 0, 0.4, allVisible, allObjects);
    expect(hit!.kind).toBe("graphic");
    expect(hit!.x).toBeCloseTo(50);
  });

  it("exposes an arc/circle centre with its radius", () => {
    const g = fixture();
    // A circle graphic centred at (60,60), r=4.
    g.graphics.push({ layer: 2, width: 0.15, kind: "circle", data: [60, 60, 4] });
    const idx = buildSnapIndex(g);
    const hit = idx.query(60, 60, 0.5, allVisible, allObjects);
    expect(hit!.kind).toBe("graphic");
    expect(hit!.radius).toBeCloseTo(4);
  });
});

describe("padCorners", () => {
  it("returns the four corners for an axis-aligned pad", () => {
    const p = { x: 0, y: 0, w: 2, h: 4, angle: 0, shape: 1, net: 0, comp: 0, num: "1", layers: [0] } as PcbPadDef;
    const c = padCorners(p);
    const xs = c.map((q) => q.x).sort((a, b) => a - b);
    const ys = c.map((q) => q.y).sort((a, b) => a - b);
    expect(xs[0]).toBeCloseTo(-1);
    expect(xs[3]).toBeCloseTo(1);
    expect(ys[0]).toBeCloseTo(-2);
    expect(ys[3]).toBeCloseTo(2);
  });

  it("rotates corners about the pad origin", () => {
    const p = { x: 0, y: 0, w: 2, h: 2, angle: 45, shape: 1, net: 0, comp: 0, num: "1", layers: [0] } as PcbPadDef;
    const c = padCorners(p);
    // A 2×2 pad rotated 45° has corners on the axes at ±√2.
    for (const q of c) expect(Math.hypot(q.x, q.y)).toBeCloseTo(Math.SQRT2);
  });
});

describe("circleFrom3", () => {
  it("recovers the circumcircle of three points", () => {
    const c = circleFrom3(1, 0, 0, 1, -1, 0);
    expect(c).not.toBeNull();
    expect(c!.cx).toBeCloseTo(0);
    expect(c!.cy).toBeCloseTo(0);
    expect(c!.r).toBeCloseTo(1);
  });

  it("returns null for colinear points", () => {
    expect(circleFrom3(0, 0, 1, 0, 2, 0)).toBeNull();
  });
});

describe("EdgeIndex", () => {
  it("projects the cursor onto the nearest track copper edge", () => {
    // Track (10,10)-(20,10) width 0.2 → copper edges at y=9.9 / 10.1. A cursor above
    // the middle snaps onto the near (top) edge, not the centreline.
    const idx = buildEdgeIndex(fixture());
    const hit = idx.query(15, 10.3, 0.5, allVisible, allObjects);
    expect(hit).not.toBeNull();
    expect(hit!.kind).toBe("track");
    expect(hit!.edge).toBe(true);
    expect(hit!.x).toBeCloseTo(15);
    expect(hit!.y).toBeCloseTo(10.1);
  });

  it("still snaps to the track centreline when the cursor is on it", () => {
    // A cursor on the routed midline snaps to the centreline (0 away), beating the
    // copper edges (0.1 away) — centre and edge snapping coexist.
    const idx = buildEdgeIndex(fixture());
    const hit = idx.query(15, 10, 0.5, allVisible, allObjects);
    expect(hit!.kind).toBe("track");
    expect(hit!.y).toBeCloseTo(10);
  });

  it("projects onto the board outline (edge-to-edge)", () => {
    const idx = buildEdgeIndex(fixture());
    const hit = idx.query(40, 0.3, 0.5, allVisible, allObjects);
    expect(hit!.kind).toBe("graphic");
    expect(hit!.y).toBeCloseTo(0);
  });

  it("projects onto a via outline and reports no radius readout", () => {
    // Via at (30,30) size 0.6 → r 0.3. Cursor just outside snaps onto the ring.
    const idx = buildEdgeIndex(fixture());
    const hit = idx.query(30.5, 30, 0.5, allVisible, allObjects);
    expect(hit!.kind).toBe("via");
    expect(hit!.x).toBeCloseTo(30.3);
    expect(hit!.radius).toBeUndefined();
  });

  it("reports radius when projecting onto a circle graphic", () => {
    const g = fixture();
    g.graphics.push({ layer: 2, width: 0.15, kind: "circle", data: [60, 60, 4] });
    const idx = buildEdgeIndex(g);
    const hit = idx.query(63.7, 60, 0.5, allVisible, allObjects);
    expect(hit!.kind).toBe("graphic");
    expect(hit!.x).toBeCloseTo(64);
    expect(hit!.radius).toBeCloseTo(4);
  });

  it("honours layer visibility on edges", () => {
    const idx = buildEdgeIndex(fixture());
    // Track is on layer 1 (B.Cu); hide it → no edge snap onto it.
    const visible = (layer: number) => layer !== 1;
    expect(idx.query(15, 10.3, 0.5, visible, allObjects)).toBeNull();
  });
});
