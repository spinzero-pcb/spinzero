import { useEffect, useMemo, useRef, useState } from "react";
import { useDiffStore } from "../../stores/diffStore";
import {
  filterChanges,
  groupChanges,
  orderedChanges,
  hasPcbAnchor,
  hasSchematicAnchor,
  tintRole,
  type Change,
  type ImpactBucket,
} from "../../lib/diff";
import { isTypingTarget } from "../../lib/keymap";
import { IconCheck, IconChevron } from "../icons";

/** The Changes panel (visual-diff §5): the left-rail tab shown only in diff mode. A tree
 *  grouped by impact (Electrical / Placement / Cosmetic — the panel's single
 *  categorization) with count badges, a free-text filter, and a stepper header
 *  ("Change 7 of 23") with prev/next and ←/→ + J/K keyboard walk.
 *
 *  Board-overlay selection: CLICK a row to focus it (solo on the board, camera lands
 *  on it); SHIFT-CLICK to add/remove a change from the board overlay without moving
 *  the camera — the way to light up several changes at once. "Show all" restores the
 *  everything-tinted overview.
 *
 *  Review progress: focusing a change automatically marks it REVIEWED (the ✓ dims the
 *  row); the per-row ✓ and the group button are manual overrides. Progress is the
 *  reviewer's own bookkeeping — it never hides a change or affects the board. */
