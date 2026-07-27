import { beforeEach, describe, expect, it } from "vitest";
import { useNetClassStore } from "./netClassStore";
import { usePcbViewStore } from "./pcbViewStore";
import { useDesignStore } from "./designStore";
import { listNetClasses, netClassColor } from "../lib/netClasses";
import type { DesignIndexes } from "../lib/design";
import type { PcbIndex } from "./designStore";

// Layer-1 coverage (docs/testing.md) for the PCB Net Classes feature: the pure
// grouping/colour helpers and the store's layer-isolation + restore behaviour.

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
  useNetClassStore.setState({ selected: [], savedHidden: null, savedActive: null });
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

describe("netClassColor", () => {
  it("assigns each class a distinct colour and greys the Default catch-all", () => {
    // `ordered` is the full class list (stable across a session); selection never
    // changes it, so a class keeps its colour no matter what's highlighted.
    const ordered = ["HV", "USB", "Default"];
    const hv = netClassColor("HV", ordered);
    const usb = netClassColor("USB", ordered);
    expect(hv).not.toBe(usb);
    expect(netClassColor("HV", ordered)).toBe(hv); // deterministic
    // Default is a fixed neutral grey, never a vivid palette slot.
    expect(netClassColor("Default", ordered)).toBe(netClassColor("Default", ["A", "Default"]));
    expect(netClassColor("Default", ordered)).not.toBe(hv);
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
