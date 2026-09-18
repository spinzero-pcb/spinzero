import { useEffect, useRef, useState } from "react";
import type { ColumnMapping, FindingsDoc } from "../../lib/findings";

// How the review read the user's own BOM columns, folded away.
//
// **Which mapping this is.** `BomMappingDialog` shows what the app's own rules WILL
// read next time. This shows what the finished review DID read. They are usually the
// same and the day they are not is the day somebody needs this panel — so it is the
// review's own record, decoded from `findings.json`, and never recomputed here.
//
// **Why it is folded.** It answers a question most readers never ask, and is the first
// thing to open for the one who reads "no manufacturer on 88 rows" and knows perfectly
// well that column is filled in.
//
// **Their column on the left, ours on the right.** The whole point is to let someone
// recognise a column of their own that was read as the wrong thing, and reading it in
// the direction the data flowed removes the question of which of two similar-looking
// words is theirs.
//
// One table, three groups: what the review read, what it ignored, and what it looked
// for and did not find. Three answers to one question, so one table.

export function ReviewMapping({ doc }: { doc: FindingsDoc | null }) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  // Same dismiss idiom as the other strip popovers: click away or Escape.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  const mapping = doc?.column_mapping;
  // The free tier emits none, and a chip that opens an empty panel is worse than no
  // chip: it promises an answer the document does not carry.
  if (!mapping || (!mapping.fields?.length && !mapping.unmapped_columns?.length)) return null;

  const { read, absent, ignored } = groups(mapping);

  return (
    <div className="review-mapping" ref={wrapRef}>
      <button
        className={`bom-check-meta ${open ? "on" : ""}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        title="Which of your BOM columns this review read, and which it did not"
        onClick={() => setOpen((v) => !v)}
      >
        Columns read
      </button>

      {open && (
        <div className="review-mapping-pop" role="dialog" aria-label="Columns this review read">
          <div className="review-outcome-hd">How your columns were read</div>
          <table className="review-mapping-table">
            <tbody>
              {read.map((r) => (
                <tr key={`${r.column}-${r.field}`}>
                  <td className="review-mapping-col">{r.column}</td>
                  <td className="review-mapping-arrow">→</td>
                  <td className="review-mapping-field">{r.field}</td>
                  {/* The document's own number, never recounted here. A manufacturer
                      column 8% filled is almost always the wrong column. */}
                  <td className={`review-mapping-fill ${r.rate !== null && r.rate < 0.5 ? "low" : ""}`}>
                    {r.rate === null ? "" : `${Math.round(r.rate * 100)}%`}
                  </td>
                </tr>
              ))}
              {ignored.length > 0 && (
                <>
                  <tr className="review-mapping-head">
                    <td colSpan={4}>Columns the review did not use</td>
                  </tr>
                  {ignored.map((c) => (
                    <tr key={c.column}>
                      <td className="review-mapping-col">{c.column}</td>
                      <td colSpan={2} />
                      <td className="review-mapping-fill">
                        {c.rate === null ? "" : `${Math.round(c.rate * 100)}%`}
                      </td>
                    </tr>
                  ))}
                </>
              )}
            </tbody>
          </table>
          {absent.length > 0 && (
            <p className="review-mapping-absent">
              Looked for and not found in this BOM: {absent.join(", ")}. Any check that needed
              one of those could not run.
            </p>
          )}
        </div>
      )}
    </div>
  );
}

/** The mapping in the three groups the panel shows. */
function groups(mapping: ColumnMapping): {
  read: { field: string; column: string; rate: number | null }[];
  absent: string[];
  ignored: { column: string; rate: number | null }[];
} {
  const fields = mapping.fields ?? [];
  const has = (c: string | null | undefined): c is string => typeof c === "string" && c.trim().length > 0;
  return {
    read: fields
      .filter((f) => has(f.column))
      .map((f) => ({
        field: f.field,
        column: (f.column as string).trim(),
        rate: typeof f.fill_rate === "number" ? f.fill_rate : null,
      }))
      .sort((a, b) => a.column.localeCompare(b.column)),
    absent: fields.filter((f) => !has(f.column)).map((f) => f.field),
    ignored: (mapping.unmapped_columns ?? []).map((c) => ({
      column: c.column,
      rate: typeof c.fill_rate === "number" ? c.fill_rate : null,
    })),
  };
}
