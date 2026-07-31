import { beforeEach, describe, expect, it } from "vitest";
import { useNetClassStore, activeNets } from "./netClassStore";
import { usePcbViewStore } from "./pcbViewStore";
import { useDesignStore } from "./designStore";
import { useSettingsStore } from "./settingsStore";
import { listNetClasses } from "../lib/netClasses";
import type { DesignIndexes } from "../lib/design";
import type { PcbIndex } from "./designStore";

// Layer-1 coverage (docs/testing.md) for the PCB Net Classes feature: the pure
// grouping helper and the store's selection, colour-resolution and layer-isolation
// (+ restore) behaviour.

/** A tiny two-class design: HV nets on F.Cu, USB on both coppers, plus the Default
 *  catch-all. Only the fields the code under test reads are populated. */
function makeIndexes(): DesignIndexes {
  return {
    layers: [
      { name: "F.Cu", svg: "f.svg" },
      { name: "B.Cu", svg: "b.svg" },
      { name: "F.SilkS", svg: "s.svg" },
      { name: "Edge.Cuts", svg: "e.svg" },
    ],
    nets: {
      HV1: { class: "HV", terminals: [], sheets: [], by_sheet: {} },
      HV2: { class: "HV", terminals: [], sheets: [], by_sheet: {} },
      "USB_D+": { class: "USB", terminals: [], sheets: [], by_sheet: {} },
      GND: { class: "Default", terminals: [], sheets: [], by_sheet: {} },
    },
  } as unknown as DesignIndexes;
}

const PCB_INDEX: PcbIndex = {
  nets: {
    HV1: { layers: ["F.Cu"], lenByLayer: {}, widths: [], vias: 0 },
    HV2: { layers: ["F.Cu"], lenByLayer: {}, widths: [], vias: 0 },
    "USB_D+": { layers: ["F.Cu", "B.Cu"], lenByLayer: {}, widths: [], vias: 0 },
    GND: { layers: ["F.Cu", "B.Cu"], lenByLayer: {}, widths: [], vias: 0 },
  },
  compSide: {},
};

beforeEach(() => {
  useNetClassStore.setState({
    selected: [],
    classColors: {},
    netOverride: {},
    netColors: {},
    projectDir: null, // no project → colour picks aren't persisted from these tests
    savedHidden: null,
    savedActive: null,
  });
  usePcbViewStore.setState({ hidden: new Set(), active: null });
  useDesignStore.setState({ indexes: makeIndexes(), pcbIndex: PCB_INDEX });
});

describe("listNetClasses", () => {
  it("groups nets by class, sorts nets, and pushes Default last", () => {
    const classes = listNetClasses(makeIndexes());
    expect(classes.map((c) => c.name)).toEqual(["HV", "USB", "Default"]);
    expect(classes[0].nets).toEqual(["HV1", "HV2"]);
    expect(classes.at(-1)!.name).toBe("Default");
  });

  it("returns [] for a null design", () => {
    expect(listNetClasses(null)).toEqual([]);
  });
});

describe("useNetClassStore isolation", () => {
  it("isolates the copper layers a class runs on and keeps Edge.Cuts", () => {
    useNetClassStore.getState().toggle("HV"); // HV nets live only on F.Cu
    const hidden = usePcbViewStore.getState().hidden;
    expect(hidden.has("F.Cu")).toBe(false); // carries HV
    expect(hidden.has("Edge.Cuts")).toBe(false); // board outline kept
    expect(hidden.has("B.Cu")).toBe(true); // no HV geometry
    expect(hidden.has("F.SilkS")).toBe(true);
  });

  it("unions layers across multiple selected classes", () => {
    useNetClassStore.getState().toggle("HV"); // F.Cu
    useNetClassStore.getState().toggle("USB"); // F.Cu + B.Cu
    const hidden = usePcbViewStore.getState().hidden;
    expect(hidden.has("F.Cu")).toBe(false);
    expect(hidden.has("B.Cu")).toBe(false);
    expect(hidden.has("F.SilkS")).toBe(true);
    expect(useNetClassStore.getState().selected).toEqual(["HV", "USB"]);
  });

  it("restores the pre-isolation layer view when the last class is cleared", () => {
    // User had F.SilkS hidden and B.Cu active before touching net classes.
    usePcbViewStore.setState({ hidden: new Set(["F.SilkS"]), active: "B.Cu" });
    useNetClassStore.getState().toggle("HV");
    useNetClassStore.getState().toggle("HV"); // toggle off → clear
    expect([...usePcbViewStore.getState().hidden]).toEqual(["F.SilkS"]);
    expect(usePcbViewStore.getState().active).toBe("B.Cu");
    expect(useNetClassStore.getState().selected).toEqual([]);
  });

  it("clear() drops every class and restores the snapshot", () => {
    usePcbViewStore.setState({ hidden: new Set(["F.SilkS"]), active: null });
    useNetClassStore.getState().toggle("HV");
    useNetClassStore.getState().toggle("USB");
    useNetClassStore.getState().clear();
    expect([...usePcbViewStore.getState().hidden]).toEqual(["F.SilkS"]);
    expect(useNetClassStore.getState().selected).toEqual([]);
  });

  it("reset() clears selection WITHOUT touching layers (new design)", () => {
    useNetClassStore.getState().toggle("HV");
    const isolated = new Set(usePcbViewStore.getState().hidden);
    useNetClassStore.getState().reset();
    expect(useNetClassStore.getState().selected).toEqual([]);
    // Layers untouched — resetForLayers owns them on a design change.
    expect(usePcbViewStore.getState().hidden).toEqual(isolated);
  });
});

