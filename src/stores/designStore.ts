import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { DesignIndexes, SheetLite } from "../lib/design";
import { parsePcbGeometry, type PcbGeometry } from "../lib/pcbGeometry";
import { applyKicadTheme, clearKicadTheme } from "../lib/kicadTheme";
import { useToastStore } from "./toastStore";
import { useProjectStore } from "./projectStore";

/** A previously-loaded artifact failed to read mid-session — the extraction
 *  folder was likely deleted/moved on disk while the app was running. Notify once
 *  (deduped) and offer a reload that re-resolves to the latest available
 *  extraction (the backend falls back automatically when the active id is gone). */
function notifyExtractionMissing() {
  useToastStore.getState().push({
    kind: "error",
    key: "extraction-missing",
    title: "Extraction data unavailable",
    message:
      "This extraction may have been deleted or moved on disk. Reload to use the latest available one.",
    action: {
      label: "Reload",
      onClick: () => {
        void useDesignStore.getState().load();
        void useProjectStore.getState().refreshIndex();
      },
    },
  });
}

// ---------------------------------------------------------------- PCB index
// Cross-probe data mined from the enriched copper-layer SVGs (items 3/5/9):
// which layers a net runs on, routed track length per layer, track widths, via
// count, and which board side each footprint sits on. Built once per revision by
// regex over the artifact text — no DOM mount needed.

export interface PcbNetInfo {
  /** copper layers carrying track/zone/pad geometry of this net, board order */
  layers: string[];
  /** routed track length per layer, mm */
  lenByLayer: Record<string, number>;
  /** distinct track widths, mm, ascending */
  widths: number[];
  vias: number;
}

export interface PcbIndex {
  nets: Record<string, PcbNetInfo>;
  compSide: Record<string, "front" | "back" | "both">;
}

const unescapeXml = (s: string) =>
  s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, "&");

function polylineLength(points: string): number {
  const nums = points.split(/[\s,]+/).map(Number).filter((n) => !Number.isNaN(n));
  let len = 0;
  for (let i = 3; i < nums.length; i += 2)
    len += Math.hypot(nums[i - 1] - nums[i - 3], nums[i] - nums[i - 2]);
  return len;
}

function scanLayerSvg(
  layer: string,
  text: string,
  out: Record<string, PcbNetInfo>,
  viaSeen: Set<string>,
  compSide: Record<string, "front" | "back" | "both">,
) {
  const entry = (net: string): PcbNetInfo =>
    (out[net] ??= { layers: [], lenByLayer: {}, widths: [], vias: 0 });
  const attr = (attrs: string, name: string) => {
    const m = attrs.match(new RegExp(`${name}="([^"]*)"`));
    return m ? unescapeXml(m[1]) : null;
  };
  const isCopper = layer.endsWith(".Cu");

  const tagRe = /<g ([^>]*data-primitive="(track|zone|pad|via|footprint)"[^>]*)>/g;
  let m: RegExpExecArray | null;
  while ((m = tagRe.exec(text))) {
    const attrs = m[1];
    const kind = m[2];
    if (kind === "footprint") {
      if (!isCopper) continue; // silk/fab bundles also carry footprints
      const comp = attr(attrs, "data-component");
      if (!comp) continue;
      const side = layer.startsWith("B") ? "back" : "front";
      const prev = compSide[comp];
      compSide[comp] = prev && prev !== side ? "both" : side;
      continue;
    }
    const net = attr(attrs, "data-net");
    if (!net || !isCopper) continue;
    const e = entry(net);
    if (kind === "via") {
      const uuid = attr(attrs, "data-uuid") ?? "";
      if (!viaSeen.has(uuid)) {
        viaSeen.add(uuid);
        e.vias++;
      }
      continue;
    }
    if (!e.layers.includes(layer)) e.layers.push(layer);
    if (kind === "track") {
      // Geometry sits within the group right after the tag; a bounded window
      // keeps the scan from bleeding into the next element.
      const win = text.slice(m.index, m.index + 2000);
      const w = win.match(/stroke-width:\s*([\d.]+)/);
      if (w) {
        const width = Number(w[1]);
        if (!e.widths.includes(width)) e.widths.push(width);
      }
      const pts = win.match(/points="([^"]+)"/);
      if (pts)
        e.lenByLayer[layer] = (e.lenByLayer[layer] ?? 0) + polylineLength(pts[1]);
    }
  }
}

interface DesignState {
  indexes: DesignIndexes | null;
  loaded: boolean;
  loadError: string | null;
  /** D1: a re-crunch finished while a design is on screen — show the reload
   *  banner instead of yanking the canvas out from under the reviewer. */
  pendingReload: boolean;
  /** sheet number -> in-flight/settled SVG-text fetch, memoised per revision. Caching the
   *  Promise (not the resolved string) dedups concurrent reads of the same multi-MB sheet
   *  (Canvas + Overview) so both share one IPC + strip. */
  svgCache: Map<number, Promise<string>>;
  /** cache-relative path -> artifact-text fetch (PCB layer SVGs), same lifecycle */
  artifactCache: Map<string, Promise<string>>;
  sheetByNum: Map<number, SheetLite>;
  /** PCB cross-probe index; null until built (lazy, once per revision) */
  pcbIndex: PcbIndex | null;
  /** Parsed PCB geometry IR (GPU renderer input); null until fetched (lazy) */
  pcbGeometry: PcbGeometry | null;

