// Net-class grouping + colour assignment for the PCB "Net Classes" panel.
// Pure helpers over the design payload — no store/DOM access — so the panel and
// the GL view can agree on the same class list and per-class colour.

import type { DesignIndexes } from "./design";

/** Distinct net-class colours, kept separate from the highlight palette so a
 *  class colour never reads as a click-selection. Assigned by class order below. */
export const NET_CLASS_PALETTE = [
  "#4f8cff", // blue
  "#ff9f43", // orange
  "#3fb950", // green
  "#d2a8ff", // violet
  "#f778ba", // pink
  "#56d4dd", // cyan
  "#e5c07b", // gold
  "#7ee787", // lime
];

/** Neutral grey for the "Default" catch-all class (most nets live here, so it
 *  should not eat a vivid palette slot). */
export const DEFAULT_CLASS_COLOR = "#8b98a5";

export interface NetClass {
  name: string;
  /** Net names in this class, sorted. */
  nets: string[];
}

/** Group the design's nets by class. "Default" is sorted last so the bulk
 *  catch-all doesn't head the list; every other class is alphabetical. */
export function listNetClasses(indexes: DesignIndexes | null | undefined): NetClass[] {
  if (!indexes) return [];
  const byClass = new Map<string, string[]>();
  for (const [name, net] of Object.entries(indexes.nets)) {
    const cls = net.class || "Default";
    let list = byClass.get(cls);
    if (!list) byClass.set(cls, (list = []));
    list.push(name);
  }
  const names = [...byClass.keys()].sort((a, b) => {
    if (a === "Default") return 1;
    if (b === "Default") return -1;
    return a.localeCompare(b);
  });
  return names.map((name) => ({ name, nets: byClass.get(name)!.sort() }));
}

/** Stable colour for a class, keyed by its position among the non-Default
 *  classes so a class keeps the same colour regardless of what's selected. */
export function netClassColor(name: string, ordered: string[]): string {
  if (name === "Default") return DEFAULT_CLASS_COLOR;
  const others = ordered.filter((n) => n !== "Default");
  const i = others.indexOf(name);
  return NET_CLASS_PALETTE[(i < 0 ? 0 : i) % NET_CLASS_PALETTE.length];
}
