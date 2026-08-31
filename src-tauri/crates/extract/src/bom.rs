//! BOM synthesis from the design model.
//!
//! Two products:
//!  * a **grouped** BOM (CSV + JSON) for the viewer app — coalesces identical
//!    parts into line items;
//!  * an **enriched review BOM** (CSV) for the AI review skills — resolves the
//!    sourcing/compliance fields scattered across each part's free-form
//!    parameters into stable columns, using a hybrid resolver (name-token
//!    scoring + value shape + fill rate) with an audit sidecar.
//!
//! No JLCPCB/parts-database lookup is performed: manufacturer/MPN come only from
//! explicit symbol properties; distributor part numbers (LCSC, Mouser, …) are
//! surfaced as-is in their own columns.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::design::Component;

/// Classification buckets excluded from the BOM (mechanical / fab artefacts).
const EXCLUDED_CLASSES: &[&str] = &["mounting_hole", "fiducial", "test_point", "pcb"];

/// A resolved sourcing/compliance field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Mpn,
    /// A documented second source. Kept separate from `Mpn` (which negates
    /// "alternate") so a part with an alternate is not read as unsourced, and so a
    /// single-source risk finding is not filed against a part that has two.
    MpnAlt,
    Manufacturer,
    Datasheet,
    Lifecycle,
    Msl,
    Rohs,
    Reach,
    Aecq,
}

impl Field {
    fn all() -> [Field; 9] {
        use Field::*;
        [Mpn, MpnAlt, Manufacturer, Datasheet, Lifecycle, Msl, Rohs, Reach, Aecq]
    }

    fn label(self) -> &'static str {
        match self {
            Field::Mpn => "mpn",
            Field::MpnAlt => "mpn_alt",
            Field::Manufacturer => "manufacturer",
            Field::Datasheet => "datasheet",
            Field::Lifecycle => "lifecycle",
            Field::Msl => "msl",
            Field::Rohs => "rohs",
            Field::Reach => "reach",
            Field::Aecq => "aecq",
        }
    }

    /// (positive substrings, negative substrings, prefers-URL-shaped-values).
    fn spec(self) -> (&'static [&'static str], &'static [&'static str], bool) {
        match self {
            Field::Mpn => (
                &["mpn", "partnumber", "partno", "ordernumber", "orderingcode", "orderno"],
                &["alternate", "supplier", "legacy", "deviceid", "internal", "distributor"],
                false,
            ),
            // "Alternate part 1" (KiCad) / "Alternate MPN" / "MPN2". The negatives keep
            // the companion "Alternate part 1 Manufacturer" column out of this field.
            Field::MpnAlt => (
                &["alternatepart", "alternatempn", "altpart", "altmpn", "mpn2", "secondsource"],
                &["manufacturer", "mfr", "mfg", "supplier", "distributor"],
                false,
            ),
            Field::Manufacturer => (
                &["manufacturer", "mfr", "mfg", "vendor", "maker", "brand"],
                &["part", "order", "status", "number"],
                false,
            ),
            Field::Datasheet => (&["datasheet"], &[], true),
            Field::Lifecycle => (
                &["lifecycle", "lifestatus", "partstatus", "productstatus", "componentstatus", "plmstatus", "obsolete"],
                &[],
                false,
            ),
            Field::Msl => (&["msl", "moisturesensitiv"], &[], false),
            Field::Rohs => (&["rohs"], &[], false),
            Field::Reach => (&["reach", "svhc"], &[], false),
            Field::Aecq => (&["aec", "automotive"], &[], false),
        }
    }
}

/// Distributor key aliases -> canonical column name.
fn distributor_name(nkey: &str) -> Option<&'static str> {
    const ALIASES: &[(&str, &str)] = &[
        ("lcsc", "LCSC"),
        ("jlcpcb", "LCSC"),
        ("mouser", "Mouser"),
        ("digikey", "Digi-Key"),
        ("dig1key", "Digi-Key"),
        ("newark", "Newark"),
        ("farnell", "Newark"),
        ("arrow", "Arrow"),
    ];
    ALIASES
        .iter()
        .find(|(a, _)| nkey.contains(a))
        .map(|(_, name)| *name)
}

/// Normalise a parameter key for matching (lowercase alphanumerics only).
fn norm(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Treat KiCad placeholders as empty.
fn clean(value: &str) -> &str {
    let v = value.trim();
    if v == "~" || v.eq_ignore_ascii_case("n/a") {
        ""
    } else {
        v
    }
}

fn is_url(value: &str) -> bool {
    let v = value.trim();
    v.starts_with("http://") || v.starts_with("https://")
}

/// The resolved mapping: each logical field to an ordered list of source keys,
/// plus the detected distributor columns.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    /// Field label -> ordered candidate parameter keys (primary first).
    pub fields: BTreeMap<&'static str, Vec<String>>,
    /// Canonical distributor name -> the parameter key carrying its part number.
    pub distributors: BTreeMap<String, String>,
}

