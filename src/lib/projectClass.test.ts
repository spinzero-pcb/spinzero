import { describe, expect, it } from "vitest";

import { BOM_PROFILES } from "./findings";
import { bomProfileForClass, normalizeClass, PROJECT_CLASSES, projectClassLabel } from "./projectClass";

// Automotive is two answers in the rule pack, so it is two answers here. A project
// that stored the single old `automotive` reads as the stricter half, because that
// is what the review runs it as.

describe("the automotive split", () => {
  it("offers both halves and not the retired one", () => {
    const values = PROJECT_CLASSES.map((c) => c.value);
    expect(values).toContain("automotive-comfort");
    expect(values).toContain("automotive-safety");
    expect(values).not.toContain("automotive");
  });

  it("maps each half to its own rule profile", () => {
    expect(bomProfileForClass("automotive-comfort")).toBe("automotive-comfort");
    expect(bomProfileForClass("automotive-safety")).toBe("automotive-safety");
  });

  it("reads the retired id as the safety half", () => {
    expect(normalizeClass("automotive")).toBe("automotive-safety");
    expect(bomProfileForClass("automotive")).toBe("automotive-safety");
    expect(projectClassLabel("automotive")).toBe("Automotive Powertrain/Safety");
  });

  it("labels each half the way the review labels its profile", () => {
    for (const id of ["automotive-comfort", "automotive-safety"] as const) {
      const cls = PROJECT_CLASSES.find((c) => c.value === id)?.label;
      const profile = BOM_PROFILES.find((p) => p.id === id)?.label;
      expect(cls).toBe(profile);
    }
  });

  it("falls back to general for anything unknown", () => {
    expect(normalizeClass(null)).toBe("general");
    expect(normalizeClass("rocket")).toBe("general");
  });
});
