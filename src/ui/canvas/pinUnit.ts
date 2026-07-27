// Which *placed symbol* owns a given pin — the multi-unit disambiguator for pin
// selections (PCB pad → schematic pin cross-probe, and clicking a pin in-schematic).
//
// A multi-unit part (U12.A/.B/.C) is placed as one symbol group per unit, each
// carrying only that unit's pin glyphs. The design index, though, keys components by
// designator and so collapses U12 onto a single placement (whichever the extractor
// emitted last — U12.C for a 3-unit MCU). Highlighting from the index therefore lights
// up the wrong unit for a pin that lives on another one; resolving through the pin's
// own DOM ancestry gets the unit right without needing a per-unit index.

/** CSS-escape a value for use inside an attribute selector. */
export function cssEsc(s: string): string {
  return typeof CSS !== "undefined" && CSS.escape ? CSS.escape(s) : s.replace(/["\\]/g, "\\$&");
}

/** The symbol groups a pin glyph may belong to. Power symbols are included so a
 *  power-flag pin resolves to its flag rather than to nothing. */
const SYMBOL_SEL = 'g[data-primitive="symbol"], g[data-primitive="power-symbol"]';

/**
 * The schematic group uuid of the symbol that owns `dsg`'s pin `pin` within `root`,
 * or undefined when that pin isn't on this sheet.
 *
 * Note the climb targets the *symbol* primitive specifically: a pin group carries its
 * own `data-uuid` (the pin's uuid), so a plain `g[data-uuid]` ancestor lookup returns
 * the pin itself — a uuid that matches no symbol and paints no highlight.
 */
export function pinUnitUuid(
  root: ParentNode | null | undefined,
  dsg: string,
  pin: string,
): string | undefined {
  const el = root?.querySelector(
    `[data-designator="${cssEsc(dsg)}"][data-pin="${cssEsc(pin)}"]`,
  );
  return el?.closest(SYMBOL_SEL)?.getAttribute("data-uuid") ?? undefined;
}
