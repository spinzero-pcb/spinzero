import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCrunchStore } from "../../stores/crunchStore";
import { useDesignStore } from "../../stores/designStore";
import { useHistoryStore } from "../../stores/historyStore";
import { useProjectStore } from "../../stores/projectStore";
import { useReviewStore } from "../../stores/reviewStore";
import { useSelectionStore } from "../../stores/selectionStore";
import { useToastStore } from "../../stores/toastStore";
import { useDiffStore } from "../../stores/diffStore";
import { useViewStore } from "../../stores/viewStore";
import { RunReviewMenu } from "../review/RunReviewMenu";
import { ipc } from "../../lib/ipc";
import { formatLocalTime, formatRelative } from "../../lib/time";
import { IconComment, IconHistory, IconLocate } from "../icons";

// The status bar says only what is true right now.
//
// Removed deliberately (2026-08-24): the project name (it lives in the title bar, once),
// "Extract Now" (extraction is automatic; the manual re-run moved to File ▸ Re-extract
// design, and the failure state below is itself the retry), and the idle crunch status
// — "up to date", "extracted · 1.2s" and the rest were near-constants, and the version
// chip beside them already carries the timestamp. What survives is state that changes
// what the window means: which revision you are on, an extraction in flight, a failure.

/** Status-bar selection mirror (spec §3): `/CANH · Isolated · 6 pins · 2 sheets`. */
function SelectionMirror() {
  const selection = useSelectionStore((s) => s.selection);
  const indexes = useDesignStore((s) => s.indexes);
  if (!selection || !indexes) return null;
  let text = "";
  if (selection.kind === "net") {
    const n = indexes.nets[selection.ref];
    if (!n) return null;
    text = `${selection.ref} · ${n.class} · ${n.terminals.length} pins · ${n.sheets.length} sheet${n.sheets.length === 1 ? "" : "s"}`;
  } else if (selection.kind === "comp") {
    const c = indexes.components[selection.ref];
    if (!c) return null;
    text = [selection.ref, c.value, c.fp].filter(Boolean).join(" · ");
  } else {
    text = `${selection.ref.designator}.${selection.ref.pin}`;
  }
  return <span className="mono statusbar-sel">{text}</span>;
}

/** The single footer version-control affordance: a chip showing the active revision
 *  that opens the revision-history graph on click. The old separate dropdown + the
 *  standalone "History" button were merged into this one chip (feedback item 1) — every
 *  per-revision action (open / rename / tag / publish / hide / delete) lives in the graph. */
function VersionChip() {
  const extractions = useProjectStore((s) => s.extractions);
  const activeExtraction = useProjectStore((s) => s.activeExtraction);
  const diffActive = useDiffStore((s) => s.active);
  const diffA = useDiffStore((s) => s.doc?.a ?? null);
  const diffB = useDiffStore((s) => s.doc?.b ?? null);
  const exitDiff = useDiffStore((s) => s.exitDiff);
  const latestId = extractions[0]?.id ?? null;
  const shownId = activeExtraction ?? latestId;
  const shown = extractions.find((e) => e.id === shownId) ?? null;
  // While comparing, the chip reflects the diff pair and exits diff mode on click
  // (a fast off-ramp that doesn't require reaching for the banner ×).
  if (diffActive && diffA && diffB) {
    return (
      <button
        className="statusbar-btn comparing"
        title="Comparing revisions — click to exit comparison"
        onClick={exitDiff}
      >
        <IconHistory size={12} />
        <span className="mono">
          {diffA.label} → {diffB.label}
        </span>
        <span className="rev-old-tag">comparing</span>
      </button>
    );
  }
  if (!shownId && extractions.length === 0) return null;
  const isOld = activeExtraction != null && activeExtraction !== latestId;
  // Footer shows the active revision's DATE and its tag, if any (item 9) — not the
  // editable label/changelog, which lives in the history graph.
  const dateText = shown ? formatLocalTime(shown.created_at) : (shownId ?? "—");
  const tag = shown?.tags[0] ?? null;
  return (
    <button
      className={`statusbar-btn ${isOld ? "viewing-old" : ""}`}
      title={
        isOld
          ? "Reviewing an older version — your KiCad files are unchanged. To write this version to disk, right-click it in the history graph and choose “Update KiCad files”. Click to open the graph."
          : "Revision history — click to open the graph"
      }
      onClick={() => useHistoryStore.getState().openGraph()}
    >
      <IconHistory size={12} />
      <span className="mono">{dateText}</span>
      {tag && <span className="rev-tag-ref">{tag}</span>}
      {isOld && <span className="rev-old-tag">reviewing old version</span>}
    </button>
  );
}

