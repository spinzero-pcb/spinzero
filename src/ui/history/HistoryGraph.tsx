import { useEffect, useMemo, useState } from "react";
import { useHistoryStore } from "../../stores/historyStore";
import { useProjectStore } from "../../stores/projectStore";
import { useDiffStore } from "../../stores/diffStore";
import { ContextMenu, type MenuItem } from "../ContextMenu";
import { IconClose, IconCompare, IconCopy, IconEdit, IconEye, IconFolder, IconRefresh, IconSparkle, IconTag, IconTrash } from "../icons";
import { formatLocalTime } from "../../lib/time";
import { layoutDag } from "./layout";
import type { ExtractionMeta } from "../../lib/types";

// Graph geometry (px). Colors come from CSS vars; only layout dimensions live here.
const LANE_W = 22;
const ROW_H = 34;
const DOT_R = 4.5;
const laneX = (lane: number) => lane * LANE_W + LANE_W / 2;
const rowY = (row: number) => row * ROW_H + ROW_H / 2;

/** Short, git-style revision id (content hash) for the row's trailing ref. */
const shortId = (id: string) => id.slice(0, 10);

/** The revision-history overlay — the single version-control surface (opened from the
 *  footer version chip). A DAG of the merged published + local-checkpoint timeline,
 *  with a "viewing" marker on the active node and per-row actions: open / update the
 *  KiCad files (the only path that writes to disk — opening a version is view-only) /
 *  rename / tag / publish (with a required changelog) / hide / delete. Local checkpoints render
 *  distinctly (hollow dot + "local" badge + italic) so a reader sees only the published
 *  line as the "normal" history. Compare/diff is a first-class feature again (visual-diff
 *  §3): "Compare with…" enters pick mode, "Compare with previous" uses the parent pointer,
 *  and "Compare tips" appears when the DAG has two heads. */
