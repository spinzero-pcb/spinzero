// Imperative bridge: the Canvas owns the camera + live SVG and registers these on
// mount; the properties card (and anything outside the canvas) calls them to drive
// selection and cross-sheet jumps without prop-drilling through React.

import type { Highlight } from "../../stores/selectionStore";
import type { CommentAnchor, CommentSeverity } from "../../lib/types";
import type { DisplayStatus } from "../../stores/reviewStore";

/** Lite view of a comment the canvases render as a numbered chip (Phase 2). */
export interface ChipComment {
  id: string;
  number: number;
  anchor: CommentAnchor;
  status: DisplayStatus;
  severity: CommentSeverity | null;
}

/** Snapshot of the canvas for the D1 reload restore: selection re-resolves by
 *  sheet *name* and net/component refs — never coordinates or DOM. */
export interface CanvasViewState {
  sheetName: string;
  cam: { x: number; y: number; s: number };
  highlights: Highlight[];
}

export interface Navigator {
  /** Select a net: highlight it on the current sheet if present, else jump to it. */
  goNet: (name: string) => void;
  /** Select a component: highlight it here if present, else jump to its home sheet. */
  goComp: (designator: string) => void;
  /** Land on a specific pin of a component (PCB pad → schematic pin, item 5). */
  goPin: (designator: string, pin: string) => void;
  /** Jump to a net's continuation on a specific sheet, landing on its geometry. */
  jumpToNet: (name: string, destSheet: number) => void;
  /** Load a sheet (from the sidebar), keeping the active highlight set re-resolved.
   *  Takes a SheetRef because sidebar numbering ≠ design-JSON numbering. */
  openSheet: (ref: import("../../lib/design").SheetRef) => void;
  /** Load a sheet by its design-JSON number (overview picker). */
  goSheetNum: (num: number) => void;
  /** Add/remove a net or component from the highlight comparison set. */
  toggleHighlight: (kind: "net" | "comp", ref: string) => void;
  /** Replace the highlight set from another view (PCB → schematic continuity) and
   *  land on the primary member's first occurrence. */
  applySelection: (list: Highlight[]) => void;
  /** Fit the current sheet to the viewport. */
  fitView: () => void;
  /** Zoom about the viewport center by a factor (keymap PgUp/PgDn). */
  zoomBy: (factor: number) => void;
  /** Show/hide the all-sheets overview contact-sheet. */
  toggleOverview: () => void;
  /** Capture the state needed to survive a design reload (D1). */
  getViewState: () => CanvasViewState | null;
  /** Phase 2: hand the current comment set to the canvas so it can paint numbered
   *  chips (object-anchored, reusing the badge de-overlap system). */
  setComments: (comments: ChipComment[]) => void;
  /** Item 8: center the camera on a comment's anchored object WITHOUT selecting or
   *  highlighting it — clicking a comment row just goes there. */
  reveal: (anchor: CommentAnchor) => void;
  /** Visual diff: load `sheet` (by design-JSON number) if needed, centre on the given
   *  element uuids, and paint a diff tint (`err`/`ok`/`warn` CSS-var role) with a pulse.
   *  Read-only; selection/history are untouched. `uuids` empty ⇒ just fit the sheet
   *  (added/removed-sheet placeholder). No-op outside diff mode. `aOnly` marks a change
   *  that exists only on the older revision (a removed object): B still loads the sheet
   *  and dims it, but leaves the camera for the A island to land (the object isn't here). */
  revealDiff: (sheet: number, uuids: string[], role: "err" | "ok" | "warn", emph?: string, aOnly?: boolean) => void;
  /** Visual diff: clear any diff tint painted by `revealDiff` (on exit / refocus). */
  clearDiff: () => void;
}

const noop = () => {};

export const nav: Navigator = {
  goNet: noop,
  goComp: noop,
  goPin: noop,
  jumpToNet: noop,
  openSheet: noop,
  goSheetNum: noop,
  toggleHighlight: noop,
  applySelection: noop,
  fitView: noop,
  zoomBy: noop,
  toggleOverview: noop,
  getViewState: () => null,
  setComments: noop,
  reveal: noop,
  revealDiff: noop,
  clearDiff: noop,
};

/** Shared-camera bridge for the schematic side-by-side (visual diff §4): the primary
 *  Canvas (B, right) publishes its live camera + current sheet each frame; the read-only
 *  A-island (left) follows it, and forwards its own pan/zoom deltas back through `drive`
 *  so panning either side moves both. `drive` is registered by Canvas; `cam`/`sheet` are
 *  written by Canvas each tick and read by the A-island. Inert outside diff mode. */
export interface CamBridge {
  /** Live smoothed camera + viewBox published by the B Canvas each frame. */
  cam: { x: number; y: number; s: number };
  vb: [number, number, number, number];
  /** The sheet number currently loaded on the B Canvas (for name-pairing the A side). */
  sheet: number | null;
  /** Apply a pan delta (screen px) + zoom factor to the B Canvas camera. */
  drive: (dx: number, dy: number, zoomFactor: number, anchorX: number, anchorY: number) => void;
  /** Absolutely centre the shared camera on a world-space (mm) bbox, framed with the same
   *  padding/fit rules as a same-sheet focus. Registered by the B Canvas; used by the A
   *  island to land a removed object that exists only on its side (B can't frame it). */
  centerWorld: (box: { x: number; y: number; width: number; height: number }) => void;
  /** Monotonic counter bumped whenever the B sheet changes, so the A island reloads. */
  epoch: number;
}

export const camBridge: CamBridge = {
  cam: { x: 0, y: 0, s: 1 },
  vb: [0, 0, 297, 210],
  sheet: null,
  drive: noop,
  centerWorld: noop,
  epoch: 0,
};

