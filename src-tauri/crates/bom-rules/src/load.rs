//! Column mapping: real BOM headers → the logical fields rules read, plus the
//! mapping report that makes a mapping miss visible instead of silently reading as
//! "the data is missing".
//!
//! Mirrors `run_bom.py::load_bom_csv`. Matching is case-insensitive and ignores
//! spaces/underscores/dots/hyphens/# so `MANUFACTURER_PART_NUMBER` and
//! `"Manufacturer Part Number"` are the same column.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::model::BomItem;
use crate::re;

/// One BOM row as it exists in the source: ordered (header, cell) pairs.
pub type Row = Vec<(String, String)>;

/// Canonical form for header matching: `Manufacturer_Part-Number.` → `manufacturerpartnumber`.
pub fn canon_header(h: &str) -> String {
    re!(r"[\s_.\-/#]+").replace_all(h, "").to_lowercase()
}

/// Canonical forms of every alias configured for one logical field. Callers that
/// synthesize a column (the app supplies designators/qty/DNP out-of-band) use this to
/// drop the source columns that would otherwise map to the same field twice.
pub fn alias_canon_set(config: &Value, logical: &str) -> BTreeSet<String> {
    config
        .get("field_aliases")
        .and_then(|f| f.get(logical))
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(canon_header)
                .collect()
        })
        .unwrap_or_default()
}

/// What mapped where — surfaced in the UI so a well-filled column that mapped to
/// nothing reads as "we didn't understand this column", not "this data is absent".
#[derive(Clone, Debug, Serialize, Default)]
pub struct MappingReport {
    /// logical field → the source column it was read from.
    pub fields: BTreeMap<String, String>,
    /// supplier label → the source columns carrying its part numbers.
    pub supplier_columns: BTreeMap<String, Vec<String>>,
    /// Columns no logical field claimed, worst-offender (most filled) first.
    pub unmapped_columns: Vec<UnmappedColumn>,
    pub row_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct UnmappedColumn {
    pub column: String,
    /// 0..1 — how many rows carry a value in this column.
    pub fill_rate: f64,
}

impl MappingReport {
    /// Unmapped columns filled on at least half the rows — the ones worth telling the
    /// user about (an unmapped column filled on 2% of rows is noise).
    pub fn notable_unmapped(&self) -> Vec<&UnmappedColumn> {
        self.unmapped_columns.iter().filter(|u| u.fill_rate >= 0.5).collect()
    }
}

/// Map raw rows onto `BomItem`s using the profile's aliases.
///
/// Rows that carry no mapped data at all are dropped (blank trailing CSV lines,
/// separator rows) — they would otherwise show up as unannotated phantom parts.
pub fn items_from_rows(rows: &[Row], config: &Value) -> (Vec<BomItem>, MappingReport) {
    let headers: Vec<String> = rows
        .first()
        .map(|r| r.iter().map(|(h, _)| h.clone()).collect())
        .unwrap_or_default();

    // First header wins for a canonical form, matching the Python `setdefault`.
    let mut by_canon: BTreeMap<String, String> = BTreeMap::new();
    for h in &headers {
        by_canon.entry(canon_header(h)).or_insert_with(|| h.clone());
    }
    let find_col = |aliases: &[&str]| -> Option<String> {
        aliases
            .iter()
            .find_map(|a| by_canon.get(&canon_header(a)).cloned())
    };

    let mut field_map: BTreeMap<String, String> = BTreeMap::new();
    if let Some(obj) = config.get("field_aliases").and_then(|f| f.as_object()) {
        for (logical, aliases) in obj {
            let list: Vec<&str> = aliases
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            if let Some(col) = find_col(&list) {
                field_map.insert(logical.clone(), col);
            }
        }
    }

    let mut supplier_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(obj) = config.get("supplier_pn_columns").and_then(|f| f.as_object()) {
        for (supplier, aliases) in obj {
            let cols: Vec<String> = aliases
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|alias| find_col(&[alias]))
                        .collect()
                })
                .unwrap_or_default();
            if !cols.is_empty() {
                supplier_map.insert(supplier.clone(), cols);
            }
        }
    }

    // "Unmapped" must mean *we did not understand this column*, not "another column
    // won the same logical field". Two things make that distinction matter: a source
    // can carry synonym columns for one field (the crunched KiCad BOM emits both the
    // symbol's `Value` and the extractor's canonical `value`), and a field has several
    // aliases (`MPN` and `Manufacturer Part Number`) of which only one can win. Either
    // would otherwise be reported as a blind spot the checker doesn't actually have.
    let mut known_canon: BTreeSet<String> = BTreeSet::new();
    for block in ["field_aliases", "supplier_pn_columns"] {
        if let Some(obj) = config.get(block).and_then(|f| f.as_object()) {
            for aliases in obj.values() {
                for alias in aliases.as_array().into_iter().flatten() {
                    if let Some(a) = alias.as_str() {
                        known_canon.insert(canon_header(a));
                    }
                }
            }
        }
    }

    let mut fill: BTreeMap<String, usize> = headers.iter().map(|h| (h.clone(), 0)).collect();
    let mut items = Vec::new();
    for row in rows {
        let cells: BTreeMap<&str, &str> =
            row.iter().map(|(h, v)| (h.as_str(), v.trim())).collect();
        for (h, v) in &cells {
            if !v.is_empty() {
                if let Some(n) = fill.get_mut(*h) {
                    *n += 1;
                }
            }
        }

        let mut raw: BTreeMap<String, String> = BTreeMap::new();
        for (logical, col) in &field_map {
            raw.insert(
                logical.clone(),
                cells.get(col.as_str()).copied().unwrap_or("").to_string(),
            );
        }

        // One part number per supplier: the first of its columns that carries one.
        let mut supplier_pns: Vec<(String, String)> = Vec::new();
        for (supplier, cols) in &supplier_map {
            for col in cols {
                let pn = cells.get(col.as_str()).copied().unwrap_or("");
                if !pn.is_empty() {
                    supplier_pns.push((supplier.clone(), pn.to_string()));
                    break;
                }
            }
        }

        if raw.values().all(|v| v.is_empty()) && supplier_pns.is_empty() {
            continue;
        }
        items.push(BomItem::new(raw, supplier_pns));
    }

    let row_count = rows.len();
    let mut unmapped: Vec<UnmappedColumn> = headers
        .iter()
        .filter(|h| !known_canon.contains(&canon_header(h)))
        .map(|h| UnmappedColumn {
            column: h.clone(),
            fill_rate: if row_count == 0 {
                0.0
            } else {
                (fill.get(h).copied().unwrap_or(0) as f64 / row_count as f64 * 1000.0).round()
                    / 1000.0
            },
        })
        .collect();
    unmapped.sort_by(|a, b| {
        b.fill_rate
            .partial_cmp(&a.fill_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.column.cmp(&b.column))
    });

    (
        items,
        MappingReport {
            fields: field_map,
            supplier_columns: supplier_map,
            unmapped_columns: unmapped,
            row_count,
        },
    )
}

