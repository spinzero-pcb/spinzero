import { useState } from "react";
import { useDesignStore } from "../../stores/designStore";
import { useViewStore } from "../../stores/viewStore";
import {
  displayInfo,
  nextSessionTitle,
  numberMap,
  refLabel,
  useReviewStore,
  type DisplayStatus,
} from "../../stores/reviewStore";
import { nav, pcbNav } from "../canvas/navigator";
import { formatRelative } from "../../lib/time";
import type { Comment, CommentSeverity, CommentView } from "../../lib/types";
import { ContextMenu, type MenuItem } from "../ContextMenu";
import { IconChecklist, IconComment, IconCopy, IconCheck, IconRefresh, IconTrash } from "../icons";

const SEVERITY_CLASS: Record<CommentSeverity, string> = {
  info: "sev-info",
  minor: "sev-minor",
  major: "sev-major",
  critical: "sev-critical",
};
/** CSS colour per severity — mirrors the .rv-row.sev-* border colours so the
 *  right-click severity submenu shows each level by its colour swatch. */
const SEVERITY_COLOR: Record<CommentSeverity, string> = {
  info: "var(--fg-2)",
  minor: "var(--accent)",
  major: "var(--warn)",
  critical: "var(--err)",
};
const SEVERITIES: CommentSeverity[] = ["info", "minor", "major", "critical"];
const VIEW_LABEL: Record<CommentView, string> = {
  schematic: "Schematic",
  pcb: "PCB",
  bom: "BOM",
};

function statusGlyph(status: DisplayStatus, number: number) {
  if (status === "recheck") return <IconRefresh size={13} />;
  if (status === "resolved") return <IconCheck size={13} />;
  if (status === "dismissed") return <span className="rv-glyph-x">×</span>;
  return <span className="rv-num">{number}</span>;
}

