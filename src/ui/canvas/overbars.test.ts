import { afterEach, describe, expect, it, vi } from "vitest";
import { drawOverbars } from "./overbars";

// jsdom has no layout, so the glyph-position calls are stubbed: the point is where the
// bar goes and that the broken decoration is replaced, not glyph metrics.
type Measured = {
  getNumberOfChars: () => number;
  getStartPositionOfChar: (i: number) => { x: number; y: number };
  getEndPositionOfChar: (i: number) => { x: number; y: number };
};
function measure(sp: Element, x1: number, x2: number, y: number, n = 3) {
  const m = sp as unknown as Measured;
  m.getNumberOfChars = () => n;
  m.getStartPositionOfChar = () => ({ x: x1, y });
  m.getEndPositionOfChar = () => ({ x: x2, y });
}
function sheet(markup: string): SVGSVGElement {
  const host = document.createElement("div");
  host.innerHTML = `<svg xmlns="http://www.w3.org/2000/svg">${markup}</svg>`;
  document.body.appendChild(host);
  return host.querySelector("svg") as SVGSVGElement;
}

describe("drawOverbars", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("draws a measured line over each overlined run, in the text's frame", () => {
    const svg = sheet(
      `<g><text x="10" y="20" font-size="2" dominant-baseline="central" transform="rotate(-90 10 20)">` +
        `<tspan text-decoration="overline" data-overbar="">RST</tspan>_N</text></g>`,
    );
    measure(svg.querySelector("tspan")!, 10, 14, 20);
    drawOverbars(svg);
    const line = svg.querySelector("line.sch-overbar") as SVGLineElement;
    expect(line).not.toBeNull();
    expect([line.getAttribute("x1"), line.getAttribute("x2")]).toEqual(["10", "14"]);
    // Central baseline: cap tops 0.37 em up, the bar 0.08 em above them.
    expect(Number(line.getAttribute("y1"))).toBeCloseTo(20 - 0.45 * 2);
    expect(line.getAttribute("transform")).toBe("rotate(-90 10 20)");
    expect(svg.querySelector("tspan")!.hasAttribute("text-decoration")).toBe(false);
  });

  it("is idempotent, so a redraw after a font loads does not stack bars", () => {
    const svg = sheet(`<text font-size="2"><tspan text-decoration="overline" data-overbar="">OE</tspan></text>`);
    measure(svg.querySelector("tspan")!, 0, 3, 5);
    drawOverbars(svg);
    drawOverbars(svg);
    expect(svg.querySelectorAll("line.sch-overbar")).toHaveLength(1);
  });

  it("leaves an unmarked (KiCad) overline to the text engine", () => {
    const svg = sheet(`<text font-size="2"><tspan text-decoration="overline">OE</tspan></text>`);
    drawOverbars(svg);
    expect(svg.querySelectorAll("line.sch-overbar")).toHaveLength(0);
    expect(svg.querySelector("tspan")!.getAttribute("text-decoration")).toBe("overline");
  });

  it("skips a run it cannot measure instead of throwing", () => {
    const svg = sheet(`<text font-size="2"><tspan text-decoration="overline" data-overbar="">X</tspan></text>`);
    const m = svg.querySelector("tspan") as unknown as Measured;
    m.getNumberOfChars = () => {
      throw new Error("not rendered");
    };
    expect(() => drawOverbars(svg)).not.toThrow();
    expect(svg.querySelectorAll("line.sch-overbar")).toHaveLength(0);
  });
});
