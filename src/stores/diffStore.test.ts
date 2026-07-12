import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { useDiffStore, normalizeOrder } from "./diffStore";
import { useProjectStore } from "./projectStore";
import { useViewStore } from "./viewStore";
import type { DiffDoc } from "../lib/diff";
import type { ExtractionMeta } from "../lib/types";

function meta(id: string, parents: string[], created_at: string): ExtractionMeta {
  return {
    id,
    label: null,
    message: null,
    created_at,
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
  };
}

const DOC: DiffDoc = {
  schema: "diff.a0",
  a: { rev: "rA", label: "older" },
  b: { rev: "rB", label: "newer" },
  changes: [
    { id: "ch_0", group: "component", kind: "modified", impact: "electrical", title: "C14 100n → 1µ", anchors: { schematic: { sheet: 2, uuids: ["u1"] } }, side: "both" },
    { id: "ch_1", group: "routing", kind: "added", impact: "electrical", title: "/VBUS rerouted", anchors: { pcb: { net: "/VBUS", layers: ["In2.Cu"] } }, side: "b" },
    { id: "ch_2", group: "component", kind: "removed", impact: "electrical", title: "R7 removed", anchors: { schematic: { sheet: 1, uuids: ["u2"] } }, side: "a" },
  ],
  stats: { electrical: 3 },
  sheetsPruned: [4],
};

/** Reset the diff store + a minimal project store to a known base before each test. */
function reset() {
  useDiffStore.setState({
    active: false,
    a: null,
    cacheKeyA: null,
    b: null,
    cacheKeyB: null,
    doc: null,
    mode: "sideBySide",
    focusedChangeId: null,
    seen: new Set(),
    preparing: false,
    priorActive: null,
    sheetSvgA: new Map(),
  });
  useViewStore.setState({ view: "schematic" });
}

describe("normalizeOrder", () => {
  const extractions = [
    meta("c", ["b"], "2026-03-03"),
    meta("b", ["a"], "2026-02-02"),
    meta("a", [], "2026-01-01"),
  ];

  it("puts an ancestor first regardless of argument order", () => {
    expect(normalizeOrder("c", "a", extractions)).toEqual(["a", "c"]);
    expect(normalizeOrder("a", "c", extractions)).toEqual(["a", "c"]);
  });

  it("falls back to timestamp for cross-branch (no ancestry) pairs", () => {
    // Two tips off a shared root: x (newer) and y (older), neither an ancestor of the other.
    const forked = [
      meta("x", ["root"], "2026-05-05"),
      meta("y", ["root"], "2026-04-04"),
      meta("root", [], "2026-01-01"),
    ];
    expect(normalizeOrder("x", "y", forked)).toEqual(["y", "x"]);
  });

  it("leaves unknown ids untouched", () => {
    expect(normalizeOrder("zzz", "b", extractions)).toEqual(["zzz", "b"]);
  });
});

