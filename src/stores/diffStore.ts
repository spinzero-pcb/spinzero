import { create } from "zustand";
import { ipc } from "../lib/ipc";
import {
  orderedChanges,
  pcbAnchorToCommentAnchor,
  type DiffDoc,
  type DiffSide,
} from "../lib/diff";
import { parsePcbGeometry, type PcbGeometry } from "../lib/pcbGeometry";
import { useProjectStore } from "./projectStore";
import { useViewStore } from "./viewStore";
import { useReviewStore } from "./reviewStore";
import { usePcbViewStore } from "./pcbViewStore";
import { useToastStore } from "./toastStore";
import { pcbNav, diffPaint } from "../ui/canvas/navigator";
import type { ExtractionMeta } from "../lib/types";

/** The side-by-side rendering mode. `combined` (single-canvas ghost overlay) is
 *  optional per plan §4 and NOT shipped in this change — the type keeps the door open
 *  but the store only ever holds `sideBySide`, and no dead toggle is rendered. */
export type DiffMode = "sideBySide";

/** PCB compare mode (plan §4): combined onion-skin (default — removed red / added
 *  green over the dimmed common base), A/B flicker (~2 Hz, Space holds), and a
 *  draggable wipe divider (A left / B right). PCB side-by-side is deferred. */
export type PcbDiffMode = "onion" | "flicker" | "wipe";

