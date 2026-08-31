//! Rule configuration: field aliases, distributor columns, per-rule enable/severity/
//! params, and the end-application profiles that override them.
//!
//! This is DATA, mirroring `.claude/skills/bom/defaults.yaml` and `profiles/*.yaml`
//! one-for-one, so the free in-app tier and the service-side Python runtime agree on
//! what a rule means. It is embedded (not read from disk) because the free tier must
//! work offline with nothing installed; a profile is selected by name, and an unknown
//! name degrades to `default` rather than failing the check.

use serde_json::{json, Value};

/// The profiles the UI offers, in the order it offers them.
///
/// `default` is deliberately NOT here, and that is the whole point of this list.
/// It is the profile that means *nobody said*, and `config_for` gives it the
/// strictest setting of every rule — see below. A profile a user can pick is a
/// profile a user has answered for; `default` is the absence of an answer, and the
/// absence of an answer must not be the absence of checks.
pub const PROFILES: &[&str] = &["commercial", "industrial", "medical", "automotive"];

/// Deep-merge `over` onto `base`: objects merge key-wise, everything else replaces.
/// Same semantics as `run_bom.py::_deep_merge`, so a profile states only its deltas.
pub fn merge(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut out = b.clone();
            for (k, v) in o {
                let merged = match b.get(k) {
                    Some(existing) => merge(existing, v),
                    None => v.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        _ => over.clone(),
    }
}

/// Full config for a profile name.
///
/// **An unknown or unstated profile gets `strictest()`, not the base table.** This
/// inverted in SpinZero plan §4. It used to fall back to `defaults()`, which is the
/// LEAST strict configuration there is — so a review nobody had answered the "what is
/// this board for" question for ran the loosest rule set we ship, and said nothing
/// about it. That is the wrong direction to fail in: a missed AEC-Q gap on a board
/// that turned out to be automotive costs a re-spin, while an AEC-Q finding on a
/// hobby board costs one dismissal.
///
/// The four names in `PROFILES` are answers, and each gets exactly the rules its
/// end application needs. `commercial` carries what used to be the bare `defaults()`
/// rule table, unchanged — so nothing about an explicitly commercial board's review
/// has moved; the name for it has.
pub fn config_for(profile: &str) -> Value {
    let base = defaults();
    match profile.trim().to_lowercase().as_str() {
        "commercial" => base,
        "industrial" => merge(&base, &industrial()),
        "medical" => merge(&base, &medical()),
        "automotive" => merge(&base, &automotive()),
        // "default", "", and anything we do not recognise.
        _ => merge(&base, &strictest()),
    }
}

/// Header matching is case-insensitive AND ignores spaces/underscores/dots/hyphens/#
/// (MANUFACTURER_PART_NUMBER is "Manufacturer Part Number"). Multi-source columns
/// (MPN1/MPN2 pairs, seen in Seeed/BeagleBoard BOMs) map to mpn + mpn_alt.
pub fn defaults() -> Value {
    json!({
      "field_aliases": {
        "reference":        ["Reference", "Ref", "RefDes", "Designator", "Designators"],
        "value":            ["Value", "Val"],
        "footprint":        ["Footprint", "Pattern"],
        "quantity":         ["Quantity", "Qty", "QUANTITY"],
        "manufacturer":     ["Manufacturer", "Mfr", "MFR", "MFG", "Mfg", "Manufacturer Name",
                             "Mfr.", "Manufacturer1", "Manufacturer_Name"],
        "mpn":              ["Manufacturer Part Number", "MPN", "MPN1", "Mfr Part #",
                             "Mfr Part No", "Mfr Part Number", "Manufacturer Part #",
                             "Manufacturer Part No", "Manufacturer1 Part Number", "P/N"],
        "mpn_alt":          ["MPN2", "Manufacturer2 Part Number", "Alternate MPN", "Alt MPN"],
        "datasheet":        ["Datasheet", "Data Sheet", "Datasheet URL"],
        "description":      ["Description", "Desc", "Comment", "Note", "Part Description"],
        "msl":              ["MSL", "MSL Level", "MSL Rating", "Moisture Sensitivity Level",
                             "Moisture Sensitivity"],
        "lifecycle":        ["Lifecycle", "Lifecycle Status", "Life Cycle", "Part Status",
                             "Product Status", "Status"],
        "aecq":             ["AEC-Q", "AECQ", "AEC-Q Qualified", "AEC-Q Grade", "AEC-Q100",
                             "AEC-Q101", "AEC-Q200", "Automotive Qualified", "Automotive Grade"],
        "rohs":             ["RoHS", "RoHS Status", "RoHS Compliant", "RoHS Compliance"],
        "reach":            ["REACH", "REACH Status", "REACH Compliant", "SVHC", "SVHC Status"],
        "dnp":              ["DNP", "Do Not Populate", "Do not populate", "Not Fitted", "DNF",
                             "Population", "DNI"],
        "exclude_from_bom": ["EXCLUDE_FROM_BOM", "Exclude from BOM", "BOM Exclude"],
        // Spec columns cross-checked against tokens embedded in the Value string /
        // footprint. NOTE: "Package" maps here (a size-code column in real BOMs), NOT
        // to footprint.
        "voltage":          ["Voltage", "Voltage Rating", "Rated Voltage", "VRating"],
        "tolerance":        ["Tolerance", "Tol"],
        "package":          ["Package", "Case", "Case Code", "Case/Package", "Package/Case"]
      },
      // Any present + non-empty column counts as a sourcing identifier. Generic
      // "Part Number" is intentionally NOT here (too ambiguous with MPN).
      "supplier_pn_columns": {
        "LCSC":     ["LCSC", "LCSC Part #", "LCSC Part Number", "JLCPCB Part #",
                     "JLCPCB Part Number", "JLCPCB"],
        "Mouser":   ["Mouser", "Mouser Part Number", "Mouser #", "Mouser Part #"],
        "Digi-Key": ["Digi-Key", "Digikey", "Digi-Key Part Number", "Digikey Part Number",
                     "Digi-Key #"],
        "Newark":   ["Newark", "Farnell", "Farnell Part Number", "Newark Part Number"],
        "Arrow":    ["Arrow", "Arrow Part Number"]
      },
      "rules": {
        "bom.missing_required_field":       { "enabled": true,  "severity": "CRITICAL",
                                              "params": { "required_fields": ["value", "footprint"] } },
        "bom.duplicate_refdes":             { "enabled": true,  "severity": "BLOCKER" },
        "bom.unannotated_refdes":           { "enabled": true,  "severity": "MAJOR" },
        "bom.invalid_refdes_format":        { "enabled": true,  "severity": "MINOR",
                                              "params": { "pattern": "^[A-Za-z][A-Za-z0-9_]*[0-9]$" } },
        "bom.invalid_quantity":             { "enabled": true,  "severity": "MAJOR" },
        // Coverage-gated: at least systemic_min parts AND systemic_ratio of the
        // eligible ones turns N per-item findings into ONE "this BOM doesn't track X".
        "bom.missing_sourcing_id":          { "enabled": true,  "severity": "CRITICAL",
                                              "params": { "systemic_ratio": 0.35, "systemic_min": 8 } },
        "bom.distributor_pn_only_no_mpn":   { "enabled": true,  "severity": "MINOR",
                                              "params": { "systemic_ratio": 0.35, "systemic_min": 8 } },
        "bom.manufacturer_mpn_unpaired":    { "enabled": true,  "severity": "MINOR",
                                              "params": { "systemic_ratio": 0.35, "systemic_min": 8 } },
        "bom.supplier_pn_format":           { "enabled": true,  "severity": "INFO",
                                              "params": { "patterns": { "LCSC": "^C\\d+$" } } },
        // OFF by design: a datasheet URL is a convenience column, not a BOM
        // requirement — the paid review resolves datasheets itself.
        "bom.missing_datasheet":            { "enabled": false, "severity": "INFO",
                                              "params": { "systemic_ratio": 0.15, "systemic_min": 5 } },
        "bom.invalid_datasheet":            { "enabled": true,  "severity": "MINOR",
                                              "params": { "allowed_schemes": ["http", "https"],
                                                          "allow_local_paths": false } },
        "bom.placeholder_part_number":      { "enabled": true,  "severity": "MAJOR",
                                              "params": { "fields": ["mpn"] } },
        "bom.value_format_consistency":     { "enabled": true,  "severity": "MINOR" },
        "bom.inconsistent_fields_same_mpn": { "enabled": true,  "severity": "MAJOR",
                                              "params": { "compare_fields": ["value", "footprint"] } },
        "bom.component_class_consistency":  { "enabled": true,  "severity": "MAJOR" },
        "bom.grouped_qty_vs_count":         { "enabled": true,  "severity": "MAJOR" },
        "bom.duplicate_line_items_same_pn": { "enabled": true,  "severity": "MINOR" },
        "bom.redundant_mpn_same_value":     { "enabled": true,  "severity": "INFO",
                                              "params": { "compare_fields": ["value", "footprint"] } },
        // An EOL/NRND part blocks the build (severity); an empty lifecycle column is
        // only unfinished homework (missing_severity). Keep them apart.
        "bom.lifecycle_status":             { "enabled": true,  "severity": "MAJOR",
                                              "params": { "missing_severity": "INFO" } },
        "bom.missing_msl":                  { "enabled": true,  "severity": "INFO",
                                              "params": { "applies_to_all": false,
                                                          "applies_to_prefixes": ["U", "Q", "D", "M", "IC", "Y", "X"] } },
        // Declared-negative and blank-cell AEC-Q stay SEPARATE findings (they are
        // different claims and the wording differs), but they share one severity
        // category: both are Major. Neither is a Critical that should stop a build on
        // its own — "NO" is a sourcing decision to revisit and a blank cell is a
        // traceability gap the datasheet usually clears — and splitting the severity
        // as well as the finding only made the report harder to triage.
        "bom.missing_aecq":                 { "enabled": false, "severity": "MAJOR",
                                              "params": { "missing_severity": "MAJOR" } },
        "bom.missing_compliance":           { "enabled": true,  "severity": "INFO",
                                              "params": { "required": ["rohs"],
                                                          "missing_severity": "MINOR" } },
        // OFF by design: sourcing data on a DNP line is the normal way to carry a
        // build option, and DNP lines are excluded from every other rule.
        "bom.dnp_but_has_sourcing":         { "enabled": false, "severity": "INFO" },
        "bom.excluded_but_sourced":         { "enabled": true,  "severity": "INFO" },
        "bom.value_aux_contradiction":      { "enabled": true,  "severity": "MAJOR" },
        "bom.package_footprint_mismatch":   { "enabled": true,  "severity": "MINOR" },
        "bom.misplaced_distributor_code":   { "enabled": true,  "severity": "MINOR",
                                              "params": { "fields": ["value", "description", "mpn"] } },
        "bom.dnp_marker_in_text":           { "enabled": true,  "severity": "MAJOR" }
      }
    })
}

/// Factory automation, test equipment, power systems, HVAC, smart meters, industrial
/// IoT: RoHS + REACH required, MPN stability matters (long field life).
fn industrial() -> Value {
    json!({
      "rules": {
        "bom.distributor_pn_only_no_mpn": { "severity": "MAJOR" },
        "bom.missing_compliance": { "severity": "MAJOR",
                                    "params": { "required": ["rohs", "reach"] } }
      }
    })
}

/// Class I/II/III devices, IVD instruments: full traceability, RoHS+REACH critical,
/// MSL on all parts, and the one case where the BOM itself must cite a datasheet
/// (the regulatory dossier).
fn medical() -> Value {
    json!({
      "rules": {
        "bom.missing_required_field": { "severity": "CRITICAL",
                                        "params": { "required_fields": ["value", "footprint", "manufacturer", "mpn"] } },
        "bom.distributor_pn_only_no_mpn": { "severity": "MAJOR" },
        "bom.missing_datasheet": { "enabled": true, "severity": "MINOR" },
        "bom.missing_msl": { "severity": "MAJOR",
                             "params": { "applies_to_all": true, "applies_to_prefixes": [] } },
        "bom.lifecycle_status": { "severity": "CRITICAL" },
        "bom.missing_compliance": { "severity": "CRITICAL",
                                    "params": { "required": ["rohs", "reach"] } }
      }
    })
}

/// ECU modules, ADAS sensors, EV BMS, body control: AEC-Q100/Q101/Q200 required,
/// IATF 16949 traceability, MSL on all parts, NRND/EOL blocks production.
fn automotive() -> Value {
    json!({
      "rules": {
        "bom.missing_required_field": { "severity": "CRITICAL",
                                        "params": { "required_fields": ["value", "footprint", "manufacturer", "mpn"] } },
        "bom.distributor_pn_only_no_mpn": { "severity": "MAJOR" },
        "bom.missing_msl": { "severity": "MAJOR",
                             "params": { "applies_to_all": true, "applies_to_prefixes": [] } },
        "bom.lifecycle_status": { "severity": "CRITICAL" },
        // Both AEC-Q findings ride at MAJOR here too — the profile turns the rule ON,
        // it does not re-rank it. A CRITICAL on either would send someone re-sourcing
        // silicon that an IATF traceability gap, not a failed part, is behind.
        "bom.missing_aecq": { "enabled": true, "severity": "MAJOR",
                              "params": { "missing_severity": "MAJOR" } },
        "bom.missing_compliance": { "severity": "CRITICAL",
                                    "params": { "required": ["rohs", "reach"] } }
      }
    })
}


/// The unstated profile: the strictest setting of every rule the four named profiles
/// touch, merged over the base table.
///
/// Composed as the per-rule maximum across `industrial`, `medical` and `automotive`
/// rather than written by hand, conceptually — each line below says which profile it
/// came from, so adding a stricter setting to one of those profiles and forgetting
/// this one is a visible omission rather than an invisible one. The test
/// `strictest_is_at_least_as_strict_as_every_named_profile` is what actually enforces
/// that, because a comment cannot.
///
/// Note what this does NOT do: it does not invent a severity no profile asks for, and
/// it does not enable a rule that is off everywhere. `bom.dnp_but_has_sourcing` stays
/// off, because a DNP line carrying sourcing data is the normal way to express a build
/// option and no end application changes that. Strictest means the union of what our
/// profiles ask for, not the maximum the schema permits.
fn strictest() -> Value {
    json!({
      "rules": {
        // medical + automotive: the four fields a traceable BOM must carry.
        "bom.missing_required_field": { "severity": "CRITICAL",
                                        "params": { "required_fields": ["value", "footprint", "manufacturer", "mpn"] } },
        // all three named profiles raise this from MINOR.
        "bom.distributor_pn_only_no_mpn": { "severity": "MAJOR" },
        // medical: the regulatory dossier must cite a datasheet per part.
        "bom.missing_datasheet": { "enabled": true, "severity": "MINOR" },
        // medical + automotive: MSL on everything, not just the moisture-sensitive
        // designator prefixes.
        "bom.missing_msl": { "severity": "MAJOR",
                             "params": { "applies_to_all": true, "applies_to_prefixes": [] } },
        // medical + automotive: an EOL/NRND part blocks production.
        "bom.lifecycle_status": { "severity": "CRITICAL" },
        // automotive: the rule is OFF by default and ON here. This is the single
        // loudest consequence of the inversion, and it is the intended one — an
        // unstated board that turns out to be automotive is exactly the board whose
        // AEC-Q gaps must not have been silently skipped. It rides at MAJOR, as it
        // does in the automotive profile: turning the rule on is not re-ranking it.
        "bom.missing_aecq": { "enabled": true, "severity": "MAJOR",
                              "params": { "missing_severity": "MAJOR" } },
        // medical + automotive: RoHS and REACH both required, both critical.
        "bom.missing_compliance": { "severity": "CRITICAL",
                                    "params": { "required": ["rohs", "reach"] } }
      }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_overrides_only_its_deltas() {
        let auto = config_for("automotive");
        let rules = &auto["rules"];
        // Overridden by the profile …
        assert_eq!(rules["bom.missing_aecq"]["enabled"], json!(true));
        assert_eq!(rules["bom.missing_aecq"]["severity"], json!("MAJOR"));
        // … while params the profile doesn't mention survive from defaults.
        assert_eq!(
            rules["bom.lifecycle_status"]["params"]["missing_severity"],
            json!("INFO")
        );
        // … and untouched rules keep their default state.
        assert_eq!(rules["bom.duplicate_refdes"]["severity"], json!("BLOCKER"));
    }

    #[test]
    fn an_unstated_profile_gets_the_strictest_rules_not_the_loosest() {
        // The inversion (SpinZero plan §4). This assertion used to read
        // `config_for("nonsense") == defaults()`, i.e. an unanswered question got the
        // loosest rule set we ship and said nothing about it.
        let unstated = config_for("nonsense");
        assert_ne!(unstated, defaults(), "an unstated profile must not be the base table");
        assert_eq!(unstated, config_for("default"), "`default` IS the unstated profile");
        assert_eq!(unstated, config_for(""), "so is an empty string");

        // The named answer for an ordinary commercial board is what `defaults()` was.
        // Nothing about an explicitly commercial review has moved.
        assert_eq!(config_for("commercial"), defaults());

        // The loudest single consequence, asserted so it cannot be a surprise later:
        // AEC-Q is OFF for a commercial board and ON when nobody has said.
        assert_eq!(defaults()["rules"]["bom.missing_aecq"]["enabled"], json!(false));
        assert_eq!(unstated["rules"]["bom.missing_aecq"]["enabled"], json!(true));
    }

    /// `strictest()` is written by hand, so nothing stops it drifting behind a profile
    /// that gets a stricter setting later. This is what stops that: for every rule any
    /// named profile touches, the unstated profile must be at least as strict.
    ///
    /// "At least as strict" is three separate things, and all three are checked —
    /// enabled beats disabled, a higher severity beats a lower one, and a `params`
    /// widening (more required fields, `applies_to_all`) must be carried too.
    #[test]
    fn strictest_is_at_least_as_strict_as_every_named_profile() {
        const RANK: &[&str] = &["INFO", "MINOR", "MAJOR", "CRITICAL", "BLOCKER"];
        let rank = |s: &Value| -> usize {
            s.as_str().and_then(|t| RANK.iter().position(|r| *r == t)).unwrap_or(0)
        };
        let unstated = config_for("default");

        for profile in PROFILES {
            let cfg = config_for(profile);
            let rules = cfg["rules"].as_object().expect("rules");
            for (rule_id, spec) in rules {
                let mine = &unstated["rules"][rule_id];
                if spec["enabled"] == json!(true) {
                    assert_eq!(
                        mine["enabled"],
                        json!(true),
                        "{profile} enables {rule_id} but the unstated profile does not"
                    );
                }
                assert!(
                    rank(&mine["severity"]) >= rank(&spec["severity"]),
                    "{rule_id}: {profile} is {} but the unstated profile is only {}",
                    spec["severity"],
                    mine["severity"]
                );
                // A params widening the unstated profile failed to carry would make it
                // nominally strict and materially looser — `required_fields` and
                // `applies_to_all` are exactly the two that decide how much is checked.
                if let Some(req) = spec["params"]["required_fields"].as_array() {
                    let ours = mine["params"]["required_fields"].as_array().cloned().unwrap_or_default();
                    for f in req {
                        assert!(ours.contains(f), "{rule_id}: {profile} requires {f} and the unstated profile does not");
                    }
                }
                if spec["params"]["applies_to_all"] == json!(true) {
                    assert_eq!(
                        mine["params"]["applies_to_all"],
                        json!(true),
                        "{rule_id}: {profile} applies to all parts and the unstated profile does not"
                    );
                }
                if let Some(req) = spec["params"]["required"].as_array() {
                    let ours = mine["params"]["required"].as_array().cloned().unwrap_or_default();
                    for f in req {
                        assert!(ours.contains(f), "{rule_id}: {profile} requires compliance field {f} and the unstated profile does not");
                    }
                }
            }
        }
    }
}
