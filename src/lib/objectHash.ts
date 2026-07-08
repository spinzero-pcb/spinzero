// Deterministic, version-independent hash + meta snapshot for a comment anchor.
//
// This is the cheap engine behind the ⟳ re-check loop (phase2-workflow.md §4–§6):
// a comment stores the anchored object's hash + a small meta snapshot at the
// revision it was filed against. On every re-crunch the live design is hashed the
// same way; a difference means the designer's edit *touched* this object, so the
// comment auto-flips to "needs re-check" and the meta diff drives the mini-diff
// ("value 10k→4.7k"). The app detects *changed*, never *fixed* — that stays a
// human click. Hashing lives in the frontend because the design is already loaded
// there (no extra backend round-trip), and it must be stable across machines, so
// it depends only on electrical fields — never on SVG bytes or coordinates.

import type { CommentAnchor } from "./types";
import type { DesignIndexes } from "./design";

/** FNV-1a over a UTF-16 string → 8-hex-char digest. Not cryptographic; we only
 *  need a stable equality key, and collisions merely miss a re-check prompt. */
function fnv1a(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

export interface AnchorState {
  hash: string;
  /** Small, human-readable field snapshot — diffed to build the mini-diff. */
  meta: Record<string, unknown>;
}

/** Hash + meta for the object an anchor points at in the given design, or null if
 *  the object no longer exists (deleted parts/nets show as drift, never crash). */
export function anchorState(
  anchor: CommentAnchor,
  indexes: DesignIndexes,
): AnchorState | null {
  // Region (box-select) anchors point at coordinates, not an electrical object, so
  // there is nothing to hash — they never drift / re-check.
  if (anchor.type === "region") return null;
  if (anchor.type === "component") {
    const c = indexes.components[anchor.ref];
    if (!c) return null;
    const meta = {
      value: c.value,
      footprint: c.fp,
      mpn: c.mpn,
      dnp: c.dnp,
      nets: [...c.nets].sort(),
    };
    return { hash: fnv1a(JSON.stringify(meta)), meta };
  }
  const n = indexes.nets[anchor.ref];
  if (!n) return null;
  const terminals = n.terminals.map((t) => `${t.d}.${t.p}`).sort();
  const meta = { class: n.class, pins: terminals.length, terminals };
  return { hash: fnv1a(JSON.stringify(meta)), meta };
}

/** Human-readable mini-diff between a comment's base meta and the live object.
 *  Returns one short line per changed field (phase2-workflow.md §5/§6.2). */
export function metaDiff(
  base: Record<string, unknown> | null,
  current: Record<string, unknown> | null,
): string[] {
  if (!base || !current) return [];
  const lines: string[] = [];
  const keys = new Set([...Object.keys(base), ...Object.keys(current)]);
  for (const k of keys) {
    const a = base[k];
    const b = current[k];
    const av = Array.isArray(a) ? a.join(",") : String(a ?? "—");
    const bv = Array.isArray(b) ? b.join(",") : String(b ?? "—");
    if (av === bv) continue;
    if (k === "terminals") {
      // Too long to print whole; summarize by count delta (pins covers the rest).
      continue;
    }
    lines.push(`${k} ${av}→${bv}`);
  }
  return lines;
}
