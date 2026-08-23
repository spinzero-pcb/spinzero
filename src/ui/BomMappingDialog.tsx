import { effectiveColumn, useBomMappingStore } from "../stores/bomMappingStore";

// BOM column mapping approval dialog.
//
// The point of this screen is not configuration — it is the moment the app admits
// that "MPN" was a guess. Every row states one rule input, the column it reads, and a
// real cell from that column, because a sample settles what a header name often does
// not ("Status" → "Active" is lifecycle; "Status" → "Approved" is not).

/** Rule inputs in the order that matters for reading a BOM, then the rest. The
 *  backend sorts alphabetically, which puts `aecq` above `mpn` — useless as an
 *  opening impression. Anything not listed keeps alphabetical order after these. */
const FIELD_ORDER = [
  "reference",
  "value",
  "footprint",
  "quantity",
  "mpn",
  "manufacturer",
  "description",
  "datasheet",
  "lifecycle",
  "aecq",
  "rohs",
  "reach",
  "msl",
];

/** Human labels for the logical fields. A field with no entry falls back to its own
 *  name, so a new rule input shows up readably without a change here. */
const FIELD_LABEL: Record<string, string> = {
  reference: "Designators",
  value: "Value",
  footprint: "Footprint",
  quantity: "Quantity",
  mpn: "Manufacturer part number",
  mpn_alt: "Alternate MPN",
  manufacturer: "Manufacturer",
  description: "Description",
  datasheet: "Datasheet",
  lifecycle: "Lifecycle status",
  aecq: "AEC-Q qualification",
  rohs: "RoHS",
  reach: "REACH / SVHC",
  msl: "Moisture sensitivity (MSL)",
  dnp: "Do not populate",
  exclude_from_bom: "Exclude from BOM",
  voltage: "Voltage rating",
  tolerance: "Tolerance",
  package: "Package / case",
};

function label(logical: string): string {
  return FIELD_LABEL[logical] ?? logical;
}

function rank(logical: string): number {
  const i = FIELD_ORDER.indexOf(logical);
  return i === -1 ? FIELD_ORDER.length : i;
}

export function BomMappingDialog() {
  const open = useBomMappingStore((s) => s.open);
  const loading = useBomMappingStore((s) => s.loading);
  const view = useBomMappingStore((s) => s.view);
  const error = useBomMappingStore((s) => s.error);
  const saving = useBomMappingStore((s) => s.saving);
  const draft = useBomMappingStore((s) => s.draft);
  const gating = useBomMappingStore((s) => s.pending !== null);
  const close = useBomMappingStore((s) => s.close);
  const setField = useBomMappingStore((s) => s.setField);
  const resetField = useBomMappingStore((s) => s.resetField);
  const approve = useBomMappingStore((s) => s.approve);

  if (!open) return null;

  const fields = [...(view?.fields ?? [])].sort(
    (a, b) => rank(a.logical) - rank(b.logical) || a.logical.localeCompare(b.logical),
  );
  const samples = new Map((view?.columns ?? []).map((c) => [c.name, c]));
  const readCount = fields.filter((f) => effectiveColumn(view!, draft, f.logical)).length;
  // Recomputed against the draft, not taken from the backend as-is: assigning a
  // column here must stop it being listed as unread in the same breath.
  const claimed = new Set(fields.map((f) => effectiveColumn(view!, draft, f.logical)));
  const unread = (view?.unmapped_columns ?? [])
    .map((u) => u.column)
    .filter((c) => !claimed.has(c));

  return (
    <div className="wizard-overlay" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div className="wizard-card bom-mapping" role="dialog" aria-label="BOM column mapping">
        <div className="wizard-head">
          <div>
            <div className="wizard-title">Check the BOM column mapping</div>
            <div className="wizard-step">
              {view
                ? `${readCount} of ${fields.length} rule inputs read · ${view.columns.length} columns · ${view.row_count} rows`
                : "Reading the BOM…"}
            </div>
          </div>
        </div>

        <div className="wizard-body">
          <p className="wizard-hint">
            The checks read named fields; your BOM has whatever columns it has. We matched
            them by name — a wrong match reads as missing data, so the review would blame
            the board for a naming mismatch. Fix anything that looks wrong before it does.
          </p>

          {error ? (
            <p className="wizard-hint review-preflight-error">{error}</p>
          ) : loading ? (
            <p className="wizard-hint">Reading the BOM…</p>
          ) : (
            <>
              <div className="bom-map-table" role="table">
                {fields.map((f) => {
                  const col = effectiveColumn(view!, draft, f.logical);
                  // One rule for "this is not what the aliases picked", so a mapping
                  // approved months ago still reads as a decision rather than as the
                  // current guess — the aliases may well have moved since.
                  const edited = col !== f.auto;
                  const sample = samples.get(col)?.sample ?? "";
                  return (
                    <div
                      className={`bom-map-row${col ? "" : " unread"}${edited ? " edited" : ""}`}
                      key={f.logical}
                      role="row"
                    >
                      <span className="bom-map-field">{label(f.logical)}</span>
                      <select
                        className="bom-select bom-map-pick"
                        value={col}
                        aria-label={`Column for ${label(f.logical)}`}
                        onChange={(e) => setField(f.logical, e.target.value)}
                      >
                        <option value="">— not in this BOM —</option>
                        {(view?.columns ?? []).map((c) => (
                          <option key={c.name} value={c.name}>
                            {c.name}
                            {c.name === f.auto ? " (auto)" : ""}
                          </option>
                        ))}
                      </select>
                      <span className="bom-map-sample" title={sample}>
                        {sample}
                      </span>
                      {edited ? (
                        <button
                          className="btn-ghost bom-map-reset"
                          title={f.auto ? `Back to the matched column: ${f.auto}` : "Back to no column"}
                          onClick={() => resetField(f.logical)}
                        >
                          reset
                        </button>
                      ) : (
                        <span className="bom-map-reset" />
                      )}
                    </div>
                  );
                })}
              </div>

              {/* Columns nothing reads. Not an error — most BOMs carry house codes and
                  stock levels no check cares about — but the one place a user can spot
                  that the column they rely on is going unread. */}
              {unread.length > 0 && (
                <p className="wizard-hint">Not read by any check: {unread.join(", ")}</p>
              )}
            </>
          )}

          <div className="wizard-actions">
            <button className="btn-ghost" onClick={close}>
              Cancel
            </button>
            <button
              className="btn-primary"
              disabled={!view || saving}
              onClick={() => void approve()}
            >
              {saving ? "Saving…" : gating ? "Approve and review" : "Approve mapping"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
