/**
 * Debug-only render probe — the Layer-2 (tauri-pilot) E2E hook.
 *
 * The schematic + PCB canvases paint to inline SVG inside a transformed `<div>`. An
 * a11y/DOM snapshot can describe that tree structurally, but it cannot tell you what
 * actually rendered: which sheet loaded, where the camera sits, which net/component is
 * highlighted, which net labels got placed, how many comment chips are anchored. So
 * tauri-pilot reads that state out of `window.__spinzero` via `eval` instead of relying
 * on a snapshot it can't interpret. See docs/testing.md → "canvas is opaque".
 *
 * Installed ONLY in dev/debug builds (`import.meta.env.DEV`). `vite build` tree-shakes
 * the registration out of a production bundle, matching the Rust pilot plugin's
 * `#[cfg(debug_assertions)]` gate — neither the probe nor the plugin ships in a release.
 *
 * Usage from tauri-pilot:
 *   tauri-pilot --window main eval "window.__spinzero.snapshot()"
 *   tauri-pilot --window main eval "window.__spinzero.view('pcb').netLabels"
 */

export type ProbeSnapshot = Record<string, unknown>;
type ProbeFn = () => ProbeSnapshot;

const probes: Record<string, ProbeFn> = {};

interface SpinzeroDebug {
  /** Merged render state of every registered view (schematic + pcb + …). */
  snapshot: () => Record<string, ProbeSnapshot>;
  /** One view's live state, or undefined when that view isn't mounted. */
  view: (name: string) => ProbeSnapshot | undefined;
  /** Names of the views currently registered. */
  views: () => string[];
}

declare global {
  interface Window {
    /** Present only in dev builds (see renderProbe.ts). */
    __spinzero?: SpinzeroDebug;
  }
}

function safe(fn: ProbeFn): ProbeSnapshot {
  try {
    return fn();
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }
}

function install(): void {
  if (window.__spinzero) return;
  window.__spinzero = {
    snapshot: () => {
      const out: Record<string, ProbeSnapshot> = {};
      for (const [name, fn] of Object.entries(probes)) out[name] = safe(fn);
      return out;
    },
    view: (name) => {
      const fn = probes[name];
      return fn ? safe(fn) : undefined;
    },
    views: () => Object.keys(probes),
  };
}

/**
 * Register a view's render-state getter under `name` (e.g. "schematic", "pcb"). The
 * getter is invoked fresh on every snapshot, so it MUST read live refs/DOM at call
 * time — never close over a value captured at registration. Returns an unregister fn
 * for the effect cleanup. Outside dev builds this is a no-op and `window.__spinzero`
 * is never installed.
 */
export function registerRenderProbe(name: string, fn: ProbeFn): () => void {
  if (!import.meta.env.DEV) return () => {};
  probes[name] = fn;
  install();
  return () => {
    if (probes[name] === fn) delete probes[name];
  };
}
