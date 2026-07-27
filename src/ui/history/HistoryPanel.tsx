import { useEffect, useMemo, useRef, useState } from "react";
import { useHistoryStore } from "../../stores/historyStore";
import { useProjectStore } from "../../stores/projectStore";
import { useViewStore } from "../../stores/viewStore";
import { ContextMenu } from "../ContextMenu";
import { IconCompare, IconHistory, IconLayers, IconSparkle } from "../icons";
import { RevisionRow } from "./RevisionRow";
import { branchTips, filterRevisions, refColumns, rowText, shortId } from "./filter";
import { useRevisionActions } from "./useRevisionActions";
import { useRevisionKeys } from "./useRevisionKeys";

const ROW_H = 32;

/** The left-rail version-control surface — the persistent home the history never had
 *  (it used to be a modal behind a footer chip labelled with a timestamp, which is to
 *  say: undiscoverable).
 *
 *  Deliberately a FLAT list, not a graph. Lanes at rail width would be a graph in name
 *  only, so when history has actually forked this panel says so and hands the reader to
 *  the History view, which has the width to draw it honestly. */
export function HistoryPanel() {
  const extractions = useProjectStore((s) => s.extractions);
  const activeExtraction = useProjectStore((s) => s.activeExtraction);
  const designHead = useProjectStore((s) => s.designHead);

  const showHidden = useHistoryStore((s) => s.showHidden);
  const toggleHidden = useHistoryStore((s) => s.toggleHidden);
  const query = useHistoryStore((s) => s.query);
  const setQuery = useHistoryStore((s) => s.setQuery);
  const selectedId = useHistoryStore((s) => s.selectedId);
  const select = useHistoryStore((s) => s.select);
  const compareFrom = useHistoryStore((s) => s.compareFrom);

  const setView = useViewStore((s) => s.setView);
  const act = useRevisionActions();
  const [ctx, setCtx] = useState<{ x: number; y: number; id: string } | null>(null);

  const latestId = extractions[0]?.id ?? null;
  const activeId = activeExtraction ?? latestId;

  const rows = useMemo(
    () => filterRevisions(extractions.filter((r) => showHidden || !r.hidden), query),
    [extractions, showHidden, query],
  );
  const refCols = refColumns(rows, activeId, designHead);
  const tips = useMemo(() => branchTips(extractions), [extractions]);
  const locals = useMemo(
    () => extractions.filter((e) => e.is_checkpoint && !e.published),
    [extractions],
  );

  const rowRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const onKeyDown = useRevisionKeys(rows, act);

  // Keep the keyboard cursor visible as ↑/↓ walk the list.
  useEffect(() => {
    if (selectedId) rowRefs.current.get(selectedId)?.scrollIntoView({ block: "nearest" });
  }, [selectedId]);

  function clickRow(id: string) {
    // Selection, not navigation: browsing history must not re-point the canvas on every
    // click. Double-click / Enter open. In compare pick-mode a click picks side B.
    if (compareFrom) {
      if (compareFrom === id) useHistoryStore.getState().setCompareFrom(null);
      else act.startCompare(compareFrom, id);
      return;
    }
    select(id);
  }

  return (
    <div className="history-panel">
      <div className="left-tabs history-panel-head">
        <span className="left-tab active as-title">
          <IconHistory size={15} />
          History
        </span>
        <span className="rev-spacer" />
        <button
          className="btn-ghost history-expand"
          title="Open the full history view — the DAG, branch forks and per-revision detail"
          onClick={() => setView("history")}
        >
          <IconLayers size={13} /> Full view
        </button>
      </div>

      <input
        className="changes-search"
        type="text"
        placeholder="Filter revisions…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      {/* Unpublished work, with the action attached instead of an instruction to go
          find it in a right-click menu. */}
      {locals.length > 0 && (
        <div className="rev-nudge rev-nudge-row">
          <span>
            {locals.length} local checkpoint{locals.length === 1 ? "" : "s"}
          </span>
          <span className="rev-spacer" />
          <button
            className="btn-ghost rev-nudge-btn"
            title="Publish the newest local checkpoint to the shared history"
            onClick={() => act.startPublish(locals[0].id)}
          >
            <IconSparkle size={12} /> Publish latest…
          </button>
        </div>
      )}

      {/* A fork is the one thing this width genuinely cannot render — say so and hand
          the reader somewhere that can, rather than drawing a misleading flat list. */}
      {tips.length >= 2 && (
        <div className="rev-nudge rev-nudge-warn rev-nudge-row">
          <span>History has diverged — {tips.length} branch heads.</span>
          <span className="rev-spacer" />
          <button className="btn-ghost rev-nudge-btn" onClick={() => setView("history")}>
            <IconCompare size={12} /> See the fork
          </button>
        </div>
      )}

      {compareFrom && (
        <div className="rev-nudge compare-nudge">
          Pick a revision to compare with{" "}
          <b>
            {(() => {
              const from = extractions.find((e) => e.id === compareFrom);
              return from ? rowText(from) : shortId(compareFrom);
            })()}
          </b>{" "}
          — or press Esc to cancel.
        </div>
      )}

      <div
        className="rev-list"
        tabIndex={0}
        role="listbox"
        aria-label="Revisions"
        onKeyDown={onKeyDown}
      >
        {rows.length === 0 ? (
          <div className="menu-empty">
            {extractions.length === 0 ? "No revisions yet." : "No revisions match the filter."}
          </div>
        ) : (
          rows.map((r) => (
            <RevisionRow
              key={r.id}
              rev={r}
              refCols={refCols}
              dot
              height={ROW_H}
              isViewing={r.id === activeId}
              isOnDisk={r.id === designHead}
              selected={r.id === selectedId}
              compareRole={compareFrom ? (compareFrom === r.id ? "from" : "target") : null}
              editor={act.inlineEditor(r)}
              onClick={() => !act.isEditing(r.id) && clickRow(r.id)}
              onDoubleClick={() => !act.isEditing(r.id) && act.openVersion(r.id)}
              onContextMenu={(e) => {
                e.preventDefault();
                select(r.id);
                setCtx({ x: e.clientX, y: e.clientY, id: r.id });
              }}
              onPublish={() => act.startPublish(r.id)}
              rowRef={(el) => {
                if (el) rowRefs.current.set(r.id, el);
                else rowRefs.current.delete(r.id);
              }}
            />
          ))
        )}
      </div>

      <label className="history-toggle history-panel-foot">
        <input type="checkbox" checked={showHidden} onChange={toggleHidden} /> Show deleted
      </label>

      {ctx &&
        (() => {
          // If the row vanished (a concurrent refresh dropped it), close the menu rather
          // than silently operating on the wrong row.
          const row = extractions.find((e) => e.id === ctx.id);
          if (!row) return null;
          return <ContextMenu x={ctx.x} y={ctx.y} items={act.menuFor(row)} onClose={() => setCtx(null)} />;
        })()}

      {act.dialogs}
    </div>
  );
}
