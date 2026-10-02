//! Decode every Capture file under a directory and summarise what was read.
//! `cargo run -p eda-parse-orcad --example survey -- <dir> [-v]`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().map(|n| n != ".git").unwrap_or(true) {
                walk(&p, out);
            }
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if ext == "dsn" || ext == "olb" {
                out.push(p);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "-v");
    let mut files = Vec::new();
    walk(Path::new(&args[1]), &mut files);
    files.sort();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let (mut ok, mut failed, mut noted) = (0, 0, 0);
    let mut totals = [0usize; 8];
    for f in &files {
        match eda_parse_orcad::capture::open(f) {
            Ok(d) => {
                ok += 1;
                let pages: Vec<_> = d.folders.iter().flat_map(|x| &x.pages).collect();
                totals[0] += pages.len();
                totals[1] += pages.iter().map(|p| p.parts.len()).sum::<usize>();
                totals[2] += pages.iter().map(|p| p.wires.len()).sum::<usize>();
                totals[3] += d.cache.symbol_count();
                totals[4] += d.cache.packages.len();
                totals[5] += d.tree.as_ref().map(|t| t.count()).unwrap_or(0);
                totals[6] += d.library_symbols.len();
                totals[7] += d.cache.symbols.values().flatten().map(|s| s.prim_mismatches).sum::<usize>();
                if !d.notes.is_empty() {
                    noted += 1;
                    for n in &d.notes {
                        let key: String = n.split(": ").skip(1).collect::<Vec<_>>().join(": ");
                        let key = key.split(" at byte").next().unwrap_or("").to_string();
                        *kinds.entry(key).or_default() += 1;
                        if verbose {
                            println!("  {}: {n}", f.display());
                        }
                    }
                }
            }
            Err(e) => {
                failed += 1;
                println!("FAIL {}: {e}", f.display());
            }
        }
    }
    println!("files {} ok {ok} failed {failed} with-notes {noted}", files.len());
    println!("pages {} parts {} wires {} cache-symbols {} packages {} occurrences {} olb-symbols {} prim-mismatch {}",
        totals[0], totals[1], totals[2], totals[3], totals[4], totals[5], totals[6], totals[7]);
    for (k, v) in kinds.iter().filter(|(_, v)| **v > 0) {
        println!("{v:6} {k}");
    }
}
