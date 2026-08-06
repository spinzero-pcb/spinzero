import type { BomLine, BomPreset, BomPresetField } from "./types";

/** The BOM table's built-in columns — the ones the row object carries natively and the
 *  ones diff mode decorates (old → new, designator chips, status tint). A preset column
 *  that names one of these maps onto it instead of reading `line.fields`. */
export type BuiltinKey =
  | "status"
  | "item"
  | "qty"
  | "designators"
  | "value"
  | "footprint"
  | "mpn"
  | "dnp";

/** One column of the active column set. `builtin` picks the native renderer/accessor;
 *  otherwise the cell reads `line.fields` by `field` (case-insensitively). */
export interface BomCol {
  /** Stable id — used for sorting, persistence of hidden columns and React keys. */
  id: string;
  label: string;
  builtin?: BuiltinKey;
  /** Raw preset field name (e.g. "Manufacturer", "${QUANTITY}"). */
  field?: string;
  /** The Δ column only exists while diff mode is active. */
  diffOnly?: boolean;
}

/** The built-in column set ("Default" in the preset dropdown) — unchanged behaviour. */
export const DEFAULT_COLS: BomCol[] = [
  { id: "item", label: "Item", builtin: "item" },
  { id: "qty", label: "Qty", builtin: "qty" },
  { id: "designators", label: "Designators", builtin: "designators" },
  { id: "value", label: "Value", builtin: "value" },
  { id: "footprint", label: "Footprint", builtin: "footprint" },
  { id: "mpn", label: "MPN", builtin: "mpn" },
  { id: "dnp", label: "DNP", builtin: "dnp" },
];

export const STATUS_COL: BomCol = { id: "status", label: "Δ", builtin: "status", diffOnly: true };

/** Loose key for case/spacing-insensitive matching: lowercased, non-alphanumerics dropped.
 *  "Manufacturer Part Number", "manufacturer_part_number" and "MPN " all collapse sanely. */
function loose(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]/g, "");
}

/** Map a KiCad preset field name onto a built-in column, or undefined for a custom one.
 *  Covers the virtual fields (`${QUANTITY}`, `${DNP}`, `${ITEM_NUMBER}`) and the natively
 *  carried columns. Matching is case-insensitive and punctuation-insensitive. */
export function builtinFor(name: string): BuiltinKey | undefined {
  switch (loose(name)) {
    case "quantity":
    case "qty":
      return "qty";
    case "dnp":
      return "dnp";
    case "itemnumber":
    case "item":
      return "item";
    case "reference":
    case "references":
    case "refdes":
    case "designator":
    case "designators":
      return "designators";
    case "value":
      return "value";
    case "footprint":
      return "footprint";
    case "mpn":
    case "manufacturerpartnumber":
      return "mpn";
    default:
      return undefined;
  }
}

/** The visible columns of a preset, in preset order. Hostile data (missing fields array,
 *  null entries, non-string names) is skipped rather than thrown on; duplicate ids get a
 *  numeric suffix so React keys and the hidden-column set stay unique. */
export function presetColumns(preset: BomPreset | undefined): BomCol[] {
  const out: BomCol[] = [];
  const seen = new Set<string>();
  const fields = Array.isArray(preset?.fields) ? preset.fields : [];
  for (const f of fields) {
    if (!f || typeof f.name !== "string" || f.show === false) continue;
    const label = typeof f.label === "string" && f.label ? f.label : f.name;
    let id = loose(f.name) || loose(label) || "col";
    if (seen.has(id)) {
      let n = 2;
      while (seen.has(`${id}-${n}`)) n++;
      id = `${id}-${n}`;
    }
    seen.add(id);
    out.push({ id, label, builtin: builtinFor(f.name), field: f.name });
  }
  return out;
}

// ---------------------------------------------------------------- preset grouping
//
// KiCad coalesces two symbols into one BOM line when every field its preset flags
// `group_by` matches. The extractor emits its own (finer) grouping, so the table
// re-groups those lines on the active preset's flagged fields — nothing about the
// grouping is hardcoded here; an empty flag set leaves the lines exactly as extracted.

