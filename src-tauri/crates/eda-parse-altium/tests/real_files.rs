//! Corpus tests against real Altium designs.
//!
//! The designs may not be redistributed, so they are located by path and the
//! tests skip with a message when absent — the same shape as
//! `eda-parse-kicad/tests/real_files.rs`. Point `SPINZERO_ALTIUM_CORPUS` at a
//! directory of reference designs to run them.
//!
//! What is checked in is the *histogram*: per document, per stream, the framing
//! and the record counts by type. A framing or decoding regression moves a
//! count, which is exactly the class of silent-wrong-answer bug the format
//! spike exists to catch. Regenerate with `SPINZERO_BLESS_FIXTURES=1`.

use std::path::{Path, PathBuf};

use eda_parse_altium::doc::histogram_text;
use eda_parse_altium::{Doc, Kind};

const DEFAULT_CORPUS: &str = r"D:\git_repo\reference_designs";
const FIXTURE: &str = "tests/fixtures/corpus_record_histogram.txt";

fn corpus() -> Option<PathBuf> {
    let p = std::env::var("SPINZERO_ALTIUM_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CORPUS));
    p.is_dir().then_some(p)
}

/// Every `.SchDoc` / `.PcbDoc` under `dir`, sorted by relative path.
fn documents(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if matches!(Kind::of(&p), Kind::Schematic | Kind::Board) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every corpus document opens, frames and decodes end to end, with the same
/// record-type histogram as the checked-in fixture.
#[test]
fn corpus_histogram_is_stable() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus (set SPINZERO_ALTIUM_CORPUS)");
        return;
    };
    let docs = documents(&root);
    if docs.is_empty() {
        eprintln!("skipping: corpus has no Altium documents");
        return;
    }
    let mut got = String::new();
    for p in &docs {
        let doc = Doc::open(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        got.push_str(&format!("# {}\n", rel(&root, p)));
        got.push_str(&histogram_text(&doc));
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    if std::env::var("SPINZERO_BLESS_FIXTURES").is_ok() {
        std::fs::create_dir_all(fixture.parent().unwrap()).expect("fixture dir");
        std::fs::write(&fixture, &got).expect("write fixture");
        return;
    }
    let want = std::fs::read_to_string(&fixture).unwrap_or_default();
    if want.is_empty() {
        eprintln!("skipping: no fixture (run with SPINZERO_BLESS_FIXTURES=1)");
        return;
    }
    if want != got {
        // Report the first differing line rather than a wall of text.
        let diff = want
            .lines()
            .zip(got.lines())
            .find(|(a, b)| a != b)
            .map(|(a, b)| format!("\n  fixture: {a}\n  now:     {b}"))
            .unwrap_or_else(|| format!(
                "\n  line count {} -> {}",
                want.lines().count(),
                got.lines().count()
            ));
        panic!("corpus record histogram changed:{diff}");
    }
}

/// Corner case 2 and 4 on real files: a description containing a non-ASCII
/// character must round-trip through the `%UTF8%` key, and no decoded value may
/// keep an unescaped pipe marker.
#[test]
fn corpus_decodes_escaped_pipes_and_utf8_descriptions() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    let mut non_ascii = 0usize;
    for p in documents(&root).iter().filter(|p| Kind::of(p) == Kind::Schematic) {
        let Ok(doc) = Doc::open(p) else { continue };
        for r in doc.text_records("FileHeader") {
            for (k, v) in &r.fields {
                assert!(
                    !v.contains('\u{8e}') && !v.contains('\u{a6}'),
                    "{}: {k} kept an escape marker: {v:?}",
                    p.display()
                );
                if v.chars().any(|c| !c.is_ascii()) {
                    non_ascii += 1;
                }
            }
        }
    }
    assert!(non_ascii > 0, "corpus should contain non-ASCII text to decode");
}
