import { useEffect, useState } from "react";
import { ActivityBar } from "./ui/shell/ActivityBar";
import { LeftPanel } from "./ui/shell/LeftPanel";
import { RightPanel } from "./ui/shell/RightPanel";
import { Home } from "./ui/shell/Home";
import { MenuBar } from "./ui/shell/MenuBar";
import { StatusBar } from "./ui/shell/StatusBar";
import { ThreadPopover } from "./ui/review/ThreadPopover";
import { CommentBridge } from "./ui/review/CommentBridge";
import { Canvas } from "./ui/canvas/Canvas";
import { DiffSchematicA } from "./ui/canvas/DiffSchematicA";
import { DiffBanner } from "./ui/diff/DiffBanner";
import { PcbGlView } from "./ui/pcb/PcbGlView";
import { BomTab } from "./ui/BomTab";
import { Palette } from "./ui/Palette";
import { PropertiesCard } from "./ui/PropertiesCard";
import { CheckoutConfirm } from "./ui/CheckoutConfirm";
import { HistoryView } from "./ui/history/HistoryView";
import { Toaster } from "./ui/Toaster";
import { KeyboardShortcuts, useShortcutsDialog } from "./ui/shell/KeyboardShortcuts";
import { useToastStore } from "./stores/toastStore";
import { useProjectStore } from "./stores/projectStore";
import { usePcbViewStore } from "./stores/pcbViewStore";
import { useCrunchStore } from "./stores/crunchStore";
import { useDesignStore } from "./stores/designStore";
import { useSelectionStore } from "./stores/selectionStore";
import { useNetClassStore } from "./stores/netClassStore";
import { useSettingsStore } from "./stores/settingsStore";
import { useViewStore, type MainView } from "./stores/viewStore";
import { useReviewStore } from "./stores/reviewStore";
import { useMeasureStore } from "./stores/measureStore";
import { useHistoryStore } from "./stores/historyStore";
import { useDiffStore } from "./stores/diffStore";
import { canvasRestore, measureNav, nav, pcbNav } from "./ui/canvas/navigator";
import { isTypingTarget, resolveKey } from "./lib/keymap";
import type { Selection } from "./lib/design";
import { ipc, onCrunchEvent } from "./lib/ipc";
import { checkForUpdates } from "./lib/updater";
import { IconFullscreen, IconFullscreenExit } from "./ui/icons";

/** Cross-probing into the PCB activates the copper layer that carries the
 *  selection (net: most routed length; component/pin: its board side). */
function activeLayerForSelection(sel: NonNullable<Selection>) {
  const pcb = useDesignStore.getState().pcbIndex;
  let layer: string | undefined;
  if (sel.kind === "net") {
    const info = pcb?.nets[sel.ref];
    if (info?.layers.length)
      layer = [...info.layers].sort(
        (a, b) => (info.lenByLayer[b] ?? 0) - (info.lenByLayer[a] ?? 0),
      )[0];
  } else {
    const dsg = sel.kind === "pin" ? sel.ref.designator : sel.ref;
    const side = pcb?.compSide[dsg];
    if (side) layer = side === "back" ? "B.Cu" : "F.Cu";
  }
  if (!layer) return;
  const pv = usePcbViewStore.getState();
  pv.showLayer(layer);
  pv.setActive(layer);
}

const VIEWS: { id: MainView; label: string }[] = [
  { id: "schematic", label: "Schematic" },
  { id: "pcb", label: "PCB" },
  // Version control is a peer view, not a modal: the DAG needs the full width to show
  // lanes and forks, and the reader leaves it the way they leave any other tab.
  { id: "history", label: "History" },
  // BOM tab hidden for the initial release — re-add when the BOM Rules Checker
  // lands. The BomTab component and the `view === "bom"` rendering below are left
  // intact so re-enabling is just restoring this entry.
  // { id: "bom", label: "BOM" },
];