export function ReviewPanel() {
  const indexes = useDesignStore((s) => s.indexes);
  const comments = useReviewStore((s) => s.comments);
  const filterStatus = useReviewStore((s) => s.filterStatus);
  const filterSeverity = useReviewStore((s) => s.filterSeverity);
  const filterView = useReviewStore((s) => s.filterView);
  const setFilterStatus = useReviewStore((s) => s.setFilterStatus);
  const setFilterSeverity = useReviewStore((s) => s.setFilterSeverity);
  const setFilterView = useReviewStore((s) => s.setFilterView);
  const openThread = useReviewStore((s) => s.openThread);
  const openThreadId = useReviewStore((s) => s.openThreadId);
  const setStatus = useReviewStore((s) => s.setStatus);
  const setSeverity = useReviewStore((s) => s.setSeverity);
  const del = useReviewStore((s) => s.del);
  const sessions = useReviewStore((s) => s.sessions);
  const activeSessionId = useReviewStore((s) => s.activeSessionId);
  const setActiveSession = useReviewStore((s) => s.setActiveSession);
  const createSession = useReviewStore((s) => s.createSession);
  const deleteSession = useReviewStore((s) => s.deleteSession);

  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);

  const numbers = numberMap(comments);

  // A comment belongs to the active session; only that session's comments are shown
  // (item 5). The "All comments" pool (activeSessionId === null) shows every comment,
  // including legacy ones filed before sessions existed.
  const inSession = (c: Comment) => activeSessionId === null || c.session_id === activeSessionId;

  // Tally derived status (⟳ re-check is computed from the live design), scoped to the
  // active session so the chips reflect the session you're looking at.
  const counts: Record<string, number> = { open: 0, recheck: 0, resolved: 0, dismissed: 0, addressed: 0 };
  const infoById = new Map<string, ReturnType<typeof displayInfo>>();
  let sessionCount = 0;
  for (const c of comments) {
    const info = displayInfo(c, indexes ?? null);
    infoById.set(c.id, info);
    if (inSession(c)) {
      counts[info.status] = (counts[info.status] ?? 0) + 1;
      sessionCount += 1;
    }
  }
  const recheckCount = counts.recheck;

  const visible = comments.filter((c) => {
    if (!inSession(c)) return false;
    const st = infoById.get(c.id)!.status;
    if (filterStatus !== "all" && st !== filterStatus) return false;
    if (filterSeverity !== "all" && c.severity !== filterSeverity) return false;
    if (filterView !== "all" && c.view !== filterView) return false;
    return true;
  });

  /** Item 8: clicking a comment GOES to it (correct canvas + camera) and opens the
   *  thread — it never selects or highlights the object. */
  function go(c: Comment) {
    useViewStore.getState().setView(c.view);
    if (c.view === "schematic") nav.reveal(c.anchor);
    else if (c.view === "pcb") pcbNav.reveal(c.anchor);
    openThread(c.id, null);
  }

  /** Right-click a row (item 11): resolve/reopen, change severity, dismiss, delete. */
  function rowMenu(e: React.MouseEvent, c: Comment) {
    e.preventDefault();
    const items: MenuItem[] = [];
    items.push({ label: "Open thread", icon: <IconComment size={14} />, onClick: () => go(c) });
    items.push({ separator: true });
    if (c.status === "resolved") {
      items.push({ label: "Reopen", icon: <IconRefresh size={14} />, onClick: () => void setStatus(c.id, "open") });
    } else {
      items.push({ label: "Done", icon: <IconCheck size={14} />, onClick: () => void setStatus(c.id, "resolved") });
    }
    if (c.status === "dismissed") {
      items.push({ label: "Reopen", icon: <IconRefresh size={14} />, onClick: () => void setStatus(c.id, "open") });
    } else {
      items.push({ label: "Dismiss", icon: <IconTrash size={14} />, onClick: () => void setStatus(c.id, "dismissed") });
    }
    items.push({
      label: "Severity",
      icon: <IconChecklist size={14} />,
      submenu: SEVERITIES.map((sev) => ({
        label: sev,
        swatch: SEVERITY_COLOR[sev],
        active: c.severity === sev,
        onClick: () => void setSeverity(c.id, sev),
      })),
    });
    items.push({ separator: true });
    items.push({
      label: c.anchor.type === "net" ? "Copy net name" : "Copy designator",
      icon: <IconCopy size={14} />,
      onClick: () => void navigator.clipboard?.writeText(c.anchor.ref),
    });
    items.push({ label: "Delete comment", icon: <IconTrash size={14} />, onClick: () => void del(c.id) });
    setCtxMenu({ x: e.clientX, y: e.clientY, items });
  }

  /** Delete the active review session AND its comments. Opens a one-step confirm menu
   *  anchored at the trash button. Disabled for "All comments" (a view, not a session). */
  function deleteSessionMenu(e: React.MouseEvent<HTMLButtonElement>) {
    if (activeSessionId === null) return;
    const session = sessions.find((s) => s.id === activeSessionId);
    if (!session) return;
    const n = comments.filter((c) => c.session_id === activeSessionId).length;
    const items: MenuItem[] = [];
    if (n > 0) {
      items.push({
        label: `Also deletes ${n} comment${n > 1 ? "s" : ""} in this session`,
        disabled: true,
      });
      items.push({ separator: true });
    }
    items.push({ label: `Delete “${session.title}”`, icon: <IconTrash size={14} />, onClick: () => void deleteSession(session.id) });
    const r = e.currentTarget.getBoundingClientRect();
    setCtxMenu({ x: r.left, y: r.bottom + 2, items });
  }

  const chip = (key: typeof filterStatus, label: string, n?: number) => (
    <button
      className={`rv-filter ${filterStatus === key ? "on" : ""}`}
      onClick={() => setFilterStatus(key)}
    >
      {label}
      {n !== undefined && <span className="rv-filter-n">{n}</span>}
    </button>
  );

  return (
    <div className="review-panel">
      {/* Review sessions: pick the active session or start a new one. New projects open
          on "Review 1" (item 4); only the selected session's comments are shown (item 5),
          while "All comments" shows every comment regardless of session. */}
      <div className="rv-sessions">
        <select
          className="rv-select rv-session-select"
          value={activeSessionId ?? "__all__"}
          onChange={(e) => setActiveSession(e.target.value === "__all__" ? null : e.target.value)}
          title="Review session"
        >
          <option value="__all__">All comments</option>
          {sessions.map((s) => (
            <option key={s.id} value={s.id}>
              {s.title}
            </option>
          ))}
        </select>
        <button
          className="rv-session-btn"
          title="Start a new review session"
          onClick={() => void createSession(nextSessionTitle(sessions))}
        >
          + Session
        </button>
        <button
          className="rv-session-btn rv-session-del"
          title={
            activeSessionId === null
              ? "Pick a session to delete (All comments can't be deleted)"
              : "Delete this review session"
          }
          disabled={activeSessionId === null}
          onClick={deleteSessionMenu}
        >
          <IconTrash size={13} />
        </button>
      </div>
      <div className="rv-head">
        <div className="rv-filters">
          {chip("all", "All", sessionCount)}
          {chip("open", "Open", counts.open)}
          {chip("recheck", "⟳", counts.recheck)}
          {chip("resolved", "Done", counts.resolved)}
          {chip("dismissed", "Dismissed", counts.dismissed)}
        </div>
      </div>

      {/* Severity + view filters (items 15/19). */}
      <div className="rv-subfilters">
        <select
          className="rv-select"
          value={filterSeverity}
          onChange={(e) => setFilterSeverity(e.target.value as typeof filterSeverity)}
          title="Filter by severity"
        >
          <option value="all">Any severity</option>
          {SEVERITIES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
        <select
          className="rv-select"
          value={filterView}
          onChange={(e) => setFilterView(e.target.value as typeof filterView)}
          title="Filter by view"
        >
          <option value="all">All views</option>
          <option value="schematic">Schematic</option>
          <option value="pcb">PCB</option>
          <option value="bom">BOM</option>
        </select>
      </div>

      {recheckCount > 0 && (
        <button
          className="rv-inbox"
          onClick={() => setFilterStatus("recheck")}
          title="Object changed since these were filed — confirm the fix"
        >
          <IconRefresh size={13} />
          {recheckCount} comment{recheckCount > 1 ? "s" : ""} need re-check
        </button>
      )}

      {visible.length === 0 ? (
        <div className="rv-empty">
          {sessionCount === 0 ? (
            <>
              No comments yet. Right-click a part or net and choose “Add comment” to
              start a review thread, anchored to that object.
            </>
          ) : (
            <>Nothing matches this filter.</>
          )}
        </div>
      ) : (
        <div className="rv-list">
          {visible.map((c) => {
            const info = infoById.get(c.id)!;
            const sev = c.severity ? SEVERITY_CLASS[c.severity] : "sev-none";
            const body = c.thread[0]?.body ?? "";
            const anchorLabel = refLabel(c);
            const lastTs = c.thread[c.thread.length - 1]?.ts ?? c.created_ts;
            return (
              <div
                key={c.id}
                role="button"
                tabIndex={0}
                className={`rv-row ${sev} st-${info.status} ${openThreadId === c.id ? "sel" : ""}`}
                onClick={() => go(c)}
                onContextMenu={(e) => rowMenu(e, c)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    go(c);
                  }
                }}
              >
                <span className={`rv-status st-${info.status}`}>
                  {statusGlyph(info.status, numbers.get(c.id) ?? 0)}
                </span>
                <span className="rv-main">
                  <span className="rv-reftop">
                    <span className="rv-ref mono">{anchorLabel}</span>
                    <span className={`rv-viewtag view-${c.view}`}>{VIEW_LABEL[c.view]}</span>
                    {c.severity && (
                      <span className={`rv-sev ${SEVERITY_CLASS[c.severity]}`}>{c.severity}</span>
                    )}
                    {c.source !== "human" && (
                      <span className={`rv-src src-${c.source}`}>{c.source}</span>
                    )}
                  </span>
                  <span className="rv-body">{body || <em className="dim">(no text)</em>}</span>
                  <span className="rv-foot">
                    <span className="dim">{formatRelative(lastTs)}</span>
                    {c.thread.length > 1 && (
                      <span>{c.thread.length - 1} repl{c.thread.length - 1 > 1 ? "ies" : "y"}</span>
                    )}
                    {c.assignee && <span className="rv-assignee">@{c.assignee}</span>}
                    {info.status === "recheck" && info.diff.length > 0 && (
                      <span className="rv-diff mono">{info.diff[0]}</span>
                    )}
                  </span>
                </span>
                {/* Inline mark-done / dismiss / reopen (item 9) — act on a comment
                    straight from the rail without opening the thread. */}
                <span className="rv-rowactions">
                  {c.status === "resolved" || c.status === "dismissed" ? (
                    <button
                      className="rv-rowbtn"
                      title="Reopen"
                      onClick={(e) => {
                        e.stopPropagation();
                        void setStatus(c.id, "open");
                      }}
                    >
                      <IconRefresh size={13} />
                    </button>
                  ) : (
                    <>
                      <button
                        className="rv-rowbtn resolve"
                        title="Done"
                        onClick={(e) => {
                          e.stopPropagation();
                          void setStatus(c.id, "resolved");
                        }}
                      >
                        <IconCheck size={13} />
                      </button>
                      <button
                        className="rv-rowbtn dismiss"
                        title="Dismiss"
                        onClick={(e) => {
                          e.stopPropagation();
                          void setStatus(c.id, "dismissed");
                        }}
                      >
                        <span className="rv-glyph-x">×</span>
                      </button>
                    </>
                  )}
                </span>
              </div>
            );
          })}
        </div>
      )}
      {ctxMenu && <ContextMenu {...ctxMenu} onClose={() => setCtxMenu(null)} />}
    </div>
  );
}
