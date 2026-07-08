import { useEffect, useRef, useState } from "react";
import { useDesignStore } from "../../stores/designStore";
import { useViewStore } from "../../stores/viewStore";
import { displayInfo, numberMap, refLabel, useReviewStore } from "../../stores/reviewStore";
import { formatLocalTime, formatRelative } from "../../lib/time";
import type { CommentSeverity } from "../../lib/types";
import { IconCheck } from "../icons";

const SEVERITIES: CommentSeverity[] = ["info", "minor", "major", "critical"];
const POP_W = 320;
const POP_H = 380;

/** Clamp a desired top-left into the viewport so the card never opens off-screen.
 *  `pos` arrives just to the right of the anchored object so the pin stays visible;
 *  if that would overflow the right edge we flip the card to the LEFT of the anchor
 *  (rather than letting the clamp slide it back over the pin). */
function clampPos(pos: { x: number; y: number } | null): { left: number; top: number } {
  if (!pos) {
    // Default: float just right of the left rail near the top.
    return { left: 360, top: 96 };
  }
  let left = pos.x;
  if (left + POP_W > window.innerWidth - 8) left = pos.x - POP_W - 28; // flip to the left
  left = Math.min(Math.max(left, 8), window.innerWidth - POP_W - 8);
  const top = Math.min(Math.max(pos.y, 8), window.innerHeight - POP_H - 8);
  return { left, top };
}

/** Makes the popover draggable by a handle (its header). Starts at `initial`; once the
 *  user drags, the moved position takes over so they can pull the card off the object
 *  to read the schematic underneath. Drags that begin on a control (button/select/etc.)
 *  are ignored so the header buttons still work. */
function useDraggablePos(initial: { left: number; top: number }) {
  const [moved, setMoved] = useState<{ left: number; top: number } | null>(null);
  const pos = moved ?? initial;
  const onPointerDown = (e: React.PointerEvent) => {
    if ((e.target as HTMLElement).closest("button, input, select, textarea, a")) return;
    e.preventDefault();
    const startX = e.clientX;
    const startY = e.clientY;
    const base = { left: pos.left, top: pos.top };
    // Capture on the header element and listen on IT (not window): if the popover unmounts
    // mid-drag the listeners die with the element, so setMoved can't fire on an unmounted
    // component and no window listener leaks (mirrors PropertiesCard's pointer capture).
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const left = Math.min(
        Math.max(base.left + (ev.clientX - startX), 8),
        window.innerWidth - POP_W - 8,
      );
      const top = Math.min(
        Math.max(base.top + (ev.clientY - startY), 8),
        window.innerHeight - 40,
      );
      setMoved({ left, top });
    };
    const up = () => {
      el.releasePointerCapture(e.pointerId);
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };
  return { style: { left: pos.left, top: pos.top } as React.CSSProperties, onPointerDown };
}

/** Two-letter avatar initials from a username (chat affordance, item 25). */
function initials(name: string): string {
  const parts = name.replace(/[._-]+/g, " ").trim().split(/\s+/);
  const a = parts[0]?.[0] ?? "?";
  const b = parts.length > 1 ? parts[parts.length - 1][0] : parts[0]?.[1] ?? "";
  return (a + b).toUpperCase();
}

/** Renders the new-comment composer (when `compose` is set) or the thread popover
 *  for the open comment — both float over the canvas (docs/phase2-ui-plan.md §4). */
export function ThreadPopover() {
  const compose = useReviewStore((s) => s.compose);
  const openThreadId = useReviewStore((s) => s.openThreadId);
  if (compose) return <Composer />;
  if (openThreadId) return <Thread id={openThreadId} />;
  return null;
}

function Composer() {
  const compose = useReviewStore((s) => s.compose)!;
  const create = useReviewStore((s) => s.create);
  const cancel = useReviewStore((s) => s.cancelCompose);
  const [body, setBody] = useState("");
  const [severity, setSeverity] = useState<CommentSeverity>("major");
  const taRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => taRef.current?.focus(), []);

  const submit = () => {
    if (!body.trim()) return;
    // Item 15: stamp the comment with the view it was authored in, so it shows on
    // that canvas and navigates back there.
    void create(compose.anchor, body.trim(), severity, useViewStore.getState().view);
  };

  const { style, onPointerDown } = useDraggablePos(clampPos(compose.pos ?? null));

  return (
    <div className="thread-pop" style={style}>
      <div className="thread-head thread-drag" onPointerDown={onPointerDown}>
        <span className="mono thread-ref">
          {compose.anchor.type === "region"
            ? `▭ ${compose.anchor.sheet ?? "region"}`
            : `${compose.anchor.type === "net" ? "net " : ""}${compose.anchor.ref}`}
        </span>
        <span className={`rv-viewtag view-${useViewStore.getState().view}`}>
          {useViewStore.getState().view}
        </span>
        <button className="thread-x" onClick={cancel} title="Cancel">
          ×
        </button>
      </div>
      <textarea
        ref={taRef}
        className="thread-input"
        placeholder={
          compose.anchor.type === "region"
            ? "Describe the issue in this area…"
            : "Describe the issue on this object…"
        }
        value={body}
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) submit();
          if (e.key === "Escape") cancel();
        }}
      />
      <div className="thread-actions">
        <select
          className="thread-sev"
          value={severity}
          onChange={(e) => setSeverity(e.target.value as CommentSeverity)}
          title="Severity"
        >
          {SEVERITIES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
        <span className="spacer" />
        <button className="btn-ghost" onClick={cancel}>
          Cancel
        </button>
        <button className="btn-primary" onClick={submit} disabled={!body.trim()}>
          Comment
        </button>
      </div>
      <div className="thread-hint">
        Ctrl+Enter to post · anchored to{" "}
        {compose.anchor.type === "region" ? "this area" : "this object"}
      </div>
    </div>
  );
}

