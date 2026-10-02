import { describe, expect, it } from "vitest";
import {
  ALTIUM_GROUP_BY,
  DEFAULT_GROUP_BY,
  MIXED_VALUES,
  builtinFor,
  customFieldValue,
  defaultGroupBy,
  groupLines,
  presetColumns,
} from "./bomColumns";
import type { BomLine, BomPreset } from "./types";

const line = (fields: Record<string, string>): BomLine => ({
  item: 1,
  qty: 2,
  designators: ["R1"],
  value: "10k",
  footprint: "0402",
  mpn: "",
  dnp: false,
  fields,
});

describe("builtinFor", () => {
  it("maps KiCad virtual fields", () => {
    expect(builtinFor("${QUANTITY}")).toBe("qty");
    expect(builtinFor("${DNP}")).toBe("dnp");
    expect(builtinFor("${ITEM_NUMBER}")).toBe("item");
  });

  it("maps the natively carried columns case-insensitively", () => {
    expect(builtinFor("Reference")).toBe("designators");
    expect(builtinFor("reference")).toBe("designators");
    expect(builtinFor("Value")).toBe("value");
    expect(builtinFor("Footprint")).toBe("footprint");
    expect(builtinFor("MPN")).toBe("mpn");
    expect(builtinFor("manufacturer_part_number")).toBe("mpn");
  });

  it("leaves custom columns unmapped", () => {
    expect(builtinFor("Manufacturer")).toBeUndefined();
    expect(builtinFor("Automotive Grade")).toBeUndefined();
  });
});

describe("presetColumns", () => {
  const preset = (fields: BomPreset["fields"]): BomPreset => ({
    name: "p",
    fields,
    sort_field: "",
    sort_asc: true,
    is_project_default: false,
    exclude_dnp: false,
    group_symbols: true,
  });

  it("keeps shown fields in order and drops hidden ones", () => {
    const cols = presetColumns(
      preset([
        { name: "Reference", label: "Refs", show: true },
        { name: "Value", label: "Value", show: false },
        { name: "Manufacturer", label: "Manufacturer", show: true },
      ]),
    );
    expect(cols.map((c) => c.label)).toEqual(["Refs", "Manufacturer"]);
    expect(cols[0].builtin).toBe("designators");
    expect(cols[1].builtin).toBeUndefined();
  });

  it("survives hostile data", () => {
    expect(presetColumns(undefined)).toEqual([]);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect(presetColumns({ fields: null } as any)).toEqual([]);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect(presetColumns({ fields: [null, { show: true }] } as any)).toEqual([]);
  });

  it("de-duplicates column ids", () => {
    const cols = presetColumns(
      preset([
        { name: "MSL", label: "MSL", show: true },
        { name: "m s l", label: "MSL again", show: true },
      ]),
    );
    expect(new Set(cols.map((c) => c.id)).size).toBe(2);
  });
});

describe("groupLines", () => {
  const bom = (over: Partial<BomLine>): BomLine => ({ ...line({}), ...over });

  it("returns the lines untouched when no field is flagged", () => {
    const lines = [bom({ item: 1 }), bom({ item: 2, value: "1k" })];
    expect(groupLines(lines, [{ name: "Value", label: "Value", show: true }])).toBe(lines);
  });

  it("folds lines that agree on every flagged field", () => {
    const out = groupLines(
      [
        bom({ item: 1, qty: 2, designators: ["R1", "R10"], footprint: "0402", fields: { MSL: "1" } }),
        bom({ item: 2, qty: 1, designators: ["R2"], footprint: "0603", fields: { MSL: "1" } }),
        bom({ item: 3, qty: 1, designators: ["R3"], value: "1k" }),
      ],
      [
        { name: "Value", label: "Value", show: true, group_by: true },
        { name: "MPN", label: "MPN", show: true, group_by: true },
        { name: "Footprint", label: "Footprint", show: true },
      ],
    );
    expect(out.map((l) => l.value)).toEqual(["10k", "1k"]);
    expect(out[0].qty).toBe(3);
    // Natural designator order across the merged lines, item numbers re-issued.
    expect(out[0].designators).toEqual(["R1", "R2", "R10"]);
    expect(out.map((l) => l.item)).toEqual([1, 2]);
    // Agreeing fields survive; a field the members disagree on reports mixed.
    expect(out[0].fields.MSL).toBe("1");
    expect(out[0].footprint).toBe(MIXED_VALUES);
  });

  it("marks a field only one member carries as mixed", () => {
    const out = groupLines(
      [bom({ fields: { MSL: "1" } }), bom({ fields: {} })],
      [{ name: "Value", label: "Value", show: true, group_by: true }],
    );
    expect(out).toHaveLength(1);
    expect(out[0].fields.MSL).toBe(MIXED_VALUES);
  });

  it("separates the key fields so a value/footprint split can't collide", () => {
    // "1" + "0k_R0402" and "10" + "k_R0402" concatenate to the same string; only the
    // U+001F separator keeps them apart. Without it these fold into one qty-2 line.
    const out = groupLines(
      [
        bom({ item: 1, qty: 1, value: "1", footprint: "0k_R0402" }),
        bom({ item: 2, qty: 1, value: "10", footprint: "k_R0402" }),
      ],
      [
        { name: "Value", label: "Value", show: true, group_by: true },
        { name: "Footprint", label: "Footprint", show: true, group_by: true },
      ],
    );
    expect(out).toHaveLength(2);
    expect(out.map((l) => l.qty)).toEqual([1, 1]);
  });

  it("keeps DNP only when every member is DNP", () => {
    const flags = [{ name: "Value", label: "Value", show: true, group_by: true }];
    expect(groupLines([bom({ dnp: true }), bom({ dnp: false })], flags)[0].dnp).toBe(false);
    expect(groupLines([bom({ dnp: true }), bom({ dnp: true })], flags)[0].dnp).toBe(true);
  });
});

