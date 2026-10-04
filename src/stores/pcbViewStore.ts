import { create } from "zustand";
import { useSettingsStore } from "./settingsStore";
import type { LayerLite } from "../lib/design";
import { edgeLayer, hiddenByDefault, outerCopper } from "../lib/layers";

/** KiCad default board theme tokens (tokens.css), keyed by canonical layer name.
 *  Shared by the PCB canvas (island colors) and the Appearance panel (swatches). */
const LAYER_TOKEN: Record<string, string> = {
  "F.Cu": "--pcb-fcu",
  "B.Cu": "--pcb-bcu",
  "F.SilkS": "--pcb-fsilk",
  "F.Silkscreen": "--pcb-fsilk",
  "B.SilkS": "--pcb-bsilk",
  "B.Silkscreen": "--pcb-bsilk",
  "F.Mask": "--pcb-fmask",
  "B.Mask": "--pcb-bmask",
  "F.Fab": "--pcb-ffab",
  "B.Fab": "--pcb-bfab",
  "F.CrtYd": "--pcb-fcrtyd",
  "F.Courtyard": "--pcb-fcrtyd",
  "B.CrtYd": "--pcb-bcrtyd",
  "B.Courtyard": "--pcb-bcrtyd",
  "F.Paste": "--pcb-fpaste",
  "B.Paste": "--pcb-bpaste",
  "Edge.Cuts": "--pcb-edge",
};

export function layerColorVar(name: string, explicit?: string | null): string {
  // A non-standard ("user") layer carries its real KiCad colour resolved at extraction
  // time (LayerLite.color) — paint it directly. Standard layers have no explicit colour
  // and theme via the CSS vars below.
  if (explicit) return explicit;
  // Inner copper (In1.Cu..InN.Cu) → its own --pcb-in{N}, set per layer from the KiCad
  // theme (kicadTheme.ts). Falls back to --pcb-in1 only when the theme omits that layer
  // (e.g. a theme-less bundle) so it's never an undefined-var black.
  const inner = name.match(/^In(\d+)\.Cu$/);
  if (inner) return `var(--pcb-in${inner[1]}, var(--pcb-in1))`;
  const token = LAYER_TOKEN[name] ?? (name.endsWith(".Cu") ? "--pcb-in1" : "--pcb-ffab");
  return `var(${token})`;
}

/** Theme token for a layer by its ROLE and SIDE, for layers whose name is not
 *  KiCad's. Altium names the top copper "Top Layer" (or whatever the designer typed),
 *  so a name lookup leaves every Altium layer the neutral fab grey. */
function roleToken(role: string | undefined, side: string | undefined): string | null {
  const back = side === "back";
  switch (role) {
    case "copper":
      return side === "inner" ? null : back ? "--pcb-bcu" : "--pcb-fcu";
    case "silkscreen":
      return back ? "--pcb-bsilk" : "--pcb-fsilk";
    case "mask":
      return back ? "--pcb-bmask" : "--pcb-fmask";
    case "paste":
      return back ? "--pcb-bpaste" : "--pcb-fpaste";
    case "courtyard":
      return back ? "--pcb-bcrtyd" : "--pcb-fcrtyd";
    case "fab":
      return back ? "--pcb-bfab" : "--pcb-ffab";
    case "edge":
      return "--pcb-edge";
    default:
      return null;
  }
}

interface LayerColorSource {
  name: string;
  color?: string | null;
  role?: string;
  side?: string;
}

/** A layer's colour, resolved by name first and by role/side second. `layers` is the
 *  board's full layer list, needed to number an inner copper layer (`--pcb-in{N}`). */
export function layerColorOf(l: LayerColorSource, layers: readonly LayerColorSource[]): string {
  if (l.color) return l.color;
  if (LAYER_TOKEN[l.name] || /^In\d+\.Cu$/.test(l.name) || !l.role) return layerColorVar(l.name);
  if (l.role === "copper" && l.side === "inner") {
    const inner = layers.filter((x) => x.role === "copper" && x.side === "inner");
    const n = Math.max(inner.indexOf(l) + 1, 1);
    return `var(--pcb-in${n}, var(--pcb-in1))`;
  }
  const token = roleToken(l.role, l.side);
  return token ? `var(${token})` : layerColorVar(l.name);
}

/** Object classes toggleable in the PCB appearance panel (item 7). */
export const PCB_OBJECT_KEYS = ["tracks", "vias", "pads", "zones", "footprints", "text"] as const;
export type PcbObjectKey = (typeof PCB_OBJECT_KEYS)[number];

export const PCB_OBJECT_LABELS: Record<PcbObjectKey, string> = {
  tracks: "Tracks",
  vias: "Vias",
  pads: "Pads",
  zones: "Zones",
  footprints: "Footprints",
  text: "Text & graphics",
};

/** The synthetic drawing-sheet "layer" the extractor emits (role "worksheet", name
 *  kept in lockstep with Rust's `WORKSHEET_LAYER`). It is page context — a frame +
 *  title block — not a board layer, so it is always painted as a background and never
 *  shown as a toggleable/selectable row. It IS part of the camera fit, so the board
 *  opens framed by its sheet with the borders visible. */
