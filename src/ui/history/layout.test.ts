import { describe, expect, it } from "vitest";
import { layoutDag } from "./layout";
import type { ExtractionMeta } from "../../lib/types";

function rev(id: string, parents: string[], extra: Partial<ExtractionMeta> = {}): ExtractionMeta {
  return {
    id,
    label: null,
    message: null,
    created_at: "2026-06-22T00:00:00Z",
    design_tool: "kicad",
    git_hash: null,
    git_branch: null,
    git_dirty: null,
    parents,
    tags: [],
    hidden: false,
    published: true,
    is_checkpoint: false,
    author: null,
    ...extra,
  };
}

describe("layoutDag", () => {
  it("lays a linear history in a single lane", () => {
    const revs = [rev("c", ["b"]), rev("b", ["a"]), rev("a", [])];
    const { nodes, edges, laneCount } = layoutDag(revs, false);
    expect(laneCount).toBe(1);
    expect(nodes.every((n) => n.lane === 0)).toBe(true);
    expect(edges).toHaveLength(2);
    expect(edges.every((e) => e.kind === "normal")).toBe(true);
  });

  it("branches a fork (two children of one parent) into separate lanes", () => {
    // a is the shared parent of both b and c → two tips converge on a.
    const revs = [rev("c", ["a"]), rev("b", ["a"]), rev("a", [])];
    const { nodes, laneCount } = layoutDag(revs, false);
    expect(laneCount).toBe(2);
    const lane = (id: string) => nodes.find((n) => n.meta.id === id)!.lane;
    expect(lane("c")).toBe(0);
    expect(lane("b")).toBe(1);
    expect(lane("a")).toBe(0); // the shared parent collapses back to the leftmost lane
  });

  it("marks a >1-parent convergence node's edges as merge", () => {
    const revs = [rev("m", ["a", "b"]), rev("b", []), rev("a", [])];
    const { edges } = layoutDag(revs, false);
    const fromM = edges.filter((e) => e.fromRow === 0);
    expect(fromM).toHaveLength(2);
    expect(fromM.every((e) => e.kind === "merge")).toBe(true);
  });

  it("drops hidden revisions and their edges unless showHidden", () => {
    const revs = [rev("c", ["b"]), rev("b", ["a"], { hidden: true }), rev("a", [])];
    const hiddenOff = layoutDag(revs, false);
    expect(hiddenOff.nodes.map((n) => n.meta.id)).toEqual(["c", "a"]);
    // c's parent b is filtered out → that edge is omitted (not a pending stub).
    expect(hiddenOff.edges.some((e) => e.kind === "pending")).toBe(false);
    const hiddenOn = layoutDag(revs, true);
    expect(hiddenOn.nodes).toHaveLength(3);
  });

  it("renders a pending stub for a parent absent from the log", () => {
    const revs = [rev("b", ["missing-root"])];
    const { edges } = layoutDag(revs, false);
    expect(edges).toHaveLength(1);
    expect(edges[0].kind).toBe("pending");
  });
});