/// Score parameter keys into logical fields and detect distributor columns.
pub fn resolve_mapping(components: &[Component]) -> Mapping {
    // Collect every parameter key with its normalised form and fill rate.
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for c in components {
        for k in c.parameters.keys() {
            keys.insert(k.clone());
        }
    }
    let total = components.len().max(1) as f64;
    let fill = |key: &str| -> f64 {
        let n = components
            .iter()
            .filter(|c| c.parameters.get(key).map(|v| !clean(v).is_empty()).unwrap_or(false))
            .count();
        n as f64 / total
    };

    let mut mapping = Mapping::default();

    for field in Field::all() {
        let (pos, neg, url) = field.spec();
        let mut scored: Vec<(f64, String)> = Vec::new();
        for key in &keys {
            let nkey = norm(key);
            if neg.iter().any(|t| nkey.contains(t)) {
                continue;
            }
            let name_hit = pos.iter().any(|t| nkey.contains(t));
            let f = fill(key);
            let score = if name_hit {
                2.0 + f
            } else if url {
                // datasheet by value shape only
                let urls = components
                    .iter()
                    .filter_map(|c| c.parameters.get(key))
                    .filter(|v| is_url(v))
                    .count() as f64;
                if urls / total > 0.5 {
                    1.0 + f
                } else {
                    continue;
                }
            } else {
                continue;
            };
            scored.push((score, key.clone()));
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        if !scored.is_empty() {
            mapping
                .fields
                .insert(field.label(), scored.into_iter().map(|(_, k)| k).collect());
        }
    }

    for key in &keys {
        if let Some(name) = distributor_name(&norm(key)) {
            // First key wins for a given canonical distributor.
            mapping
                .distributors
                .entry(name.to_string())
                .or_insert_with(|| key.clone());
        }
    }

    mapping
}

impl Mapping {
    /// Resolve a field for one component: first non-empty value across its keys.
    /// Datasheet prefers a URL-shaped value.
    fn value(&self, field: Field, c: &Component) -> String {
        let Some(keys) = self.fields.get(field.label()) else {
            return String::new();
        };
        if field == Field::Datasheet {
            for k in keys {
                if let Some(v) = c.parameters.get(k) {
                    if is_url(v) {
                        return v.trim().to_string();
                    }
                }
            }
        }
        for k in keys {
            if let Some(v) = c.parameters.get(k) {
                let v = clean(v);
                if !v.is_empty() {
                    return v.to_string();
                }
            }
        }
        String::new()
    }

    fn distributor_value(&self, name: &str, c: &Component) -> String {
        self.distributors
            .get(name)
            .and_then(|k| c.parameters.get(k))
            .map(|v| clean(v).to_string())
            .unwrap_or_default()
    }
}

/// True if a component should appear in the BOM.
fn in_bom(c: &Component) -> bool {
    if c.designator.starts_with('!') {
        return false;
    }
    if EXCLUDED_CLASSES.contains(&c.classification.kind.as_str()) {
        return false;
    }
    c.parameters.get("kicad_in_bom").map(|v| v != "false").unwrap_or(true)
}

/// One entry per physical part, in source order.
///
/// A multi-unit symbol (`U9.A`/`.B`/`.C`) is placed once per unit and so appears
/// once per unit in the design model — each unit carries its own geometry, which
/// the schematic view needs. The BOM counts parts, not units, so units past the
/// first for a given designator are dropped here. KiCad keeps a symbol's fields
/// in sync across its units, so the first unit's properties speak for the part.
fn bom_parts(components: &[Component]) -> Vec<&Component> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    components
        .iter()
        .filter(|c| in_bom(c))
        .filter(|c| seen.insert(c.designator.as_str()))
        .collect()
}

fn is_dnp(c: &Component) -> bool {
    c.parameters.get("kicad_dnp").map(|v| v == "true").unwrap_or(false)
}

