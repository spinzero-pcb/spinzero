import { describe, expect, it } from "vitest";

import { executionSummary, severityCounts, type FindingsDoc } from "./findings";

const DOC: FindingsDoc = {
  schema_version: "1.1",
  engine_version: "engine-test",
  pipeline: "bom-detailed",
  profile: "automotive",
  findings: [],
  bom_audit: [],
  stats: { item_count: 0, finding_count: 0, duration_ms: 0 },
};

describe("severityCounts", () => {
  it("drops the severities that are not present", () => {
    expect(severityCounts(DOC)).toEqual([]);
  });
});

describe("executionSummary", () => {
  it("says nothing on the free tier, which has nothing to disclose", () => {
    expect(executionSummary(DOC)).toBeNull();
    expect(executionSummary(null)).toBeNull();
  });

  it("leads with the content version, because that is what two runs are compared on", () => {
    // `Execution` was defined and read by nothing: the engine stamped `prompt_pack`
    // into every document and the engineer never saw it, which was half the point of
    // stamping it. Two reviews of one board that disagree are explained by a content
    // version far more often than by a regression.
    const out = executionSummary({
      ...DOC,
      execution: {
        surface: "mcp",
        model_reported: "claude-opus-5",
        prompt_pack: "pack/2026.08.27-1",
        rule_pack: "bom-rules 0.0.5",
      },
    });
    expect(out?.text).toBe("pack/2026.08.27-1");
    expect(out?.detail).toContain("Reviewed by your assistant");
    // "Reported, never verified" is a real caveat, so it is said rather than implied.
    expect(out?.detail).toContain("as reported by the client");
    expect(out?.detail).toContain("bom-rules 0.0.5");
  });

  it("falls back to the surface when no pack version was stamped", () => {
    expect(executionSummary({ ...DOC, execution: { surface: "local" } })?.text).toBe(
      "Reviewed in SpinZero",
    );
  });

  it("says out loud that the coverage gate was overridden", () => {
    // A deliberate downgrade, never a silent one: those parts were judged without
    // their datasheets and the result must not read as a full review.
    const out = executionSummary({
      ...DOC,
      execution: { surface: "mcp", prompt_pack: "builtin/abc123", allow_low_coverage: true },
    });
    expect(out?.detail).toContain("overridden");
  });
});
