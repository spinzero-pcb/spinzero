import { create } from "zustand";
import { ipc } from "../lib/ipc";
import {
  pcbLayerUnion,
  type Change,
  type DiffDoc,
} from "../lib/diff";
import { parsePcbGeometry, type PcbGeometry } from "../lib/pcbGeometry";
import { useProjectStore } from "./projectStore";
import { useViewStore } from "./viewStore";
import { useReviewStore } from "./reviewStore";
import { usePcbViewStore } from "./pcbViewStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";
import { pcbNav, diffPaint, bomNav } from "../ui/canvas/navigator";
import type { ExtractionMeta } from "../lib/types";

// Rendering is side-by-side only. A `combined` (single-canvas ghost overlay) mode is
// optional per plan §4 and NOT shipped in this change; when it lands it needs a stored
// mode field + a toggle, but until then the store carries no dead state for it.

/** Pre-UiSettings localStorage key for the blink toggle, read once at the first
 *  enterDiff to migrate. Droppable once no install predates the move. */
const LEGACY_BLINK_KEY = "diff.blink";

interface DiffState {
  /** True while a comparison is active (diff mode) — view-global. */
  active: boolean;
  /** Cache key for lazy A-side (older / base) artifact reads. */
  cacheKeyA: string | null;
  /** Cache key for the B-side (newer / target — the pinned active revision). */
  cacheKeyB: string | null;
  /** The comparison document. The two sides live on `doc.a` / `doc.b` — readers that
   *  want the base/target metadata read those, not a mirrored copy on the store. */
  doc: DiffDoc | null;
  /** Blink the changed copper (added/removed pulse in opposite phases over the stable
   *  grey base). A remembered user preference (ui_settings.json), not per-session. */
  blink: boolean;
  /** Hide zone pours in the PCB compare (pours re-flow around edits and can wash the
   *  view even with the semantic gate). Session-scoped; reset on enter. */
  hideZones: boolean;
  /** Changes hidden from the PCB overlay tint. Empty = show ALL changes (the default
   *  overview). Focusing a change solos it (all others land here); the per-row eye
   *  toggles individual members. Drives a GPU mask, so toggling is cheap. */
  hiddenChangeIds: Set<string>;
  /** The change the stepper is currently on (null = none focused yet). */
  focusedChangeId: string | null;
  /** Reviewer's own progress — ephemeral per app session (plan §11). */
  seen: Set<string>;
  /** True while `prepare_diff` is running ("Preparing comparison…"). */
  preparing: boolean;
  /** The active revision id to restore on exit (what B replaced). */
  priorActive: string | null;
  /** PCB layer visibility/active to restore on exit — diff mode isolates the changed
   *  layer (hides all others), so we snapshot the user's view and put it back. */
  priorPcbView: { hidden: string[]; active: string | null } | null;

  /** A-side sheet SVG text cache (by sheet number), fetched via read_artifact_from
   *  with cacheKeyA. Lives HERE, not in designStore, so exit = drop it (plan §7). */
  sheetSvgA: Map<number, Promise<string>>;
  /** Memoised A-side PCB geometry fetch (one per diff session; dropped on exit). */
  pcbGeomA: Promise<PcbGeometry | null> | null;

  /** `normalize: false` keeps the given direction verbatim (used by swap — the
   *  default ancestry normalization would immediately flip it back). */
  enterDiff: (revA: string, revB: string, opts?: { normalize?: boolean }) => Promise<void>;
  exitDiff: () => void;
  swap: () => Promise<void>;
  focusChange: (id: string) => void;
  /** Land a change on the PCB canvas: switch to the PCB view, isolate the change's
   *  layer(s), and frame its own extent. The diff-owned landing (revealChange never
   *  un-hides layers, unlike pcbNav.reveal's net path). Shared by focusChange's PCB
   *  branch and the diff cross-probe (X). Does NOT change focus/solo state. */
  revealChangeOnPcb: (change: Change) => void;
  markSeen: (id: string, seen?: boolean) => void;
  markGroupSeen: (ids: string[], seen?: boolean) => void;
  /** Fetch an A-side sheet SVG (cache-relative path), memoised per session. */
  getSheetSvgA: (num: number, relPath: string) => Promise<string>;
  setBlink: (on: boolean) => void;
  setHideZones: (on: boolean) => void;
  /** Toggle one change in/out of the PCB overlay (the row's eye button). */
  toggleChangeHidden: (id: string) => void;
  /** Back to the overview: every change tinted, relevant-layer union restored. */
  showAllChanges: () => void;
  /** Fetch + parse the A-side PCB geometry IR (same cache-relative path as B's —
   *  deterministic extraction), memoised per diff session. Null when A has no board. */
  getPcbGeometryA: (relPath: string | undefined) => Promise<PcbGeometry | null>;
}

