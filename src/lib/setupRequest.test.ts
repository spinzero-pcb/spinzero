import { describe, expect, it } from "vitest";
import { changedMapping, fieldGroup, parseSetupRequest } from "./setupRequest";

describe("parseSetupRequest", () => {
  it("reads a request the review server wrote", () => {
    const r = parseSetupRequest({
      schema_version: "mcp-setup-1.0",
      review_id: "r-1",
      board: "MC-02-CONTROL",
      row_count: 79,
      profile: { value: null, options: [{ id: "commercial", label: "Commercial" }] },
      fields: [{ field: "mpn", label: "Manufacturer part number", column: "Manufacturer Part Number" }],
      columns: [{ name: "Manufacturer Part Number", fill_rate: 1, sample: "GRT1555C1H270FA02D" }],
    });
    expect(r.board).toBe("MC-02-CONTROL");
    expect(r.profile.options).toEqual([{ id: "commercial", label: "Commercial" }]);
    expect(r.fields[0]?.column).toBe("Manufacturer Part Number");
  });

  it("turns a malformed file into empty values instead of throwing", () => {
    const r = parseSetupRequest({ fields: [{ field: "mpn", column: null }, 7], columns: "x", profile: 3 });
    expect(r.fields).toEqual([{ field: "mpn", label: "mpn", column: "" }]);
    expect(r.columns).toEqual([]);
    expect(r.profile).toEqual({ value: null, options: [] });
    expect(parseSetupRequest(null).board).toBe("Board");
  });
});

describe("changedMapping", () => {
  const fields = [
    { field: "mpn", label: "", column: "MPN" },
    { field: "datasheet", label: "", column: "" },
  ];

  it("sends only the fields the user changed", () => {
    expect(changedMapping(fields, {})).toEqual({});
    expect(changedMapping(fields, { mpn: "MPN" })).toEqual({});
    expect(changedMapping(fields, { mpn: "Mfr Part #", datasheet: "" })).toEqual({ mpn: "Mfr Part #" });
  });

  it("sends an empty string for a field the user cleared", () => {
    expect(changedMapping(fields, { mpn: "" })).toEqual({ mpn: "" });
  });
});

describe("fieldGroup", () => {
  const columns = [
    { name: "Manufacturer Part Number", label: "MPN", fill_rate: 0.95, sample: "X" },
    { name: "Datasheet", label: "Datasheet", fill_rate: 0.14, sample: "" },
  ];
  const f = (field: string, column: string) => ({ field, label: field, column });

  it("puts a missing key field and a sparse column first", () => {
    expect(fieldGroup(f("mpn", ""), columns)).toBe("check");
    expect(fieldGroup(f("datasheet", "Datasheet"), columns)).toBe("check");
  });

  it("files a well-filled column as read, and a missing optional field as absent", () => {
    expect(fieldGroup(f("mpn", "Manufacturer Part Number"), columns)).toBe("read");
    expect(fieldGroup(f("lcsc", ""), columns)).toBe("absent");
  });
});

describe("the user's own column names", () => {
  it("keeps the label the server sent and falls back to the name", () => {
    const r = parseSetupRequest({
      columns: [{ name: "Manufacturer Part Number", label: "MPN", fill_rate: 1, sample: "" }, { name: "Value" }],
    });
    expect(r.columns.map((c) => c.label)).toEqual(["MPN", "Value"]);
  });
});
