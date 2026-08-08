// BOM diff-mode helpers (visual-diff plan §8): pure functions that map the diff
// document's `group: "bom"` changes onto the BOM table's rows — status + tint per
// line, synthetic rows for removed lines (shown struck-through from revision A),
// changes-first ordering, and the "Copy BOM delta as CSV" payload. All pure and
// unit-tested in bomDiff.test.ts; the BomTab component only renders.

import type { BomLine } from "./types";
import type { BomAnchor, Change } from "./diff";

/** The visual status a BOM row carries in diff mode. */
export type BomRowStatus = "added" | "removed" | "changed";

/** A BOM table row in diff mode: the underlying line + its diff decoration.
 *  `synthetic` rows come from removed lines (they exist only on revision A and are
 *  reconstructed from the change's anchor, since B's BOM artifact no longer has them). */
export interface DiffBomRow {
  line: BomLine;
  status: BomRowStatus | null;
  /** The change ids that land on this row (stepper flash + designator links). */
  changeIds: string[];
  /** The row's anchor key when a change matched it ("" for untouched rows). */
  key: string;
  synthetic: boolean;
}

/** The `group: "bom"` subset of a changeset. */
export function bomChanges(changes: Change[]): Change[] {
  return changes.filter((c) => c.group === "bom" && !!c.anchors.bom);
}

/** The (value, short footprint, mpn) grouping key — MUST mirror `bom_key` in
 *  src-tauri/src/diff.rs (U+001F separator, library prefix stripped). */
export function lineKey(value: string, footprint: string, mpn: string): string {
  const SEP = "\u001f";
  return `${value}${SEP}${fpShort(footprint)}${SEP}${mpn}`;
}

/** Short footprint name — library prefix stripped (mirrors design.rs bom_lines). */
export function fpShort(fp: string): string {
  const i = fp.lastIndexOf(":");
  return i >= 0 ? fp.slice(i + 1) : fp;
}

/** How a change kind tints its row. Removed wins over changed; added stays added. */
function statusOf(kind: Change["kind"]): BomRowStatus {
  if (kind === "added") return "added";
  if (kind === "removed") return "removed";
  return "changed";
}

/** Match a change's anchor to a table line: exact key first, then designator overlap
 *  (the BOM artifact's fields can differ from the schematic's — MPN sourcing, value
 *  normalization — so the key alone can miss; any shared designator is decisive
 *  because a designator appears on exactly one BOM line). */
function matches(anchor: BomAnchor, line: BomLine): boolean {
  if (anchor.key === lineKey(line.value, line.footprint, line.mpn)) return true;
  const dsg = new Set(line.designators);
  return anchor.designators.some((d) => dsg.has(d));
}

/** Decorate the (B-side) BOM lines with diff statuses and append synthetic rows for
 *  removed lines. Preserves the input line order; sorting is the caller's concern. */
export function decorateBomRows(lines: BomLine[], changes: Change[]): DiffBomRow[] {
  const rows: DiffBomRow[] = lines.map((line) => ({
    line,
    status: null,
    changeIds: [],
    key: "",
    synthetic: false,
  }));
  const removed: DiffBomRow[] = [];
  let syntheticItem = Math.max(0, ...lines.map((l) => l.item));

  for (const c of bomChanges(changes)) {
    const anchor = c.anchors.bom!;
    if (c.kind === "removed") {
      // The line exists only on revision A — synthesize its row from the anchor.
      syntheticItem += 1;
      removed.push({
        line: {
          item: syntheticItem,
          qty: anchor.qtyA,
          designators: anchor.designators,
          value: anchor.value,
          footprint: anchor.footprint,
          mpn: anchor.mpn,
          dnp: false,
          fields: {},
        },
        status: "removed",
        changeIds: [c.id],
        key: anchor.key,
        synthetic: true,
      });
      continue;
    }
    const row = rows.find((r) => matches(anchor, r.line));
    if (!row) continue; // BOM artifact and schematic disagree — the change row still lives in the panel
    row.changeIds.push(c.id);
    row.key = row.key || anchor.key;
    // "added" outranks "changed" (an added line can also carry a DNP-flip row).
    if (row.status !== "added") row.status = statusOf(c.kind);
  }
  return [...rows, ...removed];
}