/** Normalize (revA, revB) to old → new: prefer DAG ancestry (is A an ancestor of B?),
 *  else fall back to created_at timestamp (older first). Returns [older, newer]. */
export function normalizeOrder(
  a: string,
  b: string,
  extractions: ExtractionMeta[],
): [string, string] {
  const byId = new Map(extractions.map((e) => [e.id, e]));
  const ea = byId.get(a);
  const eb = byId.get(b);
  if (!ea || !eb) return [a, b]; // unknown ids — leave as given

  // Ancestry: walk parents up from each node; if B reaches A, A is older.
  const ancestors = (start: ExtractionMeta): Set<string> => {
    const seen = new Set<string>();
    const stack = [...start.parents];
    while (stack.length) {
      const id = stack.pop()!;
      if (seen.has(id)) continue;
      seen.add(id);
      const node = byId.get(id);
      if (node) stack.push(...node.parents);
    }
    return seen;
  };
  if (ancestors(eb).has(a)) return [a, b]; // A is an ancestor of B ⇒ A older
  if (ancestors(ea).has(b)) return [b, a]; // B is an ancestor of A ⇒ B older

  // No ancestry link (cross-branch): fall back to timestamp, older first.
  const older = ea.created_at <= eb.created_at ? a : b;
  const newer = older === a ? b : a;
  return [older, newer];
}

/** Monotonic session token: enterDiff captures it and re-checks after every await, so a
 *  prepare superseded mid-flight (exitDiff during a swap's prepare, or a newer enter)
 *  never lands stale state. exitDiff bumps it to invalidate any in-flight prepare. */
let diffSeq = 0;

