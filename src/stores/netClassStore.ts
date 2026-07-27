import { create } from "zustand";
import { useDesignStore } from "./designStore";
import { usePcbViewStore } from "./pcbViewStore";

// PCB "Net Classes" selection (batch: net-class highlight). Selecting one or more
// classes recolours their nets in the render (the colour is derived from the class
// list — see lib/netClasses) and isolates the copper layers those nets run on,
// hiding everything else. The pre-isolation layer visibility is snapshotted on the
// first select and restored when the last class is deselected.

interface NetClassState {
  /** Selected class names, in click order. */
  selected: string[];
  /** Layer visibility snapshot captured when isolation begins; null while none selected. */
  savedHidden: string[] | null;
  savedActive: string | null;
  toggle: (name: string) => void;
  clear: () => void;
  /** Drop selection + snapshot WITHOUT touching layer state (new design: the layer
   *  set is being reset anyway, so restoring an old snapshot would clobber it). */
  reset: () => void;
}

/** Isolate the copper layers carrying the selected classes' nets: hide every other
 *  layer (Edge.Cuts kept for board context). No-op until the PCB index is built. */
function applyIsolation(selected: string[]) {
  if (selected.length === 0) return;
  const { indexes, pcbIndex } = useDesignStore.getState();
  if (!indexes || !pcbIndex) return;
  const classSet = new Set(selected);
  const layers = new Set<string>();
  for (const [name, net] of Object.entries(indexes.nets)) {
    if (!classSet.has(net.class || "Default")) continue;
    for (const l of pcbIndex.nets[name]?.layers ?? []) layers.add(l);
  }
  const allNames = indexes.layers.map((l) => l.name);
  // Keep the board outline visible so the isolated copper isn't floating in space.
  const edge = allNames.find((n) => /^Edge\.Cuts$/i.test(n));
  if (edge) layers.add(edge);
  const pv = usePcbViewStore.getState();
  pv.setHidden(allNames.filter((n) => !layers.has(n)));
  // Natural board order for the isolated layers — no single "active" layer on top.
  pv.setActive(null);
}

export const useNetClassStore = create<NetClassState>((set, get) => ({
  selected: [],
  savedHidden: null,
  savedActive: null,

  toggle: (name) => {
    const s = get();
    const has = s.selected.includes(name);
    const selected = has ? s.selected.filter((n) => n !== name) : [...s.selected, name];

    // Snapshot the layer state the first time isolation kicks in.
    let savedHidden = s.savedHidden;
    let savedActive = s.savedActive;
    if (s.selected.length === 0 && selected.length > 0) {
      const pv = usePcbViewStore.getState();
      savedHidden = [...pv.hidden];
      savedActive = pv.active;
    }

    if (selected.length === 0) {
      // Last class cleared — restore the pre-isolation view.
      if (savedHidden) {
        usePcbViewStore.getState().setHidden(savedHidden);
        usePcbViewStore.getState().setActive(savedActive ?? null);
      }
      set({ selected, savedHidden: null, savedActive: null });
    } else {
      applyIsolation(selected);
      set({ selected, savedHidden, savedActive });
    }
  },

  clear: () => {
    const s = get();
    if (s.selected.length === 0) return;
    if (s.savedHidden) {
      usePcbViewStore.getState().setHidden(s.savedHidden);
      usePcbViewStore.getState().setActive(s.savedActive ?? null);
    }
    set({ selected: [], savedHidden: null, savedActive: null });
  },

  reset: () => set({ selected: [], savedHidden: null, savedActive: null }),
}));
