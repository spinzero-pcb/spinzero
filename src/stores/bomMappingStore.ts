import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { MappingView } from "../lib/findings";
import { useRunLauncherStore } from "./runLauncherStore";

// BOM column mapping — edited inline in the BOM Review window.
//
// Every rule reads *logical* fields ("mpn", "lifecycle", "aecq"); a real BOM has
// whatever columns its author typed. The backend bridges the two with an alias table,
// and that bridge is a guess about someone else's naming. A wrong guess is silent:
// a field read from the wrong column reads downstream as "this data is missing".
//
// There is no separate mapping dialog. The mapping is a section of the BOM Review
// window, and pressing Run there IS the approval: `save()` persists what the user saw.
// A review started from anywhere else asks `ensureApproved()` first; a project that
// has never had a mapping approved gets the BOM Review window, mapping expanded,
// instead of a run.

interface BomMappingState {
  loading: boolean;
  view: MappingView | null;
  error: string | null;
  saving: boolean;
  /** Edits on top of what the backend resolved: logical field → source column,
   *  "" meaning "this field is not in this BOM". Only edited fields appear. */
  draft: Record<string, string>;
  /** Whether the mapping section of the BOM Review window is expanded. */
  expanded: boolean;

  /** Read the mapping for a profile. Keeps the draft: edits are keyed by field, and
   *  switching the end application must not throw them away. */
  load: (profile: string) => Promise<void>;
  setField: (logical: string, column: string) => void;
  /** Put a field back on the alias guess — whether the divergence came from this
   *  session's edit or from a mapping approved long ago. */
  resetField: (logical: string) => void;
  setExpanded: (expanded: boolean) => void;
  /** Persist the mapping if it has never been approved or has been edited. True when
   *  a review may go ahead; false leaves the reason in `error`. */
  save: () => Promise<boolean>;
  /** Drop the loaded view and edits (the window closed). */
  reset: () => void;
  /** Gate for a review started outside the BOM Review window: true = go ahead. False
   *  means the window was opened, mapping expanded, for the user to confirm. */
  ensureApproved: (profile: string) => Promise<boolean>;
}

/** The column a field resolves to with the current draft applied. */
export function effectiveColumn(view: MappingView, draft: Record<string, string>, logical: string): string {
  const edited = draft[logical];
  if (edited !== undefined) return edited;
  return view.fields.find((f) => f.logical === logical)?.column ?? "";
}

/** Does the draft change anything the backend would read? */
function isDirty(view: MappingView, draft: Record<string, string>): boolean {
  return view.fields.some((f) => effectiveColumn(view, draft, f.logical) !== f.column);
}

export const useBomMappingStore = create<BomMappingState>((set, get) => ({
  loading: false,
  view: null,
  error: null,
  saving: false,
  draft: {},
  expanded: false,

  load: async (profile) => {
    set({ loading: true, error: null });
    try {
      const view = await ipc.getBomMapping(profile);
      // First time through, the user has to see the guess; after that it folds away.
      set({ view, loading: false, expanded: get().expanded || !view.approved });
    } catch (e) {
      // No extraction yet is the common case; the window says so.
      set({ view: null, error: String(e), loading: false });
    }
  },

  setField: (logical, column) => set({ draft: { ...get().draft, [logical]: column } }),

  resetField: (logical) => {
    const auto = get().view?.fields.find((f) => f.logical === logical)?.auto ?? "";
    // Explicitly draft the alias guess rather than dropping the edit: dropping it
    // falls back to the *saved* column, which is the thing being reset away from.
    set({ draft: { ...get().draft, [logical]: auto } });
  },

  setExpanded: (expanded) => set({ expanded }),

  save: async () => {
    const { view, draft, saving } = get();
    if (saving) return false;
    // Nothing loaded (no extraction yet): the gate is never what blocks a review.
    if (!view) return true;
    if (view.approved && !isDirty(view, draft)) return true;
    set({ saving: true, error: null });
    // Send the whole resolved mapping, not just the edits: what the user approved is
    // what they saw. Re-deriving it from aliases on the next run would let an alias
    // table change silently rewrite a mapping someone signed off on.
    const overrides: Record<string, string> = {};
    for (const f of view.fields) overrides[f.logical] = effectiveColumn(view, draft, f.logical);
    try {
      await ipc.setBomMapping(overrides);
      set({
        saving: false,
        draft: {},
        view: {
          ...view,
          approved: true,
          fields: view.fields.map((f) => ({ ...f, column: overrides[f.logical] })),
        },
      });
      return true;
    } catch (e) {
      // Read-only project folder, sync lock: don't run a review on a mapping the
      // project will not remember.
      set({ saving: false, error: `Couldn’t save the column mapping: ${String(e)}` });
      return false;
    }
  },

  reset: () => set({ view: null, draft: {}, error: null, loading: false, expanded: false }),

  ensureApproved: async (profile) => {
    let view: MappingView | null = null;
    try {
      view = await ipc.getBomMapping(profile);
    } catch {
      /* fall through — see below */
    }
    // Can't tell (no project, no extraction): never let the gate block a review.
    if (!view || view.approved) return true;
    set({ expanded: true });
    useRunLauncherStore.getState().openSetup("bom");
    return false;
  },
}));