interface DiffState {
  /** True while a comparison is active (diff mode) — view-global. */
  active: boolean;
  /** Older / base side (A) + its cache key for lazy A-side artifact reads. */
  a: DiffSide | null;
  cacheKeyA: string | null;
  /** Newer / target side (B) — the pinned active revision. */
  b: DiffSide | null;
  cacheKeyB: string | null;
  doc: DiffDoc | null;
  mode: DiffMode;
  /** PCB compare mode (onion default); reset on every enter/exit. */
  pcbMode: PcbDiffMode;
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
  next: () => void;
  prev: () => void;
  markSeen: (id: string, seen?: boolean) => void;
  markGroupSeen: (ids: string[], seen?: boolean) => void;
  /** Fetch an A-side sheet SVG (cache-relative path), memoised per session. */
  getSheetSvgA: (num: number, relPath: string) => Promise<string>;
  setPcbMode: (m: PcbDiffMode) => void;
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

export const useDiffStore = create<DiffState>((set, get) => ({
  active: false,
  a: null,
  cacheKeyA: null,
  b: null,
  cacheKeyB: null,
  doc: null,
  mode: "sideBySide",
  pcbMode: "onion",
  focusedChangeId: null,
  seen: new Set(),
  preparing: false,
  priorActive: null,
  priorPcbView: null,
  sheetSvgA: new Map(),
  pcbGeomA: null,

  enterDiff: async (revA, revB, opts) => {
    if (get().preparing) return; // don't overlap a prepare
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
      // comments all anchor there (plan §3). setActiveExtraction is a slow disk write;
      // skip it when B is already active.
      const latestId = extractions[0]?.id ?? null;
      const activeNow = activeExtraction ?? latestId;
      if (activeNow !== newer) {
        await setActiveExtraction(newer);
      }

      const handle = await ipc.prepareDiff(older, newer);
      set({
        active: true,
        a: handle.doc.a,
        b: handle.doc.b,
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
        pcbMode: "onion",
      });
      // The Changes tab auto-activates on enter; the panel appears only in diff mode.
      useReviewStore.getState().setLeftTab("review");
      // Land on the first change so the user has an immediate anchor to step from.
      const first = orderedChanges(handle.doc.changes)[0];
      if (first) get().focusChange(first.id);
    } catch (e) {
      set({ preparing: false });
      // Un-pin: we may have switched the active revision to `newer` before the
      // prepare failed. Restore the previous pin (a failed swap goes back to the
      // still-valid B side; a failed first enter goes back to what the user had).
      const restore = get().active ? (get().b?.rev ?? priorActive) : priorActive;
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
    // Drop ALL comparison state — exiting diff mode is just dropping this store.
    diffPaint.clearA();
    // Restore the PCB layer view the changed-layer isolation replaced.
    if (priorPcbView) {
      const pv = usePcbViewStore.getState();
      pv.setHidden(priorPcbView.hidden);
      pv.setActive(priorPcbView.active);
    }
    set({
      active: false,
      a: null,
      cacheKeyA: null,
      b: null,
      cacheKeyB: null,
      doc: null,
      focusedChangeId: null,
      seen: new Set(),
      priorActive: null,
      priorPcbView: null,
      sheetSvgA: new Map(),
      pcbGeomA: null,
      pcbMode: "onion",
    });
    // Restore the revision that was active before we pinned B (best-effort — a failed
    // restore just leaves B active, which is harmless and surfaces its own toast).
    const { activeExtraction, setActiveExtraction } = useProjectStore.getState();
    if (priorActive !== activeExtraction) {
      void setActiveExtraction(priorActive);
    }
  },

  swap: async () => {
    const { a, b, active } = get();
    if (!active || !a || !b) return;
    // Flip direction verbatim: the new B (old A) becomes the pinned active revision.
    // Normalization must be bypassed — it would re-derive old→new and undo the flip.
    await get().enterDiff(b.rev, a.rev, { normalize: false });
  },

  focusChange: (id) => {
    const { doc } = get();
    const change = doc?.changes.find((c) => c.id === id);
    if (!change) return;
    set({ focusedChangeId: id });

    const sch = change.anchors.schematic;
    const pcb = change.anchors.pcb;
    // Prefer the schematic side when the change has one; else land on the PCB.
    if (sch) {
      useViewStore.getState().setView("schematic");
      // diffPaint.focus drives the B Canvas (nav.revealDiff: load sheet, centre, tint)
      // AND notifies the A-island so both sides paint the same change in lockstep.
      diffPaint.focus(change);
    } else if (pcb) {
      useViewStore.getState().setView("pcb");
      diffPaint.clearA(); // no schematic side to show
      const anchor = pcbAnchorToCommentAnchor(change);
      if (anchor) {
        // Isolate the layer(s) this change lives on: make it active and hide every other
        // layer, so the compare shows just that layer's copper (red/green over grey).
        isolateLayer(pcb.layers);
        pcbNav.reveal(anchor);
      }
    }
  },

  next: () => step(get, set, +1),
  prev: () => step(get, set, -1),

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

  setPcbMode: (m) => set({ pcbMode: m }),

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

/** Advance/retreat the stepper through the ordered change walk, focusing the target. */
function step(
  get: () => DiffState,
  _set: (partial: Partial<DiffState>) => void,
  dir: 1 | -1,
) {
  const { doc, focusedChangeId } = get();
  if (!doc) return;
  const order = orderedChanges(doc.changes);
  if (order.length === 0) return;
  const cur = order.findIndex((c) => c.id === focusedChangeId);
  const nextIdx = cur < 0 ? (dir > 0 ? 0 : order.length - 1) : cur + dir;
  const clamped = Math.max(0, Math.min(order.length - 1, nextIdx));
  const target = order[clamped];
  if (target) get().focusChange(target.id);
}

/** Isolate the layer(s) a PCB change lives on: make the first active and hide every
 *  other known layer, so the compare renders only that layer's copper. No-op when no
 *  layer is known. The pre-diff view is restored on exit (priorPcbView). */
function isolateLayer(layers: string[] | undefined) {
  const layer = layers?.[0];
  if (!layer) return;
  const pv = usePcbViewStore.getState();
  const keep = new Set(layers);
  // Hide all known layers except the change's own — `known` is the full layer table
  // (populated by resetForLayers when the board loads).
  pv.setHidden(pv.known.filter((l) => !keep.has(l)));
  pv.setActive(layer);
}
