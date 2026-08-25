//! BOM data model: one normalized row (`BomItem`), the per-rule context, severities.
//!
//! Mirrors `bom_model.py`. Logical field names (`mpn`, `lifecycle`, …) are the only
//! thing rules see; mapping real column headers onto them happens in `load.rs`.

use std::collections::{BTreeMap, BTreeSet};

use crate::re;

/// Findings-schema severity. `Question` exists in the schema but no deterministic
/// rule emits it — judgment stages do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Critical,
    Major,
    Medium,
    Low,
    Question,
}

impl Severity {
    /// Accepts both the config vocabulary (BLOCKER/CRITICAL/MAJOR/MINOR/INFO) and
    /// already-normalized schema values. Unknown text degrades to `Low` rather than
    /// failing a run — a hand-edited profile must never break the check.
    pub fn parse(raw: &str) -> Severity {
        match raw.trim().to_ascii_uppercase().as_str() {
            "BLOCKER" | "CRITICAL" => Severity::Critical,
            "MAJOR" => Severity::Major,
            "MINOR" | "MEDIUM" => Severity::Medium,
            "INFO" | "LOW" => Severity::Low,
            "QUESTION" => Severity::Question,
            _ => Severity::Low,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "Critical",
            Severity::Major => "Major",
            Severity::Medium => "Medium",
            Severity::Low => "Low",
            Severity::Question => "Question",
        }
    }

    /// Sort rank — Critical first, matching the schema's severity order.
    pub fn rank(self) -> u8 {
        match self {
            Severity::Critical => 0,
            Severity::Major => 1,
            Severity::Medium => 2,
            Severity::Low => 3,
            Severity::Question => 4,
        }
    }
}

/// One BOM row, every field normalized to its logical name.
#[derive(Clone, Debug, Default)]
pub struct BomItem {
    /// All designators on the line ("R1, R2, R3" gives three entries).
    pub refs: Vec<String>,
    /// The first designator — what a per-item finding anchors to.
    pub reference: String,
    pub quantity: String,
    pub quantity_int: Option<i64>,
    pub dnp: bool,
    pub exclude_from_bom: bool,
    /// (supplier label, part number) for every distributor column that carries one.
    pub supplier_pns: Vec<(String, String)>,
    /// Logical field name -> verbatim cell value (non-empty entries only).
    fields: BTreeMap<String, String>,
    /// Logical field names present (non-empty) on this row, plus `supplier_pns` and
    /// each supplier label lower-cased — the same `present` set the Python rules read.
    pub present: BTreeSet<String>,
}

/// Scalar logical fields a rule may address by name.
pub const SCALAR_FIELDS: &[&str] = &[
    "reference",
    "value",
    "footprint",
    "quantity",
    "manufacturer",
    "mpn",
    "mpn_alt",
    "datasheet",
    "description",
    "msl",
    "lifecycle",
    "aecq",
    "rohs",
    "reach",
    "dnp",
    "exclude_from_bom",
    "voltage",
    "tolerance",
    "package",
];

const FALSE_STRINGS: &[&str] = &["0", "false", "no", "n", ""];
/// Values meaning "this part IS populated" — seen in Population/Fitted-style columns
/// that share aliases with DNP. Must not read as DNP=true.
const POPULATED_STRINGS: &[&str] = &[
    "populate", "populated", "fit", "fitted", "place", "placed", "mount", "mounted",
    "stuff", "stuffed",
];

/// Parts that are on the board but never purchased as components: test points,
/// mounting holes, fiducials, logos, net ties, solder jumpers — and the bare board
/// itself, which real BOMs carry as a line but which has no manufacturer/MPN/MSL of
/// the component kind. Sourcing/datasheet/MSL/compliance rules must not hold these
/// to part standards.
const NON_ORDERABLE_PREFIXES: &[&str] =
    &["TP", "FID", "LOGO", "MH", "MK", "NT", "H", "G", "PCB", "BRD"];
const NON_ORDERABLE_KEYWORDS: &[&str] = &[
    "mountinghole", "mounting_hole", "mounting hole", "fiducial", "testpoint",
    "test_point", "test point", "nettie", "net_tie", "net tie", "solderjumper",
    "solder_jumper", "logo",
];

