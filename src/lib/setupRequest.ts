// The review setup window's question, as `SpinZero --setup <dir>` reads it.
//
// Written by an MCP review server; the contract is `schemas/mcp-setup-1.0.json`. The
// file comes from another program, so it is parsed here field by field: anything
// missing or of the wrong type becomes an empty value, never a crash.

export interface SetupField {
  field: string;
  label: string;
  /** The column the field reads now; "" when it reads none. */
  column: string;
}

export interface SetupColumn {
  /** The heading in the reviewed CSV. A correction names this. */
  name: string;
  /** The heading in the user's own BOM. Shown to the user. */
  label: string;
  fill_rate: number;
  sample: string;
}

export interface SetupRequest {
  review_id: string;
  board: string;
  row_count: number;
  profile: { value: string | null; options: { id: string; label: string }[] };
  fields: SetupField[];
  columns: SetupColumn[];
}

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const obj = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};

export function parseSetupRequest(raw: unknown): SetupRequest {
  const r = obj(raw);
  const p = obj(r.profile);
  return {
    review_id: str(r.review_id),
    board: str(r.board) || "Board",
    row_count: num(r.row_count) ?? 0,
    profile: {
      value: str(p.value) || null,
      options: arr(p.options)
        .map((o) => ({ id: str(obj(o).id), label: str(obj(o).label) || str(obj(o).id) }))
        .filter((o) => o.id),
    },
    fields: arr(r.fields)
      .map((f) => {
        const x = obj(f);
        return { field: str(x.field), label: str(x.label) || str(x.field), column: str(x.column) };
      })
      .filter((f) => f.field),
    columns: arr(r.columns)
      .map((c) => {
        const x = obj(c);
        return { name: str(x.name), label: str(x.label) || str(x.name), fill_rate: num(x.fill_rate) ?? 0, sample: str(x.sample) };
      })
      .filter((c) => c.name),
  };
}

/** Only the fields the user changed. An unchanged field is not sent: the server treats
 *  every field it receives as a correction to its own guess. */
export function changedMapping(fields: SetupField[], draft: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of fields) {
    const v = draft[f.field];
    if (v !== undefined && v !== f.column) out[f.field] = v;
  }
  return out;
}

/** Fields a review cannot do without. Missing one is the first thing to fix. */
const KEY_FIELDS = new Set(["refdes", "mpn", "manufacturer"]);

/** Below this share of rows filled, a mapped column is probably the wrong one. */
export const LOW_FILL = 0.7;

export type FieldGroup = "check" | "read" | "absent";

/**
 * Where a field sits in the window, decided once from the request so a row never
 * jumps while the user edits it.
 *
 *   * `check`: a key field with no column, or a column few rows fill.
 *   * `read`: a column most rows fill.
 *   * `absent`: no column, and the review can do without it.
 */
export function fieldGroup(f: SetupField, columns: SetupColumn[]): FieldGroup {
  if (!f.column) return KEY_FIELDS.has(f.field) ? "check" : "absent";
  const fill = columns.find((c) => c.name === f.column)?.fill_rate ?? 0;
  return fill < LOW_FILL ? "check" : "read";
}
