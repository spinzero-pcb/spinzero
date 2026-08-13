import { mockIPC } from "@tauri-apps/api/mocks";
import { beforeEach, describe, expect, it } from "vitest";
import { useViewStore, sanitizeWidths } from "./viewStore";
import { useSettingsStore } from "./settingsStore";

// Layer-1 coverage for the BOM persistence tiers (docs/storage-model.md): preset /
// hidden / sort ride ProjectUi keyed by project dir, chips ride top-level settings,
// widths stay in localStorage, and a pre-move install migrates once.

const DIR_A = "/p/a";
const DIR_B = "/p/b";

/** Capture what reaches set_settings so the tier of each field is assertable. */
function captureIpc(): Array<Record<string, unknown>> {
  const written: Array<Record<string, unknown>> = [];
  mockIPC((cmd, args) => {
    if (cmd === "set_settings") {
      written.push((args as { settings: Record<string, unknown> }).settings);
      return undefined;
    }
    if (cmd === "get_settings") return null;
    return undefined;
  });
  return written;
}

beforeEach(() => {
  localStorage.clear();
  useSettingsStore.setState({ projectUi: {}, bomChips: null, diffBlink: null, loaded: true });
  useViewStore.setState({
    bomChips: { dnpOnly: false, missingMpn: false, changedOnly: true },
    bomLayout: { preset: null, hidden: {}, sort: null, widths: {} },
    bomProjectDir: null,
  });
});

