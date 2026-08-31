//! `bom-rules` — the free tier's rule pack as a command.
//!
//! Same rules the app runs in-process, invocable from a terminal or by the paid
//! review engine's stage 2. That matters for one reason beyond convenience: the
//! engine must produce the *same fingerprints* as the in-app check, because a paid
//! finding is supposed to refine the free one in place rather than file a second
//! comment beside it. One implementation, two callers.
//!
//! ```text
//! bom-rules --bom bom_enriched.csv [--profile commercial] [--out findings.json]
//! ```
//!
//! findings.json v1.0 goes to `--out` or stdout; usage errors exit 2, read errors 1.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut bom: Option<String> = None;
    let mut profile = "default".to_string();
    let mut out: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bom" | "-b" => bom = args.next(),
            "--profile" | "-p" => {
                if let Some(v) = args.next() {
                    profile = v;
                }
            }
            "--out" | "-o" => out = args.next(),
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            // A bare path keeps the old `dump <csv> [profile]` habit working.
            other if !other.starts_with('-') && bom.is_none() => bom = Some(other.to_string()),
            other => {
                eprintln!("unknown argument: {other}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(bom) = bom else {
        eprintln!("--bom is required\n\n{USAGE}");
        return ExitCode::from(2);
    };

    let text = match std::fs::read_to_string(&bom) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {bom}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let rows = bom_rules::load::parse_csv(&text);
    let config = bom_rules::config::config_for(&profile);
    let (items, mapping) = bom_rules::load::items_from_rows(&rows, &config);
    let doc = bom_rules::run(&items, &profile, &mapping);
    let json = match serde_json::to_string_pretty(&doc) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("cannot serialize findings: {e}");
            return ExitCode::FAILURE;
        }
    };

    match out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, format!("{json}\n")) {
                eprintln!("cannot write {path}: {e}");
                return ExitCode::FAILURE;
            }
            eprintln!(
                "profile '{profile}': {} finding(s) -> {path}",
                doc.findings.len()
            );
        }
        None => println!("{json}"),
    }
    ExitCode::SUCCESS
}

const USAGE: &str = "bom-rules --bom <file.csv> [--profile <name>] [--out <findings.json>]

Runs the deterministic BOM rule pack and emits findings.json v1.0.
Profiles: commercial | industrial | medical | automotive
With no --profile, or an unrecognised one, the rules run UNSTATED: the strictest
setting of every rule, because an unanswered `what is this board for` must not be
the loosest review we can give. Say `--profile commercial` for an ordinary board.
With no --out the document goes to stdout.";
