import type { KeymapPreset } from "./types";

// Minimal keymap table (spec §5 WS1). Esc / Alt+arrows live in the canvas itself;
// this module covers the app-level chords for the KiCad preset (the only keymap).

export type KeyAction =
  | "palette"
  | "commands"
  | "fit"
  | "crossProbe"
  | "zoomIn"
  | "zoomOut"
  | "measure"
  | "fullscreen"
  | "runReview"
  | "shortcuts";

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
  // Ctrl+R opens the "Run a review" launcher. It shadows the webview's reload, which
  // is only a dev-build affordance and is not something a desktop app should offer.
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && k === "r") return "runReview";
  if (e.ctrlKey || e.metaKey || e.altKey) return null;

  // Full screen (F11) + the shortcuts cheat-sheet (?) are modifier-free single keys.
  if (k === "f11") return "fullscreen";
  if (k === "?") return "shortcuts";

  if (preset === "kicad" && k === "home") return "fit";

  // Zoom: PgUp/PgDn (matches the PCB toolbar tooltips) and +/- for parity with KiCad.
  // These were advertised on the toolbar but never wired — pressing them did nothing.
  if (k === "pageup" || k === "+" || k === "=") return "zoomIn";
  if (k === "pagedown" || k === "-") return "zoomOut";

  // Shared single keys.
  if (k === "x") return "crossProbe";
  return null;
}

// ---------------------------------------------------------------------------
// Shortcut reference (single source of truth for the Keyboard Shortcuts dialog
// and the menu hints). "Mod" renders as ⌘ on macOS, Ctrl elsewhere. Each entry
// may list several alternative combos (e.g. PgUp OR +).
// ---------------------------------------------------------------------------

export type ShortcutScope = "Global" | "Schematic" | "PCB" | "BOM";

export interface ShortcutDef {
  /** Alternative key combos; each inner array is one combo of key tokens. */
  combos: string[][];
  action: string;
  scope: ShortcutScope;
}

export const SHORTCUTS: ShortcutDef[] = [
  { combos: [["Mod", "F"]], action: "Search nets & components", scope: "Global" },
  { combos: [["Mod", "P"]], action: "Command palette", scope: "Global" },
  { combos: [["Mod", "R"]], action: "Run a review", scope: "Global" },
  { combos: [["X"]], action: "Cross-probe schematic ↔ PCB", scope: "Global" },
  { combos: [["Home"]], action: "Fit to screen", scope: "Global" },
  { combos: [["PgUp"], ["+"]], action: "Zoom in", scope: "Global" },
  { combos: [["PgDn"], ["−"]], action: "Zoom out", scope: "Global" },
  { combos: [["F11"]], action: "Toggle full screen", scope: "Global" },
  { combos: [["Esc"]], action: "Clear selection / exit the current mode", scope: "Global" },
  { combos: [["?"]], action: "Show this shortcuts list", scope: "Global" },

  { combos: [["Alt", "←"]], action: "Navigate back", scope: "Schematic" },
  { combos: [["Alt", "→"]], action: "Navigate forward", scope: "Schematic" },

  { combos: [["Mod", "Shift", "M"]], action: "Measure tool", scope: "PCB" },
  { combos: [["C"]], action: "Comment mode", scope: "PCB" },
  { combos: [["Mod", "C"]], action: "Measuring: copy the readout", scope: "PCB" },

  { combos: [["Mod", "Shift", "B"]], action: "Run the BOM check", scope: "BOM" },
];

/** True on macOS — swaps the "Mod" token to ⌘ and reorders nothing else. */
export function isMacPlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent || "");
}

/** Render a "Mod" token for the running platform (⌘ on macOS, Ctrl elsewhere). */
export function modToken(mac = isMacPlatform()): string {
  return mac ? "⌘" : "Ctrl";
}
