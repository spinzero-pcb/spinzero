//! Design-index assembly + artifact serving for the Phase-1 canvas.
//!
//! `get_design_indexes` ports `prototypes/extract_data2.py` to Rust: it reads the
//! crunched design JSON and emits the single payload the hyperlinked viewer needs —
//! `svg_to_net` / `svg_to_component` / per-element kind for hit-testing, plus per-net
//! (class, terminals, sheets, by-sheet element ids) and per-component card data.
//! `read_artifact` serves a `<metadata>`-stripped sheet SVG (U5's serve-time fallback).
//!
//! Both resolve their source from the active vault's cache, or from
//! `PCBREVIEW_CACHE_DIR` when set — the dev override that lets the canvas run against
//! an already-crunched `output/` bundle with no re-crunch.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// Largest artifact we will read off disk before stripping. Raw KiCad sheets top out
/// ~5 MB, but a dense PCB user/fab layer on a large board (per-footprint graphics for
/// thousands of parts) can run to 60+ MB — so this is a path-traversal / runaway-file
/// backstop, not a real limit, and is set well above any legitimate layer.
const MAX_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;

/// Resolve the crunch cache dir: explicit dev override wins, else the active
/// extraction's bundle dir (`extractions/<id>/`).
pub fn cache_dir(active_extraction: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Ok(over) = std::env::var("PCBREVIEW_CACHE_DIR") {
        if !over.is_empty() {
            return Ok(PathBuf::from(over));
        }
    }
    active_extraction.ok_or_else(|| "no extraction available (open a project)".to_string())
}

/// Locate the design subdirectory (holding `*_design.json` + `schematics/`) inside a
/// crunch bundle — mirrors index_db's manifest discovery.
fn find_design_dir(cache_dir: &Path) -> Result<PathBuf, String> {
    for sub in ["design", "design_review", "."] {
        let dir = cache_dir.join(sub);
        if dir.join("schematics").is_dir()
            || fs::read_dir(&dir).map(|mut rd| {
                rd.any(|e| {
                    e.ok().map_or(false, |e| {
                        e.file_name().to_string_lossy().ends_with("_design.json")
                    })
                })
            }).unwrap_or(false)
        {
            return Ok(dir);
        }
    }
    Err("no design directory found in crunch cache".into())
}

fn read_design_json(design_dir: &Path) -> Result<Value, String> {
    let path = fs::read_dir(design_dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .map_or(false, |n| n.to_string_lossy().ends_with("_design.json"))
        })
        .ok_or("design JSON not found in bundle")?;
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("design JSON parse: {e}"))
}

// ---------------------------------------------------------------- payload types

#[derive(Serialize)]
pub struct SheetLite {
    pub num: i64,
    pub name: String,
    /// Relative path from the cache dir, ready for `read_artifact`.
    pub svg: Option<String>,
}

#[derive(Serialize)]
pub struct TerminalLite {
    pub d: String,        // designator
    pub p: String,        // pin
    pub pn: String,       // pin name
    pub pt: String,       // pin type
}

#[derive(Serialize)]
pub struct NetLite {
    pub class: String,
    pub terminals: Vec<TerminalLite>,
    pub sheets: Vec<i64>,
    /// sheet number (as string key) -> element uuids of this net on that sheet
    pub by_sheet: HashMap<String, Vec<String>>,
}

#[derive(Serialize)]
pub struct CompLite {
    pub value: String,
    pub mpn: String,
    pub mfr: String,
    pub fp: String,
    pub desc: String,
    pub sheet: Option<i64>,
    pub dnp: bool,
    pub nets: Vec<String>,
    pub svg_id: String,
    /// Placed schematic bounding box `[x, y, w, h]` on its sheet (design.json `bbox`),
    /// when the library symbol has geometry. Lets the diff engine see symbol moves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
}

