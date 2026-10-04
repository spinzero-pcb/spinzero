// Overbars drawn as real lines, for Altium sheets.
//
// The Altium extractor marks an overbarred run (`R\S\T\`) as an overlined `<tspan>`
// with `data-overbar`. The app's webview skips that decoration's ink where it meets
// Arial's capitals, so a barred "RST" showed as two specks. Here each marked run is
// measured from its glyph positions and drawn as a line in the text's own frame.
// KiCad runs are not marked: their stroke font clears the decoration, and the webview
// lays that font out wider than it paints it, so a measured bar would miss the glyphs.

const SVG_NS = "http://www.w3.org/2000/svg";

/** Bar thickness as a fraction of the font size (Arial's own overline). */
const BAR_WIDTH_EM = 0.07;
/** How far the bar sits above the capitals, in em. */
const BAR_GAP_EM = 0.08;

/** Where the capitals' tops sit relative to the text's anchor line, in em (negative is
 *  up), for each `dominant-baseline` the extractors write. Arial's cap height is 0.72
 *  em and Times' 0.66; a central baseline sits about 0.35 em above the alphabetic one. */
function capTopEm(baseline: string | null): number {
  if (baseline === "hanging") return 0;
  if (baseline === "central" || baseline === "middle") return -0.37;
  return -0.72;
}

/** Replace every overline decoration in a mounted sheet with a measured line.
 *  Idempotent: call again after the text changes or a web font finishes loading. */
export function drawOverbars(svg: SVGSVGElement): void {
  for (const old of svg.querySelectorAll(".sch-overbar")) old.remove();
  const runs = svg.querySelectorAll<SVGTSpanElement>("tspan[data-overbar]");
  for (const sp of runs) {
    // The marker stays so a redraw finds the run; the broken decoration goes.
    sp.removeAttribute("text-decoration");
    const text = sp.closest("text");
    if (!text?.parentNode) continue;
    // The glyphs' own advance positions: a box (getBBox) is wider than the ink and its
    // height is the line box, not the font.
    let x1: number, x2: number, anchorY: number;
    try {
      const n = sp.getNumberOfChars();
      if (n < 1) continue;
      const start = sp.getStartPositionOfChar(0);
      x1 = start.x;
      anchorY = start.y;
      x2 = sp.getEndPositionOfChar(n - 1).x;
    } catch {
      continue; // not rendered (a hidden sheet) — nothing to measure
    }
    if (!(x2 > x1)) continue;
    const size = parseFloat(text.getAttribute("font-size") ?? "");
    if (!(size > 0)) continue;
    const y = anchorY + (capTopEm(text.getAttribute("dominant-baseline")) - BAR_GAP_EM) * size;
    const line = document.createElementNS(SVG_NS, "line");
    line.setAttribute("class", "sch-overbar");
    line.setAttribute("x1", String(x1));
    line.setAttribute("x2", String(x2));
    line.setAttribute("y1", String(y));
    line.setAttribute("y2", String(y));
    // Same frame as the text (its rotation or scale), same ink as its glyphs.
    const t = text.getAttribute("transform");
    if (t) line.setAttribute("transform", t);
    line.setAttribute("stroke-width", String(size * BAR_WIDTH_EM));
    line.style.setProperty("stroke", getComputedStyle(text).fill, "important");
    text.parentNode.insertBefore(line, text.nextSibling);
  }
}
