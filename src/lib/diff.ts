// Mirrors src-tauri/src/diff.rs — the `diff.a0` changeset that crosses the IPC
// boundary from `prepare_diff`. Keep field names in lockstep with the Rust structs
// (same discipline as design.ts ↔ design.rs): a rename on either side must be paired.
//
// The document is emitted by the pure Rust engine; the frontend only renders and
// steps it. The small pure helpers below (grouping / ordering / filtering / layer
// union) are unit-tested in diff.test.ts.

// ----------------------------------------------------------------- the diff document

/** `[x, y, w, h]` in board mm — a camera-landing rect on the PCB. */
export type Bbox = [number, number, number, number];

export type ChangeGroup =
  | "component"
  | "net"
  | "placement"
  | "routing"
  | "zone"
  | "silk"
  | "text"
  | "outline"
  | "sheet"
  | "doc";

export type ChangeKind = "added" | "removed" | "modified" | "renamed" | "moved";

export type ChangeImpact = "electrical" | "placement" | "cosmetic" | "doc";

/** Which canvas(es) can show a change. */
export type ChangeSide = "a" | "b" | "both";

export interface SchematicAnchor {
  sheet: number;
  /** Element uuids on that sheet (drives the highlight tint). */
  uuids: string[];
}

export interface PcbAnchor {
  /** Camera-landing rect; absent when only a net/comp id is known. */
  bbox?: Bbox;
  layers?: string[];
  comp?: string;
  net?: string;
  /** True for the routing row that owns the net's changed VIAS (a via spans several
   *  copper layers, so it can't belong to any per-layer track row). Absent = false. */
  vias?: boolean;
}

export interface ChangeAnchors {
  schematic?: SchematicAnchor;
  /** A-side schematic anchor, present only when the changed object's uuids differ
   *  between the revisions (re-annotated symbol, renamed net). The A island paints
   *  this when present, else `schematic`. */
  schematicA?: SchematicAnchor;
  pcb?: PcbAnchor;
}

export interface Change {
  id: string;
  group: ChangeGroup;
  kind: ChangeKind;
  impact: ChangeImpact;
  /** The one-line semantic statement. */
  title: string;
  /** Longer secondary explanation; absent when there is nothing to add. */
  detail?: string;
  anchors: ChangeAnchors;
  side: ChangeSide;
  /** Text to emphasize inside the A-side tint (the OLD value string of a field
   *  modification) — rendered red on the older canvas. */
  emphA?: string;
  /** Text to emphasize inside the B-side tint (the NEW value string) — green. */
  emphB?: string;
}

export interface DiffSide {
  rev: string;
  label: string;
}

/** Counts by impact class; zero values are omitted by the Rust serializer. */
export interface Stats {
  electrical?: number;
  placement?: number;
  cosmetic?: number;
  doc?: number;
}

export interface DiffDoc {
  schema: string; // "diff.a0"
  /** Older / base side. */
  a: DiffSide;
  /** Newer / target side. */
  b: DiffSide;
  changes: Change[];
  stats: Stats;
  /** Source-hash-identical schematic sheets that were skipped (sheet numbers). */
  sheetsPruned: number[];
}

/** What `prepare_diff` returns. Field names are snake_case (Rust serde default). */
export interface DiffHandle {
  doc: DiffDoc;
  /** Machine-local path of the cached diff.json (regenerable, never synced). */
  path: string;
  cache_key_a: string;
  cache_key_b: string;
  label_a: string;
  label_b: string;
}

// ----------------------------------------------------------------- pure helpers

/** The Changes tree groups by impact — the one categorization the panel shows (the
 *  old object-type grouping, Components/Nets/Sheets/…, was removed as a second
 *  competing taxonomy; `Change.group` stays in the data for the renderers). `doc`
 *  folds into "cosmetic" via impactBucket so every change lands in exactly one
 *  bucket. Display order: what can break the board first, looks last. */
export type ImpactBucket = Exclude<ChangeImpact, "doc">;

export const IMPACT_ORDER: ImpactBucket[] = ["electrical", "placement", "cosmetic"];

export const IMPACT_LABELS: Record<ImpactBucket, string> = {
  electrical: "Electrical",
  placement: "Placement",
  cosmetic: "Cosmetic",
};

/** Which impact bucket a change falls into: its own class, except `doc`, which shows
 *  under "Cosmetic" (non-electrical, non-placement) so no change is ever unreachable. */
export function impactBucket(impact: ChangeImpact): ImpactBucket {
  return impact === "doc" ? "cosmetic" : impact;
}

