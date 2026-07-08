import { describe, expect, it } from "vitest";
import { padLabelAngle } from "./glRenderer";

// Pure-logic coverage for the pad net-label orientation rule (Batch 1): the name must run
// along the placed pad's longer board axis and stay upright, so it sits inside the pad.

describe("padLabelAngle", () => {
  it("J5.9: a tall-local pad placed at 90° reads horizontally", () => {
    // 0.75 wide × 2.05 tall locally; the 90° placement makes it wide on the board → 0° text.
    expect(padLabelAngle(90, 0.75, 2.05)).toBe(0);
  });

  it("an unrotated wide pad reads horizontally, a tall one vertically", () => {
    expect(padLabelAngle(0, 2.0, 1.0)).toBe(0);
    expect(padLabelAngle(0, 1.0, 2.0)).toBe(90);
  });

  it("orients by the larger dimension even when only mildly oblong (no hysteresis)", () => {
    // A pad 1.0×1.2 is taller than wide, so the name runs vertically — the overlay's budget
    // is max(w,h), and a horizontal name here would overflow the 1.0-wide body.
    expect(padLabelAngle(0, 1.0, 1.2)).toBe(90);
    expect(padLabelAngle(0, 1.2, 1.0)).toBe(0);
  });

  it("normalises any angle into (-90, 90]", () => {
    expect(padLabelAngle(180, 2.0, 1.0)).toBe(0); // upside-down → upright
    expect(padLabelAngle(270, 2.0, 1.0)).toBe(90); // 270 → 90
    expect(padLabelAngle(135, 2.0, 1.0)).toBe(-45); // diagonal stays in range
  });
});