describe("per-net selection and colours", () => {
  const nets = () => activeNets(useNetClassStore.getState(), makeIndexes());

  it("a selected class highlights its nets with no colour (PCB layer colours)", () => {
    useNetClassStore.getState().toggle("HV");
    expect([...nets()]).toEqual([
      ["HV1", null],
      ["HV2", null],
    ]);
  });

  it("a class colour applies to its nets; a net colour overrides it", () => {
    useNetClassStore.getState().toggle("HV");
    useNetClassStore.getState().setClassColor("HV", "#ff0000");
    useNetClassStore.getState().setNetColor("HV2", "#00ff00");
    expect(nets().get("HV1")).toBe("#ff0000");
    expect(nets().get("HV2")).toBe("#00ff00");
    useNetClassStore.getState().setClassColor("HV", null);
    expect(nets().get("HV1")).toBeNull();
  });

  it("a net can be deselected inside a selected class", () => {
    useNetClassStore.getState().toggle("HV");
    useNetClassStore.getState().toggleNet("HV1");
    expect([...nets().keys()]).toEqual(["HV2"]);
    // Only F.Cu carries HV2, so isolation still holds.
    expect([...usePcbViewStore.getState().hidden].sort()).toEqual(["B.Cu", "F.SilkS"]);
  });

  it("a lone net can be selected without its class, and isolates its layers", () => {
    usePcbViewStore.setState({ hidden: new Set(["F.SilkS"]), active: null });
    useNetClassStore.getState().toggleNet("USB_D+");
    expect([...nets().keys()]).toEqual(["USB_D+"]);
    expect([...usePcbViewStore.getState().hidden]).toEqual(["F.SilkS"]);
    // Deselecting the last net restores the pre-isolation view.
    useNetClassStore.getState().toggleNet("USB_D+");
    expect(nets().size).toBe(0);
    expect([...usePcbViewStore.getState().hidden]).toEqual(["F.SilkS"]);
  });

  it("hydrate() loads this project's saved colours and drops malformed ones", async () => {
    useSettingsStore.setState({
      loaded: true,
      projectUi: {
        "/p/a": { net_class_colors: { HV: "#FF0000", USB: "red" }, net_colors: { GND: "#00ff00" } },
      },
    });
    await useNetClassStore.getState().hydrate("/p/a");
    expect(useNetClassStore.getState().classColors).toEqual({ HV: "#ff0000" }); // "red" dropped
    expect(useNetClassStore.getState().netColors).toEqual({ GND: "#00ff00" });
    // Switching to a project with nothing saved starts clean.
    await useNetClassStore.getState().hydrate("/p/b");
    expect(useNetClassStore.getState().classColors).toEqual({});
  });

  it("reset() keeps colour picks (design reload, same project)", () => {
    useNetClassStore.getState().toggle("HV");
    useNetClassStore.getState().setClassColor("HV", "#ff0000");
    useNetClassStore.getState().reset();
    expect(useNetClassStore.getState().classColors).toEqual({ HV: "#ff0000" });
  });

  it("toggling a class drops per-net tweaks inside it", () => {
    useNetClassStore.getState().toggle("HV");
    useNetClassStore.getState().toggleNet("HV1");
    useNetClassStore.getState().toggle("HV"); // off
    useNetClassStore.getState().toggle("HV"); // on again
    expect([...nets().keys()]).toEqual(["HV1", "HV2"]);
  });
});