export const WORKSHEET_LAYER = "Drawing Sheet";
export const isWorksheetLayer = (l: { name: string; role?: string }) =>
  l.role === "worksheet" || l.name === WORKSHEET_LAYER;

// PCB appearance state (item 7): KiCad-style layer list with visibility, an active
// layer that paints on top (others translucent), and object-class filters. Lives in
// a store so the Explorer's layer rows and the properties card can drive it too.

interface PcbViewState {
  /** Active layer (painted on top, full opacity); null = natural board order. */
  active: string | null;
  hidden: Set<string>;
  /** Layers already given their default visibility (don't clobber user choices). */
  known: string[];
  /** Name of the layer carrying the board profile, resolved by role. The diff
   *  view rides it along so the outline keeps framing the copper. */
  edge: string | null;
  objects: Record<PcbObjectKey, boolean>;
  /** Per-class opacity 0..1, KiCad Objects-tab style (item 23). */
  opacity: Record<PcbObjectKey, number>;
  setActive: (layer: string | null) => void;
  toggleLayer: (layer: string) => void;
  showLayer: (layer: string) => void;
  /** Show every layer (clear all hides). */
  showAllLayers: () => void;
  /** Hide every layer in the given list. */
  hideAllLayers: (layers: string[]) => void;
  /** Replace the hidden set wholesale (layer right-click presets); leaves `active` alone. */
  setHidden: (layers: string[]) => void;
  setObject: (key: PcbObjectKey, on: boolean) => void;
  setOpacity: (key: PcbObjectKey, v: number) => void;
  /** Apply persisted per-class opacity from settings (startup) — validated + clamped,
   *  unknown/absent keys keep their default so a partial save never blanks a class. */
  hydrateOpacity: (saved: Record<string, number> | null | undefined) => void;
  /** New revision: reset hides, keep the active layer if it still exists, else
   *  default to the board's own front copper — resolved by role, because an
   *  Altium stack has no layer called F.Cu. */
  resetForLayers: (layers: LayerLite[]) => void;
}

// KiCad Appearance→Objects defaults: tracks/vias/pads opaque, zones at 60% so the
// copper pour reads as a wash and tracks/pads under it stay visible.
const DEFAULT_OPACITY: Record<PcbObjectKey, number> = {
  tracks: 1, vias: 1, pads: 1, zones: 0.6, footprints: 1, text: 1,
};

export const usePcbViewStore = create<PcbViewState>((set, get) => ({
  active: null,
  hidden: new Set(),
  known: [],
  edge: null,
  objects: { tracks: true, vias: true, pads: true, zones: true, footprints: true, text: true },
  opacity: { ...DEFAULT_OPACITY },
  setActive: (active) => set({ active }),
  toggleLayer: (layer) =>
    set((s) => {
      const hidden = new Set(s.hidden);
      hidden.has(layer) ? hidden.delete(layer) : hidden.add(layer);
      return { hidden };
    }),
  showLayer: (layer) =>
    set((s) => {
      if (!s.hidden.has(layer)) return s;
      const hidden = new Set(s.hidden);
      hidden.delete(layer);
      return { hidden };
    }),
  showAllLayers: () => set({ hidden: new Set() }),
  hideAllLayers: (layers) => set({ hidden: new Set(layers), active: null }),
  setHidden: (layers) => set({ hidden: new Set(layers) }),
  setObject: (key, on) => set((s) => ({ objects: { ...s.objects, [key]: on } })),
  setOpacity: (key, v) => {
    const opacity = { ...get().opacity, [key]: v };
    set({ opacity });
    // Remember the transparency sliders across sessions (machine-local user
    // preference). Debounced inside the settings store — this fires per drag tick.
    useSettingsStore.getState().setPcbOpacity(opacity);
  },
  hydrateOpacity: (saved) => {
    if (!saved) return;
    const opacity = { ...DEFAULT_OPACITY };
    for (const key of PCB_OBJECT_KEYS) {
      const v = saved[key];
      // Clamp to the slider's 0.1..1 range; ignore anything non-finite or out of a
      // sane band so a corrupt settings file can't blank a class.
      if (typeof v === "number" && isFinite(v) && v >= 0.1 && v <= 1) opacity[key] = v;
    }
    set({ opacity });
  },
  resetForLayers: (layers) => {
    const names = layers.map((l) => l.name);
    return set((s) => {
      const hidden = new Set([...s.hidden].filter((l) => names.includes(l)));
      for (const l of layers)
        if (!s.known.includes(l.name) && hiddenByDefault(l)) hidden.add(l.name);
      return {
        hidden,
        known: names,
        edge: edgeLayer(layers) ?? null,
        // Keep a still-present active layer; otherwise open on the board's own
        // front copper instead of nothing.
        active:
          s.active && names.includes(s.active) ? s.active : (outerCopper(layers, "front") ?? null),
      };
    });
  },
}));