export const useDiffStore = create<DiffState>((set, get) => ({
  active: false,
  cacheKeyA: null,
  cacheKeyB: null,
  doc: null,
  blink: false, // replaced by hydrateBlink() on the first enterDiff
  hideZones: false,
  hiddenChangeIds: new Set(),
  focusedChangeId: null,
  seen: new Set(),
  preparing: false,
  priorActive: null,
  priorPcbView: null,
  sheetSvgA: new Map(),
  pcbGeomA: null,

  enterDiff: async (revA, revB, opts) => {
    if (get().preparing) return; // don't overlap a prepare
    hydrateBlink(); // the toggle only exists in diff mode, so this is its first read
    const token = ++diffSeq;
    const { extractions, activeExtraction, setActiveExtraction } = useProjectStore.getState();
    const [older, newer] =
      opts?.normalize === false ? [revA, revB] : normalizeOrder(revA, revB, extractions);
    // Re-entering while already comparing (swap) must keep the ORIGINAL pre-diff
    // revision as the thing exit restores — the current active is the pinned B.
    const priorActive = get().active ? get().priorActive : activeExtraction;
    // Snapshot the user's PCB layer view once (a swap keeps the original) so exit can
    // undo the changed-layer isolation focusChange applies.
    const pv = usePcbViewStore.getState();
    const priorPcbView = get().active
      ? get().priorPcbView
      : { hidden: [...pv.hidden], active: pv.active };

    set({ preparing: true });
    try {
      // Pin the active revision to B (the newer side) so selection / cross-probe /
      // comments all anchor there (plan §3). A pure viewer switch (never writes the
      // design folder) — but may re-extract, so skip it when B is already active.
      const latestId = extractions[0]?.id ?? null;
      const activeNow = activeExtraction ?? latestId;
      if (activeNow !== newer) {
        await setActiveExtraction(newer);
        // setActiveExtraction swallows failures (it toasts) and silently no-ops while
        // `busy` — verify the pin actually landed, or the compare would render a B
        // canvas showing a different revision than the doc claims.
        const nowActive = useProjectStore.getState().activeExtraction ?? latestId;
        if (nowActive !== newer) {
          throw new Error("couldn’t switch the viewer to the newer revision — try again in a moment");
        }
      }
      if (token !== diffSeq) return; // superseded while pinning — drop this prepare

      const handle = await ipc.prepareDiff(older, newer);
      if (token !== diffSeq) return; // superseded (exit or newer enter) — drop stale state
      set({
        active: true,
        cacheKeyA: handle.cache_key_a,
        cacheKeyB: handle.cache_key_b,
        doc: handle.doc,
        focusedChangeId: null,
        seen: new Set(),
        preparing: false,
        priorActive,
        priorPcbView,
        sheetSvgA: new Map(),
        pcbGeomA: null,
        hideZones: false,
        hiddenChangeIds: new Set(),
      });
      // The Changes tab auto-activates on enter; the panel appears only in diff mode.
      useReviewStore.getState().setLeftTab("changes");
      // Overview by default: EVERY change tinted, the PCB view isolated to the union
      // of layers the changes land on. No change is focused (and no camera yank) until
      // the user steps/clicks — then that change solos (see focusChange).
      applyLayerUnion(handle.doc.changes);
    } catch (e) {
      if (token !== diffSeq) return; // superseded — whoever superseded us owns the state
      set({ preparing: false });
      // Un-pin: we may have switched the active revision to `newer` before the
      // prepare failed. Restore the previous pin (a failed swap goes back to the
      // still-valid B side; a failed first enter goes back to what the user had).
      const restore = get().active ? (get().doc?.b.rev ?? priorActive) : priorActive;
      const proj = useProjectStore.getState();
      if (restore !== proj.activeExtraction) {
        void proj.setActiveExtraction(restore);
      }
      useToastStore.getState().push({
        kind: "error",
        title: "Couldn’t prepare comparison",
        message: String(e),
      });
    }
  },

  exitDiff: () => {
    const { active, priorActive, priorPcbView } = get();
    if (!active) return;
    diffSeq++; // invalidate any in-flight prepare (a swap's) so it can't land stale state
    // Drop ALL comparison state — exiting diff mode is just dropping this store.
    diffPaint.clearA();
    // The Changes tab disappears with diff mode; fall the rail back to Review so a later
    // re-mount (or the activity bar) doesn't reflect a tab that no longer exists.
    useReviewStore.getState().setLeftTab("review");
    // Restore the PCB layer view the changed-layer isolation replaced.
    if (priorPcbView) {
      const pv = usePcbViewStore.getState();
      pv.setHidden(priorPcbView.hidden);
      pv.setActive(priorPcbView.active);
    }
    set({
      active: false,
      cacheKeyA: null,
      cacheKeyB: null,
      doc: null,
      focusedChangeId: null,
      seen: new Set(),
      preparing: false, // an in-flight swap prepare was invalidated above — unblock enters
      priorActive: null,
      priorPcbView: null,
      sheetSvgA: new Map(),
      pcbGeomA: null,
      hideZones: false,
      hiddenChangeIds: new Set(),
    });
    // Restore the revision that was active before we pinned B (best-effort — a failed
    // restore just leaves B active, which is harmless and surfaces its own toast).
    const { activeExtraction, setActiveExtraction } = useProjectStore.getState();
    if (priorActive !== activeExtraction) {
      void setActiveExtraction(priorActive);
    }
  },

  swap: async () => {
    const { doc, active } = get();
    if (!active || !doc) return;
    // Flip direction verbatim: the new B (old A) becomes the pinned active revision.
    // Normalization must be bypassed — it would re-derive old→new and undo the flip.
    await get().enterDiff(doc.b.rev, doc.a.rev, { normalize: false });
  },

  focusChange: (id) => {
    const { doc } = get();
    const change = doc?.changes.find((c) => c.id === id);
    if (!change || !doc) return;
    // Selecting a change SOLOS it on the PCB overlay: every other change drops out of
    // the tint (a cheap GPU-mask update, not a rebuild). showAllChanges restores the
    // overview; shift-clicking rows builds multi-change subsets from either state.
    // Focusing does NOT mark the change reviewed — the reviewer may just be glancing
    // through; the row ✓ (and the group "Mark reviewed" button) are the only way to
    // record progress, so a tick always reflects a deliberate decision.
    const others = new Set(doc.changes.filter((c) => c.id !== id).map((c) => c.id));
    set({
      focusedChangeId: id,
      hiddenChangeIds: others,
    });

    const sch = change.anchors.schematic;
    const pcb = change.anchors.pcb;
    const bom = change.anchors.bom;
    // A BOM change lands on the BOM table (row scroll + flash, §8); its designators
    // link back to the underlying component changes from the table/panel.
    if (change.group === "bom" && bom) {
      useViewStore.getState().setView("bom");
      diffPaint.clearA(); // nothing to paint on the schematic A island
      bomNav.flashRow(bom.key);
      return;
    }
    // Prefer the schematic side when the change has one; else land on the PCB.
    if (sch) {
      useViewStore.getState().setView("schematic");
      // diffPaint.focus drives the B Canvas (nav.revealDiff: load sheet, centre, tint)
      // AND notifies the A-island so both sides paint the same change in lockstep.
      diffPaint.focus(change);
    } else if (pcb) {
      get().revealChangeOnPcb(change);
    }
  },

  revealChangeOnPcb: (change) => {
    useViewStore.getState().setView("pcb");
    diffPaint.clearA(); // PCB landing — the schematic A-island isn't shown
    // Isolate the layer(s) this change lives on so the compare shows just that copper
    // over grey — the one-visible-change case of the shift-click rule, so it uses the
    // same applyLayerUnion (Edge.Cuts rides along to frame the copper; single copper
    // layer becomes active). revealChange (unlike pcbNav.reveal's net path) never
    // un-hides layers, so the isolation sticks and it frames the change's OWN extent.
    applyLayerUnion([change]);
    pcbNav.revealChange(change);
  },

  markSeen: (id, seen = true) =>
    set((s) => {
      const next = new Set(s.seen);
      if (seen) next.add(id);
      else next.delete(id);
      return { seen: next };
    }),

  markGroupSeen: (ids, seen = true) =>
    set((s) => {
      const next = new Set(s.seen);
      for (const id of ids) {
        if (seen) next.add(id);
        else next.delete(id);
      }
      return { seen: next };
    }),

  setBlink: (on) => {
    set({ blink: on });
    void useSettingsStore.getState().setDiffBlink(on);
  },

  setHideZones: (on) => set({ hideZones: on }),

  toggleChangeHidden: (id) => {
    set((s) => {
      const next = new Set(s.hiddenChangeIds);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return { hiddenChangeIds: next };
    });
    // The layer view follows the VISIBLE subset: shift-clicking in a change that lives
    // on another layer must also reveal that layer, or the GPU mask shows it into a
    // hidden layer and nothing appears. (Union of the visible changes' layers; the
    // focused-change isolation is just the one-visible-change case of this rule.)
    const { doc, hiddenChangeIds } = get();
    if (doc) applyLayerUnion(doc.changes.filter((c) => !hiddenChangeIds.has(c.id)));
  },

  showAllChanges: () => {
    const { doc } = get();
    set({ hiddenChangeIds: new Set() });
    // Back to the overview layer set (the focused change may have isolated one layer).
    if (doc) applyLayerUnion(doc.changes);
  },

  getPcbGeometryA: (relPath) => {
    const { pcbGeomA, cacheKeyA } = get();
    if (pcbGeomA) return pcbGeomA;
    if (!cacheKeyA || !relPath) return Promise.resolve(null);
    const p = ipc
      .readArtifactFrom(cacheKeyA, relPath)
      .then((txt) => parsePcbGeometry(txt))
      .catch((): PcbGeometry | null => {
        // A schematic-only A side (or a read hiccup): compare modes stay unavailable,
        // the PCB view falls back to the plain B render. Don't cache the failure.
        set({ pcbGeomA: null });
        return null;
      });
    set({ pcbGeomA: p });
    return p;
  },

  getSheetSvgA: (num, relPath) => {
    const { sheetSvgA, cacheKeyA } = get();
    const cached = sheetSvgA.get(num);
    if (cached) return cached;
    if (!cacheKeyA) return Promise.reject(new Error("no A-side cache key"));
    const p = ipc.readArtifactFrom(cacheKeyA, relPath).catch((e) => {
      sheetSvgA.delete(num); // don't cache a failure
      throw e;
    });
    sheetSvgA.set(num, p);
    return p;
  },
}));

