import { create } from "zustand";
import { ipc } from "../lib/ipc";
import { useDesignStore } from "./designStore";
import { useBomCheckStore } from "./bomCheckStore";
import { useReviewStore } from "./reviewStore";
import { useSelectionStore } from "./selectionStore";
import { useNetClassStore } from "./netClassStore";
import { useViewStore } from "./viewStore";
import { useToastStore, type ToastInput } from "./toastStore";
import {
  normalizeExtraction,
  type ExtractionMeta,
  type LayerInfo,
  type ProjectInfo,
  type ProjectSummary,
  type SheetInfo,
} from "../lib/types";

interface ProjectState {
  project: ProjectInfo | null;
  recents: string[];
  summary: ProjectSummary | null;
  sheets: SheetInfo[];
  layers: LayerInfo[];
  extractions: ExtractionMeta[];
  /** The extraction shown in the viewer; null = latest/live. */
  activeExtraction: string | null;
  /** The revision the KiCad design folder currently matches (null = unknown) —
   *  shown as the "KiCad files" marker so viewing-vs-disk divergence is visible. */
  designHead: string | null;
  /** Read-only mode: the project's design folder is missing on this machine. */
  designPathMissing: boolean;
  busy: boolean;
  errorMsg: string | null;
  /** Pending confirmation for updateDesignFiles (set when the write would overwrite
   *  un-captured on-disk edits); the modal resolves the promise with the user's choice. */
  checkoutPrompt: { resolve: (ok: boolean) => void } | null;

  init: () => Promise<void>;
  openProject: (projectDir: string) => Promise<void>;
  createProject: (args: {
    name: string;
    designPath: string;
    projectDir: string;
    designTool?: string | null;
    class?: string | null;
  }) => Promise<void>;
  relinkDesignPath: (newDesignPath: string) => Promise<void>;
  /** Pin the viewer to a revision. Resolves `true` when the switch landed, `false` when
   *  it was skipped (a switch/update was already in flight) or the IPC failed — so a
   *  caller like the diff enter can detect a silent no-op instead of proceeding on the
   *  wrong revision. */
  setActiveExtraction: (id: string | null) => Promise<boolean>;
  updateDesignFiles: (id: string) => Promise<void>;
  labelExtraction: (id: string, label: string | null) => Promise<void>;
  setTag: (id: string, name: string, message?: string | null) => Promise<void>;
  removeTag: (name: string) => Promise<void>;
  hide: (id: string) => Promise<void>;
  unhide: (id: string) => Promise<void>;
  publish: (id: string, message: string) => Promise<void>;
  deleteCheckpoint: (id: string) => Promise<void>;
  refreshIndex: () => Promise<void>;
}

/** Clean-load: drop the previous project's design, selection and in-memory index
 *  rows so the new project loads from a blank slate (avoids the stale-`loaded`
 *  bug where the first crunch event for the new project is swallowed). */
function resetForNewProject() {
  useDesignStore.getState().clear();
  // Drop the outgoing project's net-class selection AND its colour picks (the incoming
  // project hydrates its own; reset() alone keeps colours, which is right for a redesign).
  useNetClassStore.getState().reset();
  useNetClassStore.setState({ projectDir: null, classColors: {}, netColors: {} });
  // Drop the previous project's review comments/sessions too — otherwise project A's
  // review panel bleeds into project B until the next crunch event happens to reload it.
  useReviewStore.getState().clear();
  // …and the last BOM check's summary, which describes the outgoing project's BOM.
  useBomCheckStore.getState().clear();
  const sel = useSelectionStore.getState();
  sel.setHighlights([], "sch");
  sel.setSelection(null, "sch");
  sel.setCurrentSheet(null);
  useSelectionStore.setState({ pinned: [] });
  // A checkout-confirm prompt may be awaiting the user when the project is torn down.
  // Resolve it false so the awaiting updateDesignFiles unwinds (and clears `busy`)
  // instead of stranding the promise — a stranded prompt wedges `busy` and blocks all
  // future revision switches.
  const pendingPrompt = useProjectStore.getState().checkoutPrompt;
  if (pendingPrompt) {
    pendingPrompt.resolve(false);
    useProjectStore.setState({ checkoutPrompt: null });
  }
}

/** Run a revision-list mutation, then refresh the extraction picker rows and
 *  (optionally) show a success toast. Any backend rejection becomes an error toast
 *  instead of an unhandled rejection / UI crash — the version-control mutations +
 *  labelExtraction all share this exact call/refresh/toast shape. */
async function refreshAfter(errTitle: string, run: () => Promise<unknown>, okToast?: ToastInput) {
  try {
    await run();
    useProjectStore.setState({
      extractions: (await ipc.listExtractions()).map(normalizeExtraction),
    });
    if (okToast) useToastStore.getState().push(okToast);
  } catch (e) {
    useToastStore.getState().push({ kind: "error", title: errTitle, message: String(e) });
  }
}

