import { describe, expect, it } from "vitest";
import { resolveKey, SHORTCUTS, type KeyAction } from "./keymap";

// Layer-1 coverage for the app-level key bindings. Guards the resolveKey table —
// in particular the zoom keys, which the PCB toolbar advertised (PgUp/PgDn) long
// before they were actually wired.

function key(k: string, mod: Partial<KeyboardEvent> = {}): KeyboardEvent {
  return { key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mod } as KeyboardEvent;
}

const R = (k: string, mod?: Partial<KeyboardEvent>): KeyAction | null => resolveKey(key(k, mod), "kicad");

describe("resolveKey", () => {
  it("maps the search / command chords", () => {
    expect(R("f", { ctrlKey: true })).toBe("palette");
    expect(R("f", { metaKey: true })).toBe("palette");
    expect(R("p", { ctrlKey: true })).toBe("commands");
    expect(R("m", { ctrlKey: true, shiftKey: true })).toBe("measure");
  });

  it("wires the zoom keys the toolbar advertises (PgUp/PgDn and +/-)", () => {
    expect(R("PageUp")).toBe("zoomIn");
    expect(R("+")).toBe("zoomIn");
    expect(R("=")).toBe("zoomIn");
    expect(R("PageDown")).toBe("zoomOut");
    expect(R("-")).toBe("zoomOut");
  });

  it("maps the single-key commands", () => {
    expect(R("Home")).toBe("fit");
    expect(R("x")).toBe("crossProbe");
    expect(R("o")).toBe(null);
    expect(R("F11")).toBe("fullscreen");
    expect(R("?")).toBe("shortcuts");
  });

  it("ignores modified single keys and unbound keys", () => {
    expect(R("x", { ctrlKey: true })).toBeNull();
    expect(R("o", { altKey: true })).toBeNull();
    expect(R("z")).toBeNull();
  });
});

describe("SHORTCUTS table", () => {
  it("documents every scope and stays non-empty", () => {
    const scopes = new Set(SHORTCUTS.map((s) => s.scope));
    expect(scopes).toEqual(new Set(["Global", "Schematic", "PCB"]));
    for (const s of SHORTCUTS) {
      expect(s.combos.length).toBeGreaterThan(0);
      expect(s.action.length).toBeGreaterThan(0);
    }
  });
});
