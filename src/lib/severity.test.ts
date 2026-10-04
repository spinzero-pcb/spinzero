import { describe, expect, it } from "vitest";
import { SEVERITIES, SEVERITY_LABEL, SEVERITY_WEIGHT } from "./severity";

// A detailed review's coverage gap has a level of its own. Filed as "info", it read as
// a remark about a part nobody had looked at.

describe("comment severities", () => {
  it("names the coverage gap for what it is", () => {
    expect(SEVERITY_LABEL.unverified).toBe("not verified");
  });

  it("ranks a gap above a remark and below a defect", () => {
    expect(SEVERITY_WEIGHT.unverified).toBeGreaterThan(SEVERITY_WEIGHT.minor);
    expect(SEVERITY_WEIGHT.unverified).toBeLessThan(SEVERITY_WEIGHT.major);
  });

  it("lists every level, least severe first", () => {
    const weights = SEVERITIES.map((s) => SEVERITY_WEIGHT[s]);
    expect(weights).toEqual([...weights].sort((a, b) => a - b));
    expect(new Set(SEVERITIES)).toEqual(new Set(Object.keys(SEVERITY_LABEL)));
  });
});
