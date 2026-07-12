import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCrunchStore } from "../../stores/crunchStore";
import { useDesignStore } from "../../stores/designStore";
import { useHistoryStore } from "../../stores/historyStore";
import { useProjectStore } from "../../stores/projectStore";
import { useSelectionStore } from "../../stores/selectionStore";
import { useDiffStore } from "../../stores/diffStore";
import { ipc } from "../../lib/ipc";
import { formatLocalTime, formatRelative } from "../../lib/time";
import { IconHistory, IconLocate, IconRefresh } from "../icons";

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
  const diffA = useDiffStore((s) => s.a);
  const diffB = useDiffStore((s) => s.b);
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

const PHASE_TEXT: Record<string, string> = {
  idle: "idle",
  running: "extracting…",
  succeeded: "extracted",
  failed: "extraction failed",
  skipped: "up to date",
};

export function StatusBar() {
  const summary = useProjectStore((s) => s.summary);
  const project = useProjectStore((s) => s.project);
  const designPathMissing = useProjectStore((s) => s.designPathMissing);
  const { phase, lastCrunchMs, lastFinishedTs } = useCrunchStore();

  const finished = lastFinishedTs
    ? new Date(lastFinishedTs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : null;

  return (
    <footer className="status-bar">
      <span>{summary?.name ?? project?.name ?? "no project open"}</span>
      {project && <VersionChip />}
      {project && <PresenceBanner />}
      {designPathMissing ? (
        <MissingDesignBanner />
      ) : (
        <span>
          <span className={`status-dot ${phase}`} />
          {PHASE_TEXT[phase] ?? phase}
          {phase === "succeeded" && lastCrunchMs != null && ` · ${(lastCrunchMs / 1000).toFixed(1)}s`}
          {finished && ` · ${finished}`}
        </span>
      )}
      {project && !designPathMissing && (
        <button
          className="btn-ghost statusbar-btn"
          onClick={() => void ipc.crunchNow().catch(() => {})}
          disabled={phase === "running"}
          title="Re-extract the design from source files"
        >
          <IconRefresh size={12} />
          Extract Now
        </button>
      )}
      <span style={{ flex: 1 }} />
      <SelectionMirror />
    </footer>
  );
}
