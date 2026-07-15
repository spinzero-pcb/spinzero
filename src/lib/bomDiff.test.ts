import {
  bomChanges,
  bomDeltaCsv,
  changesFirstCompare,
  decorateBomRows,
  fpShort,
  lineKey,
} from "./bomDiff";
import { groupChanges } from "./diff";
import type { BomAnchor, Change } from "./diff";
import type { BomLine } from "./types";

const SEP = "\u001f";

function line(partial: Partial<BomLine> & Pick<BomLine, "item">): BomLine {
  return {
    qty: 1,
    designators: [],
    value: "",
    footprint: "",
    mpn: "",
    dnp: false,
    ...partial,
  };
}

function bomCh(
  id: string,
  kind: Change["kind"],
  anchor: Partial<BomAnchor> & Pick<BomAnchor, "key" | "designators">,
  extra?: Partial<Change>,
): Change {
  return {
    id,
    group: "bom",
    kind,
    impact: "electrical",
    title: extra?.title ?? id,
    anchors: {
      bom: { value: "", footprint: "", mpn: "", qtyA: 0, qtyB: 0, ...anchor },
    },
    side: "both",
    ...extra,
  } as Change;
}

describe("bomDiff helpers", () => {
  describe("lineKey / fpShort", () => {
    it("strips the library prefix like design.rs bom_lines", () => {
      expect(fpShort("Resistor_SMD:R_0402_1005Metric")).toBe("R_0402_1005Metric");
      expect(fpShort("R_0402")).toBe("R_0402");
    });
    it("mirrors the Rust bom_key format", () => {
      expect(lineKey("10k", "Resistor_SMD:R_0402", "MPN1")).toBe(`10k${SEP}R_0402${SEP}MPN1`);
    });
  });

  describe("decorateBomRows", () => {
    const lines = [
      line({ item: 1, value: "10k", footprint: "R_0402", designators: ["R1", "R2"], qty: 2 }),
      line({ item: 2, value: "100n", footprint: "C_0402", designators: ["C1"], qty: 1 }),
    ];

    it("tints an added line by key match", () => {
      const rows = decorateBomRows(lines, [
        bomCh("ch_1", "added", { key: lineKey("100n", "C_0402", ""), designators: ["C1"] }),
      ]);
      expect(rows.find((r) => r.line.item === 2)?.status).toBe("added");
      expect(rows.find((r) => r.line.item === 1)?.status).toBeNull();
    });

    it("falls back to designator overlap when the key doesn't match", () => {
      // The BOM artifact's MPN differs from the schematic's — key mismatch, but R1
      // is on exactly one line.
      const rows = decorateBomRows(lines, [
        bomCh("ch_1", "modified", { key: `10k${SEP}R_0402${SEP}OTHER-MPN`, designators: ["R1"] }),
      ]);
      expect(rows.find((r) => r.line.item === 1)?.status).toBe("changed");
    });

    it("synthesizes a struck-through row for a removed line (from revision A)", () => {
      const rows = decorateBomRows(lines, [
        bomCh("ch_9", "removed", {
          key: lineKey("LED", "LED_0603", ""),
          designators: ["D3"],
          value: "LED",
          footprint: "LED_0603",
          qtyA: 1,
        }),
      ]);
      const synthetic = rows.find((r) => r.synthetic);
      expect(synthetic).toBeDefined();
      expect(synthetic?.status).toBe("removed");
      expect(synthetic?.line.value).toBe("LED");
      expect(synthetic?.line.qty).toBe(1);
      expect(synthetic?.line.designators).toEqual(["D3"]);
    });

    it("collects multiple change ids on one row without downgrading 'added'", () => {
      const key = lineKey("100n", "C_0402", "");
      const rows = decorateBomRows(lines, [
        bomCh("ch_1", "added", { key, designators: ["C1"] }),
        bomCh("ch_2", "modified", { key, designators: ["C1"] }),
      ]);
      const row = rows.find((r) => r.line.item === 2)!;
      expect(row.status).toBe("added");
      expect(row.changeIds).toEqual(["ch_1", "ch_2"]);
    });

    it("ignores unmatched changes (BOM artifact / schematic disagreement)", () => {
      const rows = decorateBomRows(lines, [
        bomCh("ch_1", "modified", { key: "nope", designators: ["Z99"] }),
      ]);
      expect(rows.every((r) => r.status === null)).toBe(true);
      expect(rows).toHaveLength(2);
    });
  });

  describe("changesFirstCompare", () => {
    it("orders removed, changed, added, untouched — then by item", () => {
      const rows = decorateBomRows(
        [
          line({ item: 1, value: "a", designators: ["R1"] }),
          line({ item: 2, value: "b", designators: ["R2"] }),
          line({ item: 3, value: "c", designators: ["R3"] }),
        ],
        [
          bomCh("ch_1", "added", { key: lineKey("b", "", ""), designators: ["R2"] }),
          bomCh("ch_2", "modified", { key: lineKey("c", "", ""), designators: ["R3"] }),
          bomCh("ch_3", "removed", { key: "gone", designators: ["D9"], value: "gone" }),
        ],
      );
      const sorted = [...rows].sort(changesFirstCompare);
      expect(sorted.map((r) => r.status)).toEqual(["removed", "changed", "added", null]);
    });
  });

  describe("bomDeltaCsv", () => {
    it("emits one row per BOM change with escaping", () => {
      const csv = bomDeltaCsv([
        bomCh(
          "ch_1",
          "modified",
          {
            key: "k",
            designators: ["R1", "R33", "R34"],
            value: "10k",
            footprint: "R_0402",
            qtyA: 1,
            qtyB: 3,
            added: ["R33", "R34"],
          },
          { title: "BOM 10k R_0402: qty 1 → 3", detail: "+2: R33, R34" },
        ),
        bomCh(
          "ch_2",
          "removed",
          { key: "k2", designators: ["D3"], value: 'has "quote", comma', qtyA: 1 },
          { title: "BOM line removed: LED" },
        ),
      ]);
      const rows = csv.split("\r\n");
      expect(rows[0]).toBe(
        "status,value,footprint,mpn,qty_a,qty_b,designators_added,designators_removed,note",
      );
      expect(rows[1]).toContain("changed,10k,R_0402,,1,3,R33 R34,,");
      expect(rows[2]).toContain('"has ""quote"", comma"');
      expect(rows).toHaveLength(3);
    });

    it("returns just the header when nothing changed", () => {
      expect(bomDeltaCsv([]).split("\r\n")).toHaveLength(1);
    });
  });

  describe("panel integration", () => {
    it("BOM changes land in their own bucket, after cosmetic", () => {
      const changes: Change[] = [
        bomCh("ch_b", "added", { key: "k", designators: ["R1"] }),
        {
          id: "ch_c",
          group: "component",
          kind: "modified",
          impact: "electrical",
          title: "C14 value",
          anchors: {},
          side: "both",
        } as Change,
      ];
      const groups = groupChanges(changes);
      expect(groups.map((g) => g.impact)).toEqual(["electrical", "bom"]);
      expect(groups[1].label).toBe("BOM");
      expect(groups[1].changes[0].id).toBe("ch_b");
    });

    it("bomChanges filters to anchored bom rows only", () => {
      const changes: Change[] = [
        bomCh("ch_b", "added", { key: "k", designators: [] }),
        {
          id: "ch_x",
          group: "net",
          kind: "added",
          impact: "electrical",
          title: "net",
          anchors: {},
          side: "b",
        } as Change,
      ];
      expect(bomChanges(changes).map((c) => c.id)).toEqual(["ch_b"]);
    });
  });
});
