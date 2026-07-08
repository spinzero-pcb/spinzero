import { create } from "zustand";
import type { Selection } from "../lib/design";
import { ipc } from "../lib/ipc";
import { useProjectStore } from "./projectStore";

/** One member of the active highlight set (multi-select). */
export interface Highlight {
  kind: "net" | "comp";
  ref: string;
  color: string;
}

// Distinct highlight colors for multi-select; nets default to blue, components amber.
// Shared by the schematic canvas and the PCB view so cross-probed nets keep their color
// on both sides (feedback item 4). The palette is defined in tokens.css (--hl-1..6) so
// every theme literal lives in one place (constraint 4); we resolve it to concrete hex
// here because these colours are persisted with pinned highlights and handed to the GL
// renderer, so they must be literal strings, not `var(...)`. The identical inline list is
// a defensible per-slot fallback for the brief window before the stylesheet is live
// (mirrors glColor's computed-style probe).
function readHlPalette(): string[] {
  const fallback = ["#4f8cff", "#ff9f43", "#3fb950", "#d2a8ff", "#f778ba", "#56d4dd"];
  if (typeof document === "undefined" || typeof getComputedStyle !== "function") return fallback;
  const root = getComputedStyle(document.documentElement);
  return fallback.map((fb, i) => root.getPropertyValue(`--hl-${i + 1}`).trim() || fb);
}

export const HL_PALETTE = readHlPalette();
export const NET_COLOR = HL_PALETTE[0];
export const COMP_COLOR = HL_PALETTE[1];

export function colorForNext(highlights: Highlight[]): string {
  const used = new Set(highlights.map((h) => h.color));
  return HL_PALETTE.find((c) => !used.has(c)) ?? HL_PALETTE[highlights.length % HL_PALETTE.length];
}

/** Which view last drove the selection — the other view syncs from it on tab switch. */
export type SelectionSource = "sch" | "pcb";

// The selection mirror. The canvas owns *rendering* (highlight overlay, badges) and
// drives it imperatively; this store is the read-only reflection that the properties
// card and status bar subscribe to. `selection` is the primary (last-added) member of
// the highlight set; `highlights` is the full set for the card's color chips.
interface SelectionState {
  selection: Selection;
  highlights: Highlight[];
  /** Persistent net/component highlights (item 22): right-click "Highlight in color"
   *  marks an object in a chosen color that survives reopening the project. Scoped per
   *  user AND project (stored server-side keyed by the active project slug). Distinct
   *  from the transient `highlights` click-selection. */
  pinned: Highlight[];
  source: SelectionSource;
  currentSheet: number | null;
  /** Screen-space (viewport) point the selection was made at, so the info panel
   *  can open next to the selection. Set by whichever view handled the click;
   *  null = no anchor yet (panel falls back to its default corner). */
  anchor: { x: number; y: number } | null;
  setSelection: (s: Selection, source?: SelectionSource) => void;
  setHighlights: (h: Highlight[], source?: SelectionSource) => void;
  setCurrentSheet: (n: number | null) => void;
  setAnchor: (a: { x: number; y: number } | null) => void;
  /** Load this project's persisted highlights (no-op if no active project). */
  loadPinned: () => Promise<void>;
  /** Add/replace a persistent highlight for an object, then persist. */
  pinHighlight: (h: Highlight) => Promise<void>;
  /** Remove a persistent highlight, then persist. */
  unpinHighlight: (kind: "net" | "comp", ref: string) => Promise<void>;
  /** Clear all persistent highlights for this project, then persist. */
  clearPinned: () => Promise<void>;
  /** The persistent color for an object, if any (selection reuses it — item 23). */
  pinnedColor: (kind: "net" | "comp", ref: string) => string | undefined;
}

// One board per project, and the highlights file already lives in the project
// folder, so the per-board scope is the folder itself — a fixed key inside.
const HL_KEY = "design";

// Serialize persistence: each pin/unpin is a read-modify-write over the shared highlights
// file, and completion order over IPC isn't call order — an earlier write finishing last
// would clobber a newer list and resurrect an unpinned highlight. Chaining the writes
// keeps them in call order; each task persists the current in-memory set (the synchronous
// source of truth), so the last write always reflects the latest state.
let persistChain: Promise<void> = Promise.resolve();
function persistPinned(): Promise<void> {
  persistChain = persistChain.then(async () => {
    if (!useProjectStore.getState().project) return;
    try {
      const all = ((await ipc.getHighlights()) ?? {}) as Record<string, Highlight[]>;
      all[HL_KEY] = useSelectionStore.getState().pinned;
      await ipc.setHighlights(all);
    } catch {
      // No project / write failure — keep the in-memory set; not worth surfacing.
    }
  });
  return persistChain;
}

export const useSelectionStore = create<SelectionState>((set, get) => ({
  selection: null,
  highlights: [],
  pinned: [],
  source: "sch",
  currentSheet: null,
  anchor: null,
  setSelection: (selection, source) =>
    set((st) => ({ selection, source: source ?? st.source })),
  setHighlights: (highlights, source) =>
    set((st) => ({ highlights, source: source ?? st.source })),
  setCurrentSheet: (currentSheet) => set({ currentSheet }),
  setAnchor: (anchor) => set({ anchor }),

  loadPinned: async () => {
    if (!useProjectStore.getState().project) {
      set({ pinned: [] });
      return;
    }
    try {
      const all = ((await ipc.getHighlights()) ?? {}) as Record<string, Highlight[]>;
      set({ pinned: Array.isArray(all[HL_KEY]) ? all[HL_KEY] : [] });
    } catch {
      set({ pinned: [] });
    }
  },
  pinHighlight: async (h) => {
    const list = get().pinned.filter((p) => !(p.kind === h.kind && p.ref === h.ref));
    list.push(h);
    set({ pinned: list });
    await persistPinned();
  },
  unpinHighlight: async (kind, ref) => {
    const list = get().pinned.filter((p) => !(p.kind === kind && p.ref === ref));
    set({ pinned: list });
    await persistPinned();
  },
  clearPinned: async () => {
    set({ pinned: [] });
    await persistPinned();
  },
  pinnedColor: (kind, ref) =>
    get().pinned.find((p) => p.kind === kind && p.ref === ref)?.color,
}));