function Thread({ id }: { id: string }) {
  const indexes = useDesignStore((s) => s.indexes);
  const comments = useReviewStore((s) => s.comments);
  const pos = useReviewStore((s) => s.threadPos);
  const close = useReviewStore((s) => s.openThread);
  const reply = useReviewStore((s) => s.reply);
  const setStatus = useReviewStore((s) => s.setStatus);
  const setSeverity = useReviewStore((s) => s.setSeverity);
  const del = useReviewStore((s) => s.del);

  const [text, setText] = useState("");
  const [dismissing, setDismissing] = useState(false);
  const [reason, setReason] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);

  const c = comments.find((x) => x.id === id);
  // Keep the latest message in view as the thread grows (chat affordance, item 25).
  useEffect(() => {
    bodyRef.current?.scrollTo({ top: bodyRef.current.scrollHeight });
  }, [c?.thread.length]);
  if (!c) return null;
  const info = displayInfo(c, indexes ?? null);
  const number = numberMap(comments).get(c.id) ?? 0;

  const sendReply = () => {
    if (!text.trim()) return;
    void reply(c.id, text.trim());
    setText("");
  };

  const { style, onPointerDown } = useDraggablePos(clampPos(pos));

  return (
    <div className="thread-pop" style={style}>
      <div className="thread-head thread-drag" onPointerDown={onPointerDown}>
        <span className={`rv-status st-${info.status}`}>
          {info.status === "resolved" ? <IconCheck size={12} /> : number}
        </span>
        <span className="mono thread-ref">{refLabel(c)}</span>
        <span className={`rv-viewtag view-${c.view}`}>{c.view}</span>
        {/* Item 10: change severity at any time, including after resolving. */}
        <select
          className="thread-sev"
          value={c.severity ?? "major"}
          onChange={(e) => void setSeverity(c.id, e.target.value as CommentSeverity)}
          title="Severity"
        >
          {SEVERITIES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
        <button className="thread-x" onClick={() => close(null)} title="Close">
          ×
        </button>
      </div>

      {info.drift && (
        <div className="thread-drift">
          on {c.base_revision.slice(0, 9) || "an earlier revision"} — object changed
          {info.diff.length > 0 && <> · {info.diff.join(" · ")}</>}
        </div>
      )}

      <div className="thread-body" ref={bodyRef}>
        {c.thread.map((t) => {
          const who = t.author_name?.trim() || t.user;
          return (
          <div className="thread-entry" key={t.event_id}>
            <div className="thread-avatar" aria-hidden title={`@${t.user}`}>
              {initials(who)}
            </div>
            <div className="thread-entry-main">
              <div className="thread-entry-head">
                <span className="thread-who" title={`@${t.user}`}>{who}</span>
                <span className="dim" title={formatLocalTime(t.ts)}>
                  {formatRelative(t.ts)}
                </span>
              </div>
              <div className="thread-text">{t.body}</div>
            </div>
          </div>
          );
        })}
        {c.status === "dismissed" && (
          <div className="thread-entry thread-dismissed">
            Dismissed{c.reason ? `: ${c.reason}` : ""}
          </div>
        )}
      </div>

      <textarea
        className="thread-input"
        placeholder="Reply…"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) sendReply();
        }}
      />

      {dismissing ? (
        <div className="thread-actions">
          <input
            className="thread-reason"
            placeholder="Reason (optional)…"
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            autoFocus
          />
          <button className="btn-ghost" onClick={() => setDismissing(false)}>
            Cancel
          </button>
          <button
            className="btn-primary"
            onClick={() => {
              void setStatus(c.id, "dismissed", reason.trim() || undefined);
              setDismissing(false);
              setReason("");
            }}
          >
            Dismiss
          </button>
        </div>
      ) : confirmDelete ? (
        <div className="thread-actions">
          <span className="thread-confirm">Delete this comment permanently?</span>
          <span className="spacer" />
          <button className="btn-ghost" onClick={() => setConfirmDelete(false)}>
            Cancel
          </button>
          <button className="btn-danger" onClick={() => void del(c.id)}>
            Delete
          </button>
        </div>
      ) : (
        <div className="thread-actionrow">
          {/* Delete (item 24). Locate / Assign were removed (item 13). */}
          <button className="thread-act dim" onClick={() => setConfirmDelete(true)} title="Delete comment">
            Delete
          </button>
          <span className="spacer" />
          {text.trim() ? (
            <button className="btn-primary" onClick={sendReply}>
              Reply
            </button>
          ) : c.status === "dismissed" ? (
            // Item 16: a dismissed comment can be reopened.
            <button className="thread-act" onClick={() => void setStatus(c.id, "open")}>
              Reopen
            </button>
          ) : c.status === "resolved" ? (
            <button className="thread-act" onClick={() => void setStatus(c.id, "open")}>
              Reopen
            </button>
          ) : (
            <>
              <button className="thread-act dim" onClick={() => setDismissing(true)}>
                Dismiss
              </button>
              <button
                className="thread-act resolve"
                onClick={() => void setStatus(c.id, "resolved")}
                title="Mark as done (closes the thread)"
              >
                <IconCheck size={13} />
                Done
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
