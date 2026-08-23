import { create } from "zustand";
import { ipc } from "../lib/ipc";
import {
  isBomProfile,
  severityCounts,
  type BomProfile,
  type CheckOutcome,
  type FindingsDoc,
} from "../lib/findings";
import { useBomMappingStore } from "./bomMappingStore";
import { useProjectStore } from "./projectStore";
import { useReviewStore } from "./reviewStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";

// BOM check (free tier) — deterministic rules run in-process by the Rust `bom-rules`
// crate. The backend does the work AND files the findings as review comments, so this
// store owns only: which profile to run, whether a run is in flight, and the last
// run's document (what the BOM tab's summary strip renders).
//
// The findings themselves are NOT state we own — they live as review comments in the
// project folder, which is what survives a restart and syncs to the team. This store
// deliberately keeps nothing that would be wrong after someone else's edit lands.

interface BomCheckState {
  running: boolean;
  /** Last run's document, for the open project only. */
  doc: FindingsDoc | null;
  /** Ingestion counts from the last run — "3 new, 1 resolved". */
  summary: Pick<CheckOutcome, "filed" | "reopened" | "unchanged" | "auto_resolved"> | null;
  /** Well-filled BOM columns the checker could not map — shown as a caveat. */
  unmappedColumns: string[];
  /** Review session the last run filed into, so the summary can jump to it. */
  sessionId: string | null;
  error: string | null;
  profile: BomProfile;
  /** Project the above belongs to, so a stale result can never render for another. */
  projectDir: string | null;

  hydrate: (projectDir: string | null) => Promise<void>;
  setProfile: (profile: BomProfile) => void;
  run: () => Promise<void>;
  clearForSession: (id: string) => void;
  clear: () => void;
}

const DEFAULT_PROFILE: BomProfile = "default";

export const useBomCheckStore = create<BomCheckState>((set, get) => ({
  running: false,
  doc: null,
  summary: null,
  unmappedColumns: [],
  error: null,
  profile: DEFAULT_PROFILE,
  projectDir: null,
  sessionId: null,

  hydrate: async (projectDir) => {
    // Settings may not have been read yet on a startup that restores a project.
    const settings = useSettingsStore.getState();
    if (!settings.loaded) {
      try {
        await settings.load();
      } catch {
        /* fall back to the default profile below */
      }
    }
    const remembered = projectDir
      ? useSettingsStore.getState().projectUi[projectDir]?.bom_check_profile
      : undefined;
    set({
      projectDir,
      profile: isBomProfile(remembered) ? remembered : DEFAULT_PROFILE,
      doc: null,
      summary: null,
      unmappedColumns: [],
      sessionId: null,
      error: null,
    });
  },

  setProfile: (profile) => {
    set({ profile });
    const dir = get().projectDir ?? useProjectStore.getState().project?.project_dir;
    // The end application is a property of the board, so it is remembered per project.
    if (dir) void useSettingsStore.getState().setProjectUi(dir, { bom_check_profile: profile });
  },

  run: async () => {
    if (get().running) return;
    // Claim the run before the first await: the mapping gate below is asynchronous,
    // and without this a second click would sail past the guard while it is pending
    // and file the whole check twice.
    set({ running: true, error: null });
    // A review is only as good as the column mapping it read. If this project has
    // never had one approved, the dialog takes over and re-enters here once it has.
    const approved = await useBomMappingStore
      .getState()
      .ensureApproved(get().profile, () => void get().run());
    if (!approved) {
      set({ running: false });
      return;
    }
    const dir = useProjectStore.getState().project?.project_dir ?? null;
    try {
      const out = await ipc.runBomCheck(get().profile);
      set({
        running: false,
        projectDir: dir,
        doc: out.findings,
        summary: {
          filed: out.filed,
          reopened: out.reopened,
          unchanged: out.unchanged,
          auto_resolved: out.auto_resolved,
        },
        unmappedColumns: out.unmapped_columns,
        sessionId: out.session_id,
        error: null,
      });
      // The backend created a session and filed comments; reload the rail so the new
      // comments, their session, and the row chips all appear together, then make that
      // session the active one — the run's findings are what the user asked to see.
      await useReviewStore.getState().load();
      if (out.session_id) useReviewStore.getState().setActiveSession(out.session_id);
      useToastStore.getState().push({
        kind: out.findings.findings.length === 0 ? "success" : "info",
        title: runTitle(out),
        message: runMessage(out),
      });
    } catch (e) {
      // The check is an optional aid: a failure must leave the BOM tab usable.
      set({ running: false, error: String(e) });
      useToastStore.getState().push({
        kind: "error",
        title: "BOM check failed",
        message: String(e),
      });
    }
  },

  /** Drop the last run's summary when its session is deleted — the counts in the
   *  strip describe comments that no longer exist. */
  clearForSession: (id) => {
    if (get().sessionId === id) {
      set({ doc: null, summary: null, unmappedColumns: [], sessionId: null, error: null });
    }
  },

  clear: () =>
    set({
      running: false,
      doc: null,
      summary: null,
      unmappedColumns: [],
      sessionId: null,
      error: null,
      projectDir: null,
      profile: DEFAULT_PROFILE,
    }),
}));

/** "No issues found" / "7 issues found" — the headline of the completion toast. */
export function runTitle(out: CheckOutcome): string {
  const n = out.findings.findings.length;
  if (n === 0) return "BOM check: no issues found";
  return `BOM check: ${n} issue${n === 1 ? "" : "s"} found`;
}

/** What changed since the last run, plus the severity mix — so a re-run that found
 *  nothing new reads differently from a first run. */
export function runMessage(out: CheckOutcome): string {
  const parts: string[] = [];
  const mix = severityCounts(out.findings)
    .map((s) => `${s.n} ${s.severity.toLowerCase()}`)
    .join(", ");
  if (mix) parts.push(mix);
  if (out.filed) parts.push(`${out.filed} new comment${out.filed === 1 ? "" : "s"}`);
  if (out.reopened) parts.push(`${out.reopened} reopened`);
  if (out.auto_resolved) parts.push(`${out.auto_resolved} auto-resolved`);
  if (out.unmapped_columns.length)
    parts.push(`unmapped column(s): ${out.unmapped_columns.slice(0, 3).join(", ")}`);
  return parts.join(" · ");
}
