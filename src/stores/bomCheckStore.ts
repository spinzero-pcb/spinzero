import { create } from "zustand";
import { ipc } from "../lib/ipc";
import { severityCounts, type BomProfile, type CheckOutcome, type FindingsDoc } from "../lib/findings";
import { bomProfileForClass } from "../lib/projectClass";
import { useBomMappingStore } from "./bomMappingStore";
import { useProjectStore } from "./projectStore";
import { useReviewRunsStore } from "./reviewRunsStore";
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
  /** Depth the BOM review's setup sheet is set to. Remembered per project so a re-run
   *  is one click. (The end application, the sheet's other scope control, is NOT here:
   *  it lives in project.json — see `currentBomProfile`.) */
  depth: BomDepth;
  /** Project the above belongs to, so a stale result can never render for another. */
  projectDir: string | null;

  hydrate: (projectDir: string | null) => Promise<void>;
  setDepth: (depth: BomDepth) => void;
  run: () => Promise<void>;
  clearForSession: (id: string) => void;
  clear: () => void;
}

/** "quick" = the included deterministic rules; "detailed" = the paid service run. */
export type BomDepth = "quick" | "detailed";
const DEFAULT_DEPTH: BomDepth = "quick";
function isBomDepth(v: unknown): v is BomDepth {
  return v === "quick" || v === "detailed";
}

/** The rule profile the open project's end application resolves to. Derived, never
 *  stored: project.json's `class` is the single home for the end application, so
 *  there is no second copy of it here to fall out of step. */
export function currentBomProfile(): BomProfile {
  return bomProfileForClass(useProjectStore.getState().project?.class);
}

export const useBomCheckStore = create<BomCheckState>((set, get) => ({
  running: false,
  doc: null,
  summary: null,
  unmappedColumns: [],
  error: null,
  depth: DEFAULT_DEPTH,
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
    const ui = projectDir ? useSettingsStore.getState().projectUi[projectDir] : undefined;
    set({
      projectDir,
      depth: isBomDepth(ui?.bom_review_depth) ? ui.bom_review_depth : DEFAULT_DEPTH,
      doc: null,
      summary: null,
      unmappedColumns: [],
      sessionId: null,
      error: null,
    });
  },

  setDepth: (depth) => {
    set({ depth });
    const dir = get().projectDir ?? useProjectStore.getState().project?.project_dir;
    if (dir) void useSettingsStore.getState().setProjectUi(dir, { bom_review_depth: depth });
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
      .ensureApproved(currentBomProfile(), () => void get().run());
    if (!approved) {
      set({ running: false });
      return;
    }
    const dir = useProjectStore.getState().project?.project_dir ?? null;
    try {
      const out = await ipc.runBomCheck(currentBomProfile());
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
      // Stamp what this run read, so the launcher can say "ran 23 Aug" and go stale
      // only when the BOM itself moves. Cosmetic — never let it fail the run.
      void useReviewRunsStore
        .getState()
        .record("bom")
        .catch(() => {});
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
      depth: DEFAULT_DEPTH,
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
