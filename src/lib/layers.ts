// Layer identity, resolved by ROLE rather than by KiCad's layer names.
//
// A bundle's layer names belong to the tool that drew the board: KiCad calls the
// front copper "F.Cu", Altium calls it whatever the designer named the top of
// the stack ("L1_Top", "TOP", "Layer 1"). The review vocabulary — `role` and
// `side` — is the same either way, so every place that used to test a name tests
// a role here instead. That is a correctness fix on its own: a KiCad board with
// a renamed user layer was already being classified by its spelling.
//
// Old bundles carry no `role`/`side`, so each helper falls back to the KiCad
// naming convention it replaces.

import type { LayerLite } from "./design";

/** Board side of a layer: the manifest's own answer, else KiCad's naming. */
export function layerSide(l: LayerLite): "front" | "back" | "inner" | undefined {
  if (l.side === "front" || l.side === "back" || l.side === "inner") return l.side;
  if (/^In\d+\.Cu$/i.test(l.name)) return "inner";
  if (/^F\./i.test(l.name)) return "front";
  if (/^B\./i.test(l.name)) return "back";
  return undefined;
}

/** True for a copper layer — where nets, tracks and pads live. */
export function isCopperLayer(l: LayerLite): boolean {
  return l.role ? l.role === "copper" : l.name.endsWith(".Cu");
}

/** The outer copper layer on a side, for cross-probing into the board. Falls
 *  back to the first copper layer so a board with an unlabelled stack still
 *  activates something rather than nothing. */
export function outerCopper(layers: LayerLite[], side: "front" | "back"): string | undefined {
  const copper = layers.filter(isCopperLayer);
  return (
    copper.find((l) => layerSide(l) === side)?.name ??
    (side === "back" ? copper[copper.length - 1] : copper[0])?.name
  );
}

/** The layer carrying the board profile. Altium draws it on a mechanical layer
 *  named for the board shape, so only the role can find it. */
export function edgeLayer(layers: LayerLite[]): string | undefined {
  return (layers.find((l) => l.role === "edge") ?? layers.find((l) => l.name === "Edge.Cuts"))
    ?.name;
}

/** Layers that start hidden: the fabrication and documentation clutter — mask,
 *  paste, fab, courtyard and user/mechanical layers. They are extracted and one
 *  toggle away; they just do not bury the copper on first open. */
export function hiddenByDefault(l: LayerLite): boolean {
  if (l.role) return ["mask", "paste", "fab", "courtyard", "user"].includes(l.role);
  return (
    /\.(CrtYd|Courtyard|Fab|Paste|Mask|Adhes)$/i.test(l.name) ||
    /^(User\.|Dwgs\.User|Cmts\.User|Eco[12]\.User|Margin$)/i.test(l.name)
  );
}