function adopt(project: ProjectInfo) {
  return {
    project,
    activeExtraction: project.active_extraction,
    designPathMissing: project.design_path != null && !project.design_path_exists,
  };
}

export const useProjectStore = create<ProjectState>((set, get) => ({
  project: null,
  recents: [],
  summary: null,
  sheets: [],
  layers: [],
  extractions: [],
  activeExtraction: null,
  designHead: null,
  designPathMissing: false,
  busy: false,
  errorMsg: null,
  checkoutPrompt: null,

  init: async () => {
    const [project, recents] = await Promise.all([
      ipc.getProject(),
      ipc.getRecentProjects(),
    ]);
    set({ recents });
    if (project) {
      set(adopt(project));
      await get().refreshIndex();
    } else if (recents.length > 0) {
      // Like VS Code: reopen the last project instead of the start page.
      try {
        await get().openProject(recents[0]);
      } catch {
        // Project gone or importer missing — fall back to the home screen.
      }
    }
  },

  openProject: async (projectDir) => {
    set({ busy: true, errorMsg: null });
    try {
      resetForNewProject();
      set({ summary: null, sheets: [], layers: [], extractions: [], designHead: null });
      const project = await ipc.openProject(projectDir);
      set(adopt(project));
      // A failed recents refresh must not report the (already-open) project as failed —
      // best-effort, separate from the open itself.
      try {
        set({ recents: await ipc.getRecentProjects() });
      } catch {
        /* ignore — recents refresh is non-critical */
      }
      // Previously-extracted projects have index rows + a bundle on disk — fill the
      // sidebar, canvas and review panel now rather than waiting on the hash-gated crunch.
      void get().refreshIndex();
      void useDesignStore.getState().load();
      void useReviewStore.getState().load();
      // Net-class/net colour picks are remembered per project (machine-local settings).
      void useNetClassStore.getState().hydrate(project.project_dir);
      // Same tier for the BOM preset / hidden columns / sort (project-defined ids).
      void useViewStore.getState().hydrateBom(project.project_dir);
      // BOM check profile: the end application belongs to the board, not the user.
      void useBomCheckStore.getState().hydrate(project.project_dir);
    } catch (e) {
      set({ errorMsg: String(e) });
      // Surface it everywhere (the Home error card is invisible once a project is
      // already open, e.g. opening a moved/deleted project from the File menu).
      useToastStore.getState().push({
        kind: "error",
        title: "Couldn’t open project",
        message: String(e),
      });
      // A missing project is pruned from recents backend-side on a failed open;
      // refresh so the dead entry leaves Open Recent immediately (best-effort).
      try {
        set({ recents: await ipc.getRecentProjects() });
      } catch {
        /* ignore — refresh is non-critical */
      }
      throw e;
    } finally {
      set({ busy: false });
    }
  },

  createProject: async (args) => {
    set({ busy: true, errorMsg: null });
    try {
      resetForNewProject();
      set({ summary: null, sheets: [], layers: [], extractions: [], designHead: null });
      const project = await ipc.createProject(args);
      set({ ...adopt(project), recents: await ipc.getRecentProjects() });
      // First extraction runs in the background; the design loads off the crunch
      // "succeeded" event. refreshIndex retries until rows appear.
      void get().refreshIndex();
      void useReviewStore.getState().load();
      void useNetClassStore.getState().hydrate(project.project_dir);
      void useViewStore.getState().hydrateBom(project.project_dir);
      // BOM check profile: the end application belongs to the board, not the user.
      void useBomCheckStore.getState().hydrate(project.project_dir);
    } catch (e) {
      set({ errorMsg: String(e) });
      throw e;
    } finally {
      set({ busy: false });
    }
  },

  relinkDesignPath: async (newDesignPath) => {
    set({ busy: true, errorMsg: null });
    try {
      const project = await ipc.relinkDesignPath(newDesignPath);
      set(adopt(project));
      void get().refreshIndex();
    } catch (e) {
      set({ errorMsg: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  // Pure viewer switch: the design folder on disk is never touched, so there is
  // nothing to confirm. Writing files back is the separate updateDesignFiles action.
  setActiveExtraction: async (id) => {
    if (get().busy) return false; // don't overlap another switch/update — caller sees the no-op
    set({ busy: true });
    try {
      await ipc.setActiveExtraction(id);
      // Switched: re-resolve highlights/selection against the newly-active bundle.
      set({ activeExtraction: id });
      const sel = useSelectionStore.getState();
      sel.setHighlights([], "sch");
      sel.setSelection(null, "sch");
      await Promise.all([get().refreshIndex(), useDesignStore.getState().load()]);
      return true;
    } catch (e) {
      useToastStore.getState().push({ kind: "error", title: "Couldn’t switch revision", message: String(e) });
      return false;
    } finally {
      set({ busy: false });
    }
  },

  // Explicitly write a revision back into the KiCad design folder (history graph →
  // "Update KiCad files…"). Un-captured on-disk edits raise the confirm dialog and are
  // captured as a local checkpoint before the overwrite. Also points the viewer at it.
  updateDesignFiles: async (id) => {
    if (get().busy) return; // a checkout is a slow disk write — don't overlap
    set({ busy: true });
    try {
      let res = await ipc.updateDesignFiles(id, false);
      if (res.status === "busy") {
        useToastStore.getState().push({
          kind: "warning",
          title: "Extraction in progress",
          message: "Try updating again in a moment.",
        });
        return;
      }
      if (res.status === "dirty") {
        // Un-captured on-disk edits would be overwritten — confirm first. Settle any stale
        // prompt before opening a new one, so a leftover promise can never strand `busy`.
        get().checkoutPrompt?.resolve(false);
        const ok = await new Promise<boolean>((resolve) => set({ checkoutPrompt: { resolve } }));
        set({ checkoutPrompt: null });
        if (!ok) return; // cancel: viewer + disk unchanged
        res = await ipc.updateDesignFiles(id, true);
        if (res.status !== "switched") {
          useToastStore.getState().push({ kind: "error", title: "Couldn’t update the KiCad files" });
          return;
        }
      }
      // Updated: the backend also pointed the viewer at this revision — mirror it and
      // re-resolve highlights/selection against the newly-active bundle.
      set({ activeExtraction: id });
      const sel = useSelectionStore.getState();
      sel.setHighlights([], "sch");
      sel.setSelection(null, "sch");
      useToastStore.getState().push({
        kind: "success",
        title: "KiCad files updated",
        message: "If the design is open in KiCad, close it and open it again to see this version.",
      });
      if (res.captured) {
        useToastStore.getState().push({
          kind: "info",
          title: "Saved your changes",
          message: "Your un-captured edits were kept as a local checkpoint.",
        });
      }
      await Promise.all([get().refreshIndex(), useDesignStore.getState().load()]);
    } catch (e) {
      useToastStore.getState().push({ kind: "error", title: "Couldn’t update the KiCad files", message: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  // Version-control mutations: call the backend, refresh just the picker list (these
  // don't change which bundle is active), and surface any error as a toast so a rejected
  // command never crashes the UI. All share refreshAfter (see its doc comment).
  labelExtraction: (id, label) =>
    refreshAfter("Couldn’t rename revision", () => ipc.labelExtraction(id, label)),
  setTag: (id, name, message) =>
    refreshAfter("Couldn’t tag revision", () => ipc.tagRevision(id, name, message ?? null)),
  removeTag: (name) => refreshAfter("Couldn’t remove tag", () => ipc.untagRevision(name)),
  hide: (id) => refreshAfter("Couldn’t delete revision", () => ipc.hideRevision(id)),
  unhide: (id) => refreshAfter("Couldn’t restore revision", () => ipc.unhideRevision(id)),
  publish: (id, message) =>
    refreshAfter("Couldn’t publish", () => ipc.publishCheckpoint(id, message), {
      kind: "success",
      title: "Published to shared history",
    }),
  deleteCheckpoint: (id) =>
    refreshAfter("Couldn’t delete checkpoint", () => ipc.deleteCheckpoint(id)),

  refreshIndex: async () => {
    // The index may still be rebuilding in the background right after open
    // (schema migration / first extraction) — retry until rows appear.
    const dir = get().project?.project_dir;
    for (let attempt = 0; attempt < 6; attempt++) {
      try {
        const [summary, sheets, layers, extractions, designHead] = await Promise.all([
          ipc.getProjectSummary(),
          ipc.listSheets(),
          ipc.listLayers(),
          ipc.listExtractions(),
          ipc.getDesignHead(),
        ]);
        // The user may have switched projects while we awaited; a stale iteration of this
        // loop (it can run up to ~10s) must not overwrite the new project's rows.
        if (get().project?.project_dir !== dir) return;
        set({ summary, sheets, layers, extractions: extractions.map(normalizeExtraction), designHead });
        if (summary && sheets.length > 0) {
          void useSelectionStore.getState().loadPinned(); // item 22: per-project highlights
          return;
        }
      } catch (e) {
        // An IPC rejected (index mid-migration / backend hiccup at startup). Don't abort
        // the poll or leak an unhandled rejection — fall through to the retry wait so the
        // loop keeps its "retry until rows appear" contract.
        if (get().project?.project_dir !== dir) return;
        void ipc.logWarn(`refreshIndex attempt ${attempt + 1} failed: ${String(e)}`);
      }
      await new Promise((r) => setTimeout(r, 2000));
    }
  },
}));
