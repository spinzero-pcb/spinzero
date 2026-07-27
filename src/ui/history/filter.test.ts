import { describe, expect, it } from "vitest";
import { branchTips, filterRevisions, initials, parentOf, refColumns, rowText } from "./filter";
import type { ExtractionMeta } from "../../lib/types";

function rev(p: Partial<ExtractionMeta> & { id: string }): ExtractionMeta {
  return {
    label: null,
    message: null,
    created_at: "2026-07-20T10:00:00Z",
    design_tool: null,
    git_hash: null,
    git_branch: null,
    git_dirty: null,
    parents: [],
    tags: [],
    hidden: false,
    published: true,
    is_checkpoint: false,
    author: null,
    ...p,
  };
}

describe("rowText", () => {
  it("prefers a manual rename over the changelog", () => {
    expect(rowText(rev({ id: "a", label: "Fab release", message: "reroute CAN" }))).toBe(
      "Fab release",
    );
  });

  it("falls back to the changelog, then the timestamp", () => {
    expect(rowText(rev({ id: "a", message: "reroute CAN" }))).toBe("reroute CAN");
    // No label and no message → the formatted timestamp, not an empty row.
    expect(rowText(rev({ id: "a" }))).not.toBe("");
  });
});

describe("filterRevisions", () => {
  const revs = [
    rev({ id: "aaaa111111zz", message: "Reroute CAN pair", author: "priya", tags: ["fab-v1"] }),
    rev({ id: "bbbb222222zz", label: "Termination fix", author: "arjun" }),
    rev({ id: "cccc333333zz", message: "Move U7" }),
  ];

  it("returns everything for a blank query", () => {
    expect(filterRevisions(revs, "")).toHaveLength(3);
    expect(filterRevisions(revs, "   ")).toHaveLength(3);
  });

  it("matches subject, author, tag and short id, case-insensitively", () => {
    expect(filterRevisions(revs, "reroute").map((r) => r.id)).toEqual(["aaaa111111zz"]);
    expect(filterRevisions(revs, "ARJUN").map((r) => r.id)).toEqual(["bbbb222222zz"]);
    expect(filterRevisions(revs, "fab-v1").map((r) => r.id)).toEqual(["aaaa111111zz"]);
    expect(filterRevisions(revs, "cccc3333").map((r) => r.id)).toEqual(["cccc333333zz"]);
  });

  it("searches the label when there is one, not the shadowed message", () => {
    expect(filterRevisions(revs, "Termination").map((r) => r.id)).toEqual(["bbbb222222zz"]);
  });

  it("preserves input order so the DAG layout stays row-aligned", () => {
    expect(filterRevisions(revs, "e").map((r) => r.id)).toEqual(
      revs.filter((r) => filterRevisions([r], "e").length).map((r) => r.id),
    );
  });
});

describe("refColumns", () => {
  const a = rev({ id: "a" });
  const b = rev({ id: "b" });

  it("is 0 when neither pointer is in the list — the slot costs no width", () => {
    expect(refColumns([a, b], "zzz", "yyy")).toBe(0);
  });

  it("is 1 when the pointers sit on different rows", () => {
    expect(refColumns([a, b], "a", "b")).toBe(1);
  });

  it("is 2 when one row carries both pointers", () => {
    expect(refColumns([a, b], "a", "a")).toBe(2);
  });

  it("ignores pointers filtered out of view", () => {
    expect(refColumns([b], "a", "a")).toBe(0);
  });
});

describe("branchTips", () => {
  it("returns the single head of a linear history", () => {
    const revs = [rev({ id: "c", parents: ["b"] }), rev({ id: "b", parents: ["a"] }), rev({ id: "a" })];
    expect(branchTips(revs).map((r) => r.id)).toEqual(["c"]);
  });

  it("returns both heads of a fork", () => {
    const revs = [
      rev({ id: "c", parents: ["a"] }),
      rev({ id: "b", parents: ["a"] }),
      rev({ id: "a" }),
    ];
    expect(branchTips(revs).map((r) => r.id)).toEqual(["c", "b"]);
  });

  it("skips tombstoned revisions so a deleted tip doesn't fake a fork", () => {
    const revs = [
      rev({ id: "c", parents: ["a"], hidden: true }),
      rev({ id: "b", parents: ["a"] }),
      rev({ id: "a" }),
    ];
    expect(branchTips(revs).map((r) => r.id)).toEqual(["b"]);
  });
});

describe("parentOf", () => {
  const all = [rev({ id: "b", parents: ["a", "missing"] }), rev({ id: "a" })];

  it("picks the first parent that is actually present locally", () => {
    expect(parentOf(all[0], all)).toBe("a");
  });

  it("is null for a root revision", () => {
    expect(parentOf(all[1], all)).toBeNull();
  });

  it("is null when every parent is absent (not synced yet)", () => {
    const orphan = rev({ id: "z", parents: ["nope"] });
    expect(parentOf(orphan, all)).toBeNull();
  });
});

describe("initials", () => {
  it("handles single names, full names and email-ish authors", () => {
    expect(initials("priya")).toBe("PR");
    expect(initials("Priya Nair")).toBe("PN");
    expect(initials("p.nair@example.com")).toBe("PN");
  });

  it("degrades to ? rather than rendering an empty circle", () => {
    expect(initials(null)).toBe("?");
    expect(initials("   ")).toBe("?");
  });
});