import type { Change } from "../../lib/diff";
import { tintRole } from "../../lib/diff";

/** Diff-focus bridge: `focusChange` in diffStore calls `focus(change)` to drive BOTH
 *  the B Canvas (via nav.revealDiff) and the A-island (which subscribes here) so the
 *  two sides paint the same change in lockstep. Kept out of the store so neither canvas
 *  needs a store import cycle. */
export interface DiffPaint {
  /** The change to render on the A island, or null to clear it. Set by `focus`/`clearA`. */
  focused: Change | null;
  /** Listeners (the A island) re-render when this changes. */
  listeners: Set<() => void>;
  focus: (change: Change) => void;
  clearA: () => void;
  subscribe: (fn: () => void) => () => void;
}

/** Tag every text node inside a cloned diff overlay whose content equals `emph`
 *  (the changed field string, e.g. a value) with `cls`, so CSS can colour exactly
 *  the edited text red (A, old) / green (B, new). Whitespace-insensitive match;
 *  no-op when `emph` is absent or nothing matches (the plain tint still shows). */
export function emphasizeDiffText(overlay: SVGElement, emph: string | undefined, cls: string) {
  const want = emph?.trim();
  if (!want) return;
  for (const t of overlay.querySelectorAll("text")) {
    if (t.textContent?.trim() === want) t.classList.add(cls);
  }
}

export const diffPaint: DiffPaint = {
  focused: null,
  listeners: new Set(),
  focus(change) {
    this.focused = change;
    // Drive the B Canvas (schematic side only; PCB focus goes through pcbNav.reveal).
    // emphB is the NEW text of a field edit — the B canvas colours it green so the
    // exact change (e.g. the value string) stands out inside the tinted symbol.
    const sch = change.anchors.schematic;
    // An A-only change (removed object) has no B-side geometry: let B load + dim the
    // sheet, but the A island lands the camera (the object lives only over there).
    if (sch) nav.revealDiff(sch.sheet, sch.uuids, tintRole(change.kind), change.emphB, change.side === "a");
    for (const fn of this.listeners) fn();
  },
  clearA() {
    this.focused = null;
    nav.clearDiff();
    for (const fn of this.listeners) fn();
  },
  subscribe(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  },
};

/** PCB-view imperative bridge: the PcbView owns its camera + islands and registers
 *  `fit` on mount so the app-level Fit keybinding can reach the board (the PCB
 *  toolbar/right-click call the component-scope `fitBoard` directly). PcbView stays
 *  mounted across tab switches, so this persists for the session. */
export interface PcbNavigator {
  /** Fit the whole board to the viewport. */
  fit: () => void;
  /** Zoom the PCB camera about the viewport centre by a factor (toolbar / PgUp-PgDn). */
  zoomBy: (factor: number) => void;
  /** Phase 2: comment chips on the board (object-anchored). */
  setComments: (comments: ChipComment[]) => void;
  /** Item 8: center on a comment's anchored object without selecting it. */
  reveal: (anchor: CommentAnchor) => void;
  /** Visual diff: land the camera on a focused change's TRUE extent — the union bbox
   *  of the primitives that change owns on both revisions (a rerouted stretch, not the
   *  whole net) — falling back to the anchor bbox/comp/net when the flags aren't ready.
   *  Never un-hides layers: diff focus owns layer isolation (unlike `reveal`, whose
   *  net path shows every layer the net touches). */
  revealChange: (change: Change) => void;
}

export const pcbNav: PcbNavigator = {
  fit: noop,
  zoomBy: noop,
  setComments: noop,
  reveal: noop,
  revealChange: noop,
};

/** BOM-table bridge (visual diff §8): stepping onto a `group: "bom"` change switches
 *  to the BOM view and lands on the change's table row (scroll + flash). The BomTab
 *  is mounted only while its view is up, so a flash requested during the view switch
 *  is held in `pending` and consumed by `register` on mount. */
export const bomNav = {
  pending: null as string | null,
  handler: null as ((key: string) => void) | null,
  /** Scroll + flash the row whose anchor key this is (queued until the tab mounts). */
  flashRow(key: string) {
    if (this.handler) this.handler(key);
    else this.pending = key;
  },
  /** BomTab registers on mount; a queued flash fires immediately. Returns unregister. */
  register(fn: (key: string) => void): () => void {
    this.handler = fn;
    const queued = this.pending;
    this.pending = null;
    if (queued) fn(queued);
    return () => {
      if (this.handler === fn) this.handler = null;
    };
  },
};

/** Measure-tool bridge: PcbGlView owns the ephemeral measurement (in refs), the app
 *  keymap drives Esc / Ctrl+C / Space. `escape` clears an in-progress measurement,
 *  returning true when it consumed the Esc so the caller knows not to also exit the mode. */
export interface MeasureNavigator {
  escape: () => boolean;
  /** Clear the in-progress measurement (right-click "Cancel measurement"). */
  clear: () => void;
  /** Copy the completed measurement's readout to the clipboard (feature 19). No-op with
   *  nothing measured; returns true when something was copied. */
  copy: () => boolean;
  /** Start a fresh measurement anchored at a world point (right-click "Measure from
   *  here" / Space at the hover point). x/y omitted ⇒ use the current hover point. */
  from: (x?: number, y?: number) => void;
}

const measureNoop: MeasureNavigator = {
  escape: () => false,
  clear: () => {},
  copy: () => false,
  from: () => {},
};

export const measureNav: MeasureNavigator = { ...measureNoop };

/** Set just before a design reload; the canvas consumes it on its next boot. */
export const canvasRestore: { state: CanvasViewState | null } = { state: null };
