// Applies the KiCad colour theme the extractor resolved (design.json `theme`,
// forwarded as DesignIndexes.theme) onto the app's CSS custom properties, so the
// viewer themes the monochrome SVGs with the *user's* real KiCad palette instead
// of the static KiCad-Default mirror in tokens.css.
//
// Tokens.css stays the fallback: we only set the variables the theme actually
// carries, and removeProperty() reverts to it. The mappings are value-preserving
// against the KiCad-Default theme — on the default theme every variable resolves
// to exactly what tokens.css already holds; a custom theme flows through.

import type { KicadTheme } from "./design";

// KiCad `schematic.*` key -> CSS custom property, mapped 1:1 so the viewer themes
// each schematic element from the key KiCad actually uses for it — no element borrows
// another's colour (pin name ≠ pin number; global label ≠ hierarchical; sheet border ≠
// sheet name). Any key the theme omits keeps its tokens.css KiCad-Default.
const SCH_MAP: Record<string, string> = {
  wire: "--sch-wire",
  bus: "--sch-bus",
  junction: "--sch-junction",
  no_connect: "--sch-noconnect",
  component_outline: "--sch-outline",
  pin: "--sch-pin",
  component_body: "--sch-body-fill",
  pin_name: "--sch-pin-name",
  pin_number: "--sch-pin-number",
  reference: "--sch-fields",
  value: "--sch-value",
  fields: "--sch-field",
  label_local: "--sch-label",
  label_global: "--sch-label-global",
  label_hier: "--sch-label-hier",
  note: "--sch-note",
  sheet: "--sch-sheet",
  sheet_name: "--sch-sheet-name",
  netclass_flag: "--sch-netclass-flag",
};

// KiCad `board.*` (flattened) key -> CSS custom property. The mask/paste layers are
// semi-transparent in KiCad; the extractor (theme.rs) composites them over the board
// background into an opaque hue that matches KiCad's display, so they flow through here
// like every other layer — no alpha to drop.
const BOARD_MAP: Record<string, string> = {
  "copper.f": "--pcb-fcu",
  "copper.b": "--pcb-bcu",
  f_silks: "--pcb-fsilk",
  b_silks: "--pcb-bsilk",
  f_mask: "--pcb-fmask",
  b_mask: "--pcb-bmask",
  f_paste: "--pcb-fpaste",
  b_paste: "--pcb-bpaste",
  f_fab: "--pcb-ffab",
  b_fab: "--pcb-bfab",
  f_crtyd: "--pcb-fcrtyd",
  b_crtyd: "--pcb-bcrtyd",
  edge_cuts: "--pcb-edge",
  via_hole_walls: "--pcb-hole",
};

const ALL_VARS = [...Object.values(SCH_MAP), ...Object.values(BOARD_MAP)];

/** KiCad supports up to 30 inner copper layers (In1.Cu..In30.Cu). Their theme colours
 *  are applied dynamically (see applyKicadTheme), so this only bounds the clear loop. */
const MAX_INNER_COPPER = 30;

/** Apply the resolved KiCad palette as `:root` CSS variables, overriding the
 *  KiCad-Default values in tokens.css. Colours the theme omits fall back to
 *  tokens.css. Pass `null`/`undefined` (theme-less bundle, or no KiCad config
 *  found) to revert to the defaults. */
export function applyKicadTheme(theme: KicadTheme | null | undefined): void {
  const root = document.documentElement;
  // Clear first so switching project/theme never leaves a stale override behind.
  for (const v of ALL_VARS) root.style.removeProperty(v);
  // Inner-copper vars are set dynamically (below), so clear the whole KiCad range too.
  for (let i = 1; i <= MAX_INNER_COPPER; i++) root.style.removeProperty(`--pcb-in${i}`);
  if (!theme) return;
  const apply = (src: Record<string, string> | undefined, map: Record<string, string>) => {
    if (!src) return;
    for (const [key, cssVar] of Object.entries(map)) {
      const hex = src[key];
      if (hex) root.style.setProperty(cssVar, hex);
    }
  };
  apply(theme.schematic, SCH_MAP);
  apply(theme.board, BOARD_MAP);
  // Inner copper: KiCad carries copper.in1..inN; map each present one to --pcb-in{N}
  // generically so EVERY inner layer of a multi-layer board gets its real theme colour
  // (not just in1/in2). layerColorVar reads the same --pcb-in{N} for each InN.Cu layer.
  if (theme.board)
    for (const [key, hex] of Object.entries(theme.board)) {
      const m = key.match(/^copper\.in(\d+)$/);
      if (m && hex) root.style.setProperty(`--pcb-in${m[1]}`, hex);
    }
}

/** Remove every KiCad-theme override, reverting the viewer to tokens.css. */
export function clearKicadTheme(): void {
  applyKicadTheme(null);
}
