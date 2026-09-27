import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ipc } from "../lib/ipc";
import {
  changedMapping,
  fieldGroup,
  LOW_FILL,
  parseSetupRequest,
  type FieldGroup,
  type SetupColumn,
  type SetupField,
  type SetupRequest,
} from "../lib/setupRequest";
import { useSettingsStore } from "../stores/settingsStore";

// The review setup window: the whole UI of `SpinZero --setup <dir>`.
//
// An MCP review (started from Claude Code, say) stops before it runs and asks two
// things: what the board is for, and which BOM column holds which field. This window
// asks both, the same way every time, whichever model started the review. Confirm
// saves the answer for the review server and closes the window; the user then goes
// back to their chat and says continue.
//
// A review started from the app does not come here: its setup sheet asks both first.

/** How long the "confirmed" message stays before the window closes itself. */
const CLOSE_AFTER_MS = 2500;

/** The groups, in the order a user should look at them. */
const GROUPS: { id: FieldGroup; title: string }[] = [
  { id: "check", title: "Check these" },
  { id: "read", title: "Read from your BOM" },
  { id: "absent", title: "Not in your BOM" },
];

type Status = "missing" | "low" | "ok" | "absent";

/** The row's colour, from what it reads NOW, so an edit shows its effect at once. */
function statusOf(f: SetupField, column: string, columns: SetupColumn[]): Status {
  if (!column) return fieldGroup(f, columns) === "check" ? "missing" : "absent";
  const fill = columns.find((c) => c.name === column)?.fill_rate ?? 0;
  return fill < LOW_FILL ? "low" : "ok";
}

const STATUS_TITLE: Record<Status, string> = {
  missing: "The review needs this field and no column holds it",
  low: "Few rows fill this column. Check it is the right one",
  ok: "Read from your BOM",
  absent: "Not in this BOM",
};

export function SetupWindow() {
  const [req, setReq] = useState<SetupRequest | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [profile, setProfile] = useState("");
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [saveErr, setSaveErr] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  useEffect(() => {
    // Read only: the accent. This window never writes settings.
    void useSettingsStore.getState().load();
    ipc
      .setupRequest()
      .then((raw) => {
        const r = parseSetupRequest(raw);
        setReq(r);
        setProfile(r.profile.value ?? "");
      })
      .catch((e) => setLoadErr(String(e)));
  }, []);

  async function confirm() {
    if (!req || saving || done) return;
    setSaving(true);
    setSaveErr(null);
    try {
      await ipc.setupSubmit(profile || null, changedMapping(req.fields, draft));
      setDone(true);
      setTimeout(() => void getCurrentWindow().close().catch(() => {}), CLOSE_AFTER_MS);
    } catch (e) {
      // The answer was not saved, so the window stays and says why.
      setSaveErr(String(e));
    } finally {
      setSaving(false);
    }
  }

  // Ctrl+Enter confirms from anywhere in the window.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        void confirm();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <div className="setup-window">
      <div className="wizard-card review-setup setup-window-card" role="dialog" aria-label="Review setup">
        <div className="wizard-head">
          <div>
            <div className="wizard-title">{req ? `Review setup · ${req.board}` : "Review setup"}</div>
            <div className="wizard-step">
              {loadErr ? "Could not read the review" : "Check these, press Confirm, then go back to your chat."}
            </div>
          </div>
        </div>

        <div className="wizard-body">
          {loadErr ? (
            <p className="wizard-hint setup-error">
              {loadErr}. Go back to your chat and answer the questions there instead.
            </p>
          ) : !req ? (
            <p className="wizard-hint">…</p>
          ) : done ? (
            <p className="wizard-hint setup-window-done">Saved. Go back to your chat and tell it to continue.</p>
          ) : (
            <>
              <div className="sw-app" role="radiogroup" aria-label="End application">
                <span className="sw-app-label">End application</span>
                {[...req.profile.options, { id: "", label: "Not stated" }].map((o) => (
                  <button
                    key={o.id || "unstated"}
                    type="button"
                    role="radio"
                    aria-checked={profile === o.id}
                    className={`sw-chip${profile === o.id ? " on" : ""}`}
                    title={o.id ? undefined : "The strictest rules apply"}
                    onClick={() => setProfile(o.id)}
                  >
                    {o.label}
                  </button>
                ))}
              </div>

              <MappingTable req={req} draft={draft} setDraft={setDraft} />

              {saveErr && <p className="wizard-hint setup-error">Not saved: {saveErr}</p>}

              <div className="wizard-actions">
                <button className="btn-primary" disabled={saving} title="Ctrl+Enter" onClick={() => void confirm()}>
                  {saving ? "Saving…" : "Confirm"}
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * The mapping, read the way the report's mapping section reads: the user's column on
 * one side, the field the review reads it as on the other. Grouped so the rows that
 * need a look come first, and coloured by what each row reads now.
 */
function MappingTable({
  req,
  draft,
  setDraft,
}: {
  req: SetupRequest;
  draft: Record<string, string>;
  setDraft: (d: Record<string, string>) => void;
}) {
  const byName = new Map(req.columns.map((c) => [c.name, c]));
  const grouped = GROUPS.map((g) => ({
    ...g,
    rows: req.fields.filter((f) => fieldGroup(f, req.columns) === g.id),
  })).filter((g) => g.rows.length > 0);

  return (
    <table className="sw-map">
      <thead>
        <tr>
          <th className="sw-dotcol" />
          <th>Review field</th>
          <th>Your BOM column</th>
          <th className="sw-num">Rows filled</th>
          <th>Example</th>
        </tr>
      </thead>
      {grouped.map((g) => (
        <tbody key={g.id} className={`sw-group sw-group-${g.id}`}>
          <tr className="sw-grouphead">
            <td colSpan={5}>
              {g.title} <span className="sw-count">{g.rows.length}</span>
            </td>
          </tr>
          {g.rows.map((f) => {
            const col = draft[f.field] ?? f.column;
            const c = byName.get(col);
            const status = statusOf(f, col, req.columns);
            const edited = col !== f.column;
            return (
              <tr key={f.field} className={`sw-row sw-${status}${edited ? " edited" : ""}`}>
                <td className="sw-dotcol">
                  <span className="sw-dot" title={STATUS_TITLE[status]} />
                </td>
                <td>
                  <span className="sw-field">{f.label}</span>
                </td>
                <td>
                  <select
                    className="bom-select sw-pick"
                    value={col}
                    aria-label={`Your BOM column for ${f.label}`}
                    onChange={(e) => {
                      const next = { ...draft };
                      if (e.target.value === f.column) delete next[f.field];
                      else next[f.field] = e.target.value;
                      setDraft(next);
                    }}
                  >
                    <option value="">— none —</option>
                    {req.columns.map((o) => (
                      <option key={o.name} value={o.name}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                </td>
                <td className="sw-num">{c ? `${Math.round(c.fill_rate * 100)}%` : ""}</td>
                <td className="sw-sample" title={c?.sample ?? ""}>
                  {c?.sample ?? ""}
                </td>
              </tr>
            );
          })}
        </tbody>
      ))}
    </table>
  );
}