describe("customFieldValue", () => {
  it("reads the verbatim key first", () => {
    expect(customFieldValue(line({ Manufacturer: "Murata" }), "Manufacturer")).toBe("Murata");
  });

  it("falls back to a case- and punctuation-insensitive match", () => {
    expect(customFieldValue(line({ automotive_grade: "AEC-Q200" }), "Automotive Grade")).toBe(
      "AEC-Q200",
    );
    expect(customFieldValue(line({ MSL: "1" }), "msl")).toBe("1");
  });

  it("tries the label when the field name misses", () => {
    expect(customFieldValue(line({ Description: "cap" }), "${DESC}", "Description")).toBe("cap");
  });

  it("returns an empty string for missing or non-string values", () => {
    expect(customFieldValue(line({}), "Manufacturer")).toBe("");
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect(customFieldValue({ ...line({}), fields: undefined } as any, "X")).toBe("");
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect(customFieldValue(line({ X: 5 as any }), "X")).toBe("");
  });
});

describe("Altium grouping", () => {
  const part = (
    des: string,
    libPart: string,
    partNumber: string,
    value: string,
    footprint = "R_0603",
  ): BomLine => ({
    item: 0,
    qty: 1,
    designators: [des],
    value,
    footprint,
    mpn: "",
    dnp: false,
    fields: { altium_library_part: libPart, PART_NUMBER: partNumber, footprint, description: "d" },
  });
  const group = (lines: BomLine[]) => groupLines(lines, ALTIUM_GROUP_BY, true);

  it("picks the Altium key only for lines that carry the library part", () => {
    expect(defaultGroupBy([part("R1", "RES", "P", "1k")])).toBe(ALTIUM_GROUP_BY);
    expect(defaultGroupBy([{ ...part("R1", "RES", "P", "1k"), fields: {} }])).toBe(DEFAULT_GROUP_BY);
  });

  it("splits one PART_NUMBER and library part on a different value", () => {
    const out = group([
      part("R4", "RES", "TMP-406", "100"),
      part("R18", "RES", "TMP-406", "100"),
      part("R8", "RES", "TMP-406", "470"),
    ]);
    expect(out.map((l) => l.designators)).toEqual([["R4", "R18"], ["R8"]]);
  });

  it("splits the same PART_NUMBER and value on a different library part", () => {
    const out = group([part("C1", "CAP_Dup1", "CAP-1", "0.1uF"), part("C13", "CAP_8", "CAP-1", "0.1uF")]);
    expect(out).toHaveLength(2);
  });

  it("merges placements that differ only in footprint and keeps the first footprint", () => {
    const out = group([
      part("R61", "RES", "RES-15", "10K", "R_0603_HD"),
      part("R62", "RES", "RES-15", "10K", "R_0603_HD"),
      part("R63", "RES", "RES-15", "10K", "R_0603"),
    ]);
    expect(out).toHaveLength(1);
    expect(out[0].qty).toBe(3);
    expect(out[0].footprint).toBe("R_0603_HD");
    expect(out[0].fields.footprint).toBe("R_0603_HD");
  });
});