/// Natural comparison of references (`R2` < `R10` < `U1`).
fn nat_key(r: &str) -> (String, u64, String) {
    let prefix: String = r.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = &r[prefix.len()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let tail = &rest[num.len()..];
    (prefix, num.parse().unwrap_or(0), tail.to_string())
}

fn sort_refs(refs: &mut [String]) {
    refs.sort_by(|a, b| nat_key(a).cmp(&nat_key(b)));
}

// ---------------------------------------------------------------------------
// Enriched review BOM
// ---------------------------------------------------------------------------

/// One enriched review BOM line.
#[derive(Debug, Clone)]
pub struct EnrichedRow {
    pub references: Vec<String>,
    pub quantity: u32,
    pub value: String,
    pub footprint: String,
    pub description: String,
    pub manufacturer: String,
    pub mpn: String,
    /// Documented second source, when the design records one. Empty otherwise.
    pub mpn_alt: String,
    pub datasheet: String,
    pub aecq: String,
    pub rohs: String,
    pub reach: String,
    pub lifecycle: String,
    pub msl: String,
    pub dnp: bool,
    pub distributors: BTreeMap<String, String>,
}

/// Build the enriched review BOM and the list of distributor columns present.
pub fn build_enriched(components: &[Component], mapping: &Mapping) -> (Vec<EnrichedRow>, Vec<String>) {
    let dist_cols: Vec<String> = mapping.distributors.keys().cloned().collect();

    // Group identical parts.
    type Key = (String, String, String, String, bool);
    let mut groups: BTreeMap<Key, EnrichedRow> = BTreeMap::new();
    for c in bom_parts(components) {
        let manufacturer = mapping.value(Field::Manufacturer, c);
        let mpn = mapping.value(Field::Mpn, c);
        let dnp = is_dnp(c);
        let key = (
            manufacturer.to_lowercase(),
            mpn.to_lowercase(),
            c.value.to_lowercase(),
            c.footprint.to_lowercase(),
            dnp,
        );
        let entry = groups.entry(key).or_insert_with(|| EnrichedRow {
            references: Vec::new(),
            quantity: 0,
            value: c.value.clone(),
            footprint: c.footprint.clone(),
            description: c.description.clone(),
            manufacturer: manufacturer.clone(),
            mpn: mpn.clone(),
            mpn_alt: mapping.value(Field::MpnAlt, c),
            datasheet: mapping.value(Field::Datasheet, c),
            aecq: mapping.value(Field::Aecq, c),
            rohs: mapping.value(Field::Rohs, c),
            reach: mapping.value(Field::Reach, c),
            lifecycle: mapping.value(Field::Lifecycle, c),
            msl: mapping.value(Field::Msl, c),
            dnp,
            distributors: BTreeMap::new(),
        });
        entry.references.push(c.designator.clone());
        entry.quantity += 1;
        // First non-empty wins for fields that may be sparse within a group.
        fill_if_empty(&mut entry.description, &c.description);
        // Sparse within a group: one member of a grouped line may carry the alternate.
        fill_if_empty(&mut entry.mpn_alt, &mapping.value(Field::MpnAlt, c));
        for name in &dist_cols {
            let v = mapping.distributor_value(name, c);
            if !v.is_empty() {
                entry.distributors.entry(name.clone()).or_insert(v);
            }
        }
    }

    let mut rows: Vec<EnrichedRow> = groups.into_values().collect();
    for r in &mut rows {
        sort_refs(&mut r.references);
    }
    rows.sort_by(|a, b| nat_key(&a.references[0]).cmp(&nat_key(&b.references[0])));
    (rows, dist_cols)
}

fn fill_if_empty(slot: &mut String, candidate: &str) {
    if slot.is_empty() && !candidate.is_empty() {
        *slot = candidate.to_string();
    }
}

/// Render the enriched BOM to CSV.
pub fn enriched_csv(rows: &[EnrichedRow], dist_cols: &[String]) -> String {
    let mut header = vec![
        "Reference",
        "Quantity",
        "Value",
        "Footprint",
        "Description",
        "Manufacturer",
        "Manufacturer Part Number",
        "Alternate MPN",
        "Datasheet",
        "AEC-Q",
        "RoHS",
        "REACH",
        "Lifecycle",
        "MSL",
        "DNP",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    header.extend(dist_cols.iter().cloned());

    let mut out = String::new();
    out.push_str(&csv_record(&header));
    for r in rows {
        let mut rec = vec![
            r.references.join(", "),
            r.quantity.to_string(),
            r.value.clone(),
            r.footprint.clone(),
            r.description.clone(),
            r.manufacturer.clone(),
            r.mpn.clone(),
            r.mpn_alt.clone(),
            r.datasheet.clone(),
            r.aecq.clone(),
            r.rohs.clone(),
            r.reach.clone(),
            r.lifecycle.clone(),
            r.msl.clone(),
            if r.dnp { "DNP".into() } else { String::new() },
        ];
        for name in dist_cols {
            rec.push(r.distributors.get(name).cloned().unwrap_or_default());
        }
        out.push_str(&csv_record(&rec));
    }
    out
}

fn csv_record(fields: &[String]) -> String {
    let mut line = fields.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(",");
    line.push('\n');
    line
}

fn csv_field(f: &str) -> String {
    if f.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", f.replace('"', "\"\""))
    } else {
        f.to_string()
    }
}

/// The mapping audit, written as a `.mapping.json` sidecar for the agent to scan.
///
/// Three keys, and the third is the one this function was missing.
///
/// * `fields` — for each logical field, the source-BOM column that fed it, the
///   columns that would have been consulted after it, and how full that column was.
/// * `distributors` — the detected distributor part-number columns.
/// * `unmapped_columns` — **every parameter key no field and no distributor
///   consumed, with its fill rate.** This is new (SpinZero plan item B5).
///
/// Why the third one matters more than it looks. A reviewer downstream cannot tell
/// "this BOM has no tolerance data" from "this BOM has a `Tol.` column we did not
/// recognise" — the two are byte-identical once the enriched CSV is written, and the
/// first produces a page of confident findings about data that was sitting right
/// there. The reader (`validateBundle.ts`) has had a check for exactly this since it
/// was written, and it could never fire, because nothing ever emitted the key. A
/// well-filled column mapped to nothing is the single most useful thing a mapping
/// audit can say, and it was the one thing this report did not say.
///
/// Fill rate, not raw count, and reported for every unconsumed column rather than
/// only the full ones: the threshold for "well filled" is a review-side policy, and
/// baking it in here would mean changing the extractor to change a judgment call.
pub fn mapping_report(mapping: &Mapping, components: &[Component]) -> serde_json::Value {
    let total = components.len().max(1) as f64;
    let fill = |key: &str| -> f64 {
        let n = components
            .iter()
            .filter(|c| c.parameters.get(key).map(|v| !clean(v).is_empty()).unwrap_or(false))
            .count();
        n as f64 / total
    };
    let coverage = |key: &str| -> u32 { (fill(key) * 100.0).round() as u32 };

    let fields: serde_json::Map<String, serde_json::Value> = mapping
        .fields
        .iter()
        .map(|(f, keys)| {
            let primary = &keys[0];
            (
                f.to_string(),
                serde_json::json!({
                    "primary": primary,
                    "fallbacks": &keys[1..],
                    "coverage_pct": coverage(primary),
                }),
            )
        })
        .collect();

    // Consumed = every candidate key of every field (not just the primary — a
    // fallback IS consulted, so reporting it as unmapped would be a false alarm) plus
    // every distributor column.
    let mut consumed: BTreeSet<&str> = BTreeSet::new();
    for keys in mapping.fields.values() {
        for k in keys {
            consumed.insert(k.as_str());
        }
    }
    for k in mapping.distributors.values() {
        consumed.insert(k.as_str());
    }

    let mut all_keys: BTreeSet<&str> = BTreeSet::new();
    for c in components {
        for k in c.parameters.keys() {
            all_keys.insert(k.as_str());
        }
    }

    // Sorted by fill rate descending, so the column most likely to be a real mapping
    // miss is first and a truncating reader still sees it. `BTreeSet` gives a stable
    // name order to break ties, so two runs of one board produce the same file.
    let mut unmapped: Vec<serde_json::Value> = all_keys
        .into_iter()
        .filter(|k| !consumed.contains(k))
        .map(|k| {
            serde_json::json!({
                "column": k,
                "fill_rate": (fill(k) * 1000.0).round() / 1000.0,
            })
        })
        .collect();
    unmapped.sort_by(|a, b| {
        let fa = a["fill_rate"].as_f64().unwrap_or(0.0);
        let fb = b["fill_rate"].as_f64().unwrap_or(0.0);
        fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal)
    });

    serde_json::json!({
        "fields": fields,
        "distributors": mapping.distributors,
        "unmapped_columns": unmapped,
    })
}

