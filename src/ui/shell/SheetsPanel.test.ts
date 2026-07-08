import { describe, expect, it } from "vitest";
import { buildSheetTree } from "./SheetsPanel";
import type { SheetInfo } from "../../lib/types";

const sheet = (o: Partial<SheetInfo> & { sheet_path: string }): SheetInfo => ({
  number: 0,
  name: "",
  svg_path: "",
  page: "",
  ...o,
});

describe("buildSheetTree", () => {
  it("orders siblings by KiCad page number, not extraction (DFS) order", () => {
    // The DFS `number` order (1,2,3) disagrees with the page order (1,3,2): the
    // panel must follow the page number it displays, matching KiCad.
    const sheets = [
      sheet({ number: 1, name: "Root", sheet_path: "/", page: "1" }),
      sheet({ number: 2, name: "Beta", sheet_path: "/Beta/", page: "3" }),
      sheet({ number: 3, name: "Alpha", sheet_path: "/Alpha/", page: "2" }),
    ];
    const roots = buildSheetTree(sheets);
    expect(roots).toHaveLength(1); // Root
    const top = roots[0].children.map((c) => c.sheet.name);
    expect(top).toEqual(["Alpha", "Beta"]); // page 2 before page 3
  });

  it("sorts hierarchical page labels naturally (2.10 after 2.2)", () => {
    const sheets = [
      sheet({ number: 1, name: "Root", sheet_path: "/", page: "1" }),
      sheet({ number: 2, name: "S", sheet_path: "/S/", page: "2" }),
      sheet({ number: 3, name: "Ten", sheet_path: "/S/Ten/", page: "2.10" }),
      sheet({ number: 4, name: "Two", sheet_path: "/S/Two/", page: "2.2" }),
    ];
    const roots = buildSheetTree(sheets);
    const s = roots[0].children[0]; // "S"
    expect(s.children.map((c) => c.sheet.name)).toEqual(["Two", "Ten"]); // 2.2 before 2.10
  });

  it("falls back to the sequential number when pages are empty (automatic numbering)", () => {
    const sheets = [
      sheet({ number: 1, name: "Root", sheet_path: "/" }),
      sheet({ number: 3, name: "C", sheet_path: "/C/" }),
      sheet({ number: 2, name: "B", sheet_path: "/B/" }),
    ];
    const roots = buildSheetTree(sheets);
    expect(roots[0].children.map((c) => c.sheet.name)).toEqual(["B", "C"]);
  });
});
