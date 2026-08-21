//! Dev helper: run the checks over a BOM CSV and print findings.json.
//! `cargo run -p bom-rules --example dump -- <bom.csv> [profile]`
fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: dump <bom.csv> [profile]");
        std::process::exit(2);
    };
    let profile = args.next().unwrap_or_else(|| "default".into());
    let text = std::fs::read_to_string(&path).expect("readable CSV");
    let rows = bom_rules::load::parse_csv(&text);
    let (items, mapping) = bom_rules::load::items_from_rows(&rows, &bom_rules::config::config_for(&profile));
    let doc = bom_rules::run(&items, &profile, &mapping);
    println!("{}", serde_json::to_string_pretty(&doc).expect("serializes"));
}
