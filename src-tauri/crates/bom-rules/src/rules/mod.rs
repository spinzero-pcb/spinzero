//! The rule registry.
//!
//! Adding a rule: implement `Rule` in the module for its category, then append it
//! here. Its default enable/severity/params belong in `config::defaults()`, and a
//! fixture proving it fires (and one proving it doesn't over-fire) in
//! `schemas/rule-fixtures/`.

use crate::Rule;

mod consistency;
mod contradictions;
mod integrity;
mod lifecycle;
mod sourcing;

pub(crate) const SECTION_INTEGRITY: &str = "BOM · Integrity";
pub(crate) const SECTION_SOURCING: &str = "BOM · Sourcing";
pub(crate) const SECTION_CONSISTENCY: &str = "BOM · Consistency";
pub(crate) const SECTION_LIFECYCLE: &str = "BOM · Lifecycle & compliance";
pub(crate) const SECTION_CONTRADICTIONS: &str = "BOM · Contradictions";

pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(integrity::DuplicateRefdes),
        Box::new(integrity::UnannotatedRefdes),
        Box::new(integrity::InvalidRefdesFormat),
        Box::new(integrity::MissingRequiredField),
        Box::new(integrity::InvalidQuantity),
        Box::new(sourcing::MissingSourcingId),
        Box::new(sourcing::DistributorPnOnlyNoMpn),
        Box::new(sourcing::ManufacturerMpnUnpaired),
        Box::new(sourcing::SupplierPnFormat),
        Box::new(sourcing::MissingDatasheet),
        Box::new(sourcing::InvalidDatasheet),
        Box::new(sourcing::PlaceholderPartNumber),
        Box::new(consistency::ValueFormatConsistency),
        Box::new(consistency::InconsistentFieldsSameMpn),
        Box::new(consistency::ComponentClassConsistency),
        Box::new(consistency::GroupedQtyVsCount),
        Box::new(consistency::DuplicateLineItemsSamePn),
        Box::new(consistency::RedundantMpnSameValue),
        Box::new(lifecycle::LifecycleStatus),
        Box::new(lifecycle::MissingMsl),
        Box::new(lifecycle::MissingAecq),
        Box::new(lifecycle::MissingCompliance),
        Box::new(contradictions::DnpButHasSourcing),
        Box::new(contradictions::ExcludedButSourced),
        Box::new(contradictions::ValueAuxContradiction),
        Box::new(contradictions::PackageFootprintMismatch),
        Box::new(contradictions::MisplacedDistributorCode),
        Box::new(contradictions::DnpMarkerInText),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_rule_has_a_unique_id_and_a_default_config_entry() {
        let defaults = crate::config::defaults();
        let cfg = defaults["rules"].as_object().expect("rules block");
        let mut seen = BTreeSet::new();
        for rule in all_rules() {
            assert!(seen.insert(rule.id()), "duplicate rule id {}", rule.id());
            assert!(
                cfg.contains_key(rule.id()),
                "{} has no entry in config::defaults()",
                rule.id()
            );
        }
        // …and no config entry names a rule that doesn't exist.
        for id in cfg.keys() {
            assert!(seen.contains(id.as_str()), "config names unknown rule {id}");
        }
    }
}
