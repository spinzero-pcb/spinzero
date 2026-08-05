import { describe, expect, it } from "vitest";
import { builtinFor, customFieldValue, presetColumns } from "./bomColumns";
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
