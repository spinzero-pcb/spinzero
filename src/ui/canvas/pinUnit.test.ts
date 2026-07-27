import { describe, expect, it } from "vitest";
import { pinUnitUuid } from "./pinUnit";

/** Two units of one designator plus a single-unit part, shaped like the extractor's
 *  schematic SVG: a symbol group per placed unit, pin groups nested inside carrying
 *  their OWN data-uuid (the pin uuid). */
function sheet(): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = `
    <g data-primitive="symbol" data-uuid="unit-a" data-ref="U12">
      <g data-primitive="pin" data-uuid="pin-a18" data-designator="U12" data-pin="18"></g>
      <g data-primitive="pin" data-uuid="pin-a19" data-designator="U12" data-pin="19"></g>
    </g>
    <g data-primitive="symbol" data-uuid="unit-c" data-ref="U12">
      <g data-primitive="pin" data-uuid="pin-c3" data-designator="U12" data-pin="3"></g>
    </g>
    <g data-primitive="power-symbol" data-uuid="flag" data-ref="#PWR01">
      <g data-primitive="pin" data-uuid="pin-p1" data-designator="#PWR01" data-pin="1"></g>
    </g>`;
  return root;
}

describe("pinUnitUuid", () => {
  it("resolves a pin to the unit that carries it, not another unit of the part", () => {
    expect(pinUnitUuid(sheet(), "U12", "18")).toBe("unit-a");
    expect(pinUnitUuid(sheet(), "U12", "3")).toBe("unit-c");
  });

  it("returns the symbol uuid, never the pin's own uuid", () => {
    // Regression: climbing to the nearest `g[data-uuid]` ancestor stops at the pin
    // group itself, yielding a uuid that matches no symbol and paints nothing.
    expect(pinUnitUuid(sheet(), "U12", "18")).not.toBe("pin-a18");
  });

  it("resolves a power flag's pin to the flag", () => {
    expect(pinUnitUuid(sheet(), "#PWR01", "1")).toBe("flag");
  });

  it("is undefined for a pin that isn't on this sheet, and for no sheet", () => {
    expect(pinUnitUuid(sheet(), "U12", "99")).toBeUndefined();
    expect(pinUnitUuid(sheet(), "U99", "1")).toBeUndefined();
    expect(pinUnitUuid(null, "U12", "18")).toBeUndefined();
  });

  it("escapes designators that carry CSS-special characters", () => {
    const root = document.createElement("div");
    root.innerHTML = `
      <g data-primitive="symbol" data-uuid="odd">
        <g data-primitive="pin" data-uuid="p" data-designator="U1.2" data-pin="A+"></g>
      </g>`;
    expect(pinUnitUuid(root, "U1.2", "A+")).toBe("odd");
  });
});
