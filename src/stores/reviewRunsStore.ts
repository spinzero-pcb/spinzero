import { create } from "zustand";
import { ipc } from "../lib/ipc";
import {
  digestOf,
  isStale,
  REVIEW_KINDS,
  sanitizeRuns,
  type ReviewInput,
  type ReviewKind,
  type ReviewKindId,
  type ReviewRun,
} from "../lib/reviewCatalog";
import { useProjectStore } from "./projectStore";
import { useSettingsStore } from "./settingsStore";

// What each review has been run against, and whether that is still true.
//
// Two things live here and nowhere else:
//
// * **Run records** — one per review kind, persisted per project so "ran 23 Aug"
//   survives a restart. The findings themselves are review comments in the project
//   folder (see bomCheckStore); this store keeps only what the launcher needs to
//   describe the run, which is why it is safe for it to be machine-local.
// * **Input digests** — a fingerprint of what each review READ, taken at run time
//   and again when the launcher opens. Comparing them is what makes staleness
//   per-review: a schematic edit moves the `schematic` digest and leaves `bom`
//   alone, so a datasheet check does not go stale because a wire moved.
//
// An input we cannot digest yet is simply absent, and `isStale` ignores absent
// inputs — the launcher never claims staleness it cannot evidence.

interface ReviewRunsState {
  /** Project the records below belong to; guards a stale render after a switch. */
  projectDir: string | null;
  runs: Partial<Record<ReviewKindId, ReviewRun>>;
  /** Digest of each input as of the last `refreshInputs()`. */
  current: Partial<Record<ReviewInput, string>>;

  hydrate: (projectDir: string | null) => Promise<void>;
  /** Re-read the digestable inputs. Cheap, but does hit IPC — call it when the
   *  launcher opens, not on every render. */
  refreshInputs: () => Promise<void>;
  /** Record a completed run of `id`, stamping the inputs it read. */
  record: (id: ReviewKindId) => Promise<void>;
  stale: (kind: ReviewKind) => boolean;
  clear: () => void;
}

/** Fingerprint of the BOM as the checks see it: the identifying fields of every
 *  line. Deliberately not the whole row — a re-ordered column or a changed sheet
 *  path is not a BOM change, and a false "stale" is worse than a missed one. */
async function bomDigest(): Promise<string | null> {
  try {
    const lines = await ipc.getBomLines();
    return digestOf(
      lines.map(
        (l) => `${l.designators.join(",")}|${l.value}|${l.footprint}|${l.mpn}|${l.qty}|${l.dnp ? 1 : 0}`,
      ),
    );
  } catch {
    // No BOM yet, or the extraction is mid-flight. Unknown, not changed.
    return null;
  }
}

export const useReviewRunsStore = create<ReviewRunsState>((set, get) => ({
  projectDir: null,
  runs: {},
  current: {},

  hydrate: async (projectDir) => {
    const settings = useSettingsStore.getState();
    if (!settings.loaded) {
      try {
        await settings.load();
      } catch {
        /* fall through to empty records — an unreadable settings file is not fatal */
      }
    }
    const stored = projectDir
      ? useSettingsStore.getState().projectUi[projectDir]?.review_runs
      : undefined;
    set({ projectDir, runs: sanitizeRuns(stored), current: {} });
  },

  refreshInputs: async () => {
    const bom = await bomDigest();
    const current: Partial<Record<ReviewInput, string>> = {};
    if (bom) current.bom = bom;
    // datasheets / schematic / pcb have no digest yet — absent means "unknown",
    // which isStale() treats as "no evidence of change".
    set({ current });
  },

  record: async (id) => {
    const dir = useProjectStore.getState().project?.project_dir ?? null;
    const bom = await bomDigest();
    const inputs: Partial<Record<ReviewInput, string>> = {};
    if (bom) inputs.bom = bom;
    const run: ReviewRun = {
      ts: new Date().toISOString(),
      extraction_id: useProjectStore.getState().activeExtraction ?? null,
      inputs,
    };
    const runs = { ...get().runs, [id]: run };
    set({ runs, current: { ...get().current, ...inputs } });
    if (dir) {
      try {
        await useSettingsStore.getState().setProjectUi(dir, { review_runs: runs });
      } catch {
        // Losing the "ran 23 Aug" line is cosmetic; the findings are already filed.
      }
    }
  },

  stale: (kind) => isStale(kind, get().runs[kind.id], get().current),

  clear: () => set({ projectDir: null, runs: {}, current: {} }),
}));

/** Every review kind with its run state resolved — what the launcher renders. */
export interface ReviewRow {
  kind: ReviewKind;
  run: ReviewRun | undefined;
  stale: boolean;
}

export function reviewRows(
  runs: Partial<Record<ReviewKindId, ReviewRun>>,
  current: Partial<Record<ReviewInput, string>>,
): ReviewRow[] {
  return REVIEW_KINDS.map((kind) => ({
    kind,
    run: runs[kind.id],
    stale: isStale(kind, runs[kind.id], current),
  }));
}
