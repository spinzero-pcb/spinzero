import { describe, expect, it } from "vitest";
import type { PcbGeometry } from "../../lib/pcbGeometry";
import { outlineBBox } from "./fitBBox";

const base = {
  layers: [
    { name: "Top Layer", role: "copper", ord: 0 },
    { name: "Board Shape", role: "edge", ord: 1 },
  ],
  tracks: { seg: { xy: [0, 0, 500, 500], w: [0.2], layer: [0], net: [0] }, arc: { xy: [], w: [], layer: [], net: [] } },
  graphics: [{ layer: 1, width: 0.1, kind: "poly", data: [10, 20, 40, 20, 40, 60, 10, 60] }],
} as unknown as PcbGeometry;

describe("outlineBBox", () => {
  it("frames the edge layer and ignores off-board copper", () => {
    expect(outlineBBox(base)).toEqual({ minx: 10, miny: 20, maxx: 40, maxy: 60 });
  });
  it("returns null without an edge layer", () => {
    const g = { ...base, layers: [base.layers[0]] } as PcbGeometry;
    expect(outlineBBox(g)).toBeNull();
  });
});
