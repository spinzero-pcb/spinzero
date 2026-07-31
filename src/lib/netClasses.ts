// Net-class grouping for the PCB "Net Classes" panel. A pure helper over the design
// payload — no store/DOM access — so the panel and the GL view agree on the same
// class list. Highlight colours are not assigned here: a class renders in its nets'
// own PCB layer colours unless the user picks one (see stores/netClassStore).

import type { DesignIndexes } from "./design";

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
