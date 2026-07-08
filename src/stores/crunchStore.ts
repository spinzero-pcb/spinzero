import { create } from "zustand";
import type { CrunchEvent, CrunchPhase, CrunchTrigger } from "../lib/types";

const MAX_LINES = 300;

interface CrunchState {
  phase: CrunchPhase;
  trigger: CrunchTrigger | null;
  lines: string[];
  artifacts: string[];
  error: { stage: string; stderrTail: string } | null;
  lastRevisionId: string | null;
  lastCrunchMs: number | null;
  lastFinishedTs: string | null;
  skipReason: string | null;
  apply: (ev: CrunchEvent) => void;
}

export const useCrunchStore = create<CrunchState>((set) => ({
  phase: "idle",
  trigger: null,
  lines: [],
  artifacts: [],
  error: null,
  lastRevisionId: null,
  lastCrunchMs: null,
  lastFinishedTs: null,
  skipReason: null,
  apply: (ev) =>
    set((s) => {
      switch (ev.kind) {
        case "started":
          return {
            phase: "running" as const,
            trigger: ev.trigger,
            lines: [],
            artifacts: [],
            error: null,
            skipReason: null,
          };
        case "progress":
          return { lines: [...s.lines.slice(-MAX_LINES + 1), ev.line] };
        case "artifact":
          return { artifacts: [...s.artifacts, ev.path] };
        case "succeeded":
          return {
            phase: "succeeded" as const,
            lastRevisionId: ev.revision_id,
            lastCrunchMs: ev.crunch_ms,
            lastFinishedTs: new Date().toISOString(),
          };
        case "failed":
          return {
            phase: "failed" as const,
            error: { stage: ev.stage, stderrTail: ev.stderr_tail },
            lastFinishedTs: new Date().toISOString(),
          };
        case "skipped":
          return { phase: "skipped" as const, skipReason: ev.reason };
      }
    }),
}));
