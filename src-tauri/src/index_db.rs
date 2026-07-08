//! SQLite index — a rebuildable cache over the IR (plan §2.2 rule 4).
//! Owned exclusively by this module; the frontend sees typed commands only.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

/// Bump when the schema changes. The DB is a cache: on mismatch it is dropped
/// and rebuilt from the extraction store on next open.
const SCHEMA_VERSION: i32 = 3;

pub fn open(project_dir: &Path) -> Result<Connection, String> {
    // The index is a rebuildable cache + machine-specific, so it lives under the
    // OS local-data dir — never inside the synced project folder.
    let dir = crate::project::local_data_root(project_dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut conn = Connection::open(dir.join("index.sqlite")).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "NORMAL").map_err(|e| e.to_string())?;
    // Writers (crunch ingest, background rebuild, set-active ingest) each open their own
    // connection; without a busy timeout a second concurrent writer gets SQLITE_BUSY
    // immediately instead of waiting for the WAL write lock.
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if version != SCHEMA_VERSION {
        // Serialize concurrent migrations: an IMMEDIATE tx grabs the write lock up
        // front, so a second connection opening right after an upgrade waits on the
        // busy timeout rather than interleaving drop/create with us. user_version is
        // bumped LAST and inside the tx, so a failed drop rolls back atomically instead
        // of leaving a bumped-but-mixed-version DB.
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        // Re-read under the write lock — another connection may already have migrated.
        let current: i32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
        if current != SCHEMA_VERSION {
            drop_tables(&tx)?;
            create_schema(&tx)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    create_schema(&conn)?;
    Ok(conn)
}

fn drop_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS revisions; DROP TABLE IF EXISTS components;
         DROP TABLE IF EXISTS nets; DROP TABLE IF EXISTS pins;
         DROP TABLE IF EXISTS sheets; DROP TABLE IF EXISTS layers;
         DROP TABLE IF EXISTS bom_lines; DROP TABLE IF EXISTS findings;
         DROP TABLE IF EXISTS search_fts;",
    )
    .map_err(|e| e.to_string())
}

/// Is the index empty (no ingested revisions)? Used to decide on auto-rebuild.
pub fn is_empty(conn: &Connection) -> bool {
    conn.query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get::<_, i64>(0))
        .map(|n| n == 0)
        .unwrap_or(true)
}

fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS revisions(
            id TEXT PRIMARY KEY, ts TEXT NOT NULL, project TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS components(
            id INTEGER PRIMARY KEY, revision_id TEXT NOT NULL, designator TEXT NOT NULL,
            value TEXT, footprint TEXT, mpn TEXT, sheet TEXT, svg_id TEXT, bbox TEXT,
            dnp INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS idx_components_rev ON components(revision_id, designator);
        CREATE TABLE IF NOT EXISTS nets(
            id INTEGER PRIMARY KEY, revision_id TEXT NOT NULL, name TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS idx_nets_rev ON nets(revision_id, name);
        CREATE TABLE IF NOT EXISTS pins(
            component_id INTEGER NOT NULL, net_id INTEGER NOT NULL,
            pin_number TEXT NOT NULL, pin_name TEXT);
        CREATE INDEX IF NOT EXISTS idx_pins_component ON pins(component_id);
        CREATE INDEX IF NOT EXISTS idx_pins_net ON pins(net_id);
        CREATE TABLE IF NOT EXISTS sheets(
            id INTEGER PRIMARY KEY, revision_id TEXT NOT NULL,
            number INTEGER, name TEXT NOT NULL, sheet_path TEXT NOT NULL DEFAULT '/',
            svg_path TEXT NOT NULL, page TEXT NOT NULL DEFAULT '');
        CREATE TABLE IF NOT EXISTS layers(
            id INTEGER PRIMARY KEY, revision_id TEXT NOT NULL,
            name TEXT NOT NULL, role TEXT, svg_path TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS bom_lines(
            id INTEGER PRIMARY KEY, revision_id TEXT NOT NULL, item INTEGER,
            qty INTEGER, designators TEXT, mpn TEXT, fields_json TEXT);
        CREATE TABLE IF NOT EXISTS findings(
            id INTEGER PRIMARY KEY, review_id TEXT, anchor_json TEXT, severity TEXT,
            title TEXT, body TEXT, source TEXT, status TEXT, fingerprint TEXT);
        CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
            kind, ref, detail, revision_id UNINDEXED);
        "#,
    )
    .map_err(|e| e.to_string())
}

pub fn delete_revision(conn: &Connection, revision_id: &str) -> Result<(), String> {
    let run = |sql: &str| conn.execute(sql, params![revision_id]).map_err(|e| e.to_string());
    run("DELETE FROM pins WHERE component_id IN (SELECT id FROM components WHERE revision_id = ?1)")?;
    run("DELETE FROM components WHERE revision_id = ?1")?;
    run("DELETE FROM nets WHERE revision_id = ?1")?;
    run("DELETE FROM sheets WHERE revision_id = ?1")?;
    run("DELETE FROM layers WHERE revision_id = ?1")?;
    run("DELETE FROM bom_lines WHERE revision_id = ?1")?;
    run("DELETE FROM search_fts WHERE revision_id = ?1")?;
    run("DELETE FROM revisions WHERE id = ?1")?;
    Ok(())
}

pub fn drop_all(pcbreview: &Path) -> Result<(), String> {
    let conn = open(pcbreview)?;
    drop_tables(&conn)?;
    create_schema(&conn)
}

// ---------------------------------------------------------------- ingest

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// Find the design manifest inside a crunched bundle. The in-house KiCad extractor
/// writes `design_review_manifest.json` at the bundle ROOT; some variants nest it
/// under `design/`/`design_review/` (or use `manifest.json`). The root (".") must be
/// checked too — this mirrors `design.rs::find_design_dir`. They had diverged (the
/// viewer read the root bundle while validate/ingest looked only in subdirs), so a
/// perfectly good root-bundle crunch reported "no design manifest" at the validate step.
fn find_manifest(cache_dir: &Path) -> Result<(Value, std::path::PathBuf), String> {
    for sub in ["design", "design_review", "."] {
        for name in ["design_review_manifest.json", "manifest.json"] {
            let p = cache_dir.join(sub).join(name);
            if p.exists() {
                let text = fs::read_to_string(&p).map_err(|e| e.to_string())?;
                let v: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("manifest parse: {e}"))?;
                let parent = p.parent().ok_or("manifest path has no parent dir")?;
                return Ok((v, parent.to_path_buf()));
            }
        }
    }
    Err("no design manifest found in crunch output".into())
}

/// Manifest schema gate — the app refuses bundles it doesn't understand.
pub fn check_manifest_schema(cache_dir: &Path) -> Result<(), String> {
    let (manifest, _) = find_manifest(cache_dir)?;
    let schema = manifest
        .get("schema")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let known = schema.contains("design_review_manifest") || schema.contains("design_review");
    if schema.is_empty() {
        // Tolerated for now: uniform schema markers are an upstream work item.
        return Ok(());
    }
    if known {
        Ok(())
    } else {
        Err(format!("unknown manifest schema '{schema}' — update the app"))
    }
}