export function HistoryGraph() {
  const open = useHistoryStore((s) => s.open);
  const showHidden = useHistoryStore((s) => s.showHidden);
  const closeGraph = useHistoryStore((s) => s.closeGraph);
  const toggleHidden = useHistoryStore((s) => s.toggleHidden);

  const extractions = useProjectStore((s) => s.extractions);
  const activeExtraction = useProjectStore((s) => s.activeExtraction);
  const setActiveExtraction = useProjectStore((s) => s.setActiveExtraction);
  const updateDesignFiles = useProjectStore((s) => s.updateDesignFiles);
  const designPathMissing = useProjectStore((s) => s.designPathMissing);
  const publish = useProjectStore((s) => s.publish);
  const hide = useProjectStore((s) => s.hide);
  const unhide = useProjectStore((s) => s.unhide);
  const labelExtraction = useProjectStore((s) => s.labelExtraction);
  const setTag = useProjectStore((s) => s.setTag);
  const removeTag = useProjectStore((s) => s.removeTag);
  const deleteCheckpoint = useProjectStore((s) => s.deleteCheckpoint);
  const enterDiff = useDiffStore((s) => s.enterDiff);

  // Compare pick-mode: after "Compare with…", the graph waits for a second row; other
  // rows get a target affordance and Esc cancels (visual-diff §3).
  const [compareFrom, setCompareFrom] = useState<string | null>(null);

  const [ctx, setCtx] = useState<{ x: number; y: number; id: string } | null>(null);
  const [editing, setEditing] = useState<string | null>(null); // rename
  const [draft, setDraft] = useState("");
  const [tagging, setTagging] = useState<string | null>(null); // add-tag
  const [tagDraft, setTagDraft] = useState("");
  const [publishing, setPublishing] = useState<string | null>(null); // changelog dialog
  const [changelog, setChangelog] = useState("");
  // Permanent (unrecoverable) delete confirmation for a local-only checkpoint — the
  // only revision kind with a hard delete; published history is soft-deleted (hide).
  const [confirmDel, setConfirmDel] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (confirmDel) setConfirmDel(null);
      else if (publishing) setPublishing(null);
      else if (compareFrom) setCompareFrom(null); // cancel compare pick-mode
      else closeGraph();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, closeGraph, publishing, confirmDel, compareFrom]);

  const layout = useMemo(() => layoutDag(extractions, showHidden), [extractions, showHidden]);

  // DAG tips = revisions that are nobody's parent (the branch heads). Two+ tips is the
  // fork case the "Compare tips" shortcut targets. Computed before the early return
  // below so hook order stays stable across open/closed renders.
  const tips = useMemo(() => {
    const isParent = new Set(extractions.flatMap((e) => e.parents));
    return extractions.filter((e) => !e.hidden && !isParent.has(e.id));
  }, [extractions]);

  if (!open) return null;

  const latestId = extractions[0]?.id ?? null;
  const activeId = activeExtraction ?? latestId;
  const localCount = extractions.filter((e) => e.is_checkpoint && !e.published).length;

  // Row text reads like a commit subject: a manual rename wins, else the publish
  // changelog, else the timestamp.
  const rowText = (r: ExtractionMeta) => r.label ?? r.message ?? formatLocalTime(r.created_at);

  const openVersion = (id: string) =>
    void setActiveExtraction(id === latestId ? null : id).then(closeGraph);

  // The nearest present parent for "Compare with previous" (first parent = nearest
  // ancestor on the active lane, per the VC plan §8). null for a root revision.
  const parentOf = (r: ExtractionMeta): string | null => {
    const present = r.parents.find((p) => extractions.some((e) => e.id === p));
    return present ?? null;
  };

  // Start a comparison and close the graph; enterDiff normalizes order + pins B active.
  const startCompare = (a: string, b: string) => {
    setCompareFrom(null);
    closeGraph();
    void enterDiff(a, b);
  };

  function saveLabel(id: string) {
    const label = draft.trim() ? draft.trim() : null;
    setEditing(null);
    void labelExtraction(id, label);
  }
  function saveTag(id: string) {
    const name = tagDraft.trim();
    setTagging(null);
    if (name) void setTag(id, name);
  }
  function doPublish() {
    const id = publishing;
    const msg = changelog.trim();
    if (!id || !msg) return; // blank changelog is not allowed (item 5)
    setPublishing(null);
    setChangelog("");
    void publish(id, msg);
  }
  function doConfirmDelete() {
    if (!confirmDel) return;
    const id = confirmDel;
    setConfirmDel(null);
    void deleteCheckpoint(id);
  }

  function rowMenu(r: ExtractionMeta): MenuItem[] {
    const localOnly = r.is_checkpoint && !r.published;
    const parent = parentOf(r);
    const items: MenuItem[] = [
      { label: "Open this version", icon: <IconEye size={14} />, onClick: () => openVersion(r.id) },
      // The ONLY way a version reaches the KiCad files on disk — opening/viewing a
      // version never writes them. Disabled when the design folder is missing here.
      {
        label: "Update KiCad files to this version…",
        icon: <IconFolder size={14} />,
        disabled: designPathMissing,
        onClick: () => {
          closeGraph();
          void updateDesignFiles(r.id);
        },
      },
      { separator: true },
      // Compare (visual-diff §3). "Compare with…" enters pick mode; "Compare with
      // previous" uses the parent pointer and is disabled for a root (no parent). Both
      // are Beta while the visual-diff surface is still stabilising.
      { label: "Compare with…", icon: <IconCompare size={14} />, badge: "Beta", onClick: () => { setEditing(null); setTagging(null); setCompareFrom(r.id); } },
      {
        label: "Compare with previous",
        icon: <IconCompare size={14} />,
        badge: "Beta",
        disabled: !parent,
        onClick: () => parent && startCompare(parent, r.id),
      },
      { separator: true },
      { label: "Rename…", icon: <IconEdit size={14} />, onClick: () => { setTagging(null); setDraft(r.label ?? ""); setEditing(r.id); } },
      { label: "Add tag…", icon: <IconTag size={14} />, onClick: () => { setEditing(null); setTagDraft(""); setTagging(r.id); } },
    ];
    for (const t of r.tags) {
      items.push({ label: `Remove tag “${t}”`, icon: <IconClose size={14} />, onClick: () => void removeTag(t) });
    }
    if (localOnly) {
      items.push({ separator: true });
      items.push({ label: "Publish…", icon: <IconSparkle size={14} />, onClick: () => { setChangelog(""); setPublishing(r.id); } });
    }
    items.push({ separator: true });
    // One delete action per row. Local checkpoints are private to this machine, so
    // their delete is the hard, confirmed removal (the "…" signals the dialog).
    // Published revisions are shared history: delete is soft (hide) and recoverable
    // via "Show deleted" → Restore — no permanent-purge in the menu.
    if (r.hidden) {
      items.push({ label: "Restore", icon: <IconRefresh size={14} />, onClick: () => void unhide(r.id) });
    }
    if (localOnly) {
      items.push({ label: "Delete permanently…", icon: <IconTrash size={14} />, onClick: () => setConfirmDel(r.id) });
    } else if (!r.hidden) {
      items.push({ label: "Delete", icon: <IconTrash size={14} />, onClick: () => void hide(r.id) });
    }
    items.push({ label: "Copy revision id", icon: <IconCopy size={14} />, onClick: () => void navigator.clipboard?.writeText(r.id) });
    return items;
  }

  const gutterW = layout.laneCount * LANE_W;
  const height = Math.max(layout.nodes.length, 1) * ROW_H;

  return (
    <div
      className="wizard-overlay history-overlay"
      onPointerDown={(e) => e.target === e.currentTarget && closeGraph()}
    >
      <div className="history-card" role="dialog" aria-label="Revision history">
        <div className="history-head">
          <div className="history-title">Revision history</div>
          <label className="history-toggle">
            <input type="checkbox" checked={showHidden} onChange={toggleHidden} /> Show deleted
          </label>
          {/* Fork awareness: when the DAG has two heads, one click compares them
              (visual-diff §3). Compares the two newest tips. */}
          {tips.length >= 2 && (
            <button
              className="btn-ghost history-compare-tips"
              title={`Compare the two branch heads (${tips[0].id.slice(0, 8)} vs ${tips[1].id.slice(0, 8)})`}
              onClick={() => startCompare(tips[1].id, tips[0].id)}
            >
              <IconCompare size={13} /> Compare tips
            </button>
          )}
          <span style={{ flex: 1 }} />
          <button className="btn-ghost" onClick={closeGraph}>
            Close
          </button>
        </div>

        {compareFrom && (
          <div className="rev-nudge compare-nudge">
            Pick a revision to compare with{" "}
            {/* The picked row can vanish mid-pick (a refresh hid it) — fall back to
                its short id instead of rowText over a hollow object. */}
            <b>
              {(() => {
                const from = extractions.find((e) => e.id === compareFrom);
                return from ? rowText(from) : shortId(compareFrom);
              })()}
            </b>{" "}
            — or press Esc to cancel.
          </div>
        )}

        {localCount > 0 && (
          <div className="rev-nudge">
            {localCount} local checkpoint{localCount === 1 ? "" : "s"} — right-click one and
            Publish to share it with your team.
          </div>
        )}

        <div className="history-body">
          {layout.nodes.length === 0 ? (
            <div className="menu-empty">No revisions yet.</div>
          ) : (
            <div className="dag" style={{ height }}>
              <svg
                className="dag-svg"
                width={gutterW}
                height={height}
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
                  ].join(" ");
                  return (
                    <circle key={n.meta.id} cx={laneX(n.lane)} cy={rowY(n.row)} r={DOT_R} className={cls} />
                  );
                })}
              </svg>
              <div className="dag-rows" style={{ marginLeft: gutterW }}>
                {layout.nodes.map((n) => {
                  const r = n.meta;
                  const localOnly = r.is_checkpoint && !r.published;
                  return (
                    <div
                      key={r.id}
                      className={`dag-row ${localOnly ? "is-local" : ""} ${r.hidden ? "is-hidden" : ""} ${r.id === activeId ? "is-viewing" : ""} ${
                        compareFrom ? (compareFrom === r.id ? "compare-from" : "compare-target") : ""
                      }`}
                      style={{ height: ROW_H }}
                      onClick={() => {
                        if (editing === r.id || tagging === r.id) return;
                        // In compare pick-mode a click picks the second revision (unless
                        // it's the same row — clicking the source again just cancels).
                        if (compareFrom) {
                          if (compareFrom === r.id) setCompareFrom(null);
                          else startCompare(compareFrom, r.id);
                          return;
                        }
                        openVersion(r.id);
                      }}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        setCtx({ x: e.clientX, y: e.clientY, id: r.id });
                      }}
                      title="Click to view this version · right-click for actions"
                    >
                      {editing === r.id ? (
                        <input
                          className="rev-label-input"
                          autoFocus
                          value={draft}
                          placeholder={formatLocalTime(r.created_at)}
                          onClick={(e) => e.stopPropagation()}
                          onChange={(e) => setDraft(e.target.value)}
                          onBlur={() => saveLabel(r.id)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") saveLabel(r.id);
                            if (e.key === "Escape") setEditing(null);
                          }}
                        />
                      ) : tagging === r.id ? (
                        <input
                          className="rev-label-input"
                          autoFocus
                          value={tagDraft}
                          placeholder="tag name (e.g. fab-v1)…"
                          onClick={(e) => e.stopPropagation()}
                          onChange={(e) => setTagDraft(e.target.value)}
                          onBlur={() => saveTag(r.id)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") saveTag(r.id);
                            if (e.key === "Escape") setTagging(null);
                          }}
                        />
                      ) : (
                        <>
                          <span className="dag-name">{rowText(r)}</span>
                          {r.id === activeId && <span className="rev-latest-tag">viewing</span>}
                          {localOnly && <span className="rev-badge rev-local">local</span>}
                          {r.hidden && <span className="rev-badge rev-hidden-badge">deleted</span>}
                          {r.tags.map((t) => (
                            <span key={t} className="rev-tag-ref">
                              {t}
                            </span>
                          ))}
                          <span style={{ flex: 1 }} />
                          {r.author && <span className="dag-author dim">{r.author}</span>}
                          <span className="dag-time dim mono">{formatLocalTime(r.created_at)}</span>
                          <span className="mono dim rev-git" title={`revision ${r.id}`}>
                            {shortId(r.id)}
                          </span>
                        </>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      </div>

      {ctx &&
        (() => {
          // If the row vanished (a concurrent refresh dropped it), close the menu rather
          // than silently operating on extractions[0] — rename/delete would hit the wrong row.
          const row = extractions.find((e) => e.id === ctx.id);
          if (!row) return null;
          return <ContextMenu x={ctx.x} y={ctx.y} items={rowMenu(row)} onClose={() => setCtx(null)} />;
        })()}

      {publishing && (
        <div
          className="wizard-overlay publish-overlay"
          onPointerDown={(e) => e.target === e.currentTarget && setPublishing(null)}
        >
          <div className="publish-card" role="dialog" aria-label="Publish revision">
            <div className="history-title">Publish to shared history</div>
            <p className="publish-hint">
              Add a short changelog so your team knows what changed — this becomes the
              revision’s message. A message is required.
            </p>
            <textarea
              className="publish-msg"
              autoFocus
              value={changelog}
              placeholder="What changed in this revision?"
              onChange={(e) => setChangelog(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) doPublish();
              }}
            />
            <div className="publish-actions">
              <button className="btn-ghost" onClick={() => setPublishing(null)}>
                Cancel
              </button>
              <button className="btn-primary" disabled={!changelog.trim()} onClick={doPublish}>
                Publish
              </button>
            </div>
          </div>
        </div>
      )}

      {confirmDel && (
        <div
          className="wizard-overlay publish-overlay"
          onPointerDown={(e) => e.target === e.currentTarget && setConfirmDel(null)}
        >
          <div className="publish-card" role="dialog" aria-label="Confirm permanent delete">
            <div className="history-title">Delete permanently?</div>
            <p className="publish-hint">
              This local checkpoint will be removed for good. This can’t be undone.
            </p>
            <div className="publish-actions">
              <button className="btn-ghost" onClick={() => setConfirmDel(null)}>
                Cancel
              </button>
              <button className="btn-primary" onClick={doConfirmDelete}>
                Delete permanently
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