// ---------------------------------------------------------------------------
// Grouped BOM (app)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GroupedLine {
    item: u32,
    quantity: u32,
    designators: Vec<String>,
    dnp: bool,
    fields: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct GroupedSource {
    path: String,
    name: String,
    stem: String,
}

/// The grouped BOM document the app ingests.
#[derive(Serialize)]
pub struct GroupedBom {
    schema: String,
    source: GroupedSource,
    variant: Option<String>,
    pub line_count: u32,
    pub component_count: u32,
    pub dnp_line_count: u32,
    lines: Vec<GroupedLine>,
}

/// Shown for a field whose members disagree inside one grouped line (KiCad's Symbol
/// Fields Table shows a similar marker rather than an arbitrary member's value).
pub const MIXED_VALUES: &str = "-- mixed values --";

/// The per-line field map contributed by one component: its own symbol parameters plus
/// the resolved sourcing fields (which take precedence).
fn line_fields(c: &Component, mapping: &Mapping) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    // Carry the symbol's own parameters so custom preset columns (MSL,
    // Automotive Grade, …) have values; skip internal KiCad bookkeeping.
    for (k, v) in &c.parameters {
        if k.starts_with("kicad_") {
            continue;
        }
        let v = clean(v);
        if !v.is_empty() {
            fields.insert(k.clone(), v.to_string());
        }
    }
    fields.insert("value".into(), c.value.clone());
    fields.insert("footprint".into(), c.footprint.clone());
    fields.insert("description".into(), c.description.clone());
    let mpn = mapping.value(Field::Mpn, c);
    if !mpn.is_empty() {
        fields.insert("manufacturer_part_number".into(), mpn);
    }
    let mfr = mapping.value(Field::Manufacturer, c);
    if !mfr.is_empty() {
        fields.insert("manufacturer".into(), mfr);
    }
    // Mirror the app's expectation: LCSC part lands as jlcpcb_part_number.
    let lcsc = mapping.distributor_value("LCSC", c);
    if !lcsc.is_empty() {
        fields.insert("jlcpcb_part_number".into(), lcsc);
    }
    fields
}

