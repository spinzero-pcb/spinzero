import { useEffect, useMemo, useRef, useState } from "react";
import { useHistoryStore } from "../../stores/historyStore";
import { useProjectStore } from "../../stores/projectStore";
import { ContextMenu } from "../ContextMenu";
import { IconCompare, IconEye, IconFolder, IconSparkle, IconTag } from "../icons";
import { formatLocalTime, formatRelative } from "../../lib/time";
import { layoutDag } from "./layout";
import { RevisionRow } from "./RevisionRow";
import { branchTips, filterRevisions, initials, parentOf, refColumns, rowText, shortId } from "./filter";
import { useRevisionActions } from "./useRevisionActions";
import { useRevisionKeys } from "./useRevisionKeys";
import type { ExtractionMeta } from "../../lib/types";

// Graph geometry (px). Colours come from CSS vars; only layout dimensions live here.
const LANE_W = 22;
const ROW_H = 34;
const DOT_R = 4.5;
const laneX = (lane: number) => lane * LANE_W + LANE_W / 2;
const rowY = (row: number) => row * ROW_H + ROW_H / 2;

/** The full-width version-control workspace — a peer of Schematic and PCB rather than
 *  the old modal overlay. This is where the DAG gets room to be a DAG: lanes, forks and
 *  merge edges are legible, and the selected revision opens a detail pane instead of
 *  cramming its changelog, author, tags and actions onto a 34px row.
 *
 *  Selection here is inspection. Only "Open" (double-click, Enter, or the detail-pane
 *  button) re-points the canvas, and doing so returns the reader to the design view
 *  they came from. */
export function HistoryView() {
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
  const setCompareFrom = useHistoryStore((s) => s.setCompareFrom);

  const act = useRevisionActions();
  const [ctx, setCtx] = useState<{ x: number; y: number; id: string } | null>(null);

  const latestId = extractions[0]?.id ?? null;
  const activeId = activeExtraction ?? latestId;

  // The DAG is laid out over the show-hidden set (lanes and edges must stay structurally
  // honest); the text filter then narrows which rows are *listed*, so filtering never
  // silently rewires the graph. When a filter is active the lane gutter is dropped —
  // half a graph is worse than none.
  const visible = useMemo(
    () => extractions.filter((r) => showHidden || !r.hidden),
    [extractions, showHidden],
  );
  const filtered = useMemo(() => filterRevisions(visible, query), [visible, query]);
  const filtering = filtered.length !== visible.length;
  const layout = useMemo(() => layoutDag(extractions, showHidden), [extractions, showHidden]);

  const rows = filtered;
  const refCols = refColumns(rows, activeId, designHead);
  const tips = useMemo(() => branchTips(extractions), [extractions]);
  const selected = extractions.find((e) => e.id === selectedId) ?? null;

  const rowRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const onKeyDown = useRevisionKeys(rows, act);

  // Land on something the moment the workspace opens, so the detail pane is never an
  // empty box and the keyboard walk has a cursor to start from.
  useEffect(() => {
    if (!selectedId && activeId) select(activeId);
  }, [selectedId, activeId, select]);

  useEffect(() => {
    if (selectedId) rowRefs.current.get(selectedId)?.scrollIntoView({ block: "nearest" });
  }, [selectedId]);

  function clickRow(id: string) {
    if (compareFrom) {
      if (compareFrom === id) setCompareFrom(null);
      else act.startCompare(compareFrom, id);
      return;
    }
    select(id);
  }

  const gutterW = filtering ? 0 : layout.laneCount * LANE_W;
  const height = Math.max(rows.length, 1) * ROW_H;

  return (
    <div className="history-view">
      <div className="history-view-main">
        <div className="history-view-head">
          <input
            className="changes-search history-view-search"
            type="text"
            placeholder="Filter by message, author, tag or id…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <label className="history-toggle">
            <input type="checkbox" checked={showHidden} onChange={toggleHidden} /> Show deleted
          </label>
          <span className="rev-spacer" />
          {tips.length >= 2 && (
            <button
              className="btn-ghost"
              title={`Compare the two newest branch heads (${shortId(tips[0].id)} vs ${shortId(tips[1].id)})`}
              onClick={() => act.startCompare(tips[1].id, tips[0].id)}
            >
              <IconCompare size={13} /> Compare branches
            </button>
          )}
        </div>

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

        <div className="history-view-body">
          {rows.length === 0 ? (
            <div className="menu-empty">
              {extractions.length === 0 ? "No revisions yet." : "No revisions match the filter."}
            </div>
          ) : (
            <div
              className="dag rev-list"
              style={{ height }}
              tabIndex={0}
              role="listbox"
              aria-label="Revisions"
              onKeyDown={onKeyDown}
            >
              {!filtering && (
                <svg
                  className="dag-svg"
                  width={gutterW}
                  height={height}
                  aria-hidden="true"
                  style={{ position: "absolute", left: 0, top: 0 }}
                >
                  {layout.edges.map((e, i) => {
                    const x1 = laneX(e.fromLane);
                    const y1 = rowY(e.fromRow);
                    const x2 = laneX(e.toLane);
                    const y2 = e.kind === "pending" ? y1 + ROW_H * 0.6 : rowY(e.toRow);
                    const mid = (y1 + y2) / 2;
                    const d =
                      x1 === x2
                        ? `M${x1},${y1} L${x2},${y2}`
                        : `M${x1},${y1} C${x1},${mid} ${x2},${mid} ${x2},${y2}`;
                    return <path key={i} d={d} className={`dag-edge dag-edge-${e.kind}`} fill="none" />;
                  })}
                  {layout.nodes.map((n) => {
                    const cls = [
                      "dag-dot",
                      n.meta.id === activeId ? "is-active" : "",
                      n.meta.is_checkpoint && !n.meta.published ? "is-local" : "is-published",
                      n.meta.id === selectedId ? "is-selected" : "",
                    ].join(" ");
                    return (
                      <circle key={n.meta.id} cx={laneX(n.lane)} cy={rowY(n.row)} r={DOT_R} className={cls} />
                    );
                  })}
                </svg>
              )}
              <div className="dag-rows" style={{ marginLeft: gutterW }}>
                {rows.map((r) => (
                  <RevisionRow
                    key={r.id}
                    rev={r}
                    refCols={refCols}
                    dot={filtering}
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
                ))}
              </div>
            </div>
          )}
        </div>
      </div>

      <aside className="history-detail">
        {selected ? (
          <RevisionDetail
            rev={selected}
            all={extractions}
            isViewing={selected.id === activeId}
            isOnDisk={selected.id === designHead}
            act={act}
          />
        ) : (
          <div className="menu-empty">Select a revision.</div>
        )}
      </aside>

      {ctx &&
        (() => {
          const row = extractions.find((e) => e.id === ctx.id);
          if (!row) return null;
          return <ContextMenu x={ctx.x} y={ctx.y} items={act.menuFor(row)} onClose={() => setCtx(null)} />;
        })()}

      {act.dialogs}
    </div>
  );
}