/** Shown for a field whose members disagree inside one grouped line (mirrors the
 *  extractor's marker in src-tauri/crates/extract/src/bom.rs). */
export const MIXED_VALUES = "-- mixed values --";

/** The value a preset field name reads on a line — built-in columns off the line
 *  itself, everything else out of `line.fields`. */
export function fieldValue(line: BomLine, name: string, label?: string): string {
  switch (builtinFor(name)) {
    case "item":
      return String(line.item);
    case "qty":
      return String(line.qty);
    case "designators":
      return line.designators.join(", ");
    case "value":
      return line.value;
    case "footprint":
      return line.footprint;
    case "mpn":
      return line.mpn;
    case "dnp":
      return line.dnp ? "DNP" : "";
    default:
      return customFieldValue(line, name, label);
  }
}

/** Merge two values of the same field: equal (ignoring case/padding) keeps the first,
 *  otherwise the line reports mixed members. */
function mergeValue(a: string, b: string): string {
  if (a === b) return a;
  if (a.trim().toLowerCase() === b.trim().toLowerCase()) return a;
  return MIXED_VALUES;
}

/** Natural designator order (R2 < R10 < U1) — the extractor sorts within a line, and a
 *  merge has to restore that across the lines it joined. */
const byDesignator = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true });

/** Re-group extracted BOM lines on the preset's `group_by` fields. Lines whose flagged
 *  fields all match fold into one: designators concatenated, quantities summed, and any
 *  other field the members disagree on collapsed to MIXED_VALUES (a field one member
 *  carries and another doesn't counts as a disagreement, as in the extractor). Item
 *  numbers are re-issued in the resulting order. No flagged fields → the input, untouched. */
export function groupLines(lines: BomLine[], keyFields: BomPresetField[]): BomLine[] {
  const flagged = keyFields.filter((f) => f.group_by);
  if (flagged.length === 0) return lines;
  const groups = new Map<string, BomLine>();
  for (const line of lines) {
    const key = flagged
      .map((f) => fieldValue(line, f.name, f.label).trim().toLowerCase())
      .join("");
    const prev = groups.get(key);
    if (!prev) {
      groups.set(key, { ...line, designators: [...line.designators], fields: { ...line.fields } });
      continue;
    }
    const fields: Record<string, string> = {};
    for (const k of new Set([...Object.keys(prev.fields), ...Object.keys(line.fields)])) {
      fields[k] = mergeValue(prev.fields[k] ?? "", line.fields[k] ?? "");
    }
    groups.set(key, {
      ...prev,
      qty: prev.qty + line.qty,
      designators: [...prev.designators, ...line.designators].sort(byDesignator),
      value: mergeValue(prev.value, line.value),
      footprint: mergeValue(prev.footprint, line.footprint),
      mpn: mergeValue(prev.mpn, line.mpn),
      // Only an all-DNP line reads as DNP (the tint and the "DNP only" chip mean
      // "nothing here gets populated").
      dnp: prev.dnp && line.dnp,
      fields,
    });
  }
  return [...groups.values()].map((l, i) => ({ ...l, item: i + 1 }));
}

/** Read a custom (non-built-in) column off a BOM line. The extractor may normalise field
 *  casing or snake_case a label, so we try the verbatim key first, then a loose match
 *  across every key on the line. Missing → "". */
export function customFieldValue(line: BomLine, field: string, label?: string): string {
  const fields = line.fields;
  if (!fields || typeof fields !== "object") return "";
  for (const candidate of [field, label]) {
    if (!candidate) continue;
    const direct = fields[candidate];
    if (typeof direct === "string" && direct !== "") return direct;
  }
  const wanted = new Set([loose(field), label ? loose(label) : ""].filter(Boolean));
  for (const [k, v] of Object.entries(fields)) {
    if (wanted.has(loose(k)) && typeof v === "string") return v;
  }
  return "";
}