/// Fold another member's fields into the line's: a field the two disagree on (including
/// one carrying it and the other not) collapses to `MIXED_VALUES`. Comparison ignores
/// case, so the case-insensitive grouping key can't make its own fields look mixed.
fn merge_fields(into: &mut BTreeMap<String, String>, add: &BTreeMap<String, String>) {
    for (k, v) in add {
        match into.get(k) {
            Some(cur) if cur == MIXED_VALUES || cur.eq_ignore_ascii_case(v) => {}
            Some(_) => {
                into.insert(k.clone(), MIXED_VALUES.into());
            }
            None => {
                into.insert(k.clone(), MIXED_VALUES.into());
            }
        }
    }
    // Fields this member is missing entirely are mixed too.
    let missing: Vec<String> = into
        .keys()
        .filter(|k| !add.contains_key(*k))
        .cloned()
        .collect();
    for k in missing {
        into.insert(k, MIXED_VALUES.into());
    }
}

/// Build the per-component BOM document the app ingests: one line per in-BOM component,
/// ungrouped. Grouping is the BOM table's job — it folds these onto the fields the active
/// KiCad preset flags `group_by`, which the extractor cannot know. Pre-grouping here would
/// cap what the table can do: fields already collapsed to `MIXED_VALUES` can never be
/// separated again. The fab CSV keeps its own grouping (`build_grouped`).
pub fn build_flat(components: &[Component], mapping: &Mapping, project_path: &str, stem: &str) -> GroupedBom {
    let mut lines: Vec<GroupedLine> = bom_parts(components)
        .into_iter()
        .map(|c| GroupedLine {
            item: 0,
            quantity: 1,
            designators: vec![c.designator.clone()],
            dnp: is_dnp(c),
            fields: line_fields(c, mapping),
        })
        .collect();
    lines.sort_by(|a, b| nat_key(&a.designators[0]).cmp(&nat_key(&b.designators[0])));
    let dnp_line_count = lines.iter().filter(|l| l.dnp).count() as u32;
    for (i, l) in lines.iter_mut().enumerate() {
        l.item = (i + 1) as u32;
    }
    let count = lines.len() as u32;

    GroupedBom {
        schema: "extract.bom.flat.a0".into(),
        source: GroupedSource {
            path: project_path.to_string(),
            name: format!("{stem}.kicad_pro"),
            stem: stem.to_string(),
        },
        variant: None,
        line_count: count,
        component_count: count,
        dnp_line_count,
        lines,
    }
}

