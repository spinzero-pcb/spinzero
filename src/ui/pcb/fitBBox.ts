import type { PcbGeometry } from "../../lib/pcbGeometry";

export interface FitBox {
  minx: number;
  miny: number;
  maxx: number;
  maxy: number;
}

/** The extent of the board outline: everything drawn on `edge`-role layers.
 *  Returns null when the board has no outline.
 *
 *  Altium boards keep notes, assembly drawings and parts on the same workspace,
 *  far from the board. The fit frames the outline so the board sits in the middle
 *  of the view, as Altium's own viewer does. */
export function outlineBBox(geom: PcbGeometry): FitBox | null {
  const edge = new Set<number>();
  geom.layers.forEach((l, i) => {
    if (l.role === "edge") edge.add(i);
  });
  if (edge.size === 0) return null;
  const b: FitBox = { minx: Infinity, miny: Infinity, maxx: -Infinity, maxy: -Infinity };
  const add = (x: number, y: number, r = 0) => {
    b.minx = Math.min(b.minx, x - r);
    b.miny = Math.min(b.miny, y - r);
    b.maxx = Math.max(b.maxx, x + r);
    b.maxy = Math.max(b.maxy, y + r);
  };
  const { seg, arc } = geom.tracks;
  seg.layer.forEach((l, i) => {
    if (edge.has(l)) {
      add(seg.xy[4 * i], seg.xy[4 * i + 1]);
      add(seg.xy[4 * i + 2], seg.xy[4 * i + 3]);
    }
  });
  arc.layer.forEach((l, i) => {
    if (!edge.has(l)) return;
    for (let k = 0; k < 6; k += 2) add(arc.xy[6 * i + k], arc.xy[6 * i + k + 1]);
  });
  for (const g of geom.graphics) {
    if (!edge.has(g.layer)) continue;
    if (g.kind === "circle") {
      if (g.data.length >= 3) add(g.data[0], g.data[1], g.data[2]);
      continue;
    }
    for (let k = 0; k + 1 < g.data.length; k += 2) add(g.data[k], g.data[k + 1]);
  }
  return b.minx <= b.maxx ? b : null;
}
