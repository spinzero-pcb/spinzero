//! A whole Capture compound file — a `.DSN` design or an `.OLB` library —
//! decoded stream by stream.

use std::collections::BTreeMap;
use std::path::Path;

use ole_cfb::Cfb;

use super::bytes::Cur;
use super::cache::{read_cache, read_package_stream, Cache};
use super::cis::{read_cis, Cis};
use super::framing::read_frame;
use super::hierarchy::{read_tree, OccTree};
use super::legacy;
use super::library::{sniff, FileRole, LibraryInfo};
use super::page::{read_page, Page, PlacedPart};
use super::symbol::{read_symbol, SymbolDef};

/// One schematic folder: an ordered set of pages.
#[derive(Debug, Clone, Default)]
pub struct Folder {
    pub name: String,
    pub pages: Vec<Page>,
    /// Listed in the Views directory (a hidden folder exists but is not shown).
    pub visible: bool,
    /// Page streams that could not be decoded, with the reason.
    pub page_errors: Vec<(String, String)>,
}

/// A decoded Capture file.
#[derive(Debug, Clone)]
pub struct CaptureDoc {
    pub lib: LibraryInfo,
    pub cache: Cache,
    /// Folders in Capture's display order.
    pub folders: Vec<Folder>,
    /// Index of the root folder, when the file is a design.
    pub root: Option<usize>,
    pub tree: Option<OccTree>,
    /// Why the occurrence tree was not read, when one was present.
    pub tree_error: Option<String>,
    /// Standalone symbols an `.OLB` stores under `Symbols/`.
    pub library_symbols: Vec<SymbolDef>,
    pub cis: Option<Cis>,
    /// Everything that was skipped, with the reason.
    pub notes: Vec<String>,
}

impl CaptureDoc {
    pub fn is_design(&self) -> bool {
        self.lib.role == FileRole::Design
    }

    pub fn folder(&self, name: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    pub fn root_folder(&self) -> Option<&Folder> {
        self.root.and_then(|i| self.folders.get(i))
    }

    /// The cached symbol a placed part draws. A cache name can hold several
    /// stored revisions; a placement uses the one whose pins land on its own
    /// stored pin positions, and the first revision when none or all do.
    pub fn symbol_for(&self, part: &PlacedPart) -> Option<&SymbolDef> {
        let revs = self.cache.symbols.get(&part.cache_name).or_else(|| {
            self.cache
                .symbols
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&part.cache_name))
                .map(|(_, v)| v)
        })?;
        if revs.len() > 1 {
            if let Some(s) = revs.iter().find(|s| fits(part, s)) {
                return Some(s);
            }
        }
        revs.first()
    }
}

/// True when every placed pin sits where the symbol puts that slot.
pub fn fits(part: &PlacedPart, s: &SymbolDef) -> bool {
    let b = (s.bbox.0 as i32, s.bbox.1 as i32, s.bbox.2 as i32, s.bbox.3 as i32);
    !part.pins.is_empty()
        && part.pins.iter().all(|pi| {
            s.pins
                .iter()
                .find(|x| x.slot == pi.slot())
                .map(|sp| part.orient.place(sp.hot, part.pos, b) == pi.pos)
                .unwrap_or(false)
        })
}

/// True when the bytes are a compound file whose `Library` stream says it is
/// a Capture design or library.
pub fn is_capture(bytes: &[u8]) -> bool {
    if !ole_cfb::is_cfb(bytes) {
        return false;
    }
    Cfb::parse(bytes)
        .ok()
        .and_then(|c| c.stream("Library").and_then(sniff))
        .is_some()
}

pub fn open(path: &Path) -> Result<CaptureDoc, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes)
}

/// Explain what a file that is not a Capture compound file is, if we can.
fn not_capture(bytes: &[u8]) -> String {
    let head: String = bytes.iter().take(64).map(|&b| b as char).collect();
    if head.trim_start().starts_with("(PCB") || head.trim_start().starts_with("(pcb") {
        return "a Specctra (.dsn) autorouter file, not an OrCAD Capture design".into();
    }
    if head.starts_with("ISIS SCHEMATIC FILE") {
        return "a Proteus ISIS schematic, not an OrCAD Capture design".into();
    }
    "not an OrCAD Capture compound file".into()
}

fn stream_order(cfb: &Cfb, folder: &str) -> Vec<String> {
    let Some(data) = cfb.stream(&format!("Views/{folder}/Schematic")) else {
        return Vec::new();
    };
    let mut c = Cur::new(data);
    let Ok(f) = read_frame(&mut c) else { return Vec::new() };
    // The header's only stop is its own body start; the order runs on inline.
    let mut c = Cur::at(data, f.body, data.len());
    let mut read = || -> Result<Vec<String>, String> {
        let _name = c.lstr().map_err(|e| e.to_string())?;
        c.skip(4).map_err(|e| e.to_string())?;
        let n = c.u16().map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(c.lstr().map_err(|e| e.to_string())?);
        }
        Ok(v)
    };
    let mut v = read().unwrap_or_default();
    // Stored last page first.
    v.reverse();
    v
}

