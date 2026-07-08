import { describe, expect, it } from "vitest";
import { sheetMatches, type SheetLite, type SheetRef } from "./design";

// Layer-1 coverage (docs/testing.md) for the sheet cross-reference used to line up
// the design-JSON sheet list with the sidebar's SQLite-indexed sheet refs. They number
// sheets differently, so the match goes by SVG filename first, then by normalised name.

const sheet = (over: Partial<SheetLite>): SheetLite => ({ num: 1, name: "Power", svg: null, ...over });
const ref = (over: Partial<SheetRef>): SheetRef => ({ number: 1, name: "Power", ...over });

describe("sheetMatches", () => {
  it("matches by SVG basename regardless of differing directory prefixes", () => {
    expect(
      sheetMatches(
        sheet({ svg: "schematics/03_Power.svg" }),
        ref({ number: 3, svg_path: "design/schematics/03_Power.svg" }),
      ),
    ).toBe(true);
  });

  it("does not match two different SVG files even when names look similar", () => {
    expect(
      sheetMatches(
        sheet({ name: "Power", svg: "schematics/03_Power.svg" }),
        ref({ name: "Power", svg_path: "schematics/04_PowerAux.svg" }),
      ),
    ).toBe(false);
  });

  it("falls back to a normalised name match when no SVG path is available", () => {
    // normName lower-cases and strips spaces/underscores, so these are the same sheet.
    expect(
      sheetMatches(sheet({ name: "User Input/Output", svg: null }), ref({ name: "user_input/output" })),
    ).toBe(true);
    expect(sheetMatches(sheet({ name: "Power" }), ref({ name: "DCDC" }))).toBe(false);
  });
});
