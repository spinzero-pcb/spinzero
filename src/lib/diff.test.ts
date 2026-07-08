import {
  countByImpact,
  filterChanges,
  groupChanges,
  orderedChanges,
  pcbAnchorToCommentAnchor,
  tintRole,
  tintsA,
  tintsB,
  type Change,
  type DiffDoc,
} from "./diff";

// A small builder so each test spells out only the fields it cares about.
function ch(partial: Partial<Change> & Pick<Change, "id" | "group" | "kind">): Change {
  return {
    impact: "electrical",
    title: partial.id,
    anchors: {},
    side: "both",
    ...partial,
  } as Change;
}

describe("diff helpers", () => {
  describe("groupChanges", () => {
    it("groups by group in GROUP_ORDER and omits empty groups", () => {
      const changes = [
        ch({ id: "a", group: "routing", kind: "added" }),
        ch({ id: "b", group: "component", kind: "modified" }),
        ch({ id: "c", group: "net", kind: "removed" }),
      ];
      const groups = groupChanges(changes);
      expect(groups.map((g) => g.group)).toEqual(["component", "net", "routing"]);
      expect(groups.map((g) => g.label)).toEqual(["Components", "Nets", "Routing"]);
    });

    it("orders within a group by sheet then position", () => {
      const changes = [
        ch({ id: "s3", group: "component", kind: "modified", anchors: { schematic: { sheet: 3, uuids: [] } } }),
        ch({ id: "s1", group: "component", kind: "modified", anchors: { schematic: { sheet: 1, uuids: [] } } }),
        ch({ id: "s2", group: "component", kind: "modified", anchors: { schematic: { sheet: 2, uuids: [] } } }),
      ];
      const [group] = groupChanges(changes);
      expect(group.changes.map((c) => c.id)).toEqual(["s1", "s2", "s3"]);
    });

    it("orders PCB changes by layer then y then x", () => {
      const pcb = (layer: string, x: number, y: number) => ({ pcb: { layers: [layer], bbox: [x, y, 1, 1] as [number, number, number, number] } });
      const changes = [
        ch({ id: "back", group: "routing", kind: "added", anchors: pcb("B.Cu", 0, 0) }),
        ch({ id: "front-low", group: "routing", kind: "added", anchors: pcb("F.Cu", 5, 9) }),
        ch({ id: "front-high", group: "routing", kind: "added", anchors: pcb("F.Cu", 5, 1) }),
      ];
      const [group] = groupChanges(changes);
      // Layer name orders first (alphabetical: "B.Cu" < "F.Cu"), then y, then x.
      expect(group.changes.map((c) => c.id)).toEqual(["back", "front-high", "front-low"]);
    });

    it("is deterministic — same input, same output", () => {
      const changes = [
        ch({ id: "z", group: "net", kind: "added" }),
        ch({ id: "a", group: "component", kind: "removed" }),
      ];
      expect(JSON.stringify(groupChanges(changes))).toEqual(JSON.stringify(groupChanges(changes)));
    });
  });

  describe("orderedChanges", () => {
    it("flattens groups into a single walk sequence", () => {
      const changes = [
        ch({ id: "r1", group: "routing", kind: "added" }),
        ch({ id: "c1", group: "component", kind: "modified" }),
      ];
      expect(orderedChanges(changes).map((c) => c.id)).toEqual(["c1", "r1"]);
    });
  });

  describe("filterChanges", () => {
    const changes = [
      ch({ id: "e", group: "net", kind: "modified", impact: "electrical", title: "net /VBUS rewired" }),
      ch({ id: "p", group: "placement", kind: "moved", impact: "placement", title: "R7 moved 3mm" }),
      ch({ id: "s", group: "silk", kind: "modified", impact: "cosmetic", title: "Silk REV A → B" }),
      ch({ id: "d", group: "doc", kind: "modified", impact: "doc", title: "Rev 1.2 → 1.3" }),
    ];

    it("empty impact set shows all", () => {
      expect(filterChanges(changes, new Set(), "").map((c) => c.id)).toEqual(["e", "p", "s", "d"]);
    });

    it("filters by an active impact chip", () => {
      expect(filterChanges(changes, new Set(["electrical"]), "").map((c) => c.id)).toEqual(["e"]);
    });

    it("folds doc impact under the cosmetic chip", () => {
      // Selecting 'cosmetic' keeps both the cosmetic and the doc change (so nothing is
      // ever unreachable behind a filter).
      expect(filterChanges(changes, new Set(["cosmetic"]), "").map((c) => c.id).sort()).toEqual(["d", "s"]);
    });

    it("free-text matches title (case-insensitive)", () => {
      expect(filterChanges(changes, new Set(), "vbus").map((c) => c.id)).toEqual(["e"]);
    });

    it("combines impact + text", () => {
      expect(filterChanges(changes, new Set(["placement"]), "R7").map((c) => c.id)).toEqual(["p"]);
      expect(filterChanges(changes, new Set(["electrical"]), "R7")).toEqual([]);
    });
  });

  describe("countByImpact", () => {
    it("reads the doc stats when present", () => {
      const doc = { stats: { electrical: 3, placement: 6 }, changes: [] } as unknown as DiffDoc;
      expect(countByImpact(doc)).toEqual({ electrical: 3, placement: 6, cosmetic: 0, doc: 0 });
    });
    it("falls back to counting the change list when stats are empty", () => {
      const doc = {
        stats: {},
        changes: [
          ch({ id: "a", group: "net", kind: "added", impact: "electrical" }),
          ch({ id: "b", group: "silk", kind: "added", impact: "cosmetic" }),
        ],
      } as unknown as DiffDoc;
      expect(countByImpact(doc)).toEqual({ electrical: 1, placement: 0, cosmetic: 1, doc: 0 });
    });
  });

  describe("tintRole", () => {
    it("maps kind to the CSS-var colour role", () => {
      expect(tintRole("removed")).toBe("err");
      expect(tintRole("added")).toBe("ok");
      expect(tintRole("modified")).toBe("warn");
      expect(tintRole("renamed")).toBe("warn");
      expect(tintRole("moved")).toBe("warn");
    });
  });

  describe("tintsA / tintsB", () => {
    it("removed tints A only", () => {
      const c = ch({ id: "x", group: "component", kind: "removed", side: "a" });
      expect(tintsA(c)).toBe(true);
      expect(tintsB(c)).toBe(false);
    });
    it("added tints B only", () => {
      const c = ch({ id: "x", group: "component", kind: "added", side: "b" });
      expect(tintsA(c)).toBe(false);
      expect(tintsB(c)).toBe(true);
    });
    it("modified tints both", () => {
      const c = ch({ id: "x", group: "component", kind: "modified", side: "both" });
      expect(tintsA(c)).toBe(true);
      expect(tintsB(c)).toBe(true);
    });
  });

  describe("pcbAnchorToCommentAnchor", () => {
    it("prefers a net anchor", () => {
      const c = ch({ id: "x", group: "routing", kind: "added", anchors: { pcb: { net: "/GND", comp: "U1", bbox: [1, 2, 3, 4] } } });
      expect(pcbAnchorToCommentAnchor(c)).toEqual({ type: "net", ref: "/GND" });
    });
    it("falls back to a component anchor", () => {
      const c = ch({ id: "x", group: "placement", kind: "moved", anchors: { pcb: { comp: "R7", bbox: [1, 2, 3, 4] } } });
      expect(pcbAnchorToCommentAnchor(c)).toEqual({ type: "component", ref: "R7" });
    });
    it("falls back to a bbox region anchor", () => {
      const c = ch({ id: "ch7", group: "outline", kind: "modified", anchors: { pcb: { bbox: [10, 20, 5, 6] } } });
      expect(pcbAnchorToCommentAnchor(c)).toEqual({
        type: "region",
        ref: "ch7",
        rect: { x: 10, y: 20, w: 5, h: 6 },
      });
    });
    it("returns null when there is no PCB anchor", () => {
      const c = ch({ id: "x", group: "net", kind: "added", anchors: { schematic: { sheet: 1, uuids: [] } } });
      expect(pcbAnchorToCommentAnchor(c)).toBeNull();
    });
  });
});