fn views_directory(cfb: &Cfb) -> Option<Vec<String>> {
    let data = cfb.stream("Views Directory")?;
    let mut c = Cur::new(data);
    c.skip(4).ok()?;
    let n = c.u16().ok()?;
    let mut v = Vec::new();
    for _ in 0..n {
        v.push(c.lstr().ok()?);
        c.skip(22).ok()?;
    }
    c.is_done().then_some(v)
}

pub fn parse(bytes: &[u8]) -> Result<CaptureDoc, String> {
    if !ole_cfb::is_cfb(bytes) {
        return Err(not_capture(bytes));
    }
    let cfb = Cfb::parse(bytes)?;
    let lib_data = cfb.stream("Library").ok_or("no Library stream: not an OrCAD Capture file")?;
    let lib = LibraryInfo::read(lib_data).map_err(|e| format!("Library stream: {e}"))?;
    if lib.version.0 < 3 {
        return legacy::parse(&cfb, lib);
    }
    let mut notes = Vec::new();

    let mut cache = match cfb.stream("Cache") {
        Some(d) => read_cache(d, &lib).unwrap_or_else(|e| {
            notes.push(format!("Cache stream: {e}"));
            Cache::default()
        }),
        None => Cache::default(),
    };
    for path in cfb.paths().filter(|p| p.starts_with("Packages/")).map(str::to_string).collect::<Vec<_>>() {
        if let Some(d) = cfb.stream(&path) {
            if let Err(e) = read_package_stream(d, &lib, &mut cache) {
                notes.push(format!("{path}: {e}"));
            }
        }
    }
    for u in &cache.undecoded {
        notes.push(format!("cache record undecoded: {u}"));
    }

    // Folders: every storage under Views/ that holds pages, in the Views
    // directory's order, with unlisted folders after it in name order.
    let mut by_folder: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in cfb.paths() {
        let parts: Vec<&str> = p.split('/').collect();
        if parts.len() == 4 && parts[0] == "Views" && parts[2] == "Pages" {
            by_folder.entry(parts[1].to_string()).or_default().push(parts[3].to_string());
        } else if parts.len() >= 2 && parts[0] == "Views" {
            by_folder.entry(parts[1].to_string()).or_default();
        }
    }
    let listed = views_directory(&cfb);
    let mut order: Vec<(String, bool)> = Vec::new();
    if let Some(l) = &listed {
        for n in l {
            if by_folder.contains_key(n) {
                order.push((n.clone(), true));
            }
        }
    }
    for n in by_folder.keys() {
        if !order.iter().any(|(o, _)| o == n) {
            order.push((n.clone(), listed.is_none()));
        }
    }

    let mut folders = Vec::new();
    for (name, visible) in order {
        let streams = by_folder.get(&name).cloned().unwrap_or_default();
        let mut ordered = stream_order(&cfb, &name);
        ordered.retain(|p| streams.contains(p));
        for s in &streams {
            if !ordered.contains(s) {
                ordered.push(s.clone());
            }
        }
        let mut folder = Folder { name: name.clone(), visible, ..Folder::default() };
        for s in ordered {
            let path = format!("Views/{name}/Pages/{s}");
            let Some(d) = cfb.stream(&path) else { continue };
            match read_page(&s, d, &lib) {
                Ok(p) => {
                    if let Some(t) = &p.truncated {
                        notes.push(format!("{path}: page read stopped early: {t}"));
                    }
                    folder.pages.push(p);
                }
                Err(e) => {
                    notes.push(format!("{path}: {e}"));
                    folder.page_errors.push((s.clone(), e.to_string()));
                }
            }
        }
        folders.push(folder);
    }

    let root = if lib.role == FileRole::Design {
        folders
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&lib.root_name))
            .or_else(|| folders.iter().position(|f| f.visible && !f.pages.is_empty()))
            .or_else(|| folders.iter().position(|f| !f.pages.is_empty()))
    } else {
        None
    };

    let mut tree = None;
    let mut tree_error = None;
    if let Some(r) = root {
        let path = format!("Views/{}/Hierarchy/Hierarchy", folders[r].name);
        if let Some(d) = cfb.stream(&path) {
            match read_tree(d, &lib) {
                Ok(t) => tree = t,
                Err(e) => {
                    notes.push(format!("{path}: {e}"));
                    tree_error = Some(e.to_string());
                }
            }
        }
    }

    let mut library_symbols = Vec::new();
    if lib.role == FileRole::Library {
        for path in cfb.paths().filter(|p| p.starts_with("Symbols/")).map(str::to_string).collect::<Vec<_>>() {
            let name = &path["Symbols/".len()..];
            if name.starts_with('$') {
                continue;
            }
            if let Some(d) = cfb.stream(&path) {
                match read_symbol(&mut Cur::new(d), &lib) {
                    Ok(s) => library_symbols.push(s),
                    Err(e) => notes.push(format!("{path}: {e}")),
                }
            }
        }
    }

    let cis = read_cis(&cfb, &lib, &folders);

    Ok(CaptureDoc { lib, cache, folders, root, tree, tree_error, library_symbols, cis, notes })
}
