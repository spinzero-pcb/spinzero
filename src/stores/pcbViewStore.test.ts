import { beforeEach, describe, expect, it } from "vitest";
import { WORKSHEET_LAYER, isWorksheetLayer, layerColorVar, usePcbViewStore } from "./pcbViewStore";

// Layer-1 coverage for the PCB appearance logic (docs/testing.md): pure store +
// helper behaviour, no webview. Locks in the non-standard-layer colouring and the
// default-visibility rules added when user/documentation layers became extractable.

describe("layerColorVar", () => {
  it("paints a user layer in its extracted KiCad colour when one is present", () => {
    // Non-standard layers carry their resolved #RRGGBB (LayerLite.color); it wins.
    expect(layerColorVar("User.3", "#C2C2C2")).toBe("#C2C2C2");
    expect(layerColorVar("F.Cu", "#abcdef")).toBe("#abcdef");
  });

  it("maps the standard fabrication layers to their CSS-var tokens", () => {
    expect(layerColorVar("F.Cu")).toBe("var(--pcb-fcu)");
    expect(layerColorVar("B.SilkS")).toBe("var(--pcb-bsilk)");
    expect(layerColorVar("Edge.Cuts")).toBe("var(--pcb-edge)");
  });

  it("gives each inner copper layer its own --pcb-in{N} with an in1 fallback", () => {
    expect(layerColorVar("In1.Cu")).toBe("var(--pcb-in1, var(--pcb-in1))");
    expect(layerColorVar("In4.Cu")).toBe("var(--pcb-in4, var(--pcb-in1))");
  });

  it("falls back to a neutral token for a colour-less user layer", () => {
    // No token + no extracted colour (theme-less bundle) → the ffab grey, not black.
    expect(layerColorVar("User.7")).toBe("var(--pcb-ffab)");
    expect(layerColorVar("Margin")).toBe("var(--pcb-ffab)");
  });
});

describe("isWorksheetLayer", () => {
  it("recognises the drawing sheet by role or name, never a real board layer", () => {
    // The appearance panel / canvas use this to keep the page background out of the
    // selectable layer list and the camera fit (it's not a board layer).
    expect(isWorksheetLayer({ name: WORKSHEET_LAYER })).toBe(true);
    expect(isWorksheetLayer({ name: "anything", role: "worksheet" })).toBe(true);
    expect(isWorksheetLayer({ name: "F.Cu", role: "copper" })).toBe(false);
    expect(isWorksheetLayer({ name: "User.2", role: "user" })).toBe(false);
  });
});

describe("pcbViewStore.resetForLayers", () => {
  beforeEach(() => {
    usePcbViewStore.setState({ active: null, hidden: new Set(), known: [] });
  });

  it("hides documentation/user + non-essential layers by default, shows copper/silk/edge", () => {
    usePcbViewStore.getState().resetForLayers([
      "F.Cu", "B.Cu", "F.SilkS", "Edge.Cuts",
      "F.Fab", "F.Mask", "F.Paste", "F.CrtYd", "F.Adhes",
      "User.3", "Dwgs.User", "Margin",
    ]);
    const { hidden } = usePcbViewStore.getState();
    // Shown by default.
    for (const l of ["F.Cu", "B.Cu", "F.SilkS", "Edge.Cuts"]) expect(hidden.has(l)).toBe(false);
    // Hidden by default (clutter + documentation layers).
    for (const l of ["F.Fab", "F.Mask", "F.Paste", "F.CrtYd", "F.Adhes", "User.3", "Dwgs.User", "Margin"])
      expect(hidden.has(l)).toBe(true);
  });

  it("defaults the active layer to F.Cu on first load", () => {
    usePcbViewStore.setState({ active: null, hidden: new Set(), known: [] });
    usePcbViewStore.getState().resetForLayers(["F.Cu", "B.Cu", "Edge.Cuts"]);
    expect(usePcbViewStore.getState().active).toBe("F.Cu");
  });

  it("falls back to the F.Cu default when the active layer no longer exists, keeps a surviving one", () => {
    // A stale active layer is replaced by the F.Cu default (when the board has one).
    usePcbViewStore.setState({ active: "User.9", hidden: new Set(), known: [] });
    usePcbViewStore.getState().resetForLayers(["F.Cu", "B.Cu"]);
    expect(usePcbViewStore.getState().active).toBe("F.Cu");

    // A surviving non-default active layer is preserved, not forced back to F.Cu.
    usePcbViewStore.setState({ active: "B.Cu", hidden: new Set(), known: ["F.Cu", "B.Cu"] });
    usePcbViewStore.getState().resetForLayers(["F.Cu", "B.Cu"]);
    expect(usePcbViewStore.getState().active).toBe("B.Cu");

    // No F.Cu on the board → nothing forced (natural board order).
    usePcbViewStore.setState({ active: "X", hidden: new Set(), known: [] });
    usePcbViewStore.getState().resetForLayers(["B.Cu", "Edge.Cuts"]);
    expect(usePcbViewStore.getState().active).toBeNull();
  });

  it("does not re-hide a layer the user already chose to show on a later revision", () => {
    // First open hides F.Fab by default…
    usePcbViewStore.getState().resetForLayers(["F.Cu", "F.Fab"]);
    expect(usePcbViewStore.getState().hidden.has("F.Fab")).toBe(true);
    // …user shows it, then a new revision re-runs resetForLayers with the same set.
    usePcbViewStore.getState().showLayer("F.Fab");
    usePcbViewStore.getState().resetForLayers(["F.Cu", "F.Fab"]);
    expect(usePcbViewStore.getState().hidden.has("F.Fab")).toBe(false);
  });
});

describe("pcbViewStore visibility actions", () => {
  beforeEach(() => {
    usePcbViewStore.setState({ active: null, hidden: new Set(), known: [] });
  });

  it("toggles, shows, hides-all and shows-all layers", () => {
    const s = () => usePcbViewStore.getState();
    s().toggleLayer("F.Cu");
    expect(s().hidden.has("F.Cu")).toBe(true);
    s().showLayer("F.Cu");
    expect(s().hidden.has("F.Cu")).toBe(false);

    s().hideAllLayers(["F.Cu", "B.Cu", "F.SilkS"]);
    expect(s().hidden.size).toBe(3);
    expect(s().active).toBeNull();

    s().showAllLayers();
    expect(s().hidden.size).toBe(0);
  });

  it("setHidden replaces the hidden set wholesale and leaves the active layer alone", () => {
    const s = () => usePcbViewStore.getState();
    usePcbViewStore.setState({ active: "F.Cu", hidden: new Set(["B.Cu"]) });
    // Layer-menu presets (e.g. "show only Cu") set the hidden set directly. Unlike
    // hideAllLayers, the active layer is untouched — "hide all but active" relies on it.
    s().setHidden(["F.SilkS", "F.Mask"]);
    expect([...s().hidden].sort()).toEqual(["F.Mask", "F.SilkS"]);
    expect(s().hidden.has("B.Cu")).toBe(false); // previous hides replaced, not merged
    expect(s().active).toBe("F.Cu");
  });
});