/** The detail pane — the payoff for giving history real width. Everything that used to
 *  be crammed onto the row (or hidden in a tooltip) reads properly here: the full
 *  changelog, who and when, the pointer state, tags, and the actions worth a button. */
function RevisionDetail({
  rev,
  all,
  isViewing,
  isOnDisk,
  act,
}: {
  rev: ExtractionMeta;
  all: ExtractionMeta[];
  isViewing: boolean;
  isOnDisk: boolean;
  act: ReturnType<typeof useRevisionActions>;
}) {
  const localOnly = rev.is_checkpoint && !rev.published;
  const parent = parentOf(rev, all);

  return (
    <div className="history-detail-inner">
      <div className="history-detail-subject">{rowText(rev)}</div>

      <div className="history-detail-who">
        <span className="rev-avatar" title={rev.author ?? "unknown author"}>
          {initials(rev.author)}
        </span>
        <span>{rev.author ?? "unknown author"}</span>
        <span className="dim" title={formatLocalTime(rev.created_at)}>
          {formatRelative(rev.created_at)}
        </span>
      </div>

      {(isViewing || isOnDisk || localOnly || rev.hidden) && (
        <div className="history-detail-refs">
          {isViewing && (
            <span className="rev-ref rev-ref-here">
              <IconEye size={11} />
              viewing
            </span>
          )}
          {isOnDisk && (
            <span className="rev-ref rev-ref-disk" title="Edits made in KiCad will continue from here">
              <IconFolder size={11} />
              on disk
            </span>
          )}
          {localOnly && <span className="rev-state">local checkpoint</span>}
          {rev.hidden && <span className="rev-state">deleted</span>}
        </div>
      )}

      {rev.tags.length > 0 && (
        <div className="history-detail-refs">
          {rev.tags.map((t) => (
            <span key={t} className="rev-tag-ref">
              <IconTag size={10} />
              {t}
            </span>
          ))}
        </div>
      )}

      {/* The changelog in full — the row can only ever show one ellipsised line of it. */}
      {rev.message && <p className="history-detail-msg">{rev.message}</p>}

      <dl className="history-detail-meta">
        <dt>Revision</dt>
        <dd className="mono" title={rev.id}>
          {shortId(rev.id)}
        </dd>
        <dt>Created</dt>
        <dd>{formatLocalTime(rev.created_at)}</dd>
        {rev.design_tool && (
          <>
            <dt>Tool</dt>
            <dd>{rev.design_tool}</dd>
          </>
        )}
        {rev.git_branch && (
          <>
            <dt>Git</dt>
            <dd className="mono">
              {rev.git_branch}
              {rev.git_hash ? ` · ${rev.git_hash.slice(0, 8)}` : ""}
              {rev.git_dirty ? " · dirty" : ""}
            </dd>
          </>
        )}
      </dl>

      <div className="history-detail-actions">
        <button
          className="btn-primary"
          disabled={isViewing}
          title={isViewing ? "Already on screen" : "Render this revision on the canvas"}
          onClick={() => act.openVersion(rev.id)}
        >
          <IconEye size={13} /> {isViewing ? "Viewing" : "Open"}
        </button>
        <button
          className="btn-ghost"
          disabled={!parent}
          title={parent ? "Compare against the previous revision" : "This is a root revision"}
          onClick={() => parent && act.startCompare(parent, rev.id)}
        >
          <IconCompare size={13} /> Compare with previous
        </button>
        {localOnly && (
          <button className="btn-ghost" onClick={() => act.startPublish(rev.id)}>
            <IconSparkle size={13} /> Publish…
          </button>
        )}
      </div>
    </div>
  );
}
