//! Golden fixtures shared with the service-side Python runtime.
//!
//! Each `schemas/rule-fixtures/<name>.csv` is a hand-written BOM where every row
//! exists to trigger (or deliberately not trigger) a specific rule, paired with
//! `<name>.expected.json` naming the rule ids that must fire and the ones that must
//! not. Both runtimes run the same set — that is the only thing keeping the free Rust
//! tier and the paid Python stage from drifting apart (plan §3.3).

use std::collections::BTreeSet;
use std::path::PathBuf;

use bom_rules::{config, load, run};

fn fixtures_dir() -> PathBuf {
    // crates/bom-rules → src-tauri → repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../schemas/rule-fixtures")
        .canonicalize()
        .expect("schemas/rule-fixtures exists next to the app source")
}

#[test]
fn fixtures_match_expected_rule_hits() {
    let dir = fixtures_dir();
    let mut checked = 0;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("readable fixtures dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("csv"))
        .collect();
    entries.sort();

    for csv_path in entries {
        let name = csv_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let expected_path = dir.join(format!("{name}.expected.json"));
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&expected_path)
                .unwrap_or_else(|e| panic!("{name}.expected.json unreadable: {e}")),
        )
        .unwrap_or_else(|e| panic!("{name}.expected.json is not JSON: {e}"));

        let profile = expected["profile"].as_str().unwrap_or("default");
        let text = std::fs::read_to_string(&csv_path).expect("fixture CSV readable");
        let rows = load::parse_csv(&text);
        let (items, mapping) = load::items_from_rows(&rows, &config::config_for(profile));
        assert!(!items.is_empty(), "{name}: fixture parsed to zero BOM lines");
        let doc = run(&items, profile, &mapping);

        let fired: BTreeSet<&str> = doc
            .findings
            .iter()
            .filter_map(|f| f.rule_id.as_deref())
            .collect();

        for rule in expected["expect_rules"].as_array().into_iter().flatten() {
            let rule = rule.as_str().unwrap_or_default();
            assert!(
                fired.contains(rule),
                "{name}: expected {rule} to fire; fired: {fired:?}"
            );
        }
        for rule in expected["forbid_rules"].as_array().into_iter().flatten() {
            let rule = rule.as_str().unwrap_or_default();
            assert!(
                !fired.contains(rule),
                "{name}: {rule} fired but is forbidden — {:?}",
                doc.findings
                    .iter()
                    .filter(|f| f.rule_id.as_deref() == Some(rule))
                    .map(|f| f.title.as_str())
                    .collect::<Vec<_>>()
            );
        }
        if let Some(sevs) = expected["expect_severities"].as_object() {
            for (rule, want) in sevs {
                let got: Vec<&str> = doc
                    .findings
                    .iter()
                    .filter(|f| f.rule_id.as_deref() == Some(rule.as_str()))
                    .map(|f| f.severity.as_str())
                    .collect();
                assert!(
                    got.contains(&want.as_str().unwrap_or_default()),
                    "{name}: {rule} severities {got:?} do not include {want}"
                );
            }
        }
        // Fingerprints are the dedupe key for review comments: a collision inside one
        // run would merge two distinct defects into one comment.
        let prints: BTreeSet<&str> = doc.findings.iter().map(|f| f.fingerprint.as_str()).collect();
        assert_eq!(
            prints.len(),
            doc.findings.len(),
            "{name}: duplicate fingerprints within one run"
        );
        checked += 1;
    }
    assert!(checked >= 6, "expected the full fixture set, saw {checked}");
}
