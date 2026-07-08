// Resolve CSS colours (hex, rgb(), or `var(--token, fallback)`) to float RGB for
// WebGL. Resolving through a live probe element lets the browser expand the CSS
// custom properties exactly as the SVG view's `layerColorVar` intends, so the GPU
// renderer paints with the user's real KiCad palette — no second colour table.

let probe: HTMLSpanElement | null = null;

function getProbe(): HTMLSpanElement {
  if (!probe) {
    probe = document.createElement("span");
    probe.style.cssText =
      "position:absolute;left:-9999px;top:-9999px;width:0;height:0;visibility:hidden;pointer-events:none";
    document.body.appendChild(probe);
  }
  return probe;
}

const FALLBACK: [number, number, number] = [0.72, 0.72, 0.72]; // #B8B8B8

function parseRgb(s: string): [number, number, number] {
  const m = s.match(/rgba?\(([^)]+)\)/i);
  if (!m) return FALLBACK;
  const p = m[1].split(",").map((x) => parseFloat(x));
  return [(p[0] || 0) / 255, (p[1] || 0) / 255, (p[2] || 0) / 255];
}

/** Resolve any CSS colour string (incl. `var(--x, fallback)`) to [r,g,b] in 0..1. */
export function resolveCssColor(css: string): [number, number, number] {
  const el = getProbe();
  // Reset first so an invalid value falls back to the previous valid computed colour
  // rather than silently keeping a stale one.
  el.style.color = "#B8B8B8";
  el.style.color = css;
  return parseRgb(getComputedStyle(el).color);
}

/** Parse a `#rgb`/`#rrggbb` hex string to [r,g,b] in 0..1. */
export function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "").trim();
  if (h.length === 3) {
    return [
      parseInt(h[0] + h[0], 16) / 255,
      parseInt(h[1] + h[1], 16) / 255,
      parseInt(h[2] + h[2], 16) / 255,
    ];
  }
  if (h.length >= 6) {
    return [
      parseInt(h.slice(0, 2), 16) / 255,
      parseInt(h.slice(2, 4), 16) / 255,
      parseInt(h.slice(4, 6), 16) / 255,
    ];
  }
  return FALLBACK;
}

/** Release the probe element (call on full teardown; optional). */
export function disposeColorProbe(): void {
  probe?.remove();
  probe = null;
}
