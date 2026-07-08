import { describe, expect, it, vi } from "vitest";
import { drawMeasure, tickFormatForScale } from "./measureOverlay";

/** A minimal recording stub for the Canvas2D methods drawMeasure touches, so the drawing
 *  path can run under jsdom (which has no canvas) and we can assert what it drew. */
function mockCtx() {
  const calls: string[] = [];
  const rec = (name: string) => vi.fn(() => calls.push(name));
  return {
    calls,
    ctx: {
      save: rec("save"),
      restore: rec("restore"),
      beginPath: rec("beginPath"),
      moveTo: rec("moveTo"),
      lineTo: rec("lineTo"),
      arc: rec("arc"),
      rect: rec("rect"),
      stroke: rec("stroke"),
      fill: rec("fill"),
      fillText: rec("fillText"),
      strokeText: rec("strokeText"),
      translate: rec("translate"),
      rotate: rec("rotate"),
      measureText: () => ({ width: 24 }),
      setLineDash: rec("setLineDash"),
    } as unknown as CanvasRenderingContext2D,
  };
}

const STYLE = { line: "#fff", shadow: "#000", text: "#fff", font: "monospace" };
const CAM = { x: 0, y: 0, scale: 20 };

// KiCad ruler graduation (common/preview_items/ruler_item.cpp getTickFormatForScale):
// spacing grows on a 1/2/5-per-decade scale until minor ticks are ≥10 screen px apart,
// and the chosen format sets how many minor ticks fall between mid/major ticks.
describe("tickFormatForScale (metric)", () => {
  it("subdivides below 1 mm at high zoom (KiCad grows from one internal unit)", () => {
    // 20 px/mm: the 1/2/5 cycle from 1e-6 mm stops at 0.5 mm (= 10 px ≥ 10 px);
    // that step lands on format |.|.| — majors (labels) every 2 ticks = every 1 mm.
    const t = tickFormatForScale(20, false);
    expect(t.spaceMm).toBeCloseTo(0.5);
    expect(t.major).toBe(2);
    expect(t.mid).toBe(0);
  });

  it("labels every 0.100 mm at the feedback/35.png zoom (~360 px/mm)", () => {
    // 0.02 mm · 360 = 7.2 px < 10 → next step 0.05 mm = 18 px; labels every 2 ticks.
    const t = tickFormatForScale(360, false);
    expect(t.spaceMm).toBeCloseTo(0.05);
    expect(t.major).toBe(2);
  });

  it("steps 1→2→5→10… as the view zooms out", () => {
    // 4 px/mm: 1mm=4px (<10) → 2mm=8px (<10) → 5mm=20px (≥10). Format 2: majors every 2.
    const t = tickFormatForScale(4, false);
    expect(t.spaceMm).toBeCloseTo(5);
    expect(t.major).toBe(2);
    expect(t.mid).toBe(0);
  });

  it("keeps stepping (1→2→5→10→20→50) when zoomed far out", () => {
    // 0.3 px/mm: 1,2,5,10,20 all give <10 px; 50 mm = 15 px stops. Format 2 (|.|.|),
    // majors every 2 (i.e. labelled every 100 mm), no mids.
    const t = tickFormatForScale(0.3, false);
    expect(t.spaceMm).toBeCloseTo(50);
    expect(t.major).toBe(2);
    expect(t.mid).toBe(0);
  });

  it("always yields a spacing with ≥10 px between minor ticks", () => {
    for (const scale of [0.05, 0.2, 1, 3, 7, 50, 200]) {
      const t = tickFormatForScale(scale, false);
      expect(t.spaceMm * scale).toBeGreaterThanOrEqual(10 - 1e-9);
    }
  });
});

describe("tickFormatForScale (imperial)", () => {
  it("grows in ×2.54 steps so ticks land on round mil/inch values", () => {
    // 10 px/mm: the 2.54×(1/2/5) cycle stops at 1.27 mm (50 mil) = 12.7 px ≥ 10.
    const t = tickFormatForScale(10, true);
    expect(t.spaceMm).toBeCloseTo(1.27);
  });

  it("steps imperial spacing on the same 1/2/5 cadence", () => {
    // 2 px/mm: 2.54mm=5.08px (<10) → ×2 =5.08mm=10.16px (≥10).
    const t = tickFormatForScale(2, true);
    expect(t.spaceMm).toBeCloseTo(5.08);
  });
});

describe("drawMeasure (smoke)", () => {
  it("draws the ruler line, ticks and cursor text for a completed measurement", () => {
    const { ctx, calls } = mockCtx();
    const A = { x: 0, y: 0, snapped: true };
    const B = { x: 10, y: 0, snapped: true };
    drawMeasure(ctx, A, B, B, "mm", CAM, 800, 600, STYLE);
    expect(calls.filter((c) => c === "stroke").length).toBeGreaterThan(1); // line + ticks
    expect(calls).toContain("fillText"); // tick + x/y/r/θ labels
    // balanced save/restore
    expect(calls.filter((c) => c === "save").length).toBe(calls.filter((c) => c === "restore").length);
  });

  it("draws only a snap marker before the first point", () => {
    const { ctx, calls } = mockCtx();
    drawMeasure(ctx, null, null, { x: 0, y: 0, snapped: true }, "mm", CAM, 800, 600, STYLE);
    expect(calls).toContain("rect"); // vertex-lock marker
    expect(calls).not.toContain("fillText"); // no readout without a radius
  });

  it("shows an R/Ø readout when hovering an arc/circle centre", () => {
    const { ctx, calls } = mockCtx();
    drawMeasure(ctx, null, null, { x: 0, y: 0, snapped: true, radius: 4 }, "mm", CAM, 800, 600, STYLE);
    expect(calls).toContain("fillText");
  });

  it("does not throw for a mil measurement", () => {
    const { ctx } = mockCtx();
    expect(() =>
      drawMeasure(ctx, { x: 0, y: 0, snapped: false }, { x: 3.2, y: 1.1, snapped: false }, { x: 3.2, y: 1.1, snapped: false }, "mil", CAM, 800, 600, STYLE),
    ).not.toThrow();
  });
});