/// Ingest one crunched bundle under `revision_id`. Idempotent: re-ingesting
/// the same revision replaces its rows.
pub fn ingest(
    conn: &mut Connection,
    cache_dir: &Path,
    revision_id: &str,
    ts: &str,
    project: &str,
) -> Result<(), String> {
    let (manifest, design_dir) = find_manifest(cache_dir)?;
    let design_rel = design_dir
        .strip_prefix(cache_dir)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    // Dev-cache mode points straight at the design dir → empty rel; "{rel}/{file}"
    // must not produce a rooted "/pcb/…" path.
    let design_rel = if design_rel.is_empty() {
        String::new()
    } else {
        format!("{design_rel}/")
    };

    // Design JSON: manifest pointer, else glob *_design.json next to it.
    let design_json_path = str_of(&manifest, "design_json")
        .map(|f| design_dir.join(f))
        .filter(|p| p.exists())
        .or_else(|| {
            fs::read_dir(&design_dir).ok().and_then(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .find(|p| p.file_name().map_or(false, |n| {
                        n.to_string_lossy().ends_with("_design.json")
                    }))
            })
        })
        .ok_or("design JSON not found in bundle")?;
    let design: Value = serde_json::from_str(
        &fs::read_to_string(&design_json_path).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("design JSON parse: {e}"))?;

    // Delete the old rows INSIDE the ingest transaction: if any insert fails (or the
    // app dies mid-ingest) the whole re-ingest rolls back to the prior revision, and a
    // concurrent reader never sees a half-deleted revision (components gone, sheets kept).
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    delete_revision(&tx, revision_id)?;
    tx.execute(
        "INSERT INTO revisions(id, ts, project) VALUES (?1, ?2, ?3)",
        params![revision_id, ts, project],
    )
    .map_err(|e| e.to_string())?;

    // Sheets + layers from the manifest.
    if let Some(sheets) = manifest.get("schematic_svgs").and_then(|s| s.as_array()) {
        for s in sheets {
            tx.execute(
                "INSERT INTO sheets(revision_id, number, name, sheet_path, svg_path, page) VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    revision_id,
                    s.get("sheet_number").and_then(|n| n.as_i64()),
                    str_of(s, "sheet_name").unwrap_or_default(),
                    str_of(s, "sheet_path").unwrap_or_else(|| "/".into()),
                    format!("{design_rel}{}", str_of(s, "file").unwrap_or_default()),
                    str_of(s, "page").unwrap_or_default(),
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if let Some(layers) = manifest.get("pcb_svgs").and_then(|s| s.as_array()) {
        for l in layers {
            tx.execute(
                "INSERT INTO layers(revision_id, name, role, svg_path) VALUES (?1,?2,?3,?4)",
                params![
                    revision_id,
                    str_of(l, "layer").unwrap_or_default(),
                    str_of(l, "role").unwrap_or_else(|| "copper".into()),
                    format!("{design_rel}{}", str_of(l, "file").unwrap_or_default()),
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    // Components.
    let mut component_ids: HashMap<String, i64> = HashMap::new();
    if let Some(components) = design.get("components").and_then(|c| c.as_array()) {
        for c in components {
            let designator = str_of(c, "designator").unwrap_or_default();
            if designator.is_empty() {
                continue;
            }
            let params_obj = c.get("parameters");
            let mpn = params_obj.and_then(|p| {
                ["MPN", "Manufacturer Part Number", "ManufacturerPartNumber", "mpn"]
                    .iter()
                    .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            });
            let dnp = params_obj
                .and_then(|p| p.get("kicad_dnp"))
                .and_then(|v| v.as_str())
                .map(|s| s.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            let sheet = c
                .get("hierarchy")
                .and_then(|h| h.get("sheet_path").or_else(|| h.get("sheet")))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            tx.execute(
                "INSERT INTO components(revision_id, designator, value, footprint, mpn, sheet, svg_id, bbox, dnp)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    revision_id,
                    designator,
                    str_of(c, "value"),
                    str_of(c, "footprint"),
                    mpn,
                    sheet,
                    str_of(c, "svg_id"),
                    c.get("bbox").map(|b| b.to_string()), // upstream work item; absent today
                    dnp as i64,
                ],
            )
            .map_err(|e| e.to_string())?;
            let id = tx.last_insert_rowid();
            component_ids.insert(designator.clone(), id);
            tx.execute(
                "INSERT INTO search_fts(kind, ref, detail, revision_id) VALUES ('component',?1,?2,?3)",
                params![
                    designator,
                    format!(
                        "{} {}",
                        str_of(c, "value").unwrap_or_default(),
                        c.get("parameters")
                            .and_then(|p| p.get("MPN"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                    ),
                    revision_id,
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    // Nets + pins.
    if let Some(nets) = design.get("nets").and_then(|n| n.as_array()) {
        for n in nets {
            let name = str_of(n, "name").unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            tx.execute(
                "INSERT INTO nets(revision_id, name) VALUES (?1,?2)",
                params![revision_id, name],
            )
            .map_err(|e| e.to_string())?;
            let net_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO search_fts(kind, ref, detail, revision_id) VALUES ('net',?1,'',?2)",
                params![name, revision_id],
            )
            .map_err(|e| e.to_string())?;
            if let Some(terminals) = n.get("terminals").and_then(|t| t.as_array()) {
                for t in terminals {
                    let designator = str_of(t, "designator").unwrap_or_default();
                    let Some(&component_id) = component_ids.get(&designator) else {
                        continue;
                    };
                    tx.execute(
                        "INSERT INTO pins(component_id, net_id, pin_number, pin_name) VALUES (?1,?2,?3,?4)",
                        params![
                            component_id,
                            net_id,
                            str_of(t, "pin").unwrap_or_default(),
                            str_of(t, "pin_name").filter(|s| s != "~"),
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
    }

    // BOM (grouped-json: {schema, lines:[{item, quantity, designators[], dnp, fields{}}]}).
    let bom_dir = cache_dir.join("bom");
    if let Ok(rd) = fs::read_dir(&bom_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Ok(bom): Result<Value, _> = serde_json::from_str(&text) else { continue };
            let Some(lines) = bom.get("lines").and_then(|l| l.as_array()) else { continue };
            for line in lines {
                let designators = line
                    .get("designators")
                    .and_then(|d| d.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                let mpn = line
                    .get("fields")
                    .and_then(|f| f.get("manufacturer_part_number"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                tx.execute(
                    "INSERT INTO bom_lines(revision_id, item, qty, designators, mpn, fields_json)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        revision_id,
                        line.get("item").and_then(|v| v.as_i64()),
                        line.get("quantity").and_then(|v| v.as_i64()),
                        designators,
                        mpn,
                        line.to_string(),
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            break; // one grouped-json BOM per bundle
        }
    }

    tx.commit().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- queries

#[derive(Serialize)]
pub struct ProjectSummary {
    pub name: String,
    pub revision_id: String,
    pub sheet_count: i64,
    pub layer_count: i64,
    pub component_count: i64,
    pub net_count: i64,
    pub bom_line_count: i64,
}

fn latest_revision(conn: &Connection) -> Option<(String, String)> {
    conn.query_row(
        "SELECT id, project FROM revisions ORDER BY ts DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

/// Resolve which revision (extraction) to read: the requested id if it is present
/// in the index, else the latest. Returns (id, project name).
fn resolve_revision(conn: &Connection, want: Option<&str>) -> Option<(String, String)> {
    if let Some(id) = want {
        if let Ok(project) = conn.query_row(
            "SELECT project FROM revisions WHERE id=?1",
            params![id],
            |r| r.get::<_, String>(0),
        ) {
            return Some((id.to_string(), project));
        }
    }
    latest_revision(conn)
}

fn count(conn: &Connection, sql: &str, rev: &str) -> i64 {
    conn.query_row(sql, params![rev], |r| r.get(0)).unwrap_or(0)
}

pub fn project_summary(conn: &Connection, want: Option<&str>) -> Option<ProjectSummary> {
    let (rev, project) = resolve_revision(conn, want)?;
    Some(ProjectSummary {
        name: project,
        sheet_count: count(conn, "SELECT COUNT(*) FROM sheets WHERE revision_id=?1", &rev),
        layer_count: count(conn, "SELECT COUNT(*) FROM layers WHERE revision_id=?1", &rev),
        component_count: count(conn, "SELECT COUNT(*) FROM components WHERE revision_id=?1", &rev),
        net_count: count(conn, "SELECT COUNT(*) FROM nets WHERE revision_id=?1", &rev),
        bom_line_count: count(conn, "SELECT COUNT(*) FROM bom_lines WHERE revision_id=?1", &rev),
        revision_id: rev,
    })
}

#[derive(Serialize)]
pub struct SheetInfo {
    pub number: i64,
    pub name: String,
    pub sheet_path: String,
    pub svg_path: String,
    /// KiCad page label; empty when the project uses automatic numbering.
    pub page: String,
}

pub fn list_sheets(conn: &Connection, want: Option<&str>) -> Vec<SheetInfo> {
    let Some((rev, _)) = resolve_revision(conn, want) else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT number, name, sheet_path, svg_path, page FROM sheets
         WHERE revision_id=?1 ORDER BY number",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![rev], |r| {
        Ok(SheetInfo {
            number: r.get(0)?,
            name: r.get(1)?,
            sheet_path: r.get(2)?,
            svg_path: r.get(3)?,
            page: r.get(4)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

#[derive(Serialize)]
pub struct LayerInfo {
    pub name: String,
    pub role: String,
    pub svg_path: String,
}

pub fn list_layers(conn: &Connection, want: Option<&str>) -> Vec<LayerInfo> {
    let Some((rev, _)) = resolve_revision(conn, want) else { return Vec::new() };
    let Ok(mut stmt) =
        conn.prepare("SELECT name, role, svg_path FROM layers WHERE revision_id=?1 ORDER BY id")
    else {
        return Vec::new();
    };
    stmt.query_map(params![rev], |r| {
        Ok(LayerInfo { name: r.get(0)?, role: r.get(1)?, svg_path: r.get(2)? })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

#[derive(Serialize)]
pub struct PinRef {
    pub net: String,
    pub pin: String,
    pub pin_name: Option<String>,
}

#[derive(Serialize)]
pub struct ComponentInfo {
    pub designator: String,
    pub value: Option<String>,
    pub footprint: Option<String>,
    pub mpn: Option<String>,
    pub sheet: Option<String>,
    pub dnp: bool,
    pub nets: Vec<PinRef>,
}

pub fn get_component(conn: &Connection, designator: &str) -> Option<ComponentInfo> {
    let (rev, _) = latest_revision(conn)?;
    let (id, info) = conn
        .query_row(
            "SELECT id, designator, value, footprint, mpn, sheet, dnp
             FROM components WHERE revision_id=?1 AND designator=?2",
            params![rev, designator],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    ComponentInfo {
                        designator: r.get(1)?,
                        value: r.get(2)?,
                        footprint: r.get(3)?,
                        mpn: r.get(4)?,
                        sheet: r.get(5)?,
                        dnp: r.get::<_, i64>(6)? != 0,
                        nets: Vec::new(),
                    },
                ))
            },
        )
        .ok()?;
    let mut info = info;
    let mut stmt = conn
        .prepare(
            "SELECT n.name, p.pin_number, p.pin_name FROM pins p
             JOIN nets n ON n.id = p.net_id WHERE p.component_id=?1 ORDER BY p.pin_number",
        )
        .ok()?;
    info.nets = stmt
        .query_map(params![id], |r| {
            Ok(PinRef { net: r.get(0)?, pin: r.get(1)?, pin_name: r.get(2)? })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    Some(info)
}

#[derive(Serialize)]
pub struct NetPin {
    pub designator: String,
    pub pin: String,
    pub pin_name: Option<String>,
}

#[derive(Serialize)]
pub struct NetInfo {
    pub name: String,
    pub pins: Vec<NetPin>,
}

pub fn get_net(conn: &Connection, name: &str) -> Option<NetInfo> {
    let (rev, _) = latest_revision(conn)?;
    let net_id: i64 = conn
        .query_row(
            "SELECT id FROM nets WHERE revision_id=?1 AND name=?2",
            params![rev, name],
            |r| r.get(0),
        )
        .ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT c.designator, p.pin_number, p.pin_name FROM pins p
             JOIN components c ON c.id = p.component_id
             WHERE p.net_id=?1 ORDER BY c.designator",
        )
        .ok()?;
    let pins = stmt
        .query_map(params![net_id], |r| {
            Ok(NetPin { designator: r.get(0)?, pin: r.get(1)?, pin_name: r.get(2)? })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    Some(NetInfo { name: name.into(), pins })
}

#[derive(Serialize)]
pub struct SearchHit {
    pub kind: String,
    pub r#ref: String,
    pub detail: String,
}

pub fn search(conn: &Connection, q: &str) -> Vec<SearchHit> {
    let Some((rev, _)) = latest_revision(conn) else { return Vec::new() };
    let cleaned: String = q
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || *c == '_' || *c == '-')
        .collect();
    if cleaned.trim().is_empty() {
        return Vec::new();
    }
    let fts_query = format!("\"{}\"*", cleaned.trim());
    let Ok(mut stmt) = conn.prepare(
        "SELECT kind, ref, detail FROM search_fts
         WHERE search_fts MATCH ?1 AND revision_id=?2 LIMIT 50",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![fts_query, rev], |r| {
        Ok(SearchHit { kind: r.get(0)?, r#ref: r.get(1)?, detail: r.get(2)? })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}
