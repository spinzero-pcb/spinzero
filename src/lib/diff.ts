// Mirrors src-tauri/src/diff.rs — the `diff.a0` changeset that crosses the IPC
// boundary from `prepare_diff`. Keep field names in lockstep with the Rust structs
// (same discipline as design.ts ↔ design.rs): a rename on either side must be paired.
//
// The document is emitted by the pure Rust engine; the frontend only renders and
// steps it. The small pure helpers below (grouping / ordering / filtering, and the
// bbox→anchor adapter) are unit-tested in diff.test.ts.

import type { CommentAnchor } from "./types";

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
}

export interface ChangeAnchors {
  schematic?: SchematicAnchor;
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

/** Display order of groups in the Changes tree (electrical first, cosmetic last). */
export const GROUP_ORDER: ChangeGroup[] = [
  "component",
  "net",
  "sheet",
  "placement",
  "routing",
  "zone",
  "outline",
  "silk",
  "text",
  "doc",
];

/** Human label for a group header. */
export const GROUP_LABELS: Record<ChangeGroup, string> = {
  component: "Components",
  net: "Nets",
  sheet: "Sheets",
  placement: "Placement",
  routing: "Routing",
  zone: "Zones",
  outline: "Outline",
  silk: "Silkscreen",
  text: "Text",
  doc: "Document",
};

/** The three impact filter chips the panel exposes (doc folds under "Cosmetic" in the
 *  UI filter but keeps its own class in the data). */
export const IMPACT_FILTERS: { id: ChangeImpact; label: string }[] = [
  { id: "electrical", label: "Electrical" },
  { id: "placement", label: "Placement" },
  { id: "cosmetic", label: "Cosmetic" },
];

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
  group: ChangeGroup;
  label: string;
  changes: Change[];
}

/** Group changes by `group`, ordered by GROUP_ORDER, with each group's changes sorted
 *  by sheet/layer then position (§5). Empty groups are omitted. Pure. */
export function groupChanges(changes: Change[]): ChangeGroupNode[] {
  const byGroup = new Map<ChangeGroup, Change[]>();
  for (const c of changes) {
    const list = byGroup.get(c.group);
    if (list) list.push(c);
    else byGroup.set(c.group, [c]);
  }
  const out: ChangeGroupNode[] = [];
  for (const group of GROUP_ORDER) {
    const list = byGroup.get(group);
    if (!list || list.length === 0) continue;
    out.push({
      group,
      label: GROUP_LABELS[group],
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

/** Which impact-filter bucket a change falls into. `doc` impact shows under the
 *  "Cosmetic" chip (it is non-electrical, non-placement) so no change is ever
 *  unreachable when a filter is active. */
function impactBucket(impact: ChangeImpact): ChangeImpact {
  return impact === "doc" ? "cosmetic" : impact;
}

/** Filter changes by an active set of impact chips (empty set = show all) and a
 *  free-text query (matches title + detail, case-insensitive). Pure. */
export function filterChanges(
  changes: Change[],
  impacts: ReadonlySet<ChangeImpact>,
  query: string,
): Change[] {
  const q = query.trim().toLowerCase();
  return changes.filter((c) => {
    if (impacts.size > 0 && !impacts.has(impactBucket(c.impact))) return false;
    if (q && !`${c.title} ${c.detail ?? ""}`.toLowerCase().includes(q)) return false;
    return true;
  });
}

/** Total change count by impact class, from the doc's stats (with a fallback that
 *  recomputes from the change list if stats are absent). Pure. */
export function countByImpact(doc: DiffDoc): Record<ChangeImpact, number> {
  const s = doc.stats;
  if (s.electrical || s.placement || s.cosmetic || s.doc) {
    return {
      electrical: s.electrical ?? 0,
      placement: s.placement ?? 0,
      cosmetic: s.cosmetic ?? 0,
      doc: s.doc ?? 0,
    };
  }
  const out: Record<ChangeImpact, number> = { electrical: 0, placement: 0, cosmetic: 0, doc: 0 };
  for (const c of doc.changes) out[c.impact]++;
  return out;
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

/** Adapt a change's PCB anchor to a `CommentAnchor` that `pcbNav.reveal` understands
 *  (§7): prefer a net, else a component, else the bbox centre as a region so the
 *  camera still lands. Returns null when the change has no PCB anchor. Pure. */
export function pcbAnchorToCommentAnchor(c: Change): CommentAnchor | null {
  const pcb = c.anchors.pcb;
  if (!pcb) return null;
  if (pcb.net) return { type: "net", ref: pcb.net };
  if (pcb.comp) return { type: "component", ref: pcb.comp };
  if (pcb.bbox) {
    const [x, y, w, h] = pcb.bbox;
    return { type: "region", ref: c.id, rect: { x, y, w, h } };
  }
  return null;
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