#[derive(Serialize)]
pub struct LayerLite {
    pub name: String,
    /// Cache-relative SVG path, ready for `read_artifact`.
    pub svg: String,
    /// EDA-agnostic layer role from the manifest (copper/silkscreen/mask/paste/
    /// courtyard/fab/edge). Drives viewer theming without hardcoding KiCad-style
    /// names. None for older copper-only bundles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Board side (front/back/inner) when the manifest carries it. KiCad
    /// encodes side in the name (F./B.), so the frontend derives it there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// Designer's display name for a renamed/user layer (`User.3` → "Mechanical
    /// Drawing"), when the board's layer table carries one. The viewer shows this
    /// instead of the canonical name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// Resolved `#RRGGBB` for a non-standard ("user") layer, from the KiCad theme.
    /// The standard fabrication layers stay `None` — they theme via CSS vars. The
    /// frontend paints this colour directly when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize)]
pub struct DesignIndexes {
    pub sheets: Vec<SheetLite>,
    /// PCB copper layers from the manifest (WS8) — sourced here rather than the
    /// SQLite index so the `PCBREVIEW_CACHE_DIR` dev override works too.
    pub layers: Vec<LayerLite>,
    pub svg_to_net: HashMap<String, String>,
    /// Multi-valued variant: on sheets instantiated several times from one source
    /// file (gate_driver U/V/W) elements share uuids across instances, so one uuid
    /// maps to one net *per instance*. The frontend disambiguates by current sheet.
    pub svg_to_nets: HashMap<String, Vec<String>>,
    pub svg_to_comp: HashMap<String, String>,
    pub elem_kind: HashMap<String, String>,
    pub nets: HashMap<String, NetLite>,
    pub components: HashMap<String, CompLite>,
    /// The KiCad colour theme the extractor resolved (`schematic`/`board` colour
    /// maps), forwarded verbatim so the viewer themes with the user's real palette
    /// instead of the static `tokens.css` mirror. `Null` for theme-less bundles.
    pub theme: Value,
    /// Cache-relative path of the structured PCB geometry IR (`pcb/geometry.json`),
    /// the GPU renderer's input. `None` for schematic-only / older bundles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pcb_geometry: Option<String>,
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// Map a net's `terminals` array into the compact `TerminalLite` list the payload
/// carries. Missing fields become "".
fn parse_terminals(net: &Value) -> Vec<TerminalLite> {
    net.get("terminals")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .map(|t| TerminalLite {
                    d: str_at(t, "designator").unwrap_or("").to_string(),
                    p: str_at(t, "pin").unwrap_or("").to_string(),
                    pn: str_at(t, "pin_name").unwrap_or("").to_string(),
                    pt: str_at(t, "pin_type").unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the viewer payload from the extracted design bundle.
pub fn build_indexes(vault_cache: Option<PathBuf>) -> Result<DesignIndexes, String> {
    let cache = cache_dir(vault_cache)?;
    let design_dir = find_design_dir(&cache)?;
    build_indexes_kicad(&cache, &design_dir)
}

/// Extra per-bundle data the diff engine needs beyond the viewer `DesignIndexes`:
/// the sheet-number → `.kicad_sch` filename map (for source-hash pruning) and the raw
/// `pcb/geometry.json` text (parsed by the diff engine into its own IR mirror). This
/// avoids widening the frontend-facing `DesignIndexes`/`design.ts` contract for
/// backend-only diff needs.
pub struct DiffBundleExtras {
    /// Sheet number → source `.kicad_sch` file (design-relative), for pruning.
    pub sheet_files: HashMap<i64, String>,
    /// Raw contents of `pcb/geometry.json`, if the bundle has a board.
    pub geometry_json: Option<String>,
    /// Raw contents of `schematics/geometry.json` (per-element schematic geometry),
    /// when the extraction emitted it. `None` for older caches → the diff falls back.
    pub sch_geometry_json: Option<String>,
    /// Per-component (refdes → the symbol's full property map) so the diff engine can
    /// report edits to arbitrary fields (Package, Tolerance, Automotive Grade, …) that
    /// aren't first-class on `CompLite`. Backend-only: keeps these off the viewer
    /// payload while still letting the changeset flag them.
    pub comp_params: HashMap<String, BTreeMap<String, String>>,
}

/// Load a bundle's diff extras from its cache dir. Best-effort on the geometry (a
/// schematic-only bundle simply has `geometry_json = None`); the sheet map is derived
/// from the design JSON's `sheets[]`.
pub fn load_diff_extras(cache: &Path) -> Result<DiffBundleExtras, String> {
    let design_dir = find_design_dir(cache)?;
    let d = read_design_json(&design_dir)?;

    let mut sheet_files: HashMap<i64, String> = HashMap::new();
    if let Some(arr) = d.get("sheets").and_then(|s| s.as_array()) {
        for s in arr {
            let Some(num) = s.get("sheet_number").and_then(|n| n.as_i64()) else { continue };
            if let Some(file) = str_at(s, "filename").filter(|f| !f.is_empty()) {
                sheet_files.insert(num, file.to_string());
            }
        }
    }

    // Geometry paths come from the manifest (same as build_indexes), read raw so the
    // diff engine can deserialize its own mirrors without loading the whole viewer IR.
    let manifest: Option<Value> = fs::read_to_string(design_dir.join("design_review_manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let read_rel = |key: &str| {
        manifest
            .as_ref()
            .and_then(|m| m.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()))
            .and_then(|rel| fs::read_to_string(design_dir.join(rel)).ok())
    };
    let geometry_json = read_rel("pcb_geometry");
    let sch_geometry_json = read_rel("schematic_geometry");

    // Per-component property maps (backend-only): keyed by designator, carrying every
    // symbol property so the diff can flag edits to fields `CompLite` doesn't surface.
    let mut comp_params: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    if let Some(arr) = d.get("components").and_then(|c| c.as_array()) {
        for c in arr {
            let Some(designator) = str_at(c, "designator") else { continue };
            let Some(params) = c.get("parameters").and_then(|p| p.as_object()) else { continue };
            let map: BTreeMap<String, String> = params
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
            comp_params.insert(designator.to_string(), map);
        }
    }

    Ok(DiffBundleExtras { sheet_files, geometry_json, sch_geometry_json, comp_params })
}

/// KiCad design-JSON → viewer payload.
fn build_indexes_kicad(cache: &Path, design_dir: &Path) -> Result<DesignIndexes, String> {
    let d = read_design_json(design_dir)?;

    let design_rel = design_dir
        .strip_prefix(cache)
        .unwrap_or(&design_dir)
        .to_string_lossy()
        .replace('\\', "/");
    let manifest: Option<Value> =
        fs::read_to_string(design_dir.join("design_review_manifest.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());

    // Sheet → SVG comes from the manifest (authoritative: name munging breaks on
    // sheets like "User Input/Output" or a root whose title ≠ filename). Keyed by
    // sheet_path, falling back to sheet_number.
    let mut svg_by_path: HashMap<String, (String, String)> = HashMap::new(); // path -> (name, file)
    let mut svg_by_num: HashMap<i64, (String, String)> = HashMap::new();
    if let Some(arr) = manifest
        .as_ref()
        .and_then(|m| m.get("schematic_svgs"))
        .and_then(|v| v.as_array())
    {
        for s in arr {
            let (Some(file), Some(name)) = (str_at(s, "file"), str_at(s, "sheet_name")) else {
                continue;
            };
            let entry = (name.to_string(), file.to_string());
            if let Some(p) = str_at(s, "sheet_path") {
                svg_by_path.insert(p.to_string(), entry.clone());
            }
            if let Some(n) = s.get("sheet_number").and_then(|n| n.as_i64()) {
                svg_by_num.insert(n, entry);
            }
        }
    }
    // Last-resort fallback for manifest-less bundles: match SVG stems ("NN_name")
    // against munged sheet names.
    let mut svg_by_stem: HashMap<String, String> = HashMap::new();
    if let Ok(rd) = fs::read_dir(design_dir.join("schematics")) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("svg") {
                continue;
            }
            if let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) {
                svg_by_stem.insert(
                    strip_leading_index(&stem).to_lowercase(),
                    format!("schematics/{}", entry.file_name().to_string_lossy()),
                );
            }
        }
    }

    // sheet_path_uuids -> sheet number, and the sheet list.
    let mut uuidpath_to_num: HashMap<String, i64> = HashMap::new();
    let mut sheets: Vec<SheetLite> = Vec::new();
    if let Some(arr) = d.get("sheets").and_then(|s| s.as_array()) {
        for s in arr {
            let num = s.get("sheet_number").and_then(|n| n.as_i64()).unwrap_or(0);
            if let Some(up) = str_at(s, "sheet_path_uuids") {
                uuidpath_to_num.insert(up.to_string(), num);
            }
            let sheet_path = str_at(s, "sheet_path").unwrap_or("");
            let derived_name = {
                let last = sheet_path.trim_matches('/').rsplit('/').next().unwrap_or("");
                if !last.is_empty() {
                    last.to_string()
                } else {
                    str_at(s, "title")
                        .filter(|t| !t.is_empty())
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| {
                            str_at(s, "filename").unwrap_or("").replace(".kicad_sch", "")
                        })
                }
            };
            let hit = svg_by_path
                .get(sheet_path)
                .or_else(|| svg_by_num.get(&num))
                .cloned();
            let (name, svg) = match hit {
                Some((name, file)) => (name, Some(format!("{design_rel}/{file}"))),
                None => {
                    let key = derived_name.to_lowercase().replace(' ', "_");
                    let svg = svg_by_stem.get(&key).map(|f| format!("{design_rel}/{f}"));
                    (derived_name, svg)
                }
            };
            sheets.push(SheetLite { num, name, svg });
        }
    }

    // PCB copper layers from the manifest (best effort — schematic-only bundles pass).
    let mut layers: Vec<LayerLite> = Vec::new();
    if let Some(arr) = manifest
        .as_ref()
        .and_then(|m| m.get("pcb_svgs"))
        .and_then(|v| v.as_array())
    {
        for l in arr {
            if let (Some(name), Some(file)) = (str_at(l, "layer"), str_at(l, "file")) {
                layers.push(LayerLite {
                    name: name.to_string(),
                    svg: format!("{design_rel}/{file}"),
                    role: str_at(l, "role").map(|s| s.to_string()),
                    side: str_at(l, "side").map(|s| s.to_string()),
                    user_name: str_at(l, "user_name").map(|s| s.to_string()),
                    color: str_at(l, "color").map(|s| s.to_string()),
                });
            }
        }
    }

    // svg_to_net(s) / svg_to_component straight from prebuilt indexes.
    let indexes = d.get("indexes").cloned().unwrap_or(Value::Null);
    let svg_to_net = string_map(indexes.get("svg_to_net"));
    let svg_to_nets = string_list_map(indexes.get("svg_to_nets"));
    let svg_to_comp = string_map(indexes.get("svg_to_component"));

    // Invert sheet_svg_to_nets: net name -> { sheet num : [element uuids] }.
    let mut by_sheet: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    if let Some(obj) = indexes.get("sheet_svg_to_nets").and_then(|v| v.as_object()) {
        for (upath, mapping) in obj {
            let Some(&num) = uuidpath_to_num.get(upath) else { continue };
            if let Some(m) = mapping.as_object() {
                for (uuid, netnames) in m {
                    if let Some(names) = netnames.as_array() {
                        for nn in names.iter().filter_map(|n| n.as_str()) {
                            by_sheet
                                .entry(nn.to_string())
                                .or_default()
                                .entry(num.to_string())
                                .or_default()
                                .push(uuid.to_string());
                        }
                    }
                }
            }
        }
    }

    // elem_kind + per-net payloads from the typed graphical lists.
    let net_classes = d.get("net_name_to_classes").cloned().unwrap_or(Value::Null);
    let mut elem_kind: HashMap<String, String> = HashMap::new();
    let mut nets: HashMap<String, NetLite> = HashMap::new();
    if let Some(arr) = d.get("nets").and_then(|n| n.as_array()) {
        for n in arr {
            let Some(name) = str_at(n, "name") else { continue };
            if let Some(g) = n.get("graphical").and_then(|g| g.as_object()) {
                for kind in ["wires", "junctions", "labels", "power_ports", "ports", "sheet_entries"] {
                    if let Some(list) = g.get(kind).and_then(|l| l.as_array()) {
                        for u in list.iter().filter_map(|u| u.as_str()) {
                            elem_kind.insert(u.to_string(), kind.to_string());
                        }
                    }
                }
            }
            let class = net_classes
                .get(name)
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|c| c.as_str())
                .unwrap_or("Default")
                .to_string();
            let terminals = parse_terminals(n);
            let sheet_nums: Vec<i64> = n
                .get("source_sheets")
                .and_then(|s| s.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|u| u.as_str())
                        .filter_map(|u| uuidpath_to_num.get(u).copied())
                        .collect()
                })
                .unwrap_or_default();
            // Merge, don't overwrite, when a name recurs. The extractor emits one
            // net entry per sheet for a local label reused across sheets that KiCad
            // joins through a bus member on a hierarchical sheet pin (e.g. AN0, a
            // member of AN[0..7]): one signal, separate entries because the
            // netlister doesn't expand buses. Unioning their sheets/terminals/
            // by_sheet makes the net card list every sheet it lives on — the old
            // `insert` kept only the last entry and dropped the others' sheets.
            let merged_by_sheet = by_sheet.remove(name).unwrap_or_default();
            let entry = nets.entry(name.to_string()).or_insert_with(|| NetLite {
                class: class.clone(),
                terminals: Vec::new(),
                sheets: Vec::new(),
                by_sheet: HashMap::new(),
            });
            if entry.class == "Default" && class != "Default" {
                entry.class = class;
            }
            entry.terminals.extend(terminals);
            entry.sheets.extend(sheet_nums);
            for (sheet, uuids) in merged_by_sheet {
                entry.by_sheet.entry(sheet).or_default().extend(uuids);
            }
        }
        // Normalise merged nets: stable sheet order, de-duped terminals.
        for net in nets.values_mut() {
            net.sheets.sort_unstable();
            net.sheets.dedup();
            net.terminals.sort_by(|a, b| (&a.d, &a.p).cmp(&(&b.d, &b.p)));
            net.terminals.dedup_by(|a, b| a.d == b.d && a.p == b.p);
        }
    }