impl BomItem {
    /// Build a row from logical-field -> raw-cell pairs. Empty cells are dropped, so
    /// `filled()` and `present` mean "carries data", not "column exists".
    pub fn new(raw: BTreeMap<String, String>, supplier_pns: Vec<(String, String)>) -> BomItem {
        let get = |k: &str| raw.get(k).map(|s| s.trim()).unwrap_or("").to_string();

        let refs: Vec<String> = get("reference")
            .split(',')
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect();
        let reference = refs.first().cloned().unwrap_or_default();

        let quantity = get("quantity");
        let quantity_int = quantity.parse::<i64>().ok();

        let dnp_raw = get("dnp").to_lowercase();
        let dnp = !dnp_raw.is_empty()
            && !FALSE_STRINGS.contains(&dnp_raw.as_str())
            && !POPULATED_STRINGS.contains(&dnp_raw.as_str());
        let exclude_raw = get("exclude_from_bom").to_lowercase();
        let exclude_from_bom =
            !exclude_raw.is_empty() && !FALSE_STRINGS.contains(&exclude_raw.as_str());

        let mut fields = BTreeMap::new();
        let mut present = BTreeSet::new();
        for name in SCALAR_FIELDS {
            let v = get(name);
            if !v.is_empty() {
                present.insert((*name).to_string());
                fields.insert((*name).to_string(), v);
            }
        }
        if !supplier_pns.is_empty() {
            present.insert("supplier_pns".into());
            for (supplier, _) in &supplier_pns {
                present.insert(supplier.to_lowercase());
            }
        }

        BomItem {
            refs,
            reference,
            quantity,
            quantity_int,
            dnp,
            exclude_from_bom,
            supplier_pns,
            fields,
            present,
        }
    }

    pub fn filled(&self, field: &str) -> bool {
        self.present.contains(field)
    }

    /// Verbatim cell for a logical field ("" when absent) — the dynamic accessor the
    /// config-driven rules (`fields: [mpn, description]`) need.
    pub fn field(&self, name: &str) -> &str {
        self.fields.get(name).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn value(&self) -> &str {
        self.field("value")
    }
    pub fn footprint(&self) -> &str {
        self.field("footprint")
    }
    pub fn manufacturer(&self) -> &str {
        self.field("manufacturer")
    }
    pub fn mpn(&self) -> &str {
        self.field("mpn")
    }
    pub fn datasheet(&self) -> &str {
        self.field("datasheet")
    }
    pub fn description(&self) -> &str {
        self.field("description")
    }
    pub fn lifecycle(&self) -> &str {
        self.field("lifecycle")
    }
    pub fn aecq(&self) -> &str {
        self.field("aecq")
    }
    pub fn voltage(&self) -> &str {
        self.field("voltage")
    }
    pub fn tolerance(&self) -> &str {
        self.field("tolerance")
    }
    pub fn package(&self) -> &str {
        self.field("package")
    }

    /// Refdes label for a finding title ("(no ref)" when the line is unannotated).
    pub fn label(&self) -> &str {
        if self.reference.is_empty() {
            "(no ref)"
        } else {
            &self.reference
        }
    }

    pub fn non_orderable(&self) -> bool {
        if self.exclude_from_bom {
            return true;
        }
        if let Some(m) = re!(r"^([A-Za-z]+)").captures(&self.reference) {
            if NON_ORDERABLE_PREFIXES.contains(&m[1].to_uppercase().as_str()) {
                return true;
            }
        }
        let haystack = format!("{} {}", self.footprint(), self.value()).to_lowercase();
        if NON_ORDERABLE_KEYWORDS.iter().any(|kw| haystack.contains(kw)) {
            return true;
        }
        // The bare board announces itself in the value ("PCB"), not the footprint —
        // and only when the WHOLE value is it ("PCB Mount Header" is a real part).
        re!(r"(?i)^(pcb|pwb|bare\s*board|blank\s*board|printed\s*circuit\s*board|(pcb|board)\s*blank)$")
            .is_match(self.value().trim())
    }
}

/// What a rule sees: the in-scope rows (DNP lines already filtered out unless the
/// rule opted in), its configured severity, and its params block.
pub struct Ctx<'a> {
    pub items: Vec<&'a BomItem>,
    /// Every row, DNP included — for the "does this BOM have the column at all?"
    /// question, which must not depend on the rule's DNP scope.
    pub all_items: &'a [BomItem],
    /// Logical fields the mapping actually found a column for. See [`Ctx::has_column`].
    pub mapped_fields: &'a BTreeSet<String>,
    pub severity: Severity,
    pub params: serde_json::Value,
}