/** A group's first change's within-group sort key: sheet (schematic) or first layer
 *  name (PCB), then the anchor position, then the id — a total order so stepping is
 *  deterministic and the same across renders. */
function orderKey(c: Change): [number, string, number, number, string] {
  const sch = c.anchors.schematic;
  const pcb = c.anchors.pcb;
  const sheet = sch ? sch.sheet : Number.MAX_SAFE_INTEGER;
  const layer = pcb?.layers?.[0] ?? "";
  const x = pcb?.bbox ? pcb.bbox[0] : 0;
  const y = pcb?.bbox ? pcb.bbox[1] : 0;
  return [sheet, layer, y, x, c.id];
}

function cmpOrderKey(a: Change, b: Change): number {
  const ka = orderKey(a);
  const kb = orderKey(b);
  for (let i = 0; i < ka.length; i++) {
    const x = ka[i];
    const y = kb[i];
    if (x < y) return -1;
    if (x > y) return 1;
  }
  return 0;
}

export interface ChangeGroupNode {
  impact: ImpactBucket;
  label: string;
  changes: Change[];
}

/** Group changes by impact bucket, ordered by IMPACT_ORDER, with each bucket's changes
 *  sorted by sheet/layer then position (§5). Empty buckets are omitted. Pure. */
export function groupChanges(changes: Change[]): ChangeGroupNode[] {
  const byBucket = new Map<ImpactBucket, Change[]>();
  for (const c of changes) {
    const bucket = impactBucket(c.impact);
    const list = byBucket.get(bucket);
    if (list) list.push(c);
    else byBucket.set(bucket, [c]);
  }
  const out: ChangeGroupNode[] = [];
  for (const impact of IMPACT_ORDER) {
    const list = byBucket.get(impact);
    if (!list || list.length === 0) continue;
    out.push({
      impact,
      label: IMPACT_LABELS[impact],
      changes: [...list].sort(cmpOrderKey),
    });
  }
  return out;
}

/** The flat, ordered walk sequence the stepper (prev/next, J/K) advances through:
 *  every visible change in group-then-within-group order. Pure. */
export function orderedChanges(changes: Change[]): Change[] {
  return groupChanges(changes).flatMap((g) => g.changes);
}

/** Filter changes by a free-text query (matches title + detail, case-insensitive).
 *  Pure. */
export function filterChanges(changes: Change[], query: string): Change[] {
  const q = query.trim().toLowerCase();
  if (!q) return changes;
  return changes.filter((c) => `${c.title} ${c.detail ?? ""}`.toLowerCase().includes(q));
}

/** Does a change land on the schematic canvas? (An empty-uuid anchor still lands —
 *  e.g. a sheet add/remove navigates to the sheet with nothing to tint.) */
export function hasSchematicAnchor(c: Change): boolean {
  return !!c.anchors.schematic;
}

/** Does a change land on the PCB canvas? */
export function hasPcbAnchor(c: Change): boolean {
  return !!c.anchors.pcb;
}

/** The union of PCB layers the changes land on — the "relevant layers" the diff view
 *  shows by default (overlay-all). Edge.Cuts rides along when the board has it so the
 *  outline keeps framing the copper. Returns [] when no change names a layer (then the
 *  caller leaves the user's layer view alone). Pure. */
export function pcbLayerUnion(changes: Change[], knownLayers: string[]): string[] {
  const known = new Set(knownLayers);
  const union = new Set<string>();
  for (const c of changes) {
    for (const l of c.anchors.pcb?.layers ?? []) {
      if (known.has(l)) union.add(l);
    }
  }
  if (union.size > 0 && known.has("Edge.Cuts")) union.add("Edge.Cuts");
  return knownLayers.filter((l) => union.has(l)); // known-table order, deterministic
}

/** The colour role a change paints with, per side (§4): removed → error red,
 *  added → ok green, modified/renamed/moved → warn amber. Returned as the CSS-var
 *  token family so components never hardcode a colour. */
export type TintRole = "err" | "ok" | "warn";

export function tintRole(kind: ChangeKind): TintRole {
  if (kind === "removed") return "err";
  if (kind === "added") return "ok";
  return "warn"; // modified | renamed | moved
}

/** Should this change tint the A (older) side? `side` states exactly which canvases
 *  hold the object: removed → "a", added → "b", modified/renamed/moved → "both". */
export function tintsA(c: Change): boolean {
  return c.side !== "b";
}

/** Should this change tint the B (newer) side? See tintsA. */
export function tintsB(c: Change): boolean {
  return c.side !== "a";
}
