import { useEffect } from "react";
import { useDesignStore } from "../../stores/designStore";
import { useViewStore } from "../../stores/viewStore";
import { displayInfo, numberMap, useReviewStore } from "../../stores/reviewStore";
import { nav, pcbNav, type ChipComment } from "../canvas/navigator";

// Keeps the canvases' object-anchored comment chips in sync with the review state
// and the live design (re-check status is derived from the design, so chips must
// recompute when either changes). Renders nothing — it only drives the imperative
// canvas bridges (docs/phase2-ui-plan.md §3, build order step 4).
export function CommentBridge() {
  const comments = useReviewStore((s) => s.comments);
  const indexes = useDesignStore((s) => s.indexes);
  // Chips follow the active review session (batch3): only the selected session's pins
  // show on the canvases; "All comments" (null) shows every session's pins together.
  const activeSessionId = useReviewStore((s) => s.activeSessionId);
  // Re-push on view change too: a hidden canvas measures getBBox as 0, so chips must
  // re-anchor once its tab becomes visible.
  const view = useViewStore((s) => s.view);

  useEffect(() => {
    const numbers = numberMap(comments);
    // Item 12: resolved/dismissed comments are "done" — no chip on the canvas.
    // Item 15: chips are view-scoped — the schematic only shows its own comments and
    // the PCB only shows its own, even when both anchor the same object.
    const chipsFor = (scope: "schematic" | "pcb"): ChipComment[] =>
      comments
        .filter((c) => c.view === scope)
        .filter((c) => activeSessionId === null || c.session_id === activeSessionId)
        .filter((c) => {
          const st = displayInfo(c, indexes ?? null).status;
          return st !== "dismissed" && st !== "resolved";
        })
        .map((c) => ({
          id: c.id,
          number: numbers.get(c.id) ?? 0,
          anchor: c.anchor,
          status: displayInfo(c, indexes ?? null).status,
          severity: c.severity,
        }));
    nav.setComments(chipsFor("schematic"));
    pcbNav.setComments(chipsFor("pcb"));
  }, [comments, indexes, view, activeSessionId]);

  return null;
}
