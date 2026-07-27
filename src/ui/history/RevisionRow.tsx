import type { ReactNode } from "react";
import { formatLocalTime, formatRelative } from "../../lib/time";
import { IconEye, IconFolder, IconSparkle, IconTag } from "../icons";
import { initials, rowText, shortId } from "./filter";
import type { ExtractionMeta } from "../../lib/types";

/** One revision row, shared by the rail panel and the History workspace.
 *
 *  The badge vocabulary here is deliberate — the old row rendered three unrelated
 *  categories as identical pills (and "on disk" and "local" were literally the same
 *  warn-yellow style), so a reader had to memorise which yellow meant what:
 *
 *  - POINTERS ("viewing", "on disk") are refs, not adjectives. They live in a
 *    fixed-width slot before the subject, so the subject starts at the same x on every
 *    row instead of shifting by whichever pointers happen to land there.
 *  - STATE (local / deleted) is carried by the row itself — a hollow dot and italics
 *    for an unpublished checkpoint, strikethrough and dimming for a tombstone. The old
 *    pills were saying the same thing a second time.
 *  - USER TAGS keep the rounded pill, which now means exactly one thing: a name a human
 *    chose. Nothing the system generates is round.
 *
 *  Colour follows the same discipline: accent = where you are, neutral outline = where
 *  the files are, green = a human's tag. Yellow is left for real warnings (fork risk,
 *  missing design folder) rather than being spent on routine information. */
export function RevisionRow({
  rev,
  refCols,
  isViewing,
  isOnDisk,
  selected,
  compareRole,
  height,
  /** Draw the node dot inline (the rail's flat list). The workspace draws its dots in
   *  the DAG's SVG lane gutter instead, so it leaves this off. */
  dot = false,
  editor,
  onClick,
  onDoubleClick,
  onContextMenu,
  onPublish,
  rowRef,
}: {
  rev: ExtractionMeta;
  /** Ref-slot width in chips, from `refColumns()` — keeps subjects aligned. */
  refCols: number;
  isViewing: boolean;
  isOnDisk: boolean;
  selected: boolean;
  /** Compare pick-mode: this row is the picked source, a candidate target, or neither. */
  compareRole: "from" | "target" | null;
  height: number;
  dot?: boolean;
  /** Inline rename/tag input, when this row is being edited — replaces the content. */
  editor: ReactNode | null;
  onClick: (e: React.MouseEvent) => void;
  onDoubleClick: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
  /** Publish this local checkpoint — surfaced on the row itself because publishing is
   *  the single most important gesture in the model and it used to be buried in a
   *  right-click menu behind a banner that told you to right-click. */
  onPublish: () => void;
  rowRef?: (el: HTMLDivElement | null) => void;
}) {
  const localOnly = rev.is_checkpoint && !rev.published;
  const cls = [
    "rev-row",
    localOnly ? "is-local" : "",
    rev.hidden ? "is-deleted" : "",
    isViewing ? "is-viewing" : "",
    selected ? "is-selected" : "",
    compareRole ? `compare-${compareRole}` : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      ref={rowRef}
      className={cls}
      style={{ height, ["--rev-ref-cols" as string]: refCols }}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
      title={
        editor
          ? undefined
          : `${rowText(rev)}\n${formatLocalTime(rev.created_at)} · ${shortId(rev.id)}\n\nClick to select · double-click to open · right-click for actions`
      }
    >
      {dot && (
        <span
          className={`rev-dot ${localOnly ? "is-local" : "is-published"} ${isViewing ? "is-active" : ""}`}
          aria-hidden="true"
        />
      )}
      {editor ?? (
        <>
          {refCols > 0 && (
            <span className="rev-refs">
              {isViewing && (
                <span className="rev-ref rev-ref-here" title="The revision currently on screen">
                  <IconEye size={11} />
                  viewing
                </span>
              )}
              {isOnDisk && (
                <span
                  className="rev-ref rev-ref-disk"
                  title="The KiCad files on disk match this revision — edits made in KiCad will continue from here"
                >
                  <IconFolder size={11} />
                  on disk
                </span>
              )}
            </span>
          )}
          <span className="rev-subject">{rowText(rev)}</span>
          {rev.tags.map((t) => (
            <span key={t} className="rev-tag-ref" title={`Tag “${t}”`}>
              <IconTag size={10} />
              {t}
            </span>
          ))}
          <span className="rev-spacer" />
          {localOnly && (
            <button
              className="rev-publish-btn"
              title="Publish this checkpoint to the shared history"
              onClick={(e) => {
                e.stopPropagation();
                onPublish();
              }}
            >
              <IconSparkle size={12} />
              Publish
            </button>
          )}
          <span className="rev-avatar" title={rev.author ?? "unknown author"}>
            {initials(rev.author)}
          </span>
          <span className="rev-when dim" title={formatLocalTime(rev.created_at)}>
            {formatRelative(rev.created_at)}
          </span>
        </>
      )}
    </div>
  );
}
