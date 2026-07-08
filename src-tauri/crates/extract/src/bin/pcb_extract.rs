//! `pcb-extract` — command-line entry to the extraction pipeline.
//!
//! Contract (kept compatible with the importer it replaces, so the app's crunch
//! pipeline and the review skills can call it unchanged):
//!
//!   pcb-extract design <project> -o <out_dir>
//!   pcb-extract bom    <project> --format grouped-csv|grouped-json -o <out_dir>
//!   pcb-extract validate --golden <bundle_dir> --input <project>
//!   pcb-extract --version

use std::path::PathBuf;
use std::process::ExitCode;

use extract::pipeline::{run_bom, run_design, Msg};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", extract::version());
        return ExitCode::SUCCESS;
    }

    match args.first().map(String::as_str) {
        Some("design") => cmd_design(&args[1..]),
        Some("bom") => cmd_bom(&args[1..]),
        Some("validate") => {
            eprintln!("pcb-extract: '{}' is not implemented yet", args[0]);
            ExitCode::from(2)
        }
        Some(other) => {
            eprintln!("pcb-extract: unknown command '{other}'");
            ExitCode::from(2)
        }
        None => {
            eprintln!("usage: pcb-extract <design|bom|validate> ...  (try --version)");
            ExitCode::from(2)
        }
    }
}

/// `pcb-extract design <project> -o <out_dir>`
fn cmd_design(args: &[String]) -> ExitCode {
    let mut project: Option<PathBuf> = None;
    let mut out_dir = PathBuf::from("output/design");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => {
                if let Some(v) = it.next() {
                    out_dir = PathBuf::from(v);
                }
            }
            _ => project = Some(PathBuf::from(a)),
        }
    }
    let Some(project) = project else {
        eprintln!("usage: pcb-extract design <project.kicad_pro> -o <out_dir>");
        return ExitCode::from(2);
    };

    // NDJSON on stdout: artifacts as {"ev":"artifact","path":...}, else progress.
    let mut emit = |m: Msg| match m {
        Msg::Artifact(p) => println!("{{\"ev\":\"artifact\",\"path\":\"{p}\"}}"),
        Msg::Progress(line) => println!("{line}"),
    };

    match run_design(&project, &out_dir, &mut emit) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("design failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `pcb-extract bom <project> --format grouped-csv|grouped-json|enriched-csv -o <out>`
fn cmd_bom(args: &[String]) -> ExitCode {
    let mut project: Option<PathBuf> = None;
    let mut out_dir = PathBuf::from("output/bom");
    let mut format = "grouped-csv".to_string();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => {
                if let Some(v) = it.next() {
                    out_dir = PathBuf::from(v);
                }
            }
            "--format" | "-f" => {
                if let Some(v) = it.next() {
                    format = v.clone();
                }
            }
            _ => project = Some(PathBuf::from(a)),
        }
    }
    let Some(project) = project else {
        eprintln!("usage: pcb-extract bom <project.kicad_pro> --format <fmt> -o <out_dir>");
        return ExitCode::from(2);
    };

    let mut emit = |m: Msg| match m {
        Msg::Artifact(p) => println!("{{\"ev\":\"artifact\",\"path\":\"{p}\"}}"),
        Msg::Progress(line) => println!("{line}"),
    };

    match run_bom(&project, &out_dir, &format, &mut emit) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bom failed: {e}");
            ExitCode::FAILURE
        }
    }
}
