import { mockIPC } from "@tauri-apps/api/mocks";
import { reviewRows, useReviewRunsStore } from "./reviewRunsStore";
import { useProjectStore } from "./projectStore";
import { useSettingsStore } from "./settingsStore";
import { reviewKind } from "../lib/reviewCatalog";
import type { BomLine } from "../lib/types";

const DIR = "C:/p";
const bom = reviewKind("bom")!;

function line(mpn: string): BomLine {
  return {
    item: 1,
    qty: 2,
    designators: ["R1", "R2"],
    value: "10k",
    footprint: "0402",
    mpn,
    dnp: false,
    fields: {},
  };
}

/** Backend stub: `get_bom_lines` answers with `lines`, `set_settings` records writes. */
function stubIpc(lines: BomLine[] | Error, saved: { last: unknown }) {
  mockIPC(async (cmd, args) => {
    if (cmd === "get_bom_lines") {
      if (lines instanceof Error) throw lines;
      return lines;
    }
    if (cmd === "get_settings") return { project_ui: {} };
    if (cmd === "set_settings") {
      saved.last = (args as { settings: unknown }).settings;
      return null;
    }
    return null;
  });
}

beforeEach(() => {
  useReviewRunsStore.getState().clear();
  useSettingsStore.setState({ projectUi: {}, loaded: true });
  useProjectStore.setState({
    project: { project_dir: DIR, name: "p" } as never,
    activeExtraction: "e9",
  });
});

describe("hydrate", () => {
  it("reads the project's run records", async () => {
    stubIpc([], { last: null });
    useSettingsStore.setState({
      projectUi: { [DIR]: { review_runs: { bom: { ts: "2026-08-23T00:00:00Z", inputs: { bom: "aaa" } } } } },
      loaded: true,
    });
    await useReviewRunsStore.getState().hydrate(DIR);
    expect(useReviewRunsStore.getState().runs.bom?.ts).toBe("2026-08-23T00:00:00Z");
  });

  it("treats an unreadable record as never run rather than throwing", async () => {
    stubIpc([], { last: null });
    useSettingsStore.setState({ projectUi: { [DIR]: { review_runs: "corrupt" } }, loaded: true });
    await useReviewRunsStore.getState().hydrate(DIR);
    expect(useReviewRunsStore.getState().runs).toEqual({});
  });

  it("clears the outgoing project's records", async () => {
    stubIpc([], { last: null });
    useReviewRunsStore.setState({ runs: { bom: { ts: "t" } }, current: { bom: "x" } });
    useReviewRunsStore.getState().clear();
    expect(useReviewRunsStore.getState().runs).toEqual({});
    expect(useReviewRunsStore.getState().current).toEqual({});
  });
});

describe("record", () => {
  it("stamps the inputs the run read and persists them", async () => {
    const saved = { last: null as unknown };
    stubIpc([line("MPN-1")], saved);
    await useReviewRunsStore.getState().record("bom");

    const run = useReviewRunsStore.getState().runs.bom!;
    expect(run.ts).toBeTruthy();
    expect(run.extraction_id).toBe("e9");
    expect(run.inputs?.bom).toBeTruthy();

    const written = (saved.last as { project_ui: Record<string, { review_runs: unknown }> }).project_ui[DIR];
    expect(written.review_runs).toMatchObject({ bom: { extraction_id: "e9" } });
  });

  it("records a run even when the BOM cannot be read", async () => {
    // No digest is not a failed run — the findings are already filed either way.
    stubIpc(new Error("no extraction"), { last: null });
    await useReviewRunsStore.getState().record("bom");
    expect(useReviewRunsStore.getState().runs.bom?.inputs).toEqual({});
  });
});

describe("staleness", () => {
  it("is false right after a run, true once the BOM moves", async () => {
    const saved = { last: null as unknown };
    stubIpc([line("MPN-1")], saved);
    await useReviewRunsStore.getState().record("bom");
    await useReviewRunsStore.getState().refreshInputs();
    expect(useReviewRunsStore.getState().stale(bom)).toBe(false);

    stubIpc([line("MPN-2")], saved); // someone changed a part number
    await useReviewRunsStore.getState().refreshInputs();
    expect(useReviewRunsStore.getState().stale(bom)).toBe(true);
  });

  it("stays false while the BOM is unreadable", async () => {
    const saved = { last: null as unknown };
    stubIpc([line("MPN-1")], saved);
    await useReviewRunsStore.getState().record("bom");
    stubIpc(new Error("mid-extraction"), saved);
    await useReviewRunsStore.getState().refreshInputs();
    expect(useReviewRunsStore.getState().stale(bom)).toBe(false);
  });
});

describe("reviewRows", () => {
  it("returns one row per catalogue entry, resolved", () => {
    const rows = reviewRows({ bom: { ts: "t", inputs: { bom: "aaa" } } }, { bom: "bbb" });
    const bomRow = rows.find((r) => r.kind.id === "bom")!;
    expect(bomRow.stale).toBe(true);
    expect(rows.find((r) => r.kind.id === "layout")!.run).toBeUndefined();
    expect(rows.every((r) => typeof r.stale === "boolean")).toBe(true);
  });
});
