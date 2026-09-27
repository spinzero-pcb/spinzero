import { describe, expect, it } from "vitest";

import {
  coverageGaps,
  executionSummary,
  partCoverage,
  reviewTallies,
  runHealthSummary,
  severityCounts,
  type FindingsDoc,
} from "./findings";

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
    expect(out?.detail).toContain("Reviewed by your AI agent");
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

// ---- what the review could NOT do ----------------------------------------
//
// These derivations are a port of the review page's own (`local/report.ts`). They are
// pinned here because the failure they prevent is silent: a review that checked half
// the board and filed nothing reads as a pass everywhere that counts findings only.

const AUDITED: FindingsDoc = {
  ...DOC,
  stats: { item_count: 88, finding_count: 0, duration_ms: 0 },
  bom_audit: [
    { item: "Distributor part data", result: "OK", note: "88 of 88 resolved." },
    {
      item: "Datasheet coverage",
      result: "GAP",
      note: "11 datasheets could not be obtained: ADI, ST and Molex refused the connection. Nothing on those parts was verified against a document.",
    },
    {
      item: "Part verification",
      result: "GAP",
      note: "77 of 88 part number(s) were accounted for. 11 were never looked at: X1, X2. Absence of a finding on those parts means nobody checked them.",
    },
    // Named as not-coverage: it is a fact about the BOM and is already a finding.
    { item: "RoHS compliance", result: "GAP", note: "The RoHS column is blank on all 88 rows." },
    // Our own pipeline's business, which is telemetry and not the engineer's problem.
    { item: "Rule candidates", result: "GAP", note: "3 candidates were dismissed." },
    { item: "Rule BOM-014", result: "GAP", note: "dismissed as a false positive." },
  ],
};

describe("coverageGaps", () => {
  it("lists what the run could not do, and drops what is not about coverage", () => {
    const items = coverageGaps(AUDITED).map((g) => g.item);
    expect(items).toEqual(["Datasheet coverage", "Part verification"]);
  });

  it("shortens a paragraph to its first sentence, keeping the whole note behind it", () => {
    const gap = coverageGaps(AUDITED)[0];
    expect(gap.note).toBe("11 datasheets could not be obtained: ADI, ST and Molex refused the connection.");
    expect(gap.full).toContain("Nothing on those parts was verified");
  });

  it("finds nothing in a document with no audit trail", () => {
    expect(coverageGaps(DOC)).toEqual([]);
    expect(coverageGaps(null)).toEqual([]);
  });
});

describe("partCoverage", () => {
  it("is the one sentence that says how much of the board was accounted for", () => {
    expect(partCoverage(AUDITED)).toBe("77 of 88 part number(s) were accounted for.");
    expect(partCoverage(DOC)).toBeNull();
  });
});

describe("reviewTallies", () => {
  it("counts the blind spots as well as the findings", () => {
    // The whole reason the third number exists: no findings, and two things nobody
    // checked. Counting findings alone reads this document as a clean board.
    expect(reviewTallies(AUDITED)).toEqual({
      critical: 0,
      nonCritical: 0,
      notVerified: 2,
      stalled: false,
    });
  });

  it("calls a run with a degraded stage stalled, which changes the heading", () => {
    const t = reviewTallies({ ...AUDITED, run_health: [{ stage: "judgment_pass", status: "failed" }] });
    expect(t.stalled).toBe(true);
  });
});

describe("runHealthSummary", () => {
  it("says nothing about a run whose stages all ran", () => {
    expect(runHealthSummary(AUDITED)).toBeNull();
  });

  it("names the worst stage in the user's words, never as a stage id", () => {
    const out = runHealthSummary({
      ...DOC,
      run_health: [
        { stage: "fetch_datasheets", status: "degraded", detail: "11 missing" },
        { stage: "judgment_pass", status: "failed", detail: "ruled on nothing" },
      ],
    });
    expect(out?.text).toBe("Reviewing against datasheets failed (+1 more)");
    expect(out?.text).not.toContain("judgment_pass");
    expect(out?.detail).toContain("Collecting datasheets degraded: 11 missing");
  });
});
