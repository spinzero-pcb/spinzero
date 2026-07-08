import { create } from "zustand";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ipc } from "../lib/ipc";
import { useProjectStore } from "./projectStore";
import { useToastStore } from "./toastStore";

// Shell-level UI state for the project entry points (New Project wizard + the
// Open Project flow), shared by the Home screen and the File menu so both can
// drive the same dialogs without duplicating logic.
interface ShellState {
  wizardOpen: boolean;
  /** Design folder to pre-fill the wizard with (e.g. when Open Project was
   *  pointed at a raw KiCad folder); null = start empty. */
  wizardInitialFolder: string | null;
  err: string | null;
  openWizard: (initialFolder?: string) => void;
  closeWizard: () => void;
  setErr: (e: string | null) => void;
  /** Pick a folder, classify it, then open / start a new project / report —
   *  never fail silently. */
  openExisting: () => Promise<void>;
}

export const useShellStore = create<ShellState>((set) => ({
  wizardOpen: false,
  wizardInitialFolder: null,
  err: null,
  openWizard: (initialFolder) =>
    set({ wizardOpen: true, wizardInitialFolder: initialFolder ?? null, err: null }),
  closeWizard: () => set({ wizardOpen: false, wizardInitialFolder: null }),
  setErr: (err) => set({ err }),

  openExisting: async () => {
    set({ err: null });
    const dir = await openDialog({ directory: true, title: "Open project folder" });
    if (typeof dir !== "string") return;
    let kind: string;
    try {
      kind = await ipc.inspectFolder(dir);
    } catch (e) {
      set({ err: String(e) });
      useToastStore.getState().push({
        kind: "error",
        title: "Couldn’t open folder",
        message: String(e),
      });
      return;
    }
    if (kind === "project") {
      await useProjectStore.getState().openProject(dir).catch(() => {});
    } else if (kind === "design") {
      // A raw KiCad design folder — jump straight into New Project with it
      // pre-filled instead of telling the user "not a project" (the old silent
      // failure on KiCad demo folders like complex_hierarchy).
      set({ wizardOpen: true, wizardInitialFolder: dir, err: null });
    } else {
      const msg =
        "That folder isn’t a SpinZero project or a KiCad design. Pick the folder that contains your .kicad_pro file.";
      set({ err: msg });
      useToastStore.getState().push({ kind: "error", title: "Nothing to open here", message: msg });
    }
  },
}));