describe("diffStore enter/exit/step/seen", () => {
  beforeEach(() => {
    reset();
    // B ("rB") is already active so enterDiff skips the setActiveExtraction viewer switch.
    useProjectStore.setState({
      extractions: [meta("rB", ["rA"], "2026-02-02"), meta("rA", [], "2026-01-01")],
      activeExtraction: "rB",
    });
  });
  afterEach(() => clearMocks());

  it("enterDiff normalizes order, loads the doc, and shows the overview (no auto-focus)", async () => {
    const calls: Array<{ cmd: string; args: unknown }> = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "prepare_diff") {
        return {
          doc: DOC,
          path: "/diffs/x.json",
          cache_key_a: "keyA",
          cache_key_b: "keyB",
          label_a: "older",
          label_b: "newer",
        };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    // Pass args reversed to prove normalization runs (rB is newer, rA older).
    await useDiffStore.getState().enterDiff("rB", "rA");

    const s = useDiffStore.getState();
    expect(s.active).toBe(true);
    expect(s.a?.rev).toBe("rA");
    expect(s.b?.rev).toBe("rB");
    expect(s.cacheKeyA).toBe("keyA");
    expect(s.doc?.changes.length).toBe(3);
    // prepare_diff was called old → new.
    expect(calls).toContainEqual({ cmd: "prepare_diff", args: { revA: "rA", revB: "rB" } });
    // Overview by default: nothing focused, every change visible on the board.
    expect(s.focusedChangeId).toBeNull();
    expect(s.hiddenChangeIds.size).toBe(0);
    expect(useViewStore.getState().view).toBe("schematic");
  });

  it("exitDiff drops all comparison state", async () => {
    mockIPC((cmd) => {
      if (cmd === "prepare_diff")
        return { doc: DOC, path: "p", cache_key_a: "keyA", cache_key_b: "keyB", label_a: "older", label_b: "newer" };
      if (cmd === "set_active_extraction") return undefined; // pure viewer switch, void
      // set_active_extraction fan-out (refreshIndex/load) — return benign shapes.
      return null;
    });
    await useDiffStore.getState().enterDiff("rA", "rB");
    expect(useDiffStore.getState().active).toBe(true);

    useDiffStore.getState().exitDiff();
    const s = useDiffStore.getState();
    expect(s.active).toBe(false);
    expect(s.doc).toBeNull();
    expect(s.a).toBeNull();
    expect(s.cacheKeyA).toBeNull();
    expect(s.seen.size).toBe(0);
  });

  it("next/prev walk the ordered change sequence", async () => {
    mockIPC((cmd) => {
      if (cmd === "prepare_diff")
        return { doc: DOC, path: "p", cache_key_a: "keyA", cache_key_b: "keyB", label_a: "older", label_b: "newer" };
      throw new Error(cmd);
    });
    await useDiffStore.getState().enterDiff("rA", "rB");
    // Nothing focused on enter; the first `next` lands on the first ordered change.
    // Ordered walk: ch_2 (comp, sheet1), ch_0 (comp, sheet2), ch_1 (routing).
    expect(useDiffStore.getState().focusedChangeId).toBeNull();
    useDiffStore.getState().next();
    expect(useDiffStore.getState().focusedChangeId).toBe("ch_2");
    useDiffStore.getState().next();
    expect(useDiffStore.getState().focusedChangeId).toBe("ch_0");
    useDiffStore.getState().next();
    expect(useDiffStore.getState().focusedChangeId).toBe("ch_1");
    useDiffStore.getState().next(); // clamps at the end
    expect(useDiffStore.getState().focusedChangeId).toBe("ch_1");
    useDiffStore.getState().prev();
    expect(useDiffStore.getState().focusedChangeId).toBe("ch_0");
  });

  it("focusChange solos the change; showAllChanges restores the overview", async () => {
    mockIPC((cmd) => {
      if (cmd === "prepare_diff")
        return { doc: DOC, path: "p", cache_key_a: "keyA", cache_key_b: "keyB", label_a: "older", label_b: "newer" };
      throw new Error(cmd);
    });
    await useDiffStore.getState().enterDiff("rA", "rB");
    useDiffStore.getState().focusChange("ch_1");
    expect(useDiffStore.getState().hiddenChangeIds).toEqual(new Set(["ch_0", "ch_2"]));
    // Focusing counts as reviewing — progress tracks what the walk visited.
    expect(useDiffStore.getState().seen.has("ch_1")).toBe(true);
    // Shift-click (toggleChangeHidden) builds subsets from either state.
    useDiffStore.getState().toggleChangeHidden("ch_0");
    expect(useDiffStore.getState().hiddenChangeIds).toEqual(new Set(["ch_2"]));
    useDiffStore.getState().showAllChanges();
    expect(useDiffStore.getState().hiddenChangeIds.size).toBe(0);
  });

  it("focusChange on a PCB-anchored change switches to the PCB view", async () => {
    mockIPC((cmd) => {
      if (cmd === "prepare_diff")
        return { doc: DOC, path: "p", cache_key_a: "keyA", cache_key_b: "keyB", label_a: "older", label_b: "newer" };
      throw new Error(cmd);
    });
    await useDiffStore.getState().enterDiff("rA", "rB");
    useDiffStore.getState().focusChange("ch_1"); // routing / PCB anchor
    expect(useViewStore.getState().view).toBe("pcb");
  });

  it("markSeen and markGroupSeen track ephemeral seen state", async () => {
    mockIPC((cmd) => {
      if (cmd === "prepare_diff")
        return { doc: DOC, path: "p", cache_key_a: "keyA", cache_key_b: "keyB", label_a: "older", label_b: "newer" };
      throw new Error(cmd);
    });
    await useDiffStore.getState().enterDiff("rA", "rB");

    useDiffStore.getState().markSeen("ch_0");
    expect(useDiffStore.getState().seen.has("ch_0")).toBe(true);
    useDiffStore.getState().markSeen("ch_0", false);
    expect(useDiffStore.getState().seen.has("ch_0")).toBe(false);

    useDiffStore.getState().markGroupSeen(["ch_0", "ch_2"]);
    expect(useDiffStore.getState().seen.has("ch_0")).toBe(true);
    expect(useDiffStore.getState().seen.has("ch_2")).toBe(true);
    useDiffStore.getState().markGroupSeen(["ch_0", "ch_2"], false);
    expect(useDiffStore.getState().seen.size).toBe(0);
  });

  it("getSheetSvgA reads from the A cache key and memoises", async () => {
    let reads = 0;
    mockIPC((cmd, args) => {
      if (cmd === "prepare_diff")
        return { doc: DOC, path: "p", cache_key_a: "keyA", cache_key_b: "keyB", label_a: "older", label_b: "newer" };
      if (cmd === "read_artifact_from") {
        reads++;
        expect((args as { cacheKey: string }).cacheKey).toBe("keyA");
        return "<svg/>";
      }
      throw new Error(cmd);
    });
    await useDiffStore.getState().enterDiff("rA", "rB");
    const p1 = useDiffStore.getState().getSheetSvgA(2, "schematics/2.svg");
    const p2 = useDiffStore.getState().getSheetSvgA(2, "schematics/2.svg");
    expect(await p1).toBe("<svg/>");
    await p2;
    expect(reads).toBe(1); // second call served from the memoised promise
  });
});
