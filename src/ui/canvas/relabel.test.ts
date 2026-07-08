import { describe, expect, it } from "vitest";
import { relabelInstances } from "./relabel";

// A placed symbol whose source sheet is re-used: the extractor bakes the BASE ref
// (R10) into the shared SVG, plus a Value field, a nested pin (with its own number
// text + data-designator), and a power symbol that must never be relabelled. Mirrors
// the real vme-wren io78 markup (verified against the crunched bundle).
function makeSvg(): SVGSVGElement {
  const host = document.createElement("div");
  host.innerHTML = `
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
      <g data-primitive="symbol" data-uuid="u1" data-ref="R10">
        <g data-primitive="pin" data-uuid="p1" data-designator="R10" data-pin="1"><text>1</text></g>
        <text x="5" y="5">R10</text>
        <text x="5" y="7">10k</text>
      </g>
      <g data-primitive="symbol" data-uuid="u2" data-ref="C3">
        <text x="9" y="5">C3</text>
        <text x="9" y="7">100nF</text>
      </g>
      <g data-primitive="power-symbol" data-uuid="pw1" data-ref="#PWR01"><text>GND</text></g>
    </svg>`;
  return host.querySelector("svg")!;
}

const refTexts = (g: Element) =>
  [...g.querySelectorAll(":scope > text")].map((t) => t.textContent);

describe("relabelInstances", () => {
  it("remaps a re-used instance's reference, data-ref and pins — not its value or others", () => {
    const svg = makeSvg();
    // Current sheet: u1 is this instance's R274; u2 is a single instance (base == instance).
    const resolve = (u: string) => ({ u1: "R274", u2: "C3" } as Record<string, string>)[u];
    relabelInstances(svg, resolve);

    const u1 = svg.querySelector('g[data-uuid="u1"]') as SVGElement;
    expect(refTexts(u1)).toEqual(["R274", "10k"]); // reference remapped, value untouched
    expect(u1.dataset.ref).toBe("R274");
    expect((u1.querySelector("g[data-primitive=pin]") as SVGElement).dataset.designator).toBe("R274");
    // The pin's own number text (nested, not a direct child) is left alone.
    expect(u1.querySelector("g[data-primitive=pin] text")?.textContent).toBe("1");

    // A single-instance symbol (want === have) is not touched.
    const u2 = svg.querySelector('g[data-uuid="u2"]') as SVGElement;
    expect(u2.dataset.ref).toBe("C3");
    expect(refTexts(u2)).toEqual(["C3", "100nF"]);
  });

  it("is idempotent and ignores power symbols / unresolved uuids", () => {
    const svg = makeSvg();
    const resolve = (u: string) => (u === "u1" ? "R274" : undefined);
    relabelInstances(svg, resolve);
    relabelInstances(svg, resolve); // a second pass must not change anything further

    const u1 = svg.querySelector('g[data-uuid="u1"]') as SVGElement;
    expect(u1.dataset.ref).toBe("R274");
    expect(refTexts(u1)).toEqual(["R274", "10k"]);

    // Power symbol is excluded by the selector and the resolver — stays on its base ref.
    const pw = svg.querySelector('g[data-uuid="pw1"]') as SVGElement;
    expect(pw.dataset.ref).toBe("#PWR01");
  });
});
