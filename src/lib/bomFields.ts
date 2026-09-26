// Display order and labels for the logical BOM fields the rules read.

/** Rule inputs in the order that matters for reading a BOM, then the rest. The
 *  backend sorts alphabetically, which puts `aecq` above `mpn` — useless as an
 *  opening impression. Anything not listed keeps alphabetical order after these. */
const FIELD_ORDER = [
  "reference",
  "value",
  "footprint",
  "quantity",
  "mpn",
  "manufacturer",
  "description",
  "datasheet",
  "lifecycle",
  "aecq",
  "rohs",
  "reach",
  "msl",
];

/** Human labels for the logical fields. A field with no entry falls back to its own
 *  name, so a new rule input shows up readably without a change here. */
const FIELD_LABEL: Record<string, string> = {
  reference: "Designators",
  value: "Value",
  footprint: "Footprint",
  quantity: "Quantity",
  mpn: "MPN",
  mpn_alt: "Alternate MPN",
  manufacturer: "Manufacturer",
  description: "Description",
  datasheet: "Datasheet",
  lifecycle: "Lifecycle",
  aecq: "AEC-Q",
  rohs: "RoHS",
  reach: "REACH",
  msl: "MSL",
  dnp: "Do not populate",
  exclude_from_bom: "Exclude from BOM",
  voltage: "Voltage rating",
  tolerance: "Tolerance",
  package: "Package",
};

export function bomFieldLabel(logical: string): string {
  return FIELD_LABEL[logical] ?? logical;
}

export function bomFieldRank(logical: string): number {
  const i = FIELD_ORDER.indexOf(logical);
  return i === -1 ? FIELD_ORDER.length : i;
}