/** Soft fork-awareness (version-control-plan.md §1): when a teammate crunched a
 *  revision recently, warn that a parallel edit may fork. Dismissible; reappears only
 *  for a newer heartbeat. Never blocks — there are no locks. */
function PresenceBanner() {
  const presence = useHistoryStore((s) => s.presence);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const latest = presence[0];
  if (!latest || dismissed === latest.last_seen) return null;
  return (
    <button
      className="statusbar-btn presence-banner"
      title="Someone else has been working on this board — coordinate to avoid a fork"
      onClick={() => setDismissed(latest.last_seen)}
    >
      ⚠️ {latest.user} crunched {formatRelative(latest.last_seen)} — you may fork ✕
    </button>
  );
}

/** Read-only banner shown when the project's design folder is missing on this
 *  machine: extractions still view, but no new crunch until it is re-linked. */
function MissingDesignBanner() {
  const relinkDesignPath = useProjectStore((s) => s.relinkDesignPath);
  async function relink() {
    const dir = await openDialog({ directory: true, title: "Locate the design folder" });
    if (typeof dir === "string") await relinkDesignPath(dir);
  }
  return (
    <button className="statusbar-btn missing-design" onClick={() => void relink()}>
      <IconLocate size={12} />
      Design folder not found · Re-link…
    </button>
  );
}

/** Ask the backend to re-extract. The crunch-event listener in App reports a failed
 *  extraction, but a rejected call never produces an event — so say so here rather than
 *  leaving a deliberate click with no answer at all. */
export async function reExtract(): Promise<void> {
  try {
    await ipc.crunchNow();
  } catch (e) {
    useToastStore.getState().push({
      kind: "error",
      key: "crunch-now",
      title: "Couldn’t start extraction",
      message: String(e),
    });
  }
}

/** Extraction state, only while it is worth saying. Idle and "succeeded" are silent:
 *  extraction is automatic and up-to-date is the normal condition, so announcing it
 *  permanently teaches the user to ignore the one line that matters when it fails. */
function CrunchStatus() {
  const { phase } = useCrunchStore();
  if (phase === "running") {
    return (
      <span className="statusbar-crunch">
        <span className="status-dot running" />
        extracting…
      </span>
    );
  }
  if (phase === "failed") {
    return (
      <button
        className="statusbar-btn crunch-failed"
        title="The last extraction failed — click to try again"
        onClick={() => void reExtract()}
      >
        <span className="status-dot failed" />
        extraction failed · retry
      </button>
    );
  }
  return null;
}

/** Open comments, beside the launcher — and the way to them. It is a control in both
 *  states, because the count is only half of what the user wants: clicking reveals the
 *  panel if it is collapsed and switches it to All, so the chip always lands you on
 *  the whole list rather than on whatever tab was last left showing. */
function CommentsChip() {
  const setFullscreen = useViewStore((s) => s.setFullscreen);
  const comments = useReviewStore((s) => s.comments);
  const setFilterStatus = useReviewStore((s) => s.setFilterStatus);
  const open = comments.filter((c) => c.status === "open").length;
  return (
    <button
      className="statusbar-btn"
      title="Show every comment in the panel"
      onClick={() => {
        setFullscreen(false);
        setFilterStatus("all");
      }}
    >
      <IconComment size={12} />
      {open} open comment{open === 1 ? "" : "s"}
    </button>
  );
}

export function StatusBar() {
  const project = useProjectStore((s) => s.project);
  const designPathMissing = useProjectStore((s) => s.designPathMissing);

  return (
    <footer className="status-bar">
      {project && <VersionChip />}
      {project && <PresenceBanner />}
      {designPathMissing ? <MissingDesignBanner /> : <CrunchStatus />}
      <span style={{ flex: 1 }} />
      <SelectionMirror />
      {project && <CommentsChip />}
      {project && <RunReviewMenu />}
    </footer>
  );
}
