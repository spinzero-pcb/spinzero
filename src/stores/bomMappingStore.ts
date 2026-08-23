import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { MappingView } from "../lib/findings";

// BOM column mapping approval.
//
// Every rule reads *logical* fields ("mpn", "lifecycle", "aecq"); a real BOM has
// whatever columns its author typed. The backend bridges the two with an alias table,
// and that bridge is a guess about someone else's naming. A wrong guess is silent:
// a field read from the wrong column, or from no column, reads downstream as "this
// data is missing" — so the review comes back clean, or full of missing-data findings,
// for a reason that has nothing to do with the board.
//
// This store owns the one moment that guess is stated out loud. A review asks
// `ensureApproved()` first; if the project has never recorded a mapping, the dialog
// opens with the review parked in `pending` and runs it once the user has decided.
// Approving with no edits is a decision too — what gets written is the record that
// the user was asked, which is why the dialog then stops interrupting.

interface BomMappingState {
  open: boolean;
  loading: boolean;
  view: MappingView | null;
  error: string | null;
  saving: boolean;
  /** Edits on top of what the backend resolved: logical field → source column,
   *  "" meaning "this field is not in this BOM". Only edited fields appear. */
  draft: Record<string, string>;
  /** The review that is waiting on this decision, if the dialog opened to gate one. */
  pending: (() => void) | null;

  /** Open the dialog. `pending` runs after the user approves (not after a cancel). */
  openDialog: (profile: string, pending?: () => void) => Promise<void>;
  close: () => void;
  setField: (logical: string, column: string) => void;
  /** Put a field back on the alias guess — whether the divergence came from this
   *  session's edit or from a mapping approved long ago. */
  resetField: (logical: string) => void;
  approve: () => Promise<void>;
  /** Gate for a review: true = go ahead. False means the dialog took over and will
   *  run `pending` itself once the user has decided. */
  ensureApproved: (profile: string, pending: () => void) => Promise<boolean>;
}

/** The column a field resolves to with the current draft applied. */
export function effectiveColumn(view: MappingView, draft: Record<string, string>, logical: string): string {
  const edited = draft[logical];
  if (edited !== undefined) return edited;
  return view.fields.find((f) => f.logical === logical)?.column ?? "";
}

export const useBomMappingStore = create<BomMappingState>((set, get) => ({
  open: false,
  loading: false,
  view: null,
  error: null,
  saving: false,
  draft: {},
  pending: null,

  openDialog: async (profile, pending) => {
    set({ open: true, loading: true, view: null, error: null, draft: {}, pending: pending ?? null });
    try {
      set({ view: await ipc.getBomMapping(profile), loading: false });
    } catch (e) {
      // No extraction yet is the common case; the dialog says so rather than
      // blocking the review behind an error the user cannot act on.
      set({ error: String(e), loading: false });
    }
  },

  close: () => set({ open: false, pending: null, draft: {}, view: null, error: null }),

  setField: (logical, column) => set({ draft: { ...get().draft, [logical]: column } }),

  resetField: (logical) => {
    const auto = get().view?.fields.find((f) => f.logical === logical)?.auto ?? "";
    // Explicitly draft the alias guess rather than dropping the edit: dropping it
    // falls back to the *saved* column, which is the thing being reset away from.
    set({ draft: { ...get().draft, [logical]: auto } });
  },

  approve: async () => {
    const { view, draft, pending, saving } = get();
    if (saving) return;
    set({ saving: true });
    // Send the whole resolved mapping, not just the edits: what the user approved is
    // what they saw. Re-deriving it from aliases on the next run would let an alias
    // table change silently rewrite a mapping someone signed off on.
    const overrides: Record<string, string> = {};
    for (const f of view?.fields ?? []) {
      overrides[f.logical] = draft[f.logical] ?? f.column;
    }
    try {
      await ipc.setBomMapping(overrides);
      set({ open: false, pending: null, draft: {}, view: null, saving: false });
      pending?.();
    } catch (e) {
      // The mapping could not be persisted (read-only project folder, sync lock).
      // Leave the dialog up with the reason rather than running a review on a
      // mapping the project will not remember.
      set({ error: String(e), saving: false });
    }
  },

  ensureApproved: async (profile, pending) => {
    let view: MappingView | null = null;
    try {
      view = await ipc.getBomMapping(profile);
    } catch {
      /* fall through — see below */
    }
    // Can't tell (no project, no extraction, a backend that answered with nothing):
    // never let the gate be the thing that blocks a review.
    if (!view || view.approved) return true;
    // Seed the dialog from the answer we already have rather than asking twice.
    set({ open: true, loading: false, view, error: null, draft: {}, pending });
    return false;
  },
}));