/// Build the grouped BOM for the fab CSV (groups by value/footprint/lib_ref/desc).
pub fn build_grouped(components: &[Component], mapping: &Mapping, project_path: &str, stem: &str) -> GroupedBom {
    type Key = (String, String, String, String, String, bool);
    let mut groups: BTreeMap<Key, GroupedLine> = BTreeMap::new();
    let mut component_count = 0u32;
    for c in bom_parts(components) {
        component_count += 1;
        let dnp = is_dnp(c);
        // MPN participates in the key: two parts that differ only by MPN are
        // distinct BOM lines (matches KiCad's field-based grouping).
        let mpn = mapping.value(Field::Mpn, c);
        let key = (
            c.value.to_lowercase(),
            c.footprint.to_lowercase(),
            c.library_ref.to_lowercase(),
            c.description.to_lowercase(),
            mpn.to_lowercase(),
            dnp,
        );
        let fields = line_fields(c, mapping);
        match groups.entry(key) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(GroupedLine {
                    item: 0,
                    quantity: 1,
                    designators: vec![c.designator.clone()],
                    dnp,
                    fields,
                });
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                let line = e.get_mut();
                merge_fields(&mut line.fields, &fields);
                line.designators.push(c.designator.clone());
                line.quantity += 1;
            }
        }
    }

    let mut lines: Vec<GroupedLine> = groups.into_values().collect();
    for l in &mut lines {
        sort_refs(&mut l.designators);
    }
    lines.sort_by(|a, b| nat_key(&a.designators[0]).cmp(&nat_key(&b.designators[0])));
    let dnp_line_count = lines.iter().filter(|l| l.dnp).count() as u32;
    for (i, l) in lines.iter_mut().enumerate() {
        l.item = (i + 1) as u32;
    }

    GroupedBom {
        schema: "extract.bom.grouped.a0".into(),
        source: GroupedSource {
            path: project_path.to_string(),
            name: format!("{stem}.kicad_pro"),
            stem: stem.to_string(),
        },
        variant: None,
        line_count: lines.len() as u32,
        component_count,
        dnp_line_count,
        lines,
    }
}

