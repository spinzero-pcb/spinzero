import { create } from "zustand";
import { useDesignStore } from "./designStore";
import { usePcbViewStore } from "./pcbViewStore";
import { useSettingsStore } from "./settingsStore";
import type { DesignIndexes } from "../lib/design";

// PCB "Net Classes" selection (batch: net-class highlight). Selecting one or more
// classes highlights their nets in the render — in the nets' own PCB layer colours
// unless a colour was picked for the class — and isolates the copper layers those
// nets run on, hiding everything else. Individual nets can be toggled and coloured
// on their own; a per-net entry overrides whatever its class says. The pre-isolation
// layer visibility is snapshotted on the first select and restored when the last
// selection goes away.

interface NetClassState {
  /** Selected class names, in click order. */
  selected: string[];
  /** Custom colour per class; absent = highlight in the nets' own layer colours. */
  classColors: Record<string, string>;
  /** Explicit per-net on/off, overriding the net's class selection. */
  netOverride: Record<string, boolean>;
  /** Custom colour per net; absent = fall back to the class colour / layer colours. */
  netColors: Record<string, string>;
  /** The project the colours were loaded for — colour picks persist under it. */
  projectDir: string | null;
  /** Layer visibility snapshot captured when isolation begins; null while nothing selected. */
  savedHidden: string[] | null;
  savedActive: string | null;
  toggle: (name: string) => void;
  toggleNet: (net: string) => void;
  /** null clears the custom colour (back to layer colours). */
  setClassColor: (name: string, color: string | null) => void;
  setNetColor: (net: string, color: string | null) => void;
  clear: () => void;
  /** Drop selection + snapshot WITHOUT touching layer state (new design: the layer
   *  set is being reset anyway, so restoring an old snapshot would clobber it).
   *  Colour picks survive — they belong to the project, not to one design load. */
  reset: () => void;
  /** Load this project's saved colour picks (project change). Also the point where the
   *  previous project's picks are dropped. */
  hydrate: (projectDir: string | null) => Promise<void>;
}

/** The highlighted nets and the colour each should take: a hex string, or null to
 *  highlight the net in its own PCB layer colours. Shared by the isolation logic
 *  here and the GL view's recolour mask. */
export function activeNets(
  s: Pick<NetClassState, "selected" | "classColors" | "netOverride" | "netColors">,
  indexes: DesignIndexes | null | undefined,
): Map<string, string | null> {
  const out = new Map<string, string | null>();
  if (!indexes) return out;
  const sel = new Set(s.selected);
  for (const [name, net] of Object.entries(indexes.nets)) {
    const cls = net.class || "Default";
    if (!(s.netOverride[name] ?? sel.has(cls))) continue;
    out.set(name, s.netColors[name] ?? s.classColors[cls] ?? null);
  }
  return out;
}

/** Isolate the copper layers carrying the highlighted nets: hide every other
 *  layer (Edge.Cuts kept for board context). No-op until the PCB index is built. */
function applyIsolation(nets: Iterable<string>) {
  const { indexes, pcbIndex } = useDesignStore.getState();
  if (!indexes || !pcbIndex) return;
  const layers = new Set<string>();
  for (const name of nets) for (const l of pcbIndex.nets[name]?.layers ?? []) layers.add(l);
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
  classColors: {},
  netOverride: {},
  netColors: {},
  projectDir: null,
  savedHidden: null,
  savedActive: null,

  toggle: (name) => {
    const s = get();
    const has = s.selected.includes(name);
    const selected = has ? s.selected.filter((n) => n !== name) : [...s.selected, name];

    // A class click is authoritative over per-net tweaks inside it.
    const indexes = useDesignStore.getState().indexes;
    const netOverride = { ...s.netOverride };
    for (const [net, meta] of Object.entries(indexes?.nets ?? {}))
      if ((meta.class || "Default") === name) delete netOverride[net];

    applySelection(s, set, { ...s, selected, netOverride });
  },

  toggleNet: (net) => {
    const s = get();
    const indexes = useDesignStore.getState().indexes;
    const cls = indexes?.nets[net]?.class || "Default";
    const on = s.netOverride[net] ?? s.selected.includes(cls);
    applySelection(s, set, { ...s, netOverride: { ...s.netOverride, [net]: !on } });
  },

  setClassColor: (name, color) => {
    const classColors = { ...get().classColors };
    if (color) classColors[name] = color;
    else delete classColors[name];
    set({ classColors });
    persistColors();
  },

  setNetColor: (net, color) => {
    const netColors = { ...get().netColors };
    if (color) netColors[net] = color;
    else delete netColors[net];
    set({ netColors });
    persistColors();
  },

  clear: () => {
    const s = get();
    if (s.selected.length === 0 && Object.keys(s.netOverride).length === 0) return;
    if (s.savedHidden) {
      usePcbViewStore.getState().setHidden(s.savedHidden);
      usePcbViewStore.getState().setActive(s.savedActive ?? null);
    }
    set({ selected: [], netOverride: {}, savedHidden: null, savedActive: null });
  },

  reset: () =>
    set({ selected: [], netOverride: {}, savedHidden: null, savedActive: null }),

  hydrate: async (projectDir) => {
    // Settings may not have been read yet on a startup that restores a project.
    if (!useSettingsStore.getState().loaded) {
      try {
        await useSettingsStore.getState().load();
      } catch {
        /* fall back to no saved colours */
      }
    }
    const ui = projectDir ? useSettingsStore.getState().projectUi[projectDir] : undefined;
    set({
      projectDir,
      classColors: sanitizeColors(ui?.net_class_colors),
      netColors: sanitizeColors(ui?.net_colors),
    });
  },
}));

/** Settings are machine-local JSON a user can hand-edit — keep only #rrggbb entries
 *  so a malformed value can't reach the GL colour parser. */
function sanitizeColors(v: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  if (!v || typeof v !== "object") return out;
  for (const [k, c] of Object.entries(v as Record<string, unknown>))
    if (typeof c === "string" && /^#[0-9a-fA-F]{6}$/.test(c)) out[k] = c.toLowerCase();
  return out;
}

/** Remember this project's colour picks machine-locally. No-op with no project open. */
function persistColors() {
  const { projectDir, classColors, netColors } = useNetClassStore.getState();
  if (!projectDir) return;
  void useSettingsStore
    .getState()
    .setProjectUi(projectDir, { net_class_colors: classColors, net_colors: netColors });
}

/** Commit a selection change: snapshot the layer view on the first highlight,
 *  re-isolate while anything is highlighted, restore the snapshot when the last
 *  net goes away. */
function applySelection(
  prev: NetClassState,
  set: (partial: Partial<NetClassState>) => void,
  next: NetClassState,
) {
  const indexes = useDesignStore.getState().indexes;
  const before = activeNets(prev, indexes);
  const after = activeNets(next, indexes);

  let savedHidden = prev.savedHidden;
  let savedActive = prev.savedActive;
  if (before.size === 0 && after.size > 0) {
    const pv = usePcbViewStore.getState();
    savedHidden = [...pv.hidden];
    savedActive = pv.active;
  }

  if (after.size === 0) {
    // Last net cleared — restore the pre-isolation view.
    if (savedHidden) {
      usePcbViewStore.getState().setHidden(savedHidden);
      usePcbViewStore.getState().setActive(savedActive ?? null);
    }
    set({
      selected: next.selected,
      netOverride: next.netOverride,
      savedHidden: null,
      savedActive: null,
    });
  } else {
    applyIsolation(after.keys());
    set({ selected: next.selected, netOverride: next.netOverride, savedHidden, savedActive });
  }
}