export default function App() {
  const project = useProjectStore((s) => s.project);
  const init = useProjectStore((s) => s.init);
  const refreshIndex = useProjectStore((s) => s.refreshIndex);
  const applyCrunchEvent = useCrunchStore((s) => s.apply);
  const loadDesign = useDesignStore((s) => s.load);
  const designLoaded = useDesignStore((s) => s.loaded);
  const loadError = useDesignStore((s) => s.loadError);
  const pendingReload = useDesignStore((s) => s.pendingReload);
  const keymap = useSettingsStore((s) => s.keymap);
  const loadSettings = useSettingsStore((s) => s.load);
  const view = useViewStore((s) => s.view);
  const setView = useViewStore((s) => s.setView);
  const fullscreen = useViewStore((s) => s.fullscreen);
  const toggleFullscreen = useViewStore((s) => s.toggleFullscreen);
  const loadReviews = useReviewStore((s) => s.load);
  const diffActive = useDiffStore((s) => s.active);
  const [palette, setPalette] = useState<null | "search" | "commands">(null);

  useEffect(() => {
    // loadDesign must wait for init: it serves from the active extraction, and
    // init is what reopens the last project on the backend. Both loaders catch their
    // own errors; the .catch here guards a rejected init() (a backend startup hiccup)
    // so it can't permanently strand the app on "Preparing design…".
    void init()
      .catch((e) => void ipc.logError(`project init failed: ${String(e)}`))
      .finally(() => {
        loadDesign();
        loadReviews();
      });
    loadSettings();
    // Check GitHub Releases for a newer signed build (item 6). Fire-and-forget and
    // self-silencing: a no-op in the browser dev server or when offline.
    void checkForUpdates();
    const unlisten = onCrunchEvent((ev) => {
      applyCrunchEvent(ev);
      if (ev.kind === "failed") {
        // Make the failure impossible to miss — the Output panel + status bar are
        // easy to overlook, especially on a re-extraction with a board on screen.
        const tail = ev.stderr_tail?.trim().split("\n").pop()?.trim();
        useToastStore.getState().push({
          kind: "error",
          key: "crunch-failed",
          title: "Extraction failed",
          message: `Step “${ev.stage}”.${tail ? ` ${tail}` : ""}`,
          action: { label: "Retry", onClick: () => void ipc.crunchNow() },
        });
      }
      if (ev.kind === "succeeded" || ev.kind === "skipped") {
        refreshIndex();
        loadReviews(); // pick up teammates' events synced into reviews/
        void useHistoryStore.getState().refreshPresence(); // and their presence
      }
      if (ev.kind === "succeeded" || ev.kind === "skipped") {
        // D1: never yank the canvas mid-review — banner + explicit reload when a
        // design is on screen; load directly only on the very first crunch.
        // "skipped" (unchanged design, cache already crunched) must load too —
        // it is the normal first event when reopening a vault.
        const d = useDesignStore.getState();
        if (!d.loaded) loadDesign();
        else if (ev.kind === "succeeded") d.markPendingReload();
      }
    });
    return () => {
      // listen() rejects on a plain browser dev server / plugin failure — swallow so
      // neither the setup promise nor this cleanup leaks an unhandled rejection.
      void unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [init, loadSettings, refreshIndex, applyCrunchEvent, loadDesign, loadReviews]);

  // Soft presence/awareness: poll teammates' recent activity while a project is open
  // (60s — matches the "N min ago" granularity of the fork banner).
  useEffect(() => {
    if (!project) return;
    const refresh = useHistoryStore.getState().refreshPresence;
    void refresh();
    const t = setInterval(() => void refresh(), 60_000);
    return () => clearInterval(t);
  }, [project]);

  // App-level keymap (preset-divergent chords; Esc/Alt+arrows live in the canvas).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTypingTarget(e)) return;
      // A focused revision list owns its walk keys (↑/↓/Home/End/Enter/C/Esc) — without
      // this, C would arm comment mode while the reader is stepping through history.
      // Every other chord (palette, fit, X…) still works from inside the list.
      if (
        (e.target as HTMLElement | null)?.closest?.(".rev-list") &&
        ["ArrowUp", "ArrowDown", "Home", "End", "Enter", "Escape", "c", "C"].includes(e.key)
      ) {
        return;
      }
      // Phase 2: C arms comment mode (next object click opens the composer); Esc
      // cancels an open composer/thread or disarms — before the canvas Esc clears.
      const review = useReviewStore.getState();
      if ((e.key === "c" || e.key === "C") && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        useMeasureStore.getState().setActive(false); // measure ⇄ comment are exclusive
        review.arm(!review.armed);
        return;
      }
      // Esc while measuring: clear an in-progress measurement first, then exit the mode.
      if (e.key === "Escape" && useMeasureStore.getState().active) {
        e.preventDefault();
        if (!measureNav.escape()) useMeasureStore.getState().setActive(false);
        return;
      }
      // While measuring: Space anchors a fresh measurement at the cursor ("measure from
      // here"); Ctrl/Cmd+C copies a completed measurement's readout to the clipboard.
      if (useMeasureStore.getState().active) {
        if (e.key === " ") {
          e.preventDefault();
          measureNav.from();
          return;
        }
        if ((e.key === "c" || e.key === "C") && (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey) {
          if (measureNav.copy()) e.preventDefault();
          return;
        }
      }
      if (e.key === "Escape" && (review.armed || review.compose || review.openThreadId)) {
        e.preventDefault();
        review.arm(false);
        review.cancelCompose();
        review.openThread(null);
        return;
      }
      // Esc on the PCB view clears the transient net/selection (the schematic Canvas
      // has its own Esc; PCB is a sibling GL view with no keydown of its own). Pinned
      // highlights survive — they're deliberate, persistent marks.
      if (e.key === "Escape" && useViewStore.getState().view === "pcb") {
        const sel = useSelectionStore.getState();
        const nc = useNetClassStore.getState();
        if (sel.highlights.length || sel.selection || nc.selected.length) {
          e.preventDefault();
          sel.setHighlights([], "pcb");
          sel.setSelection(null, "pcb");
          nc.clear(); // also drops net-class isolation + restores the layer view
          return;
        }
      }
      // Esc leaves full screen once the view-local Esc handlers above have had their
      // turn (clearing a selection/measurement takes priority over exiting).
      if (e.key === "Escape" && useViewStore.getState().fullscreen) {
        e.preventDefault();
        useViewStore.getState().setFullscreen(false);
        return;
      }
      const action = resolveKey(e, keymap ?? "kicad");
      if (!action) return;
      const inSchematic = useViewStore.getState().view === "schematic";
      e.preventDefault();
      switch (action) {
        case "palette":
          setPalette("search");
          break;
        case "commands":
          setPalette("commands");
          break;
        case "fit":
          // Route Fit to whichever view is up (the hidden one has zero extent).
          if (inSchematic) nav.fitView();
          else if (useViewStore.getState().view === "pcb") pcbNav.fit();
          break;
        case "overview":
          if (inSchematic) nav.toggleOverview();
          break;
        case "zoomIn":
          if (inSchematic) nav.zoomBy(1.4);
          else if (useViewStore.getState().view === "pcb") pcbNav.zoomBy(1.4);
          break;
        case "zoomOut":
          if (inSchematic) nav.zoomBy(1 / 1.4);
          else if (useViewStore.getState().view === "pcb") pcbNav.zoomBy(1 / 1.4);
          break;
        case "measure":
          // PCB-only mode toggle; disarms comment mode (mutually exclusive).
          if (useViewStore.getState().view === "pcb") {
            useReviewStore.getState().arm(false);
            useMeasureStore.getState().toggle();
          }
          break;
        case "fullscreen":
          useViewStore.getState().toggleFullscreen();
          break;
        case "shortcuts":
          useShortcutsDialog.getState().setOpen(true);
          break;
        case "crossProbe": {
          // In diff mode, X toggles the FOCUSED change between its schematic and PCB
          // anchors (a both-anchored change is one entry — §5). Falls through to the
          // normal selection cross-probe when the change has only one anchor.
          const diff = useDiffStore.getState();
          if (diff.active && diff.focusedChangeId) {
            const change = diff.doc?.changes.find((c) => c.id === diff.focusedChangeId);
            const hasSch = !!change?.anchors.schematic;
            const hasPcb = !!change?.anchors.pcb;
            if (change && hasSch && hasPcb) {
              const { view: v } = useViewStore.getState();
              // Re-focus targeting the other canvas. focusChange prefers schematic, so
              // each direction uses the diff-owned landing for the side we want:
              // revealChangeOnPcb isolates the changed layer and frames the change's own
              // extent (pcbNav.reveal's net path would un-hide layers and frame the whole
              // net, undoing the isolation); focusChange re-runs the schematic landing.
              if (v === "schematic") {
                diff.revealChangeOnPcb(change);
              } else {
                setView("schematic");
                diff.focusChange(change.id); // re-runs schematic landing + tint
              }
              break;
            }
          }
          // X: schematic ↔ PCB with the same selection (WS8).
          const { view: cur } = useViewStore.getState();
          const sel = useSelectionStore.getState().selection;
          if (cur === "pcb") {
            setView("schematic");
            if (sel?.kind === "pin") {
              nav.goPin(sel.ref.designator, sel.ref.pin); // pad → pin (item 5)
            } else {
              const hl = useSelectionStore.getState().highlights;
              if (hl.length) nav.applySelection(hl);
              else if (sel?.kind === "net") nav.goNet(sel.ref);
              else if (sel?.kind === "comp") nav.goComp(sel.ref);
            }
          } else if (sel) {
            activeLayerForSelection(sel); // land with the right copper layer active
            // Zoom the PCB camera onto the selection (mirrors the PCB tab-switch); the reveal
            // is deferred inside the view until the canvas is visible and sized.
            const anchor =
              sel.kind === "net"
                ? { type: "net" as const, ref: sel.ref }
                : {
                    type: "component" as const,
                    ref: sel.kind === "pin" ? sel.ref.designator : sel.ref,
                  };
            pcbNav.reveal(anchor);
            setView("pcb");
          }
          break;
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [keymap, setView]);

  // Suppress the WebView's native context menu (Reload / Save image / Inspect …)
  // everywhere it isn't applicable — it has no place in a desktop tool. The canvas,
  // PCB view and review rows open their own ContextMenu where they have options;
  // editable fields keep the native menu so right-click copy/paste still works.
  useEffect(() => {
    const onCtx = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (t?.closest('input, textarea, [contenteditable]:not([contenteditable="false"])')) return;
      e.preventDefault();
    };
    window.addEventListener("contextmenu", onCtx);
    return () => window.removeEventListener("contextmenu", onCtx);
  }, []);

  async function reloadDesign() {
    canvasRestore.state = nav.getViewState();
    // Reset activeExtraction to null ("track latest") so the extraction picker
    // in the footer immediately reflects the new bundle after reload. Without this,
    // a pinned extraction ID (set on init from project.active_extraction) keeps
    // the footer showing the old extraction even after a successful re-extract.
    useProjectStore.setState({ activeExtraction: null });
    await Promise.all([loadDesign(), refreshIndex()]);
  }

  if (!project && !designLoaded) {
    return (
      <div className="app">
        <MenuBar />
        <ActivityBar />
        <div style={{ gridColumn: "2 / -1", gridRow: 2 }}>
          <Home />
        </div>
        <StatusBar />
        <Toaster />
        <KeyboardShortcuts />
      </div>
    );
  }

  return (
    <div
      className={`app ${view === "bom" || view === "history" ? "app--no-right" : ""} ${
        fullscreen ? "app--fullscreen" : ""
      }`}
    >
      <MenuBar />
      <ActivityBar />
      <aside className="side-panel">
        <LeftPanel />
      </aside>
      <main className="main-area">
        <div className="view-tabs">
          {VIEWS.map((v) => (
            <button
              key={v.id}
              className={`view-tab ${view === v.id ? "active" : ""}`}
              onClick={() => {
                // PCB → schematic continuity (item 6): hand the PCB-made selection
                // to the canvas so it lands on the net's first schematic home.
                if (v.id === "schematic" && view !== "schematic") {
                  const st = useSelectionStore.getState();
                  if (st.source === "pcb") {
                    // A pad selection carries its pin: land on the pin itself (item 5),
                    // like the X key does. Going through applySelection would drop the
                    // pin and land on the designator's default unit — the wrong symbol
                    // for a multi-unit part (U12.C for a pin that lives on U12.A).
                    if (st.selection?.kind === "pin")
                      nav.goPin(st.selection.ref.designator, st.selection.ref.pin);
                    else if (st.highlights.length) nav.applySelection(st.highlights);
                  }
                }
                // Schematic → PCB cross-probe (item 12): land the PCB camera on the
                // selected net/part (showing its copper first), mirroring the X key —
                // before this the camera stayed put on a tab switch.
                if (v.id === "pcb" && view !== "pcb") {
                  const sel = useSelectionStore.getState().selection;
                  if (sel) {
                    activeLayerForSelection(sel);
                    const anchor =
                      sel.kind === "net"
                        ? { type: "net" as const, ref: sel.ref }
                        : {
                            type: "component" as const,
                            ref: sel.kind === "pin" ? sel.ref.designator : sel.ref,
                          };
                    pcbNav.reveal(anchor);
                  }
                }
                setView(v.id);
              }}
            >
              {v.label}
            </button>
          ))}
          <button
            className={`view-fullscreen-btn ${fullscreen ? "on" : ""}`}
            title={fullscreen ? "Exit full screen (Esc)" : "Full screen"}
            aria-label={fullscreen ? "Exit full screen" : "Full screen"}
            aria-pressed={fullscreen}
            onClick={() => toggleFullscreen()}
          >
            {fullscreen ? <IconFullscreenExit size={16} /> : <IconFullscreen size={16} />}
          </button>
        </div>
        <div className={`canvas-area ${view === "pcb" ? "pcb" : ""} ${diffActive ? "diffing" : ""}`}>
          {/* Diff-mode banner (visual-diff §3): view-global — sits above whichever
              canvas (schematic / PCB) is up. Renders nothing outside diff mode. */}
          <DiffBanner />
          {/* History is browsable even when the design hasn't loaded — a failed or
              still-running import is exactly when "open an older version" matters, so it
              sits outside the designLoaded gate. */}
          {view === "history" ? (
            <HistoryView />
          ) : designLoaded ? (
            <>
              {/* Schematic and PCB both stay mounted across view switches so their
                  camera, history and highlights survive a round-trip (item 3). In diff
                  mode the schematic view becomes side-by-side: read-only A (left) | B. */}
              <div
                className={`view-fill ${diffActive ? "diff-side-by-side" : ""}`}
                style={{ display: view === "schematic" ? undefined : "none" }}
              >
                {diffActive && <DiffSchematicA />}
                <Canvas />
              </div>
              <div className="view-fill" style={{ display: view === "pcb" ? undefined : "none" }}>
                <PcbGlView visible={view === "pcb"} />
              </div>
              {view === "bom" && (
                <div className="view-fill">
                  <BomTab />
                </div>
              )}
              {view !== "bom" && <PropertiesCard />}
              {pendingReload && (
                <div className="reload-banner">
                  Design changed
                  <button className="btn-primary" onClick={() => void reloadDesign()}>
                    Reload
                  </button>
                </div>
              )}
              <ThreadPopover />
              <CommentBridge />
            </>
          ) : (
            <CrunchProgress loadError={loadError} />
          )}
        </div>
      </main>
      {view !== "bom" && view !== "history" && (
        <aside className="right-panel">
          <RightPanel />
        </aside>
      )}
      <StatusBar />
      {palette && (
        <Palette
          initial={palette === "commands" ? ">" : ""}
          onClose={() => setPalette(null)}
        />
      )}
      <CheckoutConfirm />
      <Toaster />
      <KeyboardShortcuts />
    </div>
  );
}

/** First-open placeholder (item 5): the shell + sidebar are already live, so show the
 *  crunch progress here instead of a blank "Waiting…". The bottom panel carries the
 *  full streamed log; the board loads automatically the moment the crunch finishes. */
function CrunchProgress({ loadError }: { loadError: string | null }) {
  const phase = useCrunchStore((s) => s.phase);
  const lastLine = useCrunchStore((s) => s.lines[s.lines.length - 1]);
  const error = useCrunchStore((s) => s.error);
  const failed = phase === "failed";
  // Show the actual failure here — there is no separate Output panel to send the user
  // to (batch2). `error` (stage + stderr tail) comes from the crunch event; `loadError`
  // is the design-load fallback.
  const detail = error?.stderrTail?.trim() || loadError || null;
  return (
    <div className="crunch-progress">
      <span className={`status-dot ${phase}`} />
      <div className="crunch-progress-text">
        <div className="crunch-progress-title">
          {failed ? "Import failed" : phase === "running" ? "Importing design…" : "Preparing design…"}
        </div>
        <div className="crunch-progress-sub">
          {failed
            ? error
              ? `Failed during “${error.stage}”.`
              : detail ?? "The import didn’t finish."
            : lastLine ?? "The first import can take ~30–60s — the board appears as soon as it’s ready."}
        </div>
        {failed && detail && <pre className="crunch-progress-err">{detail}</pre>}
        {failed && (
          <button className="btn-primary crunch-retry" onClick={() => void ipc.crunchNow()}>
            Retry import
          </button>
        )}
      </div>
    </div>
  );
}