describe("viewStore BOM persistence", () => {
  it("writes preset / hidden / sort to this project's ProjectUi, not a global field", async () => {
    const written = captureIpc();
    await useViewStore.getState().hydrateBom(DIR_A);

    useViewStore.getState().setBomPreset("Assembly");
    useViewStore.getState().toggleBomColumn("Assembly", "mpn");
    useViewStore.getState().setBomSort({ key: "value", dir: -1 });
    await Promise.resolve();

    const last = written.at(-1)!;
    expect(last.bom_preset).toBeUndefined(); // never a top-level field
    const ui = (last.project_ui as Record<string, Record<string, unknown>>)[DIR_A];
    expect(ui.bom_preset).toBe("Assembly");
    expect(ui.bom_hidden).toEqual({ Assembly: ["mpn"] });
    expect(ui.bom_sort).toEqual({ key: "value", dir: -1 });
  });

  it("does not bleed one project's columns into another that shares a preset name", async () => {
    captureIpc();
    useSettingsStore.setState({
      projectUi: {
        [DIR_A]: { bom_preset: "Assembly", bom_hidden: { Assembly: ["mpn"] } },
        [DIR_B]: { bom_preset: "Assembly", bom_hidden: {} },
      },
    });

    await useViewStore.getState().hydrateBom(DIR_A);
    expect(useViewStore.getState().bomLayout.hidden).toEqual({ Assembly: ["mpn"] });

    await useViewStore.getState().hydrateBom(DIR_B);
    expect(useViewStore.getState().bomLayout.hidden).toEqual({});
  });

  it("stores column widths per project, and never in localStorage", async () => {
    const written = captureIpc();
    await useViewStore.getState().hydrateBom(DIR_A);

    useViewStore.getState().setBomColWidths("Assembly", { value: 120 });
    await Promise.resolve();
    await Promise.resolve();

    const ui = (written.at(-1)!.project_ui as Record<string, Record<string, unknown>>)[DIR_A];
    expect(ui.bom_widths).toEqual({ Assembly: { value: 120 } });
    expect(localStorage.getItem("bom.widths")).toBeNull(); // the tier is gone
  });

  it("widths follow the project, like the hidden columns", async () => {
    captureIpc();
    useSettingsStore.setState({
      projectUi: {
        [DIR_A]: { bom_widths: { Assembly: { value: 120 } } },
        [DIR_B]: {},
      },
    });

    await useViewStore.getState().hydrateBom(DIR_A);
    expect(useViewStore.getState().bomLayout.widths).toEqual({ Assembly: { value: 120 } });

    await useViewStore.getState().hydrateBom(DIR_B);
    expect(useViewStore.getState().bomLayout.widths).toEqual({});
  });

  it("chips are app-global and changedOnly stays opt-out through a round-trip", async () => {
    const written = captureIpc();
    await useViewStore.getState().hydrateBom(DIR_A);
    // Never saved → the default is NOT all-false.
    expect(useViewStore.getState().bomChips.changedOnly).toBe(true);

    useViewStore.getState().setBomChip("changedOnly", false);
    await Promise.resolve();
    expect(written.at(-1)!.bom_chips).toEqual({
      dnpOnly: false,
      missingMpn: false,
      changedOnly: false,
    });

    // An explicit false must survive; a settings file hand-trimmed to {} must not.
    useSettingsStore.setState({ bomChips: { dnpOnly: false, missingMpn: false, changedOnly: false } });
    await useViewStore.getState().hydrateBom(DIR_A);
    expect(useViewStore.getState().bomChips.changedOnly).toBe(false);

    useSettingsStore.setState({ bomChips: {} });
    await useViewStore.getState().hydrateBom(DIR_A);
    expect(useViewStore.getState().bomChips.changedOnly).toBe(true);
  });

  it("migrates a pre-move localStorage layout into the first project opened", async () => {
    const written = captureIpc();
    localStorage.setItem(
      "bom.layout",
      JSON.stringify({
        preset: "Assembly",
        hidden: { Assembly: ["mpn"] },
        sort: { key: "value", dir: -1 },
        widths: { Assembly: { value: 90 } },
      }),
    );
    localStorage.setItem("bom.chips", JSON.stringify({ dnpOnly: true, changedOnly: false }));

    await useViewStore.getState().hydrateBom(DIR_A);
    await new Promise((r) => setTimeout(r, 0)); // let the migration writes settle

    const s = useViewStore.getState();
    expect(s.bomLayout.preset).toBe("Assembly");
    expect(s.bomLayout.hidden).toEqual({ Assembly: ["mpn"] });
    expect(s.bomLayout.sort).toEqual({ key: "value", dir: -1 });
    expect(s.bomLayout.widths).toEqual({ Assembly: { value: 90 } });
    expect(s.bomChips).toEqual({ dnpOnly: true, missingMpn: false, changedOnly: false });
    // The migration persists, so the next open reads settings rather than the blob.
    const ui = (written.at(-1)!.project_ui as Record<string, Record<string, unknown>>)[DIR_A];
    expect(ui.bom_preset).toBe("Assembly");
    expect(ui.bom_widths).toEqual({ Assembly: { value: 90 } });
    // …and the legacy keys are dropped only once that write lands.
    expect(localStorage.getItem("bom.layout")).toBeNull();
    expect(localStorage.getItem("bom.chips")).toBeNull();
  });

  it("survives a hostile settings payload without breaking the table", async () => {
    captureIpc();
    useSettingsStore.setState({
      projectUi: {
        [DIR_A]: {
          bom_preset: 42 as unknown as string,
          bom_hidden: { Assembly: ["ok", 7 as unknown as string] },
          bom_sort: { key: "value", dir: 99 },
        },
      },
    });

    await useViewStore.getState().hydrateBom(DIR_A);

    const s = useViewStore.getState();
    expect(s.bomLayout.preset).toBeNull(); // non-string → "never chose"
    expect(s.bomLayout.hidden).toEqual({ Assembly: ["ok"] });
    expect(s.bomLayout.sort).toEqual({ key: "value", dir: 1 }); // out-of-range dir clamped
  });

  it("drops non-positive stored widths so a column can't collapse to 0px", () => {
    expect(sanitizeWidths({ Assembly: { value: 0, mpn: -5, ref: 80, bad: "wide", n: NaN } })).toEqual(
      { Assembly: { ref: 80 } },
    );
    expect(sanitizeWidths("not an object")).toEqual({});
    expect(sanitizeWidths(null)).toEqual({});
  });
});
