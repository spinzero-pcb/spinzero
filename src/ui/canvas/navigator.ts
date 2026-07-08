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
}

export const pcbNav: PcbNavigator = { fit: noop, zoomBy: noop, setComments: noop, reveal: noop };

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