export function ChangesPanel() {
  const doc = useDiffStore((s) => s.doc);
  const focusedId = useDiffStore((s) => s.focusedChangeId);
  const seen = useDiffStore((s) => s.seen);
  const focusChange = useDiffStore((s) => s.focusChange);
  const markSeen = useDiffStore((s) => s.markSeen);
  const markGroupSeen = useDiffStore((s) => s.markGroupSeen);
  const preparing = useDiffStore((s) => s.preparing);
  const hiddenIds = useDiffStore((s) => s.hiddenChangeIds);
  const toggleChangeHidden = useDiffStore((s) => s.toggleChangeHidden);
  const showAllChanges = useDiffStore((s) => s.showAllChanges);

  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Set<ImpactBucket>>(new Set());

  const allChanges = doc?.changes ?? [];
  // Apply the text filter, then re-group for the tree and re-flatten for the stepper
  // walk, so "Change N of M" and prev/next count only the *visible* changes.
  const visible = useMemo(() => filterChanges(allChanges, query), [allChanges, query]);
  const groups = useMemo(() => groupChanges(visible), [visible]);
  const ordered = useMemo(() => orderedChanges(visible), [visible]);
  const focusIdx = ordered.findIndex((c) => c.id === focusedId);

  // Step within the *visible* (filtered) sequence — the same set the "Change N of M"
  // header counts — so an active filter never steps onto a hidden change. (The store's
  // next/prev walk the unfiltered doc; the panel owns the filter, so it owns the walk.)
  function stepVisible(dir: 1 | -1) {
    if (ordered.length === 0) return;
    const cur = ordered.findIndex((c) => c.id === focusedId);
    const idx =
      cur < 0
        ? dir > 0
          ? 0
          : ordered.length - 1
        : Math.max(0, Math.min(ordered.length - 1, cur + dir));
    focusChange(ordered[idx].id);
  }

  // ←/→ and J/K walk the stepper (guarded against stealing keys while typing in the
  // search box / any input — follows the App.tsx isTypingTarget pattern).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTypingTarget(e)) return;
      if (e.altKey || e.ctrlKey || e.metaKey) return;
      if (e.key === "ArrowRight" || e.key === "j" || e.key === "J") {
        e.preventDefault();
        stepVisible(+1);
      } else if (e.key === "ArrowLeft" || e.key === "k" || e.key === "K") {
        e.preventDefault();
        stepVisible(-1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // On entering diff mode, auto-focus the first change so the canvas opens ON a change
  // (centred + tinted) instead of a blank whole-sheet fit, and keeps it active (batch 2).
  // Guarded on focusedId == null so it fires once per comparison; the user's own
  // stepping/clicking owns focus after that.
  useEffect(() => {
    if (focusedId == null && ordered.length > 0) focusChange(ordered[0].id);
  }, [focusedId, ordered, focusChange]);

  // Keep the focused row scrolled into view as the stepper advances.
  const rowRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  useEffect(() => {
    if (focusedId) rowRefs.current.get(focusedId)?.scrollIntoView({ block: "nearest" });
  }, [focusedId]);

  function toggleCollapse(g: ImpactBucket) {
    setCollapsed((prev) => {
      const nextSet = new Set(prev);
      if (nextSet.has(g)) nextSet.delete(g);
      else nextSet.add(g);
      return nextSet;
    });
  }

  if (!doc) {
    return <div className="changes-panel"><div className="menu-empty">{preparing ? "Preparing comparison…" : "No comparison."}</div></div>;
  }

  if (allChanges.length === 0) {
    return (
      <div className="changes-panel">
        <div className="menu-empty">
          No differences between <b>{doc.a.label}</b> and <b>{doc.b.label}</b>.
        </div>
      </div>
    );
  }

  return (
    <div className="changes-panel">
      {/* Stepper header */}
      <div className="changes-stepper">
        <button className="btn-ghost step-btn" title="Previous change (←/K)" onClick={() => stepVisible(-1)}>
          ‹
        </button>
        <span className="step-count">
          {focusIdx >= 0 ? `Change ${focusIdx + 1} of ${ordered.length}` : `${ordered.length} changes`}
          {seen.size > 0 && (
            <span className="step-reviewed" title="Changes you have focused (or checked off) so far">
              {" "}· {seen.size} reviewed
            </span>
          )}
        </span>
        <button className="btn-ghost step-btn" title="Next change (→/J)" onClick={() => stepVisible(+1)}>
          ›
        </button>
      </div>

      <input
        className="changes-search"
        type="text"
        placeholder="Filter changes…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      {/* Board-overlay visibility: by default every change is tinted; clicking a row
          solos it and shift-clicking rows builds subsets. This bar is the way back. */}
      {hiddenIds.size > 0 && (
        <div className="changes-visbar">
          <span title="Shift-click rows to add or remove changes from the board overlay">
            {Math.max(allChanges.length - hiddenIds.size, 0)} of {allChanges.length} on board
            <span className="visbar-hint"> · shift-click adds</span>
          </span>
          <button className="btn-ghost changes-visbar-btn" onClick={showAllChanges}>
            Show all
          </button>
        </div>
      )}

      {/* Tree */}
      <div className="changes-tree">
        {groups.length === 0 ? (
          <div className="menu-empty">No changes match the filter.</div>
        ) : (
          groups.map((grp) => {
            const isCollapsed = collapsed.has(grp.impact);
            const groupIds = grp.changes.map((c) => c.id);
            const seenCount = groupIds.filter((id) => seen.has(id)).length;
            const allSeen = seenCount === groupIds.length;
            return (
              <div key={grp.impact} className="changes-group">
                <div className="changes-group-head">
                  <button className="group-toggle" onClick={() => toggleCollapse(grp.impact)}>
                    <span className={`chevron ${isCollapsed ? "collapsed" : ""}`}>
                      <IconChevron size={12} />
                    </span>
                    {grp.label}
                    <span
                      className="group-count"
                      title={seenCount > 0 ? `${seenCount} of ${grp.changes.length} reviewed` : undefined}
                    >
                      {seenCount > 0 ? `${seenCount}/${grp.changes.length}` : grp.changes.length}
                    </span>
                  </button>
                  <button
                    className="group-markall"
                    title={
                      allSeen
                        ? "Reset this group's review progress"
                        : "Mark every change in this group reviewed (e.g. to skip a group you don't need to walk)"
                    }
                    onClick={() => markGroupSeen(groupIds, !allSeen)}
                  >
                    {allSeen ? "Reset" : "Mark reviewed"}
                  </button>
                </div>
                {!isCollapsed &&
                  grp.changes.map((c) => (
                    <ChangeRow
                      key={c.id}
                      change={c}
                      focused={c.id === focusedId}
                      seen={seen.has(c.id)}
                      hidden={hiddenIds.has(c.id)}
                      subsetActive={hiddenIds.size > 0}
                      onFocus={() => focusChange(c.id)}
                      onToggleSeen={() => markSeen(c.id, !seen.has(c.id))}
                      onToggleOnBoard={() => toggleChangeHidden(c.id)}
                      rowRef={(el) => {
                        if (el) rowRefs.current.set(c.id, el);
                        else rowRefs.current.delete(c.id);
                      }}
                    />
                  ))}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

function ChangeRow({
  change,
  focused,
  seen,
  hidden,
  subsetActive,
  onFocus,
  onToggleSeen,
  onToggleOnBoard,
  rowRef,
}: {
  change: Change;
  focused: boolean;
  /** Reviewed (auto-marked on focus; the ✓ is a manual override). */
  seen: boolean;
  /** Not currently tinted on the PCB overlay (shift-click toggles), NOT list-filtered. */
  hidden: boolean;
  /** A shift-click subset is composed (some change is hidden) — highlight the members. */
  subsetActive: boolean;
  onFocus: () => void;
  onToggleSeen: () => void;
  onToggleOnBoard: () => void;
  rowRef: (el: HTMLDivElement | null) => void;
}) {
  const role = tintRole(change.kind); // err/ok/warn → drives the dot colour via CSS var
  const onBoard = hasPcbAnchor(change); // only board-anchored changes join the overlay
  const hint = onBoard ? "\n\nClick: focus · Shift-click: add/remove on board" : "";
  return (
    <div
      ref={rowRef}
      className={`change-row ${focused ? "focused" : ""} ${seen ? "seen" : ""} ${hidden ? "board-hidden" : ""} ${subsetActive && !hidden && onBoard ? "board-on" : ""}`}
      onClick={(e) => {
        // Shift-click composes a multi-change board overlay (same gesture as the PCB
        // canvas's shift-click net composing) — no camera move, no refocus.
        if (e.shiftKey && onBoard) onToggleOnBoard();
        else onFocus();
      }}
      // Shift-click must not start a text selection sweep across rows.
      onMouseDown={(e) => {
        if (e.shiftKey) e.preventDefault();
      }}
      title={(change.detail ? `${change.title}\n${change.detail}` : change.title) + hint}
    >
      <span className={`change-dot change-dot-${role}`} />
      <div className="change-body">
        <div className="change-title">{change.title}</div>
        {change.detail && <div className="change-detail">{change.detail}</div>}
      </div>
      <div className="change-badges">
        {hasSchematicAnchor(change) && onBoard && (
          <span className="change-both" title="On both canvases — press X to cross-probe">
            SCH·PCB
          </span>
        )}
      </div>
      <button
        className={`change-seen ${seen ? "on" : ""}`}
        title={seen ? "Reviewed (click to mark as not reviewed)" : "Mark reviewed without visiting"}
        onClick={(e) => {
          e.stopPropagation();
          onToggleSeen();
        }}
      >
        <IconCheck size={12} />
      </button>
    </div>
  );
}