    // Components — card payloads.
    let component_to_nets = indexes.get("component_to_nets").cloned().unwrap_or(Value::Null);
    let mut components: HashMap<String, CompLite> = HashMap::new();
    if let Some(arr) = d.get("components").and_then(|c| c.as_array()) {
        for c in arr {
            let Some(designator) = str_at(c, "designator") else { continue };
            let params = c.get("parameters");
            let get_param = |k: &str| {
                params
                    .and_then(|p| p.get(k))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let fp = str_at(c, "footprint")
                .unwrap_or("")
                .rsplit(':')
                .next()
                .unwrap_or("")
                .to_string();
            let desc: String = str_at(c, "description").unwrap_or("").chars().take(160).collect();
            let sheet = c
                .get("hierarchy")
                .and_then(|h| h.get("sheet_path_uuids"))
                .and_then(|v| v.as_str())
                .and_then(|u| uuidpath_to_num.get(u).copied());
            let dnp = get_param("kicad_dnp").eq_ignore_ascii_case("true");
            let nets_of = component_to_nets
                .get(designator)
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).map(String::from).collect())
                .unwrap_or_default();
            let bbox = c.get("bbox").and_then(|b| {
                let f = |k: &str| b.get(k).and_then(|v| v.as_f64());
                Some([f("x")?, f("y")?, f("w")?, f("h")?])
            });
            components.insert(
                designator.to_string(),
                CompLite {
                    value: str_at(c, "value").unwrap_or("").to_string(),
                    mpn: get_param("MPN"),
                    mfr: get_param("Manufacturer"),
                    fp,
                    desc,
                    sheet,
                    dnp,
                    nets: nets_of,
                    svg_id: str_at(c, "svg_id").unwrap_or("").to_string(),
                    bbox,
                },
            );
        }
    }

    // Structured geometry IR path (GPU renderer input), cache-relative like the SVGs.
    let pcb_geometry = manifest
        .as_ref()
        .and_then(|m| m.get("pcb_geometry"))
        .and_then(|v| v.as_str())
        .map(|f| format!("{design_rel}/{f}"));

    Ok(DesignIndexes {
        sheets,
        layers,
        svg_to_net,
        svg_to_nets,
        svg_to_comp,
        elem_kind,
        nets,
        components,
        theme: d.get("theme").cloned().unwrap_or(Value::Null),
        pcb_geometry,
    })
}