let blinkHydrated = false;

/** Adopt the saved blink preference the first time a comparison is entered. Settings
 *  are already loaded by then (a project is open), so this stays synchronous. An
 *  install that predates the settings field migrates its localStorage value once. */
function hydrateBlink() {
  if (blinkHydrated) return;
  blinkHydrated = true;
  const settings = useSettingsStore.getState();
  if (settings.diffBlink !== null) {
    useDiffStore.setState({ blink: settings.diffBlink });
    return;
  }
  let legacy = false;
  try {
    legacy = localStorage.getItem(LEGACY_BLINK_KEY) === "1";
  } catch {
    /* blocked storage — the default (off) stands */
  }
  if (legacy) {
    useDiffStore.setState({ blink: true });
    void settings.setDiffBlink(true).then(() => localStorage.removeItem(LEGACY_BLINK_KEY));
  }
}

/** Show the union of layers the given changes land on ("relevant layers"), hiding the
 *  rest — the whole changeset on enter / show-all, the visible subset after shift-click
 *  composing. Leaves the user's layer view alone when no change names a layer
 *  (schematic-only diffs / an emptied subset). A single-layer union gets the
 *  active-layer emphasis; a multi-layer union has no active layer (the diff-mode
 *  copper-stack fade differentiates depth instead). Restored on exit (priorPcbView). */
function applyLayerUnion(changes: Change[]) {
  const pv = usePcbViewStore.getState();
  const union = pcbLayerUnion(changes, pv.known);
  if (union.length === 0) return;
  const keep = new Set(union);
  pv.setHidden(pv.known.filter((l) => !keep.has(l)));
  const copper = union.filter((l) => l.endsWith(".Cu"));
  pv.setActive(copper.length === 1 ? copper[0] : null);
}