impl Ctx<'_> {
    /// Does the BOM carry a column for this logical field?
    ///
    /// This is a question about the HEADER, not about the data, and the two answers
    /// lead to different advice. "Add a REACH column" sends an engineer looking for
    /// something already there; "populate the REACH column you have" is the actual
    /// job. Asking the rows — `all_items.any(|i| i.filled(field))` — cannot tell the
    /// two apart, and on a BOM whose compliance columns are present but blank on
    /// every line it always answers with the wrong one.
    ///
    /// An empty mapping (a caller that built items without one) falls back to the
    /// data test, so this can only ever be more accurate than what it replaced.
    pub fn has_column(&self, field: &str) -> bool {
        if self.mapped_fields.is_empty() {
            return self.all_items.iter().any(|i| i.filled(field));
        }
        self.mapped_fields.contains(field) || self.all_items.iter().any(|i| i.filled(field))
    }

    /// Does the BOM carry the column but leave it blank on every row? The gap the
    /// old "no column" finding was really describing on most real boards.
    pub fn column_wholly_blank(&self, field: &str) -> bool {
        self.has_column(field) && !self.all_items.iter().any(|i| i.filled(field))
    }

    pub fn param_f64(&self, key: &str, default: f64) -> f64 {
        self.params.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
    }
    pub fn param_usize(&self, key: &str, default: usize) -> usize {
        self.params
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(default)
    }
    pub fn param_bool(&self, key: &str, default: bool) -> bool {
        self.params.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }
    /// String list param, or `None` when the key is absent (so a rule can fall back to
    /// its built-in vocabulary rather than treating "absent" as "empty").
    pub fn param_strings(&self, key: &str) -> Option<Vec<String>> {
        let arr = self.params.get(key)?.as_array()?;
        Some(
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect(),
        )
    }
    /// Object param as (key, string-value) pairs.
    pub fn param_map(&self, key: &str) -> Option<Vec<(String, String)>> {
        let obj = self.params.get(key)?.as_object()?;
        Some(
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect(),
        )
    }
    /// Severity for an *absent data* finding, distinct from the rule's own severity:
    /// "this BOM has no lifecycle column" and "this part is EOL" are different claims.
    pub fn missing_sev(&self, default: Severity) -> Severity {
        self.params
            .get("missing_severity")
            .and_then(|v| v.as_str())
            .map(Severity::parse)
            .unwrap_or(default)
    }
}

/// True when a per-item gap is a project-wide process gap, not a per-part mistake.
///
/// Calibrated against 38 reference projects (2026-07): without this gate, per-item
/// "missing datasheet/sourcing" findings were 72% of all output on real designs.
pub fn systemic(
    missing: usize,
    eligible: usize,
    ctx: &Ctx,
    default_ratio: f64,
    default_min: usize,
) -> bool {
    if eligible == 0 {
        return false;
    }
    // Nothing has the field at all → systemic even on a tiny BOM.
    if missing == eligible && missing >= 2 {
        return true;
    }
    let ratio = ctx.param_f64("systemic_ratio", default_ratio);
    let min_n = ctx.param_usize("systemic_min", default_min);
    missing >= min_n && (missing as f64 / eligible as f64) >= ratio
}

/// Up to `limit` designators, with a "… +N more" tail — evidence stays readable on a
/// finding that covers half the BOM.
pub fn refs_of(items: &[&BomItem], limit: usize) -> Vec<String> {
    let refs: Vec<String> = items
        .iter()
        .filter(|i| !i.reference.is_empty())
        .map(|i| i.reference.clone())
        .collect();
    if refs.len() > limit {
        let mut out = refs[..limit].to_vec();
        out.push(format!("… +{} more", refs.len() - limit));
        out
    } else {
        refs
    }
}
