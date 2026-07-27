import { useCallback } from "react";
import { useHistoryStore } from "../../stores/historyStore";
import type { ExtractionMeta } from "../../lib/types";
import type { useRevisionActions } from "./useRevisionActions";

/** Keyboard walk for a revision list, shared by the rail panel and the History workspace.
 *
 *  Scoped to the focused list (not a window listener) so it can't collide with the
 *  app-level chords or the Changes panel's ←/→ stepper, and every handled key stops
 *  propagating so App.tsx's global handler doesn't also see it (C would otherwise arm
 *  comment mode while the reader is walking history).
 *
 *  ↑/↓ move · Home/End jump · Enter opens · C compares · Esc unwinds one level. */
export function useRevisionKeys(
  rows: ExtractionMeta[],
  act: ReturnType<typeof useRevisionActions>,
) {
  return useCallback(
    (e: React.KeyboardEvent) => {
      const { selectedId, compareFrom, select, setCompareFrom } = useHistoryStore.getState();

      if (e.key === "Escape") {
        // Dialogs and pick-mode first; only then give the key back to the app (which
        // uses it for full-screen and selection clearing).
        if (act.escape()) {
          e.preventDefault();
          e.stopPropagation();
        }
        return;
      }
      // While a dialog or an inline editor is up, the list must not move under it.
      if (act.busy || rows.length === 0) return;

      const cur = rows.findIndex((r) => r.id === selectedId);
      const move = (to: number) => {
        e.preventDefault();
        e.stopPropagation();
        select(rows[Math.max(0, Math.min(rows.length - 1, to))].id);
      };

      switch (e.key) {
        case "ArrowDown":
          // No selection yet → start at the top rather than jumping to row 1.
          return move(cur < 0 ? 0 : cur + 1);
        case "ArrowUp":
          return move(cur < 0 ? 0 : cur - 1);
        case "Home":
          return move(0);
        case "End":
          return move(rows.length - 1);
        case "Enter":
          if (cur >= 0) {
            e.preventDefault();
            e.stopPropagation();
            act.openVersion(rows[cur].id);
          }
          return;
        case "c":
        case "C": {
          if (e.ctrlKey || e.metaKey || e.altKey || cur < 0) return;
          e.preventDefault();
          e.stopPropagation();
          // First C arms the pick from the cursor; the second C on a different row
          // completes it — so a compare is two keystrokes with no mouse at all.
          if (!compareFrom) setCompareFrom(rows[cur].id);
          else if (compareFrom === rows[cur].id) setCompareFrom(null);
          else act.startCompare(compareFrom, rows[cur].id);
          return;
        }
      }
    },
    [rows, act],
  );
}
