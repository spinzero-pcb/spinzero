import { useEffect, useMemo, useRef, useState } from "react";
import { useDiffStore } from "../../stores/diffStore";
import {
  countByImpact,
  filterChanges,
  groupChanges,
  orderedChanges,
  hasPcbAnchor,
  hasSchematicAnchor,
  IMPACT_FILTERS,
  tintRole,
  type Change,
  type ChangeImpact,
  type ChangeGroup,
} from "../../lib/diff";
import { isTypingTarget } from "../../lib/keymap";
import { IconCheck, IconChevron, IconEye, IconEyeOff } from "../icons";

/** The Changes panel (visual-diff §5): the left-rail tab shown only in diff mode. A tree
 *  grouped by change group with count badges, impact + free-text filters, a stepper
 *  header ("Change 7 of 23") with prev/next and ←/→ + J/K keyboard walk, per-change seen
 *  checkmarks, and click-to-focus. */
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

  const [impacts, setImpacts] = useState<Set<ChangeImpact>>(new Set());
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Set<ChangeGroup>>(new Set());

  const allChanges = doc?.changes ?? [];
  // Apply filters, then re-group for the tree and re-flatten for the stepper walk, so
  // "Change N of M" and prev/next count only the *visible* changes.
  const visible = useMemo(
    () => filterChanges(allChanges, impacts, query),
    [allChanges, impacts, query],
  );
  const groups = useMemo(() => groupChanges(visible), [visible]);
  const ordered = useMemo(() => orderedChanges(visible), [visible]);
  // Only offer chips that would show something: a bucket with zero changes is noise
  // (the Cosmetic chip's bucket also covers doc-impact changes — see impactBucket).
  const chips = useMemo(() => {
    if (!doc) return IMPACT_FILTERS;
    const counts = countByImpact(doc);
    return IMPACT_FILTERS.filter(
      (f) => counts[f.id] + (f.id === "cosmetic" ? counts.doc : 0) > 0,
    );
  }, [doc]);
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

  // Keep the focused row scrolled into view as the stepper advances.
  const rowRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  useEffect(() => {
    if (focusedId) rowRefs.current.get(focusedId)?.scrollIntoView({ block: "nearest" });
  }, [focusedId]);

  function toggleImpact(id: ChangeImpact) {
    setImpacts((prev) => {
      const nextSet = new Set(prev);
      if (nextSet.has(id)) nextSet.delete(id);
      else nextSet.add(id);
      return nextSet;
    });
  }
  function toggleCollapse(g: ChangeGroup) {
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
        </span>
        <button className="btn-ghost step-btn" title="Next change (→/J)" onClick={() => stepVisible(+1)}>
          ›
        </button>
      </div>

      {/* Filters */}
      <div className="changes-filters">
        {chips.map((f) => (
          <button
            key={f.id}
            className={`filter-chip ${impacts.has(f.id) ? "active" : ""}`}
            onClick={() => toggleImpact(f.id)}
          >
            {f.label}
          </button>
        ))}
      </div>
      <input
        className="changes-search"
        type="text"
        placeholder="Filter changes…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      {/* Board-overlay visibility: by default every change is tinted; clicking a row
          solos it and the eye buttons build subsets. This bar is the way back. */}
      {hiddenIds.size > 0 && (
        <div className="changes-visbar">
          <span>
            Showing {Math.max(allChanges.length - hiddenIds.size, 0)} of {allChanges.length} on board
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
            const isCollapsed = collapsed.has(grp.group);
            const groupIds = grp.changes.map((c) => c.id);
            const allSeen = groupIds.every((id) => seen.has(id));
            return (
              <div key={grp.group} className="changes-group">
                <div className="changes-group-head">
                  <button className="group-toggle" onClick={() => toggleCollapse(grp.group)}>
                    <span className={`chevron ${isCollapsed ? "collapsed" : ""}`}>
                      <IconChevron size={12} />
                    </span>
                    {grp.label}
                    <span className="group-count">{grp.changes.length}</span>
                  </button>
                  <button
                    className="group-markall"
                    title={allSeen ? "Mark all unseen" : "Mark all in group as seen"}
                    onClick={() => markGroupSeen(groupIds, !allSeen)}
                  >
                    {allSeen ? "Unsee" : "Seen"}
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
                      onFocus={() => focusChange(c.id)}
                      onToggleSeen={() => markSeen(c.id, !seen.has(c.id))}
                      onToggleHidden={() => toggleChangeHidden(c.id)}
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
  onFocus,
  onToggleSeen,
  onToggleHidden,
  rowRef,
}: {
  change: Change;
  focused: boolean;
  seen: boolean;
  /** Hidden from the PCB overlay tint (the eye toggle), NOT filtered from the list. */
  hidden: boolean;
  onFocus: () => void;
  onToggleSeen: () => void;
  onToggleHidden: () => void;
  rowRef: (el: HTMLDivElement | null) => void;
}) {
  const role = tintRole(change.kind); // err/ok/warn → drives the dot colour via CSS var
  const onBoard = hasPcbAnchor(change); // only board-anchored changes can be eye-toggled
  return (
    <div
      ref={rowRef}
      className={`change-row ${focused ? "focused" : ""} ${seen ? "seen" : ""} ${hidden ? "board-hidden" : ""}`}
      onClick={onFocus}
      title={change.detail ? `${change.title}\n${change.detail}` : change.title}
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
      {onBoard && (
        <button
          className={`change-eye ${hidden ? "off" : ""}`}
          title={hidden ? "Show this change on the board" : "Hide this change on the board"}
          onClick={(e) => {
            e.stopPropagation(); // eye builds subsets — it must not refocus/solo
            onToggleHidden();
          }}
        >
          {hidden ? <IconEyeOff size={12} /> : <IconEye size={12} />}
        </button>
      )}
      <button
        className={`change-seen ${seen ? "on" : ""}`}
        title={seen ? "Mark unseen" : "Mark seen"}
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
