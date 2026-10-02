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

/** A KiCad bundle's layer row: no `role`, so these exercise the name fallback
 *  every pre-Altium bundle still relies on. */
const L = (...names: string[]) => names.map((name) => ({ name, svg: `${name}.svg` }));

/** An Altium bundle's row: the designer's own name plus the review role, which
 *  is the only thing that can classify it. */
const A = (name: string, role: string, side?: string) => ({ name, svg: `${name}.svg`, role, side });

describe("pcbViewStore.resetForLayers", () => {
  beforeEach(() => {
    usePcbViewStore.setState({ active: null, hidden: new Set(), known: [], edge: null });
  });

  it("hides documentation/user + non-essential layers by default, shows copper/silk/edge", () => {
    usePcbViewStore.getState().resetForLayers(L(
      "F.Cu", "B.Cu", "F.SilkS", "Edge.Cuts",
      "F.Fab", "F.Mask", "F.Paste", "F.CrtYd", "F.Adhes",
      "User.3", "Dwgs.User", "Margin",
    ));
    const { hidden } = usePcbViewStore.getState();
    // Shown by default.
    for (const l of ["F.Cu", "B.Cu", "F.SilkS", "Edge.Cuts"]) expect(hidden.has(l)).toBe(false);
    // Hidden by default (clutter + documentation layers).
    for (const l of ["F.Fab", "F.Mask", "F.Paste", "F.CrtYd", "F.Adhes", "User.3", "Dwgs.User", "Margin"])
      expect(hidden.has(l)).toBe(true);
  });

  it("defaults the active layer to F.Cu on first load", () => {
    usePcbViewStore.setState({ active: null, hidden: new Set(), known: [], edge: null });
    usePcbViewStore.getState().resetForLayers(L("F.Cu", "B.Cu", "Edge.Cuts"));
    expect(usePcbViewStore.getState().active).toBe("F.Cu");
    expect(usePcbViewStore.getState().edge).toBe("Edge.Cuts");
  });

  // An Altium stack has no layer called F.Cu and draws its profile on a
  // mechanical layer, so a board that classified layers by NAME opened with
  // nothing active, nothing hidden, and no outline riding along in the diff.
  it("classifies an Altium stack by role, not by KiCad's names", () => {
    usePcbViewStore.setState({ active: null, hidden: new Set(), known: [], edge: null });
    usePcbViewStore.getState().resetForLayers([
      A("L1_Top", "copper", "front"),
      A("L2_GND", "copper", "inner"),
      A("L4_Bot", "copper", "back"),
      A("Top Overlay", "silkscreen", "front"),
      A("Top Solder", "mask", "front"),
      A("Board Shape", "edge"),
      A("M15 (CMP_Courtyard_Top)", "user"),
    ]);
    const { active, hidden, edge } = usePcbViewStore.getState();
    expect(active).toBe("L1_Top");
    expect(edge).toBe("Board Shape");
    for (const l of ["L1_Top", "L2_GND", "L4_Bot", "Top Overlay", "Board Shape"])
      expect(hidden.has(l)).toBe(false);
    for (const l of ["Top Solder", "M15 (CMP_Courtyard_Top)"]) expect(hidden.has(l)).toBe(true);
  });

  it("falls back to the front-copper default when the active layer no longer exists, keeps a surviving one", () => {
    // A stale active layer is replaced by the front-copper default.
    usePcbViewStore.setState({ active: "User.9", hidden: new Set(), known: [], edge: null });
    usePcbViewStore.getState().resetForLayers(L("F.Cu", "B.Cu"));
    expect(usePcbViewStore.getState().active).toBe("F.Cu");

    // A surviving non-default active layer is preserved, not forced back to F.Cu.
    usePcbViewStore.setState({ active: "B.Cu", hidden: new Set(), known: ["F.Cu", "B.Cu"], edge: null });
    usePcbViewStore.getState().resetForLayers(L("F.Cu", "B.Cu"));
    expect(usePcbViewStore.getState().active).toBe("B.Cu");

    // No front copper on the board → the first copper layer, which is a layer
    // the reviewer can see rather than the blank board the old default gave.
    usePcbViewStore.setState({ active: "X", hidden: new Set(), known: [], edge: null });
    usePcbViewStore.getState().resetForLayers(L("B.Cu", "Edge.Cuts"));
    expect(usePcbViewStore.getState().active).toBe("B.Cu");

    // Nothing but documentation → nothing forced.
    usePcbViewStore.setState({ active: "X", hidden: new Set(), known: [], edge: null });
    usePcbViewStore.getState().resetForLayers(L("Dwgs.User"));
    expect(usePcbViewStore.getState().active).toBeNull();
  });

  it("does not re-hide a layer the user already chose to show on a later revision", () => {
    // First open hides F.Fab by default…
    usePcbViewStore.getState().resetForLayers(L("F.Cu", "F.Fab"));
    expect(usePcbViewStore.getState().hidden.has("F.Fab")).toBe(true);
    // …user shows it, then a new revision re-runs resetForLayers with the same set.
    usePcbViewStore.getState().showLayer("F.Fab");
    usePcbViewStore.getState().resetForLayers(L("F.Cu", "F.Fab"));
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

describe("zones opacity default", () => {
  const layers = [{ name: "Top Layer", svg: "x.svg", role: "copper", side: "front" }];
  beforeEach(() => {
    usePcbViewStore.setState({ active: null, hidden: new Set(), known: [], edge: null, userSet: new Set() });
    usePcbViewStore.setState({ opacity: { ...usePcbViewStore.getState().opacity, zones: 0.6 } });
  });

  it("is full strength for an Altium board and 60% for KiCad", () => {
    usePcbViewStore.getState().resetForLayers(layers, "altium");
    expect(usePcbViewStore.getState().opacity.zones).toBe(1);
    usePcbViewStore.getState().resetForLayers(layers, null);
    expect(usePcbViewStore.getState().opacity.zones).toBe(0.6);
  });

  it("keeps a value the user set", () => {
    usePcbViewStore.getState().setOpacity("zones", 0.3);
    usePcbViewStore.getState().resetForLayers(layers, "altium");
    expect(usePcbViewStore.getState().opacity.zones).toBe(0.3);
  });

  it("saves only the sliders the user moved", () => {
    usePcbViewStore.getState().setOpacity("tracks", 0.5);
    expect([...usePcbViewStore.getState().userSet]).toEqual(["tracks"]);
  });
});
