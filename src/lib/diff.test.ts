import {
  filterChanges,
  groupChanges,
  orderedChanges,
  pcbLayerUnion,
  tintRole,
  tintsA,
  tintsB,
  type Change,
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
    it("groups by impact in IMPACT_ORDER and omits empty buckets", () => {
      const changes = [
        ch({ id: "a", group: "silk", kind: "added", impact: "cosmetic" }),
        ch({ id: "b", group: "component", kind: "modified", impact: "electrical" }),
        ch({ id: "c", group: "placement", kind: "moved", impact: "placement" }),
      ];
      const groups = groupChanges(changes);
      expect(groups.map((g) => g.impact)).toEqual(["electrical", "placement", "cosmetic"]);
      expect(groups.map((g) => g.label)).toEqual(["Electrical", "Placement", "Cosmetic"]);
    });

    it("folds doc impact into the Cosmetic bucket", () => {
      const changes = [
        ch({ id: "d", group: "doc", kind: "modified", impact: "doc" }),
        ch({ id: "s", group: "silk", kind: "modified", impact: "cosmetic" }),
      ];
      const groups = groupChanges(changes);
      expect(groups.map((g) => g.impact)).toEqual(["cosmetic"]);
      expect(groups[0].changes.map((c) => c.id).sort()).toEqual(["d", "s"]);
    });

    it("orders within a bucket by sheet then position", () => {
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
    it("flattens buckets into a single walk sequence (electrical first)", () => {
      const changes = [
        ch({ id: "s1", group: "silk", kind: "added", impact: "cosmetic" }),
        ch({ id: "c1", group: "component", kind: "modified", impact: "electrical" }),
      ];
      expect(orderedChanges(changes).map((c) => c.id)).toEqual(["c1", "s1"]);
    });
  });

  describe("filterChanges", () => {
    const changes = [
      ch({ id: "e", group: "net", kind: "modified", impact: "electrical", title: "net /VBUS rewired" }),
      ch({ id: "p", group: "placement", kind: "moved", impact: "placement", title: "R7 moved 3mm" }),
      ch({ id: "s", group: "silk", kind: "modified", impact: "cosmetic", title: "Silk REV A → B" }),
    ];

    it("empty query shows all", () => {
      expect(filterChanges(changes, "").map((c) => c.id)).toEqual(["e", "p", "s"]);
    });

    it("free-text matches title (case-insensitive)", () => {
      expect(filterChanges(changes, "vbus").map((c) => c.id)).toEqual(["e"]);
    });

    it("matches detail text too", () => {
      const withDetail = [ch({ id: "d", group: "net", kind: "modified", title: "GND", detail: "stitching vias" })];
      expect(filterChanges(withDetail, "stitching").map((c) => c.id)).toEqual(["d"]);
      expect(filterChanges(withDetail, "nothing")).toEqual([]);
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

  describe("pcbLayerUnion", () => {
    const known = ["F.Cu", "In1.Cu", "B.Cu", "F.SilkS", "Edge.Cuts"];
    it("unions the changes' layers in known-table order, adding Edge.Cuts for context", () => {
      const changes = [
        ch({ id: "a", group: "routing", kind: "added", anchors: { pcb: { layers: ["B.Cu"] } } }),
        ch({ id: "b", group: "silk", kind: "modified", anchors: { pcb: { layers: ["F.SilkS"] } } }),
        ch({ id: "c", group: "routing", kind: "removed", anchors: { pcb: { layers: ["B.Cu"] } } }),
      ];
      expect(pcbLayerUnion(changes, known)).toEqual(["B.Cu", "F.SilkS", "Edge.Cuts"]);
    });
    it("ignores layers the board doesn't have and returns [] when nothing lands", () => {
      const changes = [
        ch({ id: "a", group: "routing", kind: "added", anchors: { pcb: { layers: ["In9.Cu"] } } }),
        ch({ id: "b", group: "net", kind: "added", anchors: { schematic: { sheet: 1, uuids: [] } } }),
      ];
      expect(pcbLayerUnion(changes, known)).toEqual([]);
    });
  });
});
