//! `pcb-extract` — command-line entry to the extraction pipeline.
//!
//! Contract (kept compatible with the importer it replaces, so the app's crunch
//! pipeline and the review skills can call it unchanged):
//!
//!   pcb-extract design <project> -o <out_dir>
//!   pcb-extract bom    <project> --format grouped-csv|grouped-json -o <out_dir>
//!   pcb-extract dump   <file.SchDoc|.PcbDoc> [--full] [--stream <n>] [--head <n>]
//!   pcb-extract validate --golden <bundle_dir> --input <project>
//!   pcb-extract --version

use std::path::PathBuf;
use std::process::ExitCode;

use extract::altium::dump::Level;
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
        Some("dump") => cmd_dump(&args[1..]),
        Some("validate") => {
            eprintln!("pcb-extract: '{}' is not implemented yet", args[0]);
            ExitCode::from(2)
        }
        Some(other) => {
            eprintln!("pcb-extract: unknown command '{other}'");
            ExitCode::from(2)
        }
        None => {
            eprintln!("usage: pcb-extract <design|bom|dump|validate> ...  (try --version)");
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

/// `pcb-extract dump <file.SchDoc|.PcbDoc> [--full] [--stream <name>]`
///
/// Debug view of an Altium document: streams, record counts by type, and with
/// `--full` every decoded record. Development tool — no bundle output.
fn cmd_dump(args: &[String]) -> ExitCode {
    let mut file: Option<PathBuf> = None;
    let mut level = Level::Summary;
    let mut stream: Option<String> = None;
    let mut head = 48usize;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--full" => level = Level::Full,
            "--stream" | "-s" => stream = it.next().cloned(),
            "--head" => head = it.next().and_then(|v| v.parse().ok()).unwrap_or(head),
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let Some(file) = file else {
        eprintln!("usage: pcb-extract dump <file.SchDoc|.PcbDoc|.DSN|.OLB|.brd> [--full] [--stream <name>]");
        return ExitCode::from(2);
    };
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let result = if matches!(ext.as_str(), "dsn" | "olb" | "brd") {
        extract::orcad::dump::dump(&file, level)
    } else {
        extract::altium::dump::dump_head(&file, level, stream.as_deref(), head)
    };
    match result {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dump failed: {e}");
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
