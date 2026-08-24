import {
  digestOf,
  isStale,
  reviewKind,
  REVIEW_KINDS,
  sanitizeRun,
  sanitizeRuns,
  type ReviewKind,
} from "./reviewCatalog";

const bom = reviewKind("bom")!;
const datasheets = reviewKind("datasheets")!;
const schematic = reviewKind("schematic")!;

describe("catalogue", () => {
  it("lists unbuilt reviews rather than hiding them", () => {
    // The picker doubles as the coverage readout, so a review that is absent reads
    // like one that passed. Unbuilt entries are present and flagged.
    expect(REVIEW_KINDS.length).toBeGreaterThan(2);
    expect(REVIEW_KINDS.some((k) => !k.ready)).toBe(true);
    expect(bom.ready).toBe(true);
  });

  it("gives every review at least one input, so staleness is always decidable", () => {
    for (const k of REVIEW_KINDS) expect(k.inputs.length).toBeGreaterThan(0);
  });
});

describe("digestOf", () => {
  it("is stable for the same lines and changes when one does", () => {
    expect(digestOf(["a", "b"])).toBe(digestOf(["a", "b"]));
    expect(digestOf(["a", "b"])).not.toBe(digestOf(["a", "c"]));
  });

  it("separates lines, so a moved boundary is a change", () => {
    expect(digestOf(["ab", "c"])).not.toBe(digestOf(["a", "bc"]));
  });
});

describe("isStale", () => {
  it("treats a never-run review as unrun, not stale", () => {
    expect(isStale(bom, undefined, { bom: "x" })).toBe(false);
  });

  it("goes stale when an input it reads has moved", () => {
    const run = { ts: "2026-08-23T00:00:00Z", inputs: { bom: "aaa" } };
    expect(isStale(bom, run, { bom: "bbb" })).toBe(true);
    expect(isStale(bom, run, { bom: "aaa" })).toBe(false);
  });

  it("ignores movement in an input it does not read", () => {
    // The whole point of per-review staleness: a schematic edit must not invalidate
    // a review that only ever read the BOM.
    const run = { ts: "2026-08-23T00:00:00Z", inputs: { bom: "aaa" } };
    expect(isStale(bom, run, { bom: "aaa", schematic: "moved" })).toBe(false);
    expect(schematic.inputs).toContain("schematic");
    expect(schematic.inputs).not.toContain("bom");
  });

  it("stales a multi-input review when any one of its inputs moves", () => {
    const run = { ts: "2026-08-23T00:00:00Z", inputs: { bom: "aaa", datasheets: "ddd" } };
    expect(isStale(datasheets, run, { bom: "aaa", datasheets: "eee" })).toBe(true);
  });

  it("never claims staleness for an input it cannot digest", () => {
    // A missing digest is ignorance, not evidence — the launcher must not cry wolf.
    const run = { ts: "2026-08-23T00:00:00Z", inputs: {} };
    expect(isStale(bom, run, { bom: "bbb" })).toBe(false);
    const run2 = { ts: "2026-08-23T00:00:00Z", inputs: { bom: "aaa" } };
    expect(isStale(bom, run2, {})).toBe(false);
  });
});

describe("sanitizeRun", () => {
  it("accepts a well-formed record", () => {
    const r = sanitizeRun({ ts: "2026-08-23T00:00:00Z", extraction_id: "e1", inputs: { bom: "aaa" } });
    expect(r).toEqual({ ts: "2026-08-23T00:00:00Z", extraction_id: "e1", inputs: { bom: "aaa" } });
  });

  it("rejects records with no timestamp — settings are hand-editable", () => {
    expect(sanitizeRun(null)).toBeNull();
    expect(sanitizeRun("nope")).toBeNull();
    expect(sanitizeRun({ inputs: { bom: "a" } })).toBeNull();
    expect(sanitizeRun({ ts: 7 })).toBeNull();
  });

  it("drops unknown inputs and non-string digests", () => {
    const r = sanitizeRun({ ts: "t", inputs: { bom: "aaa", gerbers: "x", schematic: 3 } });
    expect(r?.inputs).toEqual({ bom: "aaa" });
  });

  it("defaults a missing extraction id rather than throwing", () => {
    expect(sanitizeRun({ ts: "t" })?.extraction_id).toBeNull();
  });
});

describe("sanitizeRuns", () => {
  it("keeps only known review ids", () => {
    const runs = sanitizeRuns({
      bom: { ts: "t1" },
      not_a_review: { ts: "t2" },
      schematic: { ts: "t3" },
    });
    expect(Object.keys(runs).sort()).toEqual(["bom", "schematic"]);
  });

  it("survives junk", () => {
    expect(sanitizeRuns(undefined)).toEqual({});
    expect(sanitizeRuns("nope")).toEqual({});
    expect(sanitizeRuns({ bom: 4 })).toEqual({});
  });
});

describe("reviewKind", () => {
  it("resolves known ids and nothing else", () => {
    expect(reviewKind("bom")?.label).toBe("BOM Review");
    expect(reviewKind("nope" as ReviewKind["id"])).toBeUndefined();
  });
});