  load: () => Promise<void>;
  /** Drop the loaded design (open-folder: never show the previous project). */
  clear: () => void;
  markPendingReload: () => void;
  getSheetSvg: (num: number) => Promise<string>;
  getArtifact: (relPath: string) => Promise<string>;
  buildPcbIndex: () => Promise<void>;
  /** Fetch + parse the PCB geometry IR (once per revision); null when the bundle
   *  has none (schematic-only / older extraction). */
  getPcbGeometry: () => Promise<PcbGeometry | null>;
}

// Monotonic token so an out-of-order `load()` (a slow project A resolving after the
// user switched to B, or overlapping crunch-event reloads) can't commit stale indexes
// over newer ones — only the most recently started load wins.
let loadGen = 0;

export const useDesignStore = create<DesignState>((set, get) => ({
  indexes: null,
  loaded: false,
  loadError: null,
  pendingReload: false,
  svgCache: new Map(),
  artifactCache: new Map(),
  sheetByNum: new Map(),
  pcbIndex: null,
  pcbGeometry: null,

  load: async () => {
    const gen = ++loadGen;
    try {
      const indexes = await ipc.getDesignIndexes();
      if (gen !== loadGen) return; // a newer load() started while we awaited
      const sheetByNum = new Map(indexes.sheets.map((s) => [s.num, s]));
      set({
        indexes,
        sheetByNum,
        loaded: true,
        loadError: null,
        pendingReload: false,
        svgCache: new Map(),
        artifactCache: new Map(),
        pcbIndex: null,
        pcbGeometry: null,
      });
      // Theme the viewer with the user's real KiCad palette (or revert to the
      // tokens.css default when the bundle carries no theme).
      applyKicadTheme(indexes.theme);
      void get().buildPcbIndex();
    } catch (e) {
      if (gen !== loadGen) return; // superseded — don't clobber the newer load's state
      // No crunched bundle available yet (no vault open / no dev cache override).
      clearKicadTheme();
      set({ loaded: false, loadError: String(e) });
    }
  },

  clear: () => {
    clearKicadTheme();
    set({
      indexes: null,
      loaded: false,
      loadError: null,
      pendingReload: false,
      svgCache: new Map(),
      artifactCache: new Map(),
      sheetByNum: new Map(),
      pcbIndex: null,
      pcbGeometry: null,
    });
  },

  markPendingReload: () => set({ pendingReload: true }),

  getSheetSvg: (num) => {
    const { svgCache, sheetByNum } = get();
    const cached = svgCache.get(num);
    if (cached) return cached;
    const sheet = sheetByNum.get(num);
    if (!sheet?.svg) return Promise.reject(new Error(`sheet ${num} has no SVG`));
    const p = ipc.readArtifact(sheet.svg).catch((e) => {
      svgCache.delete(num); // don't cache a failure — let a later reload re-fetch
      notifyExtractionMissing();
      throw e;
    });
    svgCache.set(num, p); // Map is mutated in place; not part of reactive state
    return p;
  },

  getArtifact: (relPath) => {
    const { artifactCache } = get();
    const cached = artifactCache.get(relPath);
    if (cached) return cached;
    const p = ipc.readArtifact(relPath).catch((e) => {
      artifactCache.delete(relPath);
      notifyExtractionMissing();
      throw e;
    });
    artifactCache.set(relPath, p);
    return p;
  },

  buildPcbIndex: async () => {
    const { indexes, getArtifact } = get();
    if (!indexes || indexes.layers.length === 0) return;
    const nets: Record<string, PcbNetInfo> = {};
    const compSide: Record<string, "front" | "back" | "both"> = {};
    const viaSeen = new Set<string>();
    // Copper carries all the net/side facts; skip silk/mask/fab bundles (they are
    // fetched lazily when the PCB view mounts).
    for (const layer of indexes.layers.filter((l) => l.name.endsWith(".Cu"))) {
      try {
        scanLayerSvg(layer.name, await getArtifact(layer.svg), nets, viaSeen, compSide);
      } catch {
        /* missing artifact — index stays partial */
      }
    }
    // Board order F → inner → B for the card's layer rows.
    const order = indexes.layers.map((l) => l.name);
    for (const n of Object.values(nets)) {
      n.layers.sort((a, b) => order.indexOf(a) - order.indexOf(b));
      n.widths.sort((a, b) => a - b);
    }
    // Stale-guard: a reload may have swapped indexes while we scanned.
    if (get().indexes === indexes) set({ pcbIndex: { nets, compSide } });
  },

  getPcbGeometry: async () => {
    const { pcbGeometry, indexes } = get();
    if (pcbGeometry) return pcbGeometry;
    const rel = indexes?.pcb_geometry;
    if (!rel) return null;
    // Read directly (not via getArtifact) so the multi-MB IR text isn't also
    // retained in artifactCache — we keep only the parsed object.
    let text: string;
    try {
      text = await ipc.readArtifact(rel);
    } catch (e) {
      // The read itself failed — the extraction was likely deleted/moved on disk.
      notifyExtractionMissing();
      void ipc.logWarn(`PCB geometry IR read failed: ${String(e)}`);
      return null;
    }
    try {
      const geom = parsePcbGeometry(text);
      if (get().indexes === indexes) set({ pcbGeometry: geom });
      return geom;
    } catch (e) {
      // Bytes present but don't parse (a stale / schema-incompatible IR from an older
      // bundle) — NOT a missing extraction. Degrade quietly to the SVG renderer rather
      // than falsely telling the user their extraction was deleted.
      void ipc.logWarn(`PCB geometry IR parse failed: ${String(e)}`);
      return null;
    }
  },
}));
