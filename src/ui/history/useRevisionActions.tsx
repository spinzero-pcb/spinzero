import { useState, type ReactNode } from "react";
import { useProjectStore } from "../../stores/projectStore";
import { useHistoryStore } from "../../stores/historyStore";
import { useDiffStore } from "../../stores/diffStore";
import { useViewStore } from "../../stores/viewStore";
import { type MenuItem } from "../ContextMenu";
import {
  IconClose,
  IconCompare,
  IconCopy,
  IconEdit,
  IconEye,
  IconFolder,
  IconRefresh,
  IconSparkle,
  IconTag,
  IconTrash,
} from "../icons";
import { formatLocalTime } from "../../lib/time";
import { parentOf } from "./filter";
import type { ExtractionMeta } from "../../lib/types";

/** Everything a revision row can DO, shared by the rail panel and the History workspace
 *  so the two surfaces can never drift into offering different actions: the right-click
 *  menu, the inline rename/tag editors, and the publish / permanent-delete dialogs.
 *
 *  The surfaces own layout; this owns behaviour. Both render `dialogs` once. */
export function useRevisionActions() {
  const extractions = useProjectStore((s) => s.extractions);
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
  const setCompareFrom = useHistoryStore((s) => s.setCompareFrom);
  const select = useHistoryStore((s) => s.select);
  const exitHistory = useViewStore((s) => s.exitHistory);

  const [editing, setEditing] = useState<string | null>(null); // rename
  const [draft, setDraft] = useState("");
  const [tagging, setTagging] = useState<string | null>(null); // add-tag
  const [tagDraft, setTagDraft] = useState("");
  const [publishing, setPublishing] = useState<string | null>(null); // changelog dialog
  const [changelog, setChangelog] = useState("");
  // Permanent (unrecoverable) delete confirmation for a local-only checkpoint — the
  // only revision kind with a hard delete; published history is soft-deleted (hide).
  const [confirmDel, setConfirmDel] = useState<string | null>(null);

  const latestId = extractions[0]?.id ?? null;

  /** Point the canvas at a revision. Deliberately NOT what a click does any more —
   *  only Enter, double-click and the explicit menu item get here. */
  const openVersion = (id: string) => {
    select(id);
    void setActiveExtraction(id === latestId ? null : id);
    exitHistory(); // if we're in the workspace, go look at the board we just loaded
  };

  /** Start a comparison. Leaves the workspace for the board — the diff renders there. */
  const startCompare = (a: string, b: string) => {
    setCompareFrom(null);
    exitHistory();
    void enterDiff(a, b);
  };

  const startPublish = (id: string) => {
    setChangelog("");
    setPublishing(id);
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
    if (!id || !msg) return; // blank changelog is not allowed
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

  /** True while any dialog/editor is up — the surfaces suppress their keyboard walk so
   *  ↑/↓ don't move the cursor behind an open modal. */
  const busy = !!(publishing || confirmDel || editing || tagging);

  /** Esc unwinds one level: dialog → editor → compare pick-mode. Returns true if it
   *  consumed the key, so the surface knows whether to handle Esc itself. */
  function escape(): boolean {
    if (confirmDel) {
      setConfirmDel(null);
    } else if (publishing) {
      setPublishing(null);
    } else if (editing) {
      setEditing(null);
    } else if (tagging) {
      setTagging(null);
    } else if (useHistoryStore.getState().compareFrom) {
      setCompareFrom(null);
    } else {
      return false;
    }
    return true;
  }

  function menuFor(r: ExtractionMeta): MenuItem[] {
    const localOnly = r.is_checkpoint && !r.published;
    const parent = parentOf(r, extractions);
    const items: MenuItem[] = [
      { label: "Open this version", icon: <IconEye size={14} />, onClick: () => openVersion(r.id) },
      // The ONLY way a version reaches the KiCad files on disk — opening/viewing a
      // version never writes them. Disabled when the design folder is missing here.
      {
        label: "Update KiCad files to this version…",
        icon: <IconFolder size={14} />,
        disabled: designPathMissing,
        onClick: () => {
          exitHistory();
          void updateDesignFiles(r.id);
        },
      },
      { separator: true },
      // Compare (visual-diff §3). "Compare with…" enters pick mode — which no longer
      // spans a closing modal, so the reader can see what they're picking between.
      {
        label: "Compare with…",
        icon: <IconCompare size={14} />,
        badge: "Beta",
        onClick: () => {
          setEditing(null);
          setTagging(null);
          setCompareFrom(r.id);
        },
      },
      {
        label: "Compare with previous",
        icon: <IconCompare size={14} />,
        badge: "Beta",
        disabled: !parent,
        onClick: () => parent && startCompare(parent, r.id),
      },
      { separator: true },
      {
        label: "Rename…",
        icon: <IconEdit size={14} />,
        onClick: () => {
          setTagging(null);
          setDraft(r.label ?? "");
          setEditing(r.id);
        },
      },
      {
        label: "Add tag…",
        icon: <IconTag size={14} />,
        onClick: () => {
          setEditing(null);
          setTagDraft("");
          setTagging(r.id);
        },
      },
    ];
    for (const t of r.tags) {
      items.push({
        label: `Remove tag “${t}”`,
        icon: <IconClose size={14} />,
        onClick: () => void removeTag(t),
      });
    }
    if (localOnly) {
      items.push({ separator: true });
      items.push({
        label: "Publish…",
        icon: <IconSparkle size={14} />,
        onClick: () => startPublish(r.id),
      });
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
      items.push({
        label: "Delete permanently…",
        icon: <IconTrash size={14} />,
        onClick: () => setConfirmDel(r.id),
      });
    } else if (!r.hidden) {
      items.push({ label: "Delete", icon: <IconTrash size={14} />, onClick: () => void hide(r.id) });
    }
    items.push({
      label: "Copy revision id",
      icon: <IconCopy size={14} />,
      onClick: () => void navigator.clipboard?.writeText(r.id),
    });
    return items;
  }

  /** The inline rename / add-tag editor for a row, or null when that row isn't being
   *  edited. Rendered by the row in place of its normal content. */
  function inlineEditor(r: ExtractionMeta): ReactNode | null {
    if (editing === r.id) {
      return (
        <input
          className="rev-label-input"
          autoFocus
          value={draft}
          placeholder={formatLocalTime(r.created_at)}
          onClick={(e) => e.stopPropagation()}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => saveLabel(r.id)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") saveLabel(r.id);
            if (e.key === "Escape") setEditing(null);
          }}
        />
      );
    }
    if (tagging === r.id) {
      return (
        <input
          className="rev-label-input"
          autoFocus
          value={tagDraft}
          placeholder="tag name (e.g. fab-v1)…"
          onClick={(e) => e.stopPropagation()}
          onChange={(e) => setTagDraft(e.target.value)}
          onBlur={() => saveTag(r.id)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") saveTag(r.id);
            if (e.key === "Escape") setTagging(null);
          }}
        />
      );
    }
    return null;
  }

  const dialogs = (
    <>
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
                e.stopPropagation();
                if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) doPublish();
                if (e.key === "Escape") setPublishing(null);
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
    </>
  );

  return {
    menuFor,
    inlineEditor,
    dialogs,
    openVersion,
    startCompare,
    startPublish,
    escape,
    busy,
    isEditing: (id: string) => editing === id || tagging === id,
  };
}