/// Render a grouped BOM to the sparse fab-style CSV.
pub fn grouped_csv(bom: &GroupedBom) -> String {
    let mut out = String::new();
    out.push_str(&csv_record(&[
        "mfg".into(),
        "mpn".into(),
        "description".into(),
        "quantity".into(),
        "designators".into(),
    ]));
    for l in &bom.lines {
        out.push_str(&csv_record(&[
            l.fields.get("manufacturer").cloned().unwrap_or_default(),
            l.fields.get("manufacturer_part_number").cloned().unwrap_or_default(),
            l.fields.get("description").cloned().unwrap_or_default(),
            l.quantity.to_string(),
            l.designators.join(", "),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{Classification, Component, Hierarchy};

    fn comp(des: &str, value: &str, fp: &str, kind: &str, params: &[(&str, &str)]) -> Component {
        let mut parameters: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in params {
            parameters.insert((*k).into(), (*v).into());
        }
        parameters.entry("kicad_in_bom".into()).or_insert("true".into());
        Component {
            designator: des.into(),
            svg_id: format!("{des}-uuid"),
            value: value.into(),
            footprint: fp.into(),
            library_ref: "Device:C".into(),
            description: "cap".into(),
            hierarchy: Hierarchy {
                base_designator: des.into(),
                channel: None,
                channel_index: None,
                sheet: "/".into(),
                sheet_path: "/".into(),
                sheet_path_uuids: "/".into(),
            },
            classification: Classification {
                prefix: crate::design::prefix_of(des),
                kind: kind.into(),
                pin_count: 2,
            },
            parameters,
            bbox: None,
        }
    }

    #[test]
    fn groups_and_excludes() {
        let comps = vec![
            comp("C1", "470n", "0603", "passive_2pin", &[("LCSC", "C1623")]),
            comp("C2", "470n", "0603", "passive_2pin", &[("LCSC", "C1623")]),
            comp("MH1", "M2", "Hole", "mounting_hole", &[("kicad_in_bom", "false")]),
            comp("U1", "REG", "SOT23", "ic", &[("Manufacturer", "TI"), ("MPN", "TPS")]),
        ];
        let mapping = resolve_mapping(&comps);
        // LCSC detected as a distributor, MPN/Manufacturer resolved.
        assert_eq!(mapping.distributors.get("LCSC").map(String::as_str), Some("LCSC"));
        assert!(mapping.fields.contains_key("mpn"));
        assert!(mapping.fields.contains_key("manufacturer"));

        let (rows, dist) = build_enriched(&comps, &mapping);
        assert_eq!(dist, vec!["LCSC".to_string()]);
        // C1+C2 coalesce; MH1 excluded; U1 separate -> 2 lines.
        assert_eq!(rows.len(), 2);
        let caps = rows.iter().find(|r| r.value == "470n").unwrap();
        assert_eq!(caps.quantity, 2);
        assert_eq!(caps.references, vec!["C1", "C2"]);
        assert_eq!(caps.distributors.get("LCSC").map(String::as_str), Some("C1623"));
        let u1 = rows.iter().find(|r| r.mpn == "TPS").unwrap();
        assert_eq!(u1.manufacturer, "TI");

        let grouped = build_grouped(&comps, &mapping, "p", "p");
        assert_eq!(grouped.component_count, 3); // MH1 excluded
        assert_eq!(grouped.line_count, 2);
    }

    #[test]
    fn multi_unit_symbol_counts_once() {
        // U9 is a dual op-amp: units A/B plus the power unit, three placements, one part.
        let comps = vec![
            comp("U9", "TLV9062", "SOIC8", "ic", &[("MPN", "TLV9062QDRQ1")]),
            comp("U9", "TLV9062", "SOIC8", "ic", &[("MPN", "TLV9062QDRQ1")]),
            comp("U9", "TLV9062", "SOIC8", "ic", &[("MPN", "TLV9062QDRQ1")]),
            comp("U10", "TLV9062", "SOIC8", "ic", &[("MPN", "TLV9062QDRQ1")]),
        ];
        let mapping = resolve_mapping(&comps);
        let (rows, _) = build_enriched(&comps, &mapping);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].references, vec!["U9", "U10"]);
        assert_eq!(rows[0].quantity, 2);

        let grouped = build_grouped(&comps, &mapping, "p", "p");
        assert_eq!(grouped.component_count, 2);
        assert_eq!(grouped.lines[0].quantity, 2);
        assert_eq!(grouped.lines[0].designators, ["U9", "U10"]);

        let flat = build_flat(&comps, &mapping, "p", "p");
        assert_eq!(flat.line_count, 2);
    }

    #[test]
    fn flat_keeps_one_line_per_component() {
        let comps = vec![
            comp("C2", "470n", "0603", "passive_2pin", &[("MSL", "1")]),
            comp("C1", "470n", "0603", "passive_2pin", &[]),
            comp("MH1", "M2", "Hole", "mounting_hole", &[("kicad_in_bom", "false")]),
        ];
        let mapping = resolve_mapping(&comps);
        let flat = build_flat(&comps, &mapping, "p", "p");
        // MH1 excluded; C1/C2 stay apart so the table can group them per preset.
        assert_eq!(flat.line_count, 2);
        assert_eq!(flat.component_count, 2);
        let refs: Vec<&str> = flat.lines.iter().map(|l| l.designators[0].as_str()).collect();
        assert_eq!(refs, vec!["C1", "C2"]); // natural designator order, items 1..n
        assert_eq!(flat.lines.iter().map(|l| l.item).collect::<Vec<_>>(), vec![1, 2]);
        assert!(flat.lines.iter().all(|l| l.quantity == 1));
        // Nothing is collapsed to a mixed marker: C1 simply has no MSL.
        assert_eq!(flat.lines[0].fields.get("MSL"), None);
        assert_eq!(flat.lines[1].fields.get("MSL").map(String::as_str), Some("1"));
    }

    #[test]
    fn grouped_carries_custom_params_and_splits_on_mpn() {
        let comps = vec![
            comp("R37", "9.1k", "0402", "passive_2pin", &[("MPN", "ERA-2AEB912X"), ("MSL", "1")]),
            comp(
                "R38",
                "9.1k",
                "0402",
                "passive_2pin",
                &[("MPN", "ERA-2AEB912X try"), ("MSL", "1")],
            ),
        ];
        let mapping = resolve_mapping(&comps);
        let grouped = build_grouped(&comps, &mapping, "p", "p");
        // Differing MPN splits the group.
        assert_eq!(grouped.line_count, 2);
        let r38 = grouped.lines.iter().find(|l| l.designators == ["R38"]).unwrap();
        assert_eq!(
            r38.fields.get("manufacturer_part_number").map(String::as_str),
            Some("ERA-2AEB912X try")
        );
        // Custom parameter carried through for preset columns.
        assert_eq!(r38.fields.get("MSL").map(String::as_str), Some("1"));
        // Internal KiCad bookkeeping params are not leaked.
        assert!(!r38.fields.keys().any(|k| k.starts_with("kicad_")));
    }

    #[test]
    fn grouped_marks_fields_that_differ_within_a_group() {
        let comps = vec![
            comp("C1", "470n", "0603", "passive_2pin", &[("MSL", "1"), ("Tol", "10%")]),
            comp("C2", "470n", "0603", "passive_2pin", &[("MSL", "3"), ("Tol", "10%")]),
            // No MSL at all — still a disagreement with the members that carry one.
            comp("C3", "470n", "0603", "passive_2pin", &[("Tol", "10%")]),
        ];
        let mapping = resolve_mapping(&comps);
        let grouped = build_grouped(&comps, &mapping, "p", "p");
        assert_eq!(grouped.line_count, 1);
        let l = &grouped.lines[0];
        assert_eq!(l.quantity, 3);
        assert_eq!(l.designators, ["C1", "C2", "C3"]);
        assert_eq!(l.fields.get("MSL").map(String::as_str), Some(MIXED_VALUES));
        // Fields the members agree on keep their value.
        assert_eq!(l.fields.get("Tol").map(String::as_str), Some("10%"));
        assert_eq!(l.fields.get("value").map(String::as_str), Some("470n"));
    }

    #[test]
    fn negative_guard_keeps_alternate_out_of_mpn() {
        let comps = vec![comp(
            "U1",
            "X",
            "Y",
            "ic",
            &[("Alternate Part Number", "ALT"), ("MPN", "REAL")],
        )];
        let mapping = resolve_mapping(&comps);
        let (rows, _) = build_enriched(&comps, &mapping);
        assert_eq!(rows[0].mpn, "REAL");
        // …and the alternate is captured rather than dropped: a part with a documented
        // second source must not read as single-sourced downstream.
        assert_eq!(rows[0].mpn_alt, "ALT");
    }

    #[test]
    fn kicad_alternate_part_becomes_the_alternate_mpn_column() {
        // The property names KiCad designs actually use (seen on MC-02-CONTROL, where
        // 62 of 394 components carry a documented second source that never reached the
        // review BOM). The companion "… Manufacturer" column must NOT land in mpn_alt.
        let comps = vec![comp(
            "C1",
            "22uF",
            "C_1210",
            "cap",
            &[
                ("MPN", "CL32Y226KAVVPJE"),
                ("Manufacturer", "Samsung"),
                ("Alternate part 1", "TMK325B7226MMHP"),
                ("Alternate part 1 Manufacturer", "Taiyo Yuden"),
            ],
        )];
        let mapping = resolve_mapping(&comps);
        let (rows, _) = build_enriched(&comps, &mapping);
        assert_eq!(rows[0].mpn, "CL32Y226KAVVPJE");
        assert_eq!(rows[0].mpn_alt, "TMK325B7226MMHP");
        assert_eq!(rows[0].manufacturer, "Samsung");

        // The header name is the contract with the rule pack: bom-rules maps
        // "Alternate MPN" to its `mpn_alt` field, which three sourcing rules read.
        let csv = enriched_csv(&rows, &[]);
        let header = csv.lines().next().unwrap();
        assert!(header.contains("Alternate MPN"), "header was: {header}");
        let cols: Vec<&str> = header.split(',').collect();
        let idx = cols.iter().position(|c| *c == "Alternate MPN").unwrap();
        let row: Vec<&str> = csv.lines().nth(1).unwrap().split(',').collect();
        assert_eq!(row[idx], "TMK325B7226MMHP");
    }

    /// B5. The reader has always had a "a well-filled column mapped to nothing" check
    /// and it could never fire, because this key was never written. Assert the three
    /// things that check needs: the column appears, its fill rate is right, and a
    /// column that IS consumed does not appear.
    #[test]
    fn mapping_report_names_the_columns_it_did_not_consume() {
        let components = vec![
            comp("C1", "100n", "0402", "capacitor", &[
                ("MPN", "CC0402KRX7R7BB104"),
                ("Tol.", "10%"),
                ("Internal Stock Code", "UV-0001"),
            ]),
            comp("C2", "1u", "0402", "capacitor", &[
                ("MPN", "CC0402MRX5R5BB105"),
                ("Internal Stock Code", "UV-0002"),
            ]),
        ];
        let mapping = resolve_mapping(&components);
        let report = mapping_report(&mapping, &components);
        let unmapped = report["unmapped_columns"].as_array().expect("unmapped_columns");

        let by_name = |name: &str| -> Option<f64> {
            unmapped
                .iter()
                .find(|u| u["column"] == serde_json::json!(name))
                .and_then(|u| u["fill_rate"].as_f64())
        };

        // A fully-populated column nothing understood — exactly the case the reader's
        // GAP exists for, and exactly the case that has been invisible until now.
        assert_eq!(by_name("Internal Stock Code"), Some(1.0));
        // Half-filled, and reported as such rather than being thresholded away here:
        // "well filled" is the review's policy, not the extractor's.
        assert_eq!(by_name("Tol."), Some(0.5));
        // MPN was consumed, so it must NOT be reported as unmapped. Without this the
        // test would pass on a function that simply listed every column.
        assert_eq!(by_name("MPN"), None);
        // Sorted by fill rate, descending, so a truncating reader sees the worst first.
        let rates: Vec<f64> = unmapped.iter().filter_map(|u| u["fill_rate"].as_f64()).collect();
        assert!(rates.windows(2).all(|w| w[0] >= w[1]), "not sorted by fill rate: {rates:?}");
    }
}
