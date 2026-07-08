import type { KeymapPreset } from "./types";

// Minimal keymap table (spec §5 WS1). Esc / Alt+arrows live in the canvas itself;
// this module covers the app-level chords for the KiCad preset (the only keymap).

export type KeyAction =
  | "palette"
  | "commands"
  | "fit"
  | "overview"
  | "crossProbe"
  | "zoomIn"
  | "zoomOut"
  | "measure";

/** True when the event targets a text-entry surface — keymaps must stay out. */
export function isTypingTarget(e: KeyboardEvent): boolean {
  const el = e.target as HTMLElement | null;
  if (!el) return false;
  return (
    el.tagName === "INPUT" ||
    el.tagName === "TEXTAREA" ||
    el.tagName === "SELECT" ||
    el.isContentEditable
  );
}

/** Resolve a keydown into an app-level action, or null. */
export function resolveKey(e: KeyboardEvent, preset: KeymapPreset): KeyAction | null {
  const k = e.key.toLowerCase();

  // Ctrl+F searches nets/components, Ctrl+P is commands only (feedback item 20 —
  // search belongs on the find chord, not the command chord).
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && k === "f") return "palette";
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && k === "p") return "commands";
  // Ctrl+Shift+M toggles the measure tool (KiCad parity). Matched before the
  // modifier bail-out below, which otherwise swallows every modifier chord.
  if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && k === "m") return "measure";
  if (e.ctrlKey || e.metaKey || e.altKey) return null;

  if (preset === "kicad" && k === "home") return "fit";

  // Shared single keys.
  if (k === "x") return "crossProbe";
  if (k === "o") return "overview";
  return null;
}