/** The OLD (revision A) field values behind a "changed" row, for the inline
 *  "old → new" cell rendering. Fields absent when nothing extractable changed. */
export interface BomOldValues {
  value?: string;
  footprint?: string;
  mpn?: string;
  qty?: number;
  /** Old values of the symbol properties the line's members changed (MSL, Automotive
   *  Grade, …), keyed by `loose()` field name so a preset column matches whatever
   *  spelling/casing KiCad used. */
  fields: Record<string, string>;
}

/** "∅" is the Rust side's empty-field placeholder (see `disp` in diff.rs). */
const EMPTY_MARK = "∅";

/** Extract the old field values a row's changes describe: the "value A → B" /
 *  "footprint A → B" / "MPN A → B" bits of `detail` (emitted by diff_bom for a
 *  folded BOM line), plus qty from the anchor's qtyA/qtyB. Only "modified"
 *  changes contribute — added/removed rows have no old-vs-new within one line. */
export function bomOldValues(changes: Change[]): BomOldValues {
  const out: BomOldValues = { fields: {} };
  const FIELD: Record<string, "value" | "footprint" | "mpn"> = {
    value: "value",
    footprint: "footprint",
    MPN: "mpn",
  };
  for (const c of changes) {
    if (c.group !== "bom" || c.kind === "added" || c.kind === "removed") continue;
    const anchor = c.anchors.bom;
    if (anchor && anchor.qtyA !== anchor.qtyB && out.qty === undefined) out.qty = anchor.qtyA;
    // Symbol-property edits ride structured on the anchor (their names and values are
    // free-form, so they can't be recovered from `detail` unambiguously).
    for (const e of anchor?.fields ?? []) {
      const key = looseField(e.field);
      if (key && out.fields[key] === undefined) out.fields[key] = e.old;
    }
    for (const bit of (c.detail ?? "").split(";")) {
      const m = /^(value|footprint|MPN)\s+(.*?)\s+→\s+(.*)$/.exec(bit.trim());
      if (!m) continue;
      const field = FIELD[m[1]];
      if (out[field] === undefined) out[field] = m[2] === EMPTY_MARK ? "" : m[2];
    }
  }
  return out;
}

/** Loose field key for matching a diff bit's field name to a preset column — lowercased
 *  with non-alphanumerics dropped (mirrors `loose` in bomColumns.ts). */
export function looseField(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]/g, "");
}

/** Changes-first comparator (the diff-mode default sort): tinted rows before
 *  untouched ones — removed/changed/added in review-priority order — then item. */
export function changesFirstCompare(a: DiffBomRow, b: DiffBomRow): number {
  const rank = (r: DiffBomRow) =>
    r.status === "removed" ? 0 : r.status === "changed" ? 1 : r.status === "added" ? 2 : 3;
  const d = rank(a) - rank(b);
  if (d !== 0) return d;
  return a.line.item - b.line.item;
}

/** RFC-4180-ish CSV escaping: quote when the cell contains a comma, quote or newline. */
function csvCell(s: string): string {
  return /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

/** The "Copy BOM delta as CSV" payload (plan §8): one row per BOM change, for the
 *  purchasing conversation. Deterministic — follows the changeset order. */
export function bomDeltaCsv(changes: Change[]): string {
  const header = [
    "status",
    "value",
    "footprint",
    "mpn",
    "qty_a",
    "qty_b",
    "designators_added",
    "designators_removed",
    "note",
  ];
  const kindLabel = (k: Change["kind"]) =>
    k === "added" ? "added" : k === "removed" ? "removed" : "changed";
  const lines = [header.join(",")];
  for (const c of bomChanges(changes)) {
    const a = c.anchors.bom!;
    lines.push(
      [
        kindLabel(c.kind),
        a.value,
        a.footprint,
        a.mpn,
        String(a.qtyA),
        String(a.qtyB),
        (a.added ?? []).join(" "),
        (a.removed ?? []).join(" "),
        c.detail ? `${c.title} — ${c.detail}` : c.title,
      ]
        .map(csvCell)
        .join(","),
    );
  }
  return lines.join("\r\n");
}
