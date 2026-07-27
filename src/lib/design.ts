// Mirrors src-tauri/src/design.rs — the single payload the hyperlinked canvas runs on
// (port of prototypes/extract_data2.py). Field names are snake_case over IPC.

export interface SheetLite {
  num: number;
  name: string;
  /** cache-relative path for read_artifact, or null if no SVG matched */
  svg: string | null;
}

export interface TerminalLite {
  d: string; // designator
  p: string; // pin
  pn: string; // pin name
  pt: string; // pin type
}

export interface NetLite {
  class: string;
  terminals: TerminalLite[];
  sheets: number[];
  /** sheet number (string key) -> element uuids of this net on that sheet */
  by_sheet: Record<string, string[]>;
}

export interface CompLite {
  value: string;
  mpn: string;
  mfr: string;
  fp: string;
  desc: string;
  sheet: number | null;
  dnp: boolean;
  nets: string[];
  svg_id: string;
  /** Placed schematic bounding box [x, y, w, h] on its sheet, when the library
   *  symbol has geometry (used by the diff engine to detect symbol moves). */
  bbox?: [number, number, number, number];
}

export interface LayerLite {
  name: string;
  /** cache-relative SVG path, ready for read_artifact */
  svg: string;
  /** EDA-agnostic role (copper/silkscreen/mask/paste/courtyard/fab/edge/user) from the
   *  manifest; drives theming without hardcoding layer names. Absent on older bundles. */
  role?: string;
  /** front/back/inner when the manifest carries it; derived from the layer name otherwise. */
  side?: string;
  /** Designer's display name for a renamed/user layer ("Mechanical Drawing" for User.3);
   *  the appearance panel shows this instead of the canonical name when present. */
  user_name?: string;
  /** Resolved #RRGGBB for a non-standard ("user") layer, from the KiCad theme. Standard
   *  fabrication layers omit this and theme via CSS vars (see {@link applyKicadTheme}). */
  color?: string;
}

/** The KiCad colour theme the extractor resolved from the user's active theme.
 *  `schematic` holds flat keys (wire, worksheet, junction, …); `board` flattens
 *  the copper group (copper.f, copper.in1) alongside the flat ones (f_silks, …).
 *  Absent/null for theme-less bundles — the viewer then keeps its tokens.css. */
export interface KicadTheme {
  schematic?: Record<string, string>;
  board?: Record<string, string>;
}

export interface DesignIndexes {
  sheets: SheetLite[];
  /** PCB copper layers from the manifest (WS8); empty for schematic-only bundles */
  layers: LayerLite[];
  /** KiCad palette the viewer themes with (see {@link applyKicadTheme}). */
  theme?: KicadTheme | null;
  svg_to_net: Record<string, string>;
  /** Multi-valued: shared-file sheet instances (gate_driver U/V/W) map one element
   *  uuid to one net per instance; disambiguate by the current sheet. */
  svg_to_nets: Record<string, string[]>;
  svg_to_comp: Record<string, string>;
  elem_kind: Record<string, string>;
  nets: Record<string, NetLite>;
  components: Record<string, CompLite>;
  /** Cache-relative path of the structured PCB geometry IR (`pcb/geometry.json`),
   *  the GPU renderer's input. Absent for schematic-only/older bundles —
   *  the renderer then falls back to the SVG layers. See {@link PcbGeometry}. */
  pcb_geometry?: string;
}

/** A sidebar sheet reference. The SQLite index (manifest) and the design JSON number
 *  sheets differently, so cross-referencing goes by SVG filename, then by name. */
export interface SheetRef {
  number: number;
  name: string;
  svg_path?: string;
}

const normName = (s: string) => s.toLowerCase().replace(/[\s_]+/g, "");
const baseName = (p: string | null | undefined) =>
  p?.split(/[\\/]/).pop()?.toLowerCase() ?? null;

/** Does design sheet `d` refer to the same sheet as sidebar ref `ref`? */
export function sheetMatches(d: SheetLite, ref: SheetRef): boolean {
  const a = baseName(d.svg);
  const b = baseName(ref.svg_path);
  if (a && b) return a === b;
  return normName(d.name) === normName(ref.name);
}

/** What a click resolves to, and what the properties card / status bar render. */
export type Selection =
  | { kind: "net"; ref: string }
  | { kind: "comp"; ref: string }
  | { kind: "pin"; ref: { designator: string; pin: string } }
  | null;
