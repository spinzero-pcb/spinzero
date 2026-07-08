// Re-label re-used hierarchical sheet instances on the schematic canvas.
//
// The extractor renders ONE SVG per source .kicad_sch file and bakes each symbol's
// BASE reference (the file's `Reference` property, e.g. R10) into it. But a sheet
// instantiated several times shows a different designator per instance (R274 in
// bank2/io78, R49 elsewhere, …). The viewer already resolves the per-instance
// designator for the current sheet (that's why clicks land on the right part); this
// applies the same answer to the baked label + identity attributes so the canvas
// reads like KiCad instead of repeating the base ref on every instance.

/** Rewrite each placed symbol's visible reference and identity attributes to the
 *  current sheet instance's designator. `designatorFor` maps a symbol's SVG uuid to
 *  the designator on the current sheet (the canvas's per-sheet `compOf`). Idempotent:
 *  a symbol already carrying its resolved designator is left untouched, so repeated
 *  calls — and single-instance / root sheets, where base == instance — are no-ops. */
export function relabelInstances(
  svg: ParentNode,
  designatorFor: (uuid: string) => string | undefined,
): void {
  for (const node of svg.querySelectorAll('g[data-primitive="symbol"][data-uuid]')) {
    const sym = node as SVGElement;
    const uuid = sym.dataset.uuid;
    if (!uuid) continue;
    const want = designatorFor(uuid);
    const have = sym.dataset.ref;
    // Only rewrite when we have a different, resolved designator for this instance.
    if (!want || !have || want === have) continue;
    // The visible reference is the symbol group's own <text> equal to the baked base
    // ref. Value/other fields hold different text; pin texts live in nested pin
    // groups, so `:scope > text` (direct children) can't touch them.
    for (const t of sym.querySelectorAll(":scope > text")) {
      if ((t.textContent ?? "").trim() === have) {
        t.textContent = want;
        break;
      }
    }
    // Keep identity attributes in sync so pin clicks, net-terminal lookups and
    // PCB→pin cross-probes resolve to this instance's designator too.
    sym.dataset.ref = want;
    for (const pin of sym.querySelectorAll('g[data-primitive="pin"][data-designator]')) {
      (pin as SVGElement).dataset.designator = want;
    }
  }
}
