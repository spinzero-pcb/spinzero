import { mockIPC } from "@tauri-apps/api/mocks";
import { currentBomProfile, useBomCheckStore, runMessage, runTitle } from "./bomCheckStore";
import { useProjectStore } from "./projectStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";
import type { CheckOutcome, FindingsDoc } from "../lib/findings";

function doc(partial: Partial<FindingsDoc> = {}): FindingsDoc {
  return {
    schema_version: "1.0",
    engine_version: "bom-rules/0.0.1",
    pipeline: "bom-rules",
    profile: "default",
    findings: [],
    bom_audit: [],
    stats: { item_count: 3, finding_count: 0, duration_ms: 2 },
    ...partial,
  };
}

function outcome(partial: Partial<CheckOutcome> = {}): CheckOutcome {
  return {
    findings: doc(),
    session_id: "s_1",
    filed: 0,
    reopened: 0,
    unchanged: 0,
    auto_resolved: 0,
    unmapped_columns: [],
    comments: [],
    ...partial,
  };
}

const finding = (severity: "Important" | "Observation", fingerprint: string) => ({
  id: "B01",
  section: "BOM · Integrity",
  severity,
  confidence: "Unvalidated" as const,
  rule_id: "bom.duplicate_refdes",
  title: "Duplicate reference designator: R1",
  anchors: [{ type: "bom_row" as const, refdes: ["R1"] }],
  fingerprint,
});

describe("bomCheckStore", () => {
  beforeEach(() => {
    useBomCheckStore.getState().clear();
    useToastStore.setState({ toasts: [] });
    useSettingsStore.setState({ loaded: true, projectUi: {} });
    useProjectStore.setState({ project: null });
  });

  it("runs the check with the selected profile and keeps the result", async () => {
    const calls: Array<{ cmd: string; args: unknown }> = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "run_bom_check")
        return outcome({
          findings: doc({ findings: [finding("Important", "abc123")] }),
          filed: 1,
        });
      if (cmd === "list_comments") return [];
      if (cmd === "list_review_sessions") return [{ id: "s_1", title: "BOM check 2026-08-21" }];
      if (cmd === "get_review_author") return "you";
      return undefined;
    });

    useProjectStore.setState({ project: { project_dir: "C:/p", class: "automotive" } as never });
    await useBomCheckStore.getState().run();

    expect(calls).toContainEqual({
      cmd: "run_bom_check",
      args: { profile: "automotive" },
    });
    const state = useBomCheckStore.getState();
    expect(state.running).toBe(false);
    expect(state.doc?.findings).toHaveLength(1);
    expect(state.sessionId).toBe("s_1");
    expect(state.error).toBeNull();
    // The run reports itself, so a check with no visible new comments still lands.
    expect(useToastStore.getState().toasts[0]?.title).toContain("1 issue found");
  });

  it("surfaces a backend failure as an error toast and leaves the tab usable", async () => {
    mockIPC((cmd) => {
      if (cmd === "run_bom_check") throw new Error("no BOM in crunch cache");
      return undefined;
    });

    await useBomCheckStore.getState().run();

    const state = useBomCheckStore.getState();
    expect(state.running).toBe(false);
    expect(state.doc).toBeNull();
    expect(state.error).toContain("no BOM in crunch cache");
    const toast = useToastStore.getState().toasts[0];
    expect(toast?.kind).toBe("error");
  });

  it("ignores a second run while one is in flight", async () => {
    let runs = 0;
    mockIPC((cmd) => {
      if (cmd === "run_bom_check") {
        runs += 1;
        return outcome();
      }
      if (cmd === "list_comments") return [];
      if (cmd === "list_review_sessions") return [];
      if (cmd === "get_review_author") return "you";
      return undefined;
    });

    const first = useBomCheckStore.getState().run();
    await useBomCheckStore.getState().run();
    await first;

    expect(runs).toBe(1);
  });

  it("derives the rule profile from the project's end application", () => {
    // The end application is asked once, at import, and stored once, in project.json.
    // Nothing here holds a second copy that could disagree with it.
    useProjectStore.setState({ project: { project_dir: "C:/p", class: "medical" } as never });
    expect(currentBomProfile()).toBe("medical");

    // Classes with no rule set of their own fold onto the nearest one that exists.
    useProjectStore.setState({ project: { project_dir: "C:/p", class: "space" } as never });
    expect(currentBomProfile()).toBe("industrial");

    // A hand-edited project.json must not put an unknown profile in front of the
    // rules. It folds onto `commercial`, NOT onto `default` — `default` now means
    // "nobody said" and runs the strictest rules, and a project.json with a typo in
    // it is not the same thing as an unanswered question. See `bomProfileForClass`.
    useProjectStore.setState({ project: { project_dir: "C:/p", class: "nonsense" } as never });
    expect(currentBomProfile()).toBe("commercial");

    // The general/hobby class and an explicitly commercial one are one rule set, and
    // neither is the unstated profile.
    useProjectStore.setState({ project: { project_dir: "C:/p", class: "general" } as never });
    expect(currentBomProfile()).toBe("commercial");
    useProjectStore.setState({ project: { project_dir: "C:/p", class: "commercial" } as never });
    expect(currentBomProfile()).toBe("commercial");
  });

  it("summarizes a run for the toast", () => {
    const out = outcome({
      findings: doc({
        findings: [finding("Important", "a"), finding("Observation", "b")],
      }),
      filed: 1,
      auto_resolved: 2,
      unmapped_columns: ["House Code"],
    });
    expect(runTitle(out)).toBe("BOM check: 2 issues found");
    const msg = runMessage(out);
    expect(msg).toContain("1 important");
    expect(msg).toContain("1 new comment");
    expect(msg).toContain("2 auto-resolved");
    expect(msg).toContain("House Code");
    expect(runTitle(outcome())).toBe("BOM check: no issues found");
  });
});