/// Minimal RFC 4180 CSV reader (quoted fields, doubled quotes, CRLF, BOM) — enough
/// for BOM fixtures and the dev/CLI path, without pulling a dependency into the app.
/// Returns one `Row` per data line, each carrying the header names.
pub fn parse_csv(text: &str) -> Vec<Row> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let records = split_records(text);
    let Some(headers) = records.first() else {
        return Vec::new();
    };
    records
        .iter()
        .skip(1)
        .map(|rec| {
            headers
                .iter()
                .enumerate()
                .map(|(i, h)| (h.clone(), rec.get(i).cloned().unwrap_or_default()))
                .collect()
        })
        .collect()
}

fn split_records(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => record.push(std::mem::take(&mut field)),
            '\r' if !in_quotes => {}
            '\n' if !in_quotes => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    // A trailing newline leaves one empty record; drop rows that are entirely blank.
    records.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn headers_match_ignoring_punctuation_and_case() {
        let csv = "Designator,MANUFACTURER_PART_NUMBER,Qty,LCSC Part #\nR1,RC0402FR-0710KL,1,C25744\n";
        let rows = parse_csv(csv);
        let (items, report) = items_from_rows(&rows, &config::defaults());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reference, "R1");
        assert_eq!(items[0].mpn(), "RC0402FR-0710KL");
        assert_eq!(items[0].quantity_int, Some(1));
        assert_eq!(items[0].supplier_pns, vec![("LCSC".into(), "C25744".into())]);
        assert_eq!(report.fields.get("mpn").map(String::as_str), Some("MANUFACTURER_PART_NUMBER"));
        assert!(report.unmapped_columns.is_empty());
    }

    #[test]
    fn quoted_multi_designator_lines_split() {
        let csv = "Reference,Value,Quantity\n\"R1, R2, R3\",10k,3\n";
        let rows = parse_csv(csv);
        let (items, _) = items_from_rows(&rows, &config::defaults());
        assert_eq!(items[0].refs, vec!["R1", "R2", "R3"]);
    }

    #[test]
    fn synonym_columns_do_not_read_as_unmapped() {
        // The crunched KiCad BOM carries both the symbol's own field names and the
        // extractor's canonical lower-case duplicates; neither is a blind spot.
        let csv = "Reference,Value,value,MPN,Manufacturer Part Number
R1,10k,10k,RC1,RC1
";
        let rows = parse_csv(csv);
        let (_, report) = items_from_rows(&rows, &config::defaults());
        assert!(
            report.unmapped_columns.is_empty(),
            "synonyms reported as unmapped: {:?}",
            report.unmapped_columns
        );
    }

    #[test]
    fn unmapped_columns_are_reported_with_fill_rates() {
        let csv = "Reference,Value,House Code\nR1,10k,HC-1\nR2,4k7,\n";
        let rows = parse_csv(csv);
        let (_, report) = items_from_rows(&rows, &config::defaults());
        let unmapped = report.notable_unmapped();
        assert_eq!(unmapped.len(), 1);
        assert_eq!(unmapped[0].column, "House Code");
        assert!((unmapped[0].fill_rate - 0.5).abs() < 1e-9);
    }
}
