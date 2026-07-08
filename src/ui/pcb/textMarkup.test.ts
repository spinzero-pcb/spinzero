import { describe, expect, it } from "vitest";
import { markupPlain, parseMarkup } from "./textMarkup";

describe("parseMarkup", () => {
  it("returns a single plain run for markup-free text", () => {
    expect(parseMarkup("GND")).toEqual([{ text: "GND", over: false }]);
  });

  it("overlines a whole ~{…} group", () => {
    expect(parseMarkup("~{project_rst}")).toEqual([
      { text: "project_rst", over: true },
    ]);
  });

  it("splits an overbarred prefix from the plain remainder and resolves {slash}", () => {
    // KiCad: overlined "crst" then "/out2".
    expect(parseMarkup("~{crst}{slash}out2")).toEqual([
      { text: "crst", over: true },
      { text: "/out2", over: false },
    ]);
  });

  it("resolves {slash} with no overbar as one plain run", () => {
    expect(parseMarkup("SDI{slash}out0")).toEqual([
      { text: "SDI/out0", over: false },
    ]);
  });

  it("merges adjacent runs of the same over state around a group", () => {
    expect(parseMarkup("A~{B}C")).toEqual([
      { text: "A", over: false },
      { text: "B", over: true },
      { text: "C", over: false },
    ]);
  });

  it("flattens sub/superscript to plain runs (no overlay baseline shift)", () => {
    expect(parseMarkup("V_{cc}")).toEqual([{ text: "Vcc", over: false }]);
    expect(parseMarkup("x^{2}")).toEqual([{ text: "x2", over: false }]);
  });

  it("leaves unknown/unbalanced braces literal", () => {
    expect(parseMarkup("a{bogus}b")).toEqual([{ text: "a{bogus}b", over: false }]);
    expect(parseMarkup("~{oops")).toEqual([{ text: "~{oops", over: false }]);
  });

  it("markupPlain strips markup and resolves escapes", () => {
    expect(markupPlain("~{crst}{slash}out2")).toBe("crst/out2");
    expect(markupPlain("GND")).toBe("GND");
  });
});