fn strip_leading_index(stem: &str) -> &str {
    // "23_can" -> "can"; "01_MAIN-BOARD" -> "MAIN-BOARD".
    match stem.split_once('_') {
        Some((head, rest)) if !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => stem,
    }
}

fn string_map(v: Option<&Value>) -> HashMap<String, String> {
    v.and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn string_list_map(v: Option<&Value>) -> HashMap<String, Vec<String>> {
    v.and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, val)| {
                    val.as_array().map(|a| {
                        (
                            k.clone(),
                            a.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect(),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------- BOM lines

#[derive(Serialize)]
pub struct BomLine {
    pub item: i64,
    pub qty: i64,
    pub designators: Vec<String>,
    pub value: String,
    pub footprint: String,
    pub mpn: String,
    pub dnp: bool,
    /// Every string field the crunched BOM carries for this line, verbatim (Description,
    /// Manufacturer, MSL, "Automotive Grade", …). Lets the frontend render arbitrary
    /// user/custom columns named by a `.kicad_pro` BOM preset.
    pub fields: HashMap<String, String>,
}

/// BOM table payload (WS7), read straight from the crunched grouped-json bundle so
/// it works in both vault mode and the `PCBREVIEW_CACHE_DIR` dev override (which has
/// no SQLite index).
pub fn bom_lines(vault_cache: Option<PathBuf>) -> Result<Vec<BomLine>, String> {
    let cache = cache_dir(vault_cache)?;
    let bom_dir = cache.join("bom");
    let path = fs::read_dir(&bom_dir)
        .map_err(|_| "no BOM in crunch cache".to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .ok_or("no BOM JSON in crunch cache")?;
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("BOM parse: {e}"))?;
    let lines = v
        .get("lines")
        .and_then(|l| l.as_array())
        .ok_or("BOM JSON has no lines")?;
    let field = |line: &Value, keys: &[&str]| -> String {
        line.get("fields")
            .and_then(|f| f.as_object())
            .and_then(|f| {
                keys.iter().find_map(|k| {
                    f.get(*k).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
                })
            })
            .unwrap_or("")
            .to_string()
    };
    Ok(lines
        .iter()
        .map(|l| BomLine {
            item: l.get("item").and_then(|v| v.as_i64()).unwrap_or(0),
            qty: l.get("quantity").and_then(|v| v.as_i64()).unwrap_or(0),
            designators: l
                .get("designators")
                .and_then(|d| d.as_array())
                .map(|a| {
                    a.iter().filter_map(|v| v.as_str()).map(String::from).collect()
                })
                .unwrap_or_default(),
            value: field(l, &["value", "Value"]),
            footprint: field(l, &["footprint", "Footprint"])
                .rsplit(':')
                .next()
                .unwrap_or("")
                .to_string(),
            mpn: field(l, &["manufacturer_part_number", "mpn", "MPN"]),
            dnp: l.get("dnp").and_then(|v| v.as_bool()).unwrap_or(false),
            fields: l
                .get("fields")
                .and_then(|f| f.as_object())
                .map(|f| {
                    f.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect())
}

// ---------------------------------------------------------------- BOM presets

/// One column in a KiCad BOM preset (`fields_ordered` entry). `name` is the field key
/// to look up on a `BomLine` — either a real symbol field or a KiCad virtual field
/// (`${QUANTITY}`, `${DNP}`, `${ITEM_NUMBER}`), passed through as-is for the frontend
/// to map. `label` is the column header the user chose.
#[derive(Serialize, PartialEq, Debug)]
pub struct BomPresetField {
    pub name: String,
    pub label: String,
    pub show: bool,
}

/// A KiCad BOM column set, from the project's `.kicad_pro`.
#[derive(Serialize, PartialEq, Debug)]
pub struct BomPreset {
    pub name: String,
    pub fields: Vec<BomPresetField>,
    pub sort_field: String,
    pub sort_asc: bool,
    pub exclude_dnp: bool,
    pub group_symbols: bool,
    /// True for the entry built from the live `schematic.bom_settings` block — the column
    /// set KiCad itself currently has selected for this project.
    pub is_project_default: bool,
}

/// Parse one `bom_settings`/preset object. `None` when it carries no `fields_ordered`
/// (an empty/absent settings block is not a usable preset).
fn parse_bom_preset(v: &Value, fallback_name: &str) -> Option<BomPreset> {
    let ordered = v.get("fields_ordered")?.as_array()?;
    if ordered.is_empty() {
        return None;
    }
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let fields = ordered
        .iter()
        .filter_map(|f| {
            let name = f.get("name")?.as_str()?.to_string();
            let label = f
                .get("label")
                .and_then(|l| l.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&name)
                .to_string();
            Some(BomPresetField {
                name,
                label,
                show: f.get("show").and_then(|s| s.as_bool()).unwrap_or(true),
            })
        })
        .collect();
    Some(BomPreset {
        name: if name.is_empty() { fallback_name.to_string() } else { name.to_string() },
        fields,
        sort_field: v
            .get("sort_field")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        sort_asc: v.get("sort_asc").and_then(|s| s.as_bool()).unwrap_or(true),
        exclude_dnp: v.get("exclude_dnp").and_then(|s| s.as_bool()).unwrap_or(false),
        group_symbols: v.get("group_symbols").and_then(|s| s.as_bool()).unwrap_or(true),
        is_project_default: false,
    })
}

/// Extract the BOM presets from a parsed `.kicad_pro`: the live `schematic.bom_settings`
/// first (named "Project" when KiCad left its name empty), then any named entries in
/// `schematic.bom_presets` (KiCad 8/9 writes them there; older/hand-edited files may
/// nest them under `bom_settings.presets`, so both are read).
fn bom_presets_from_pro(pro: &Value) -> Vec<BomPreset> {
    let Some(sch) = pro.get("schematic") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let settings = sch.get("bom_settings");
    if let Some(mut p) = settings.and_then(|s| parse_bom_preset(s, "Project")) {
        // KiCad records "which preset is selected" only as this live block (its `name` is
        // the preset it was applied from, empty when the user customised it), so this is
        // the project's default column set.
        p.is_project_default = true;
        out.push(p);
    }
    let named = sch
        .get("bom_presets")
        .or_else(|| settings.and_then(|s| s.get("presets")))
        .and_then(|p| p.as_array());
    for entry in named.into_iter().flatten() {
        if let Some(p) = parse_bom_preset(entry, "Preset") {
            if !out.iter().any(|e| e.name == p.name) {
                out.push(p);
            }
        }
    }
    out
}

/// BOM presets for the open project's `.kicad_pro`. A viewing aid only: a missing path,
/// unreadable file or absent settings yields an empty list, never an error.
pub fn bom_presets(pro_path: Option<PathBuf>) -> Vec<BomPreset> {
    let Some(path) = pro_path else {
        log::debug!("bom_presets: no EDA project file for the open project");
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        log::debug!("bom_presets: cannot read {}", path.display());
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        log::debug!("bom_presets: {} is not JSON (legacy project?)", path.display());
        return Vec::new();
    };
    let presets = bom_presets_from_pro(&v);
    log::debug!("bom_presets: {} preset(s) in {}", presets.len(), path.display());
    presets
}

// ---------------------------------------------------------------- artifact serving

/// Serve a sheet SVG (or other cached artifact) by its cache-relative path, with the
/// metadata block stripped (70-90% of KiCad SVG bytes). Path-validated against the
/// cache root so the frontend can never read outside the bundle.
pub fn read_artifact(vault_cache: Option<PathBuf>, rel_path: &str) -> Result<String, String> {
    let cache = cache_dir(vault_cache)?;
    let cache = fs::canonicalize(&cache).map_err(|e| e.to_string())?;
    // Dev-cache mode indexes paths as "/pcb/…" (empty design_rel); a leading slash
    // would make join() jump to the drive root on Windows.
    let target = cache.join(rel_path.trim_start_matches(['/', '\\']));
    let target = fs::canonicalize(&target)
        .map_err(|_| format!("artifact not found: {rel_path}"))?;
    if !target.starts_with(&cache) {
        return Err("artifact path escapes the cache directory".into());
    }
    let meta = fs::metadata(&target).map_err(|e| e.to_string())?;
    if meta.len() > MAX_ARTIFACT_BYTES {
        return Err(format!("artifact too large: {} bytes", meta.len()));
    }
    let text = fs::read_to_string(&target).map_err(|e| e.to_string())?;
    Ok(strip_metadata(&text))
}

/// Remove `<metadata>…</metadata>` (KiCad embeds the full source there).
fn strip_metadata(svg: &str) -> String {
    let (Some(start), Some(end)) = (svg.find("<metadata"), svg.find("</metadata>")) else {
        return svg.to_string();
    };
    if end < start {
        return svg.to_string();
    }
    let mut out = String::with_capacity(svg.len());
    out.push_str(&svg[..start]);
    out.push_str(&svg[end + "</metadata>".len()..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // BeagleConnect Freedom (public reference design): 11 sheets incl. a root whose
    // title != filename and a "User Input/Output" sheet - the cases that force
    // manifest-based SVG matching. Set SPINZERO_TEST_CACHE to a crunched bundle
    // (.pcbcache or PCBREVIEW_CACHE_DIR-style dir); the test skips when unset.
    fn freedom_fixture() -> Option<PathBuf> {
        let p = PathBuf::from(std::env::var("SPINZERO_TEST_CACHE").ok()?);
        p.join("design").join("schematics").is_dir().then_some(p)
    }

    #[test]
    fn freedom_sheets_all_match_svgs() {
        let Some(root) = freedom_fixture() else { return };
        let ix = build_indexes(Some(root.clone())).expect("indexes build");
        assert_eq!(ix.sheets.len(), 11, "11 schematic sheets");
        for s in &ix.sheets {
            assert!(s.svg.is_some(), "sheet {} '{}' matched an SVG", s.num, s.name);
        }
        assert!(ix.sheets.iter().any(|s| s.name == "User Input/Output"));
        assert!(!ix.nets.is_empty() && !ix.components.is_empty());
        assert!(ix.nets.values().any(|n| n.sheets.len() > 1), "cross-sheet nets exist");
        let bom = bom_lines(Some(root)).expect("bom lines");
        assert!(!bom.is_empty(), "BOM lines parsed");
        assert!(
            bom.iter().any(|l| !l.fields.is_empty()),
            "raw per-line fields passed through for custom columns"
        );
    }

    #[test]
    fn kicad_pro_bom_presets_parse() {
        let pro: Value = serde_json::from_str(
            r#"{
              "schematic": {
                "bom_settings": {
                  "name": "",
                  "sort_field": "Description",
                  "sort_asc": true,
                  "filter_string": "",
                  "exclude_dnp": false,
                  "group_symbols": true,
                  "fields_ordered": [
                    { "name": "Reference", "label": "Reference", "show": true, "group_by": false },
                    { "name": "${QUANTITY}", "label": "Qty", "show": true, "group_by": false },
                    { "name": "${DNP}", "label": "DNP", "show": true, "group_by": true },
                    { "name": "Automotive Grade", "label": "", "show": false, "group_by": false }
                  ]
                },
                "bom_presets": [
                  {
                    "name": "manufacturing_bom",
                    "sort_field": "MPN",
                    "sort_asc": false,
                    "exclude_dnp": true,
                    "group_symbols": false,
                    "fields_ordered": [
                      { "name": "MPN", "label": "MPN", "show": true }
                    ]
                  }
                ]
              }
            }"#,
        )
        .unwrap();
        let presets = bom_presets_from_pro(&pro);
        assert_eq!(presets.len(), 2);

        let project = &presets[0];
        assert_eq!(project.name, "Project", "empty settings name falls back");
        assert_eq!(project.sort_field, "Description");
        assert!(project.sort_asc && !project.exclude_dnp && project.group_symbols);
        // Virtual fields pass through verbatim; a blank label falls back to the name.
        assert_eq!(
            project.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["Reference", "${QUANTITY}", "${DNP}", "Automotive Grade"]
        );
        assert_eq!(project.fields[1].label, "Qty");
        assert_eq!(project.fields[3].label, "Automotive Grade");
        assert!(!project.fields[3].show);
        assert!(project.is_project_default, "live bom_settings is the project default");

        let named = &presets[1];
        assert_eq!(named.name, "manufacturing_bom");
        assert!(!named.sort_asc && named.exclude_dnp && !named.group_symbols);
        assert!(!named.is_project_default, "named presets are not the default");

        // Legacy shape: presets nested under bom_settings.
        let nested: Value = serde_json::from_str(
            r#"{"schematic":{"bom_settings":{"name":"","presets":[
                 {"name":"Alt","fields_ordered":[{"name":"Value","label":"Value","show":true}]}]}}}"#,
        )
        .unwrap();
        let nested = bom_presets_from_pro(&nested);
        assert_eq!(nested.len(), 1, "settings without fields_ordered is not a preset");
        assert_eq!(nested[0].name, "Alt");

        // Nothing usable → empty, never an error.
        assert!(bom_presets_from_pro(&serde_json::json!({})).is_empty());
        assert!(bom_presets_from_pro(&serde_json::json!({"schematic": {}})).is_empty());
        assert!(bom_presets(None).is_empty());
        assert!(bom_presets(Some(PathBuf::from("no-such-file.kicad_pro"))).is_empty());
    }

    #[test]
    fn read_artifact_strips_metadata() {
        let Some(root) = freedom_fixture() else { return };
        let ix = build_indexes(Some(root.clone())).unwrap();
        let rel = ix.sheets.iter().find_map(|s| s.svg.clone()).unwrap();
        let svg = read_artifact(Some(root), &rel).expect("serve svg");
        assert!(svg.contains("<svg"), "still an SVG");
        assert!(!svg.contains("<metadata"), "metadata stripped");
        assert!(svg.contains("data-uuid"), "hit-test anchors preserved");
    }
}
