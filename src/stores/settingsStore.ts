import { create } from "zustand";
import { ipc } from "../lib/ipc";
import { usePcbViewStore } from "./pcbViewStore";
import type { KeymapPreset, ProjectUi } from "../lib/types";

// App-level UI preferences. `keymap === null` after load means the user has
// never chosen — App shows the first-launch preset picker (spec: onboarding).
interface SettingsState {
  keymap: KeymapPreset | null;
  /** Remembered parent folder for new projects (asked once, reused after). */
  projectRoot: string | null;
  /** User-chosen accent colour (#rrggbb). `null` = the token default (blue). */
  accentColor: string | null;
  /** Display name for this user's review comments; `null` = the OS-derived slug. */
  authorName: string | null;
  /** Per-project remembered review UI (last session + tab), keyed by project dir. */
  projectUi: Record<string, ProjectUi>;
  /** Remembered PCB per-class transparency (0..1 per object class); null = defaults. */
  pcbOpacity: Record<string, number> | null;
  loaded: boolean;
  load: () => Promise<void>;
  setKeymap: (k: KeymapPreset) => Promise<void>;
  setProjectRoot: (p: string) => Promise<void>;
  setAccentColor: (c: string | null) => Promise<void>;
  setAuthorName: (n: string | null) => Promise<void>;
  /** Merge a patch into one project's remembered review UI and persist. */
  setProjectUi: (projectDir: string, patch: Partial<ProjectUi>) => Promise<void>;
  /** Persist the PCB transparency sliders (whole map, so one class never clobbers another). */
  setPcbOpacity: (opacity: Record<string, number>) => Promise<void>;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  keymap: null,
  projectRoot: null,
  accentColor: null,
  authorName: null,
  projectUi: {},
  pcbOpacity: null,
  loaded: false,

  load: async () => {
    try {
      const s = await ipc.getSettings();
      const accent = normalizeHex(s?.accent_color ?? null);
      applyAccent(accent);
      const pcbOpacity =
        s && typeof s.pcb_opacity === "object" && s.pcb_opacity !== null
          ? (s.pcb_opacity as Record<string, number>)
          : null;
      set({
        keymap: s?.keymap_preset ?? null,
        projectRoot: s?.project_root ?? null,
        accentColor: accent,
        authorName: normalizeName(s?.author_name ?? null),
        projectUi:
          s && typeof s.project_ui === "object" && s.project_ui !== null
            ? (s.project_ui as Record<string, ProjectUi>)
            : {},
        pcbOpacity,
        loaded: true,
      });
      // Push the saved transparency into the PCB view store so the sliders open where
      // the user left them (validated + clamped there).
      usePcbViewStore.getState().hydrateOpacity(pcbOpacity);
    } catch {
      set({ loaded: true });
    }
  },

  setKeymap: async (keymap) => {
    set({ keymap });
    await persist();
  },

  setProjectRoot: async (projectRoot) => {
    set({ projectRoot });
    await persist();
  },

  setAccentColor: async (accentColor) => {
    const next = normalizeHex(accentColor);
    applyAccent(next);
    set({ accentColor: next });
    await persist();
  },

  setAuthorName: async (name) => {
    set({ authorName: normalizeName(name) });
    await persist();
  },

  setProjectUi: async (projectDir, patch) => {
    const prev = get().projectUi[projectDir] ?? {};
    set({ projectUi: { ...get().projectUi, [projectDir]: { ...prev, ...patch } } });
    await persist();
  },

  setPcbOpacity: async (opacity) => {
    set({ pcbOpacity: opacity });
    await persist();
  },
}));

// Persist the full settings object so one setter never clobbers another's field.
async function persist() {
  const { keymap, projectRoot, accentColor, authorName, projectUi, pcbOpacity } =
    useSettingsStore.getState();
  try {
    await ipc.setSettings({
      keymap_preset: keymap,
      project_root: projectRoot,
      accent_color: accentColor,
      author_name: authorName,
      project_ui: projectUi,
      pcb_opacity: pcbOpacity,
    });
  } catch {
    // Persisting failed (e.g. read-only config dir) — the in-memory choice stands.
  }
}

// A display name is trimmed; blank collapses to null (fall back to the identity slug).
function normalizeName(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const t = v.trim();
  return t.length ? t.slice(0, 60) : null;
}

// ---- accent colour --------------------------------------------------------
// The accent is a single token trio (--accent / --accent-fg / --accent-dim).
// A user choice is applied as inline properties on :root, overriding tokens.css;
// clearing it (null) removes the overrides so the CSS default (blue) takes over.

function normalizeHex(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const m = /^#?([0-9a-fA-F]{6})$/.exec(v.trim());
  return m ? `#${m[1].toLowerCase()}` : null;
}

/** Apply an accent colour to :root, deriving a readable fg and a dim tint.
 *  `null` removes the overrides, falling back to the tokens.css default. */
export function applyAccent(hex: string | null) {
  const root = document.documentElement;
  if (!hex) {
    root.style.removeProperty("--accent");
    root.style.removeProperty("--accent-fg");
    root.style.removeProperty("--accent-dim");
    return;
  }
  const n = parseInt(hex.slice(1), 16);
  const r = (n >> 16) & 255;
  const g = (n >> 8) & 255;
  const b = n & 255;
  // Perceived brightness: light accents get dark text, dark accents get white.
  const light = (0.299 * r + 0.587 * g + 0.114 * b) / 255 > 0.55;
  root.style.setProperty("--accent", hex);
  root.style.setProperty("--accent-fg", light ? "#0b1210" : "#ffffff");
  root.style.setProperty("--accent-dim", `rgba(${r}, ${g}, ${b}, 0.16)`);
}

/** Accent presets shown in the Appearance dialog. `null` = the built-in default. */
export const ACCENT_PRESETS: { label: string; value: string | null }[] = [
  { label: "Blue (default)", value: null },
  { label: "Green", value: "#3fb950" },
  { label: "Copper", value: "#c87f42" },
  { label: "Violet", value: "#a371f7" },
  { label: "Teal", value: "#2bb3c0" },
  { label: "Rose", value: "#f06f92" },
];

/** The swatch colour to render for a preset (the default renders as blue). */
export const ACCENT_DEFAULT = "#4f8cff";
