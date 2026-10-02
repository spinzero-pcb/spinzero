//! Capture CIS variants (`CIS/VariantStore/...`).
//!
//! A design with CIS variants keeps, beside its pages:
//!
//! - `BOM/BOMDataStream`: the BOM variant names;
//! - `BOM/<variant>/<variant>`: the part groups that variant selects;
//! - `BOM/<variant>/BOMPartData`: the occurrence ids its BOM contains;
//! - `Groups/GroupsDataStream`: the part groups;
//! - `Groups/<group>/<group>`: the group's members, each with a 0/1 state;
//! - optional per-group property updates (`UpdateStorage...DataStream`).
//!
//! Each of these is a u32 payload length then text: counted lists are
//! 0xF9-separated with the decimal count first; memberships are '~'-separated
//! `state 0xB0 id` records; property updates are '~'-separated `id 0xB0
//! names 0xC0 values` with '^' between names and between values.
//! Occurrence ids are the ids the occurrence tree (or, for a design annotated
//! per instance, the placed part) carries.

use std::collections::BTreeMap;

use ole_cfb::Cfb;

use super::bytes::decode_1252;
use super::doc::Folder;
use super::library::LibraryInfo;

const LIST_SEP: u8 = 0xF9;
const FIELD_SEP: u8 = 0xB0;
const VALUES_SEP: u8 = 0xC0;

/// One BOM variant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CisVariant {
    pub name: String,
    /// The part groups the variant selects.
    pub groups: Vec<String>,
    /// Occurrence ids the variant's BOM contains, when it stores them.
    pub bom_parts: Option<Vec<u32>>,
}

/// One part group.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CisGroup {
    pub name: String,
    /// (state, occurrence id): state `true` is stuffed in the group.
    pub members: Vec<(bool, u32)>,
    /// Occurrence id -> property overrides the group applies.
    pub updates: BTreeMap<u32, Vec<(String, String)>>,
}

#[derive(Debug, Clone, Default)]
pub struct Cis {
    pub variants: Vec<CisVariant>,
    pub groups: BTreeMap<String, CisGroup>,
}

/// A stream's text payload after its u32 length, when the length agrees.
fn payload(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 4 {
        return None;
    }
    let n = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    (n + 4 <= data.len()).then(|| &data[4..4 + n])
}

/// A counted list: decimal count, then that many items, all 0xF9-separated.
fn counted(data: &[u8]) -> Option<Vec<String>> {
    let p = payload(data)?;
    let mut items = p.split(|&b| b == LIST_SEP).map(decode_1252);
    let n: usize = items.next()?.trim().parse().ok()?;
    let v: Vec<String> = items.filter(|s| !s.is_empty()).collect();
    (v.len() == n).then_some(v)
}

fn memberships(data: &[u8]) -> Vec<(bool, u32)> {
    let Some(p) = payload(data) else { return Vec::new() };
    p.split(|&b| b == b'~')
        .filter_map(|r| {
            let mut f = r.split(|&b| b == FIELD_SEP);
            let state = f.next()?;
            let id: u32 = decode_1252(f.next()?).trim().parse().ok()?;
            Some((state == b"1", id))
        })
        .collect()
}

fn updates(data: &[u8]) -> BTreeMap<u32, Vec<(String, String)>> {
    let mut out = BTreeMap::new();
    let Some(p) = payload(data) else { return out };
    for r in p.split(|&b| b == b'~') {
        let mut f = r.splitn(2, |&b| b == FIELD_SEP);
        let Some(id) = f.next().and_then(|s| decode_1252(s).trim().parse::<u32>().ok()) else { continue };
        let Some(rest) = f.next() else { continue };
        let mut nv = rest.splitn(2, |&b| b == VALUES_SEP);
        let names: Vec<String> = decode_1252(nv.next().unwrap_or(&[])).split('^').map(str::to_string).collect();
        let values: Vec<String> = decode_1252(nv.next().unwrap_or(&[])).split('^').map(str::to_string).collect();
        if names.len() != values.len() || names.iter().any(|n| n.is_empty()) {
            continue;
        }
        out.entry(id).or_insert_with(Vec::new).extend(names.into_iter().zip(values));
    }
    out
}

pub fn read_cis(cfb: &Cfb, _lib: &LibraryInfo, _folders: &[Folder]) -> Option<Cis> {
    let base = "CIS/VariantStore";
    let mut cis = Cis::default();
    for name in cfb.stream(&format!("{base}/BOM/BOMDataStream")).and_then(counted).unwrap_or_default() {
        let groups = cfb.stream(&format!("{base}/BOM/{name}/{name}")).and_then(counted).unwrap_or_default();
        let bom_parts = cfb.stream(&format!("{base}/BOM/{name}/BOMPartData")).and_then(counted).map(|v| {
            v.iter().filter_map(|s| s.trim().parse().ok()).collect()
        });
        cis.variants.push(CisVariant { name, groups, bom_parts });
    }
    let prefix = format!("{base}/Groups/");
    let mut names: Vec<String> = cfb
        .paths()
        .filter_map(|p| p.strip_prefix(&prefix))
        .filter_map(|r| r.split_once('/').map(|(g, _)| g.to_string()))
        .collect();
    names.sort();
    names.dedup();
    for g in names {
        let mut group = CisGroup { name: g.clone(), ..Default::default() };
        if let Some(d) = cfb.stream(&format!("{prefix}{g}/{g}")) {
            group.members = memberships(d);
        }
        for p in cfb.paths().filter(|p| p.starts_with(&format!("{prefix}{g}/UpdateStorage"))) {
            if let Some(d) = cfb.stream(p) {
                for (id, props) in updates(d) {
                    group.updates.entry(id).or_default().extend(props);
                }
            }
        }
        cis.groups.insert(g, group);
    }
    (!cis.variants.is_empty() || !cis.groups.is_empty()).then_some(cis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(text: &[u8]) -> Vec<u8> {
        let mut v = (text.len() as u32).to_le_bytes().to_vec();
        v.extend_from_slice(text);
        v
    }

    #[test]
    fn counted_lists_check_their_count() {
        assert_eq!(counted(&stream(b"2\xF9Common\xF9DNM")), Some(vec!["Common".into(), "DNM".into()]));
        assert_eq!(counted(&stream(b"3\xF9Common\xF9DNM")), None);
    }

    #[test]
    fn memberships_and_updates_parse() {
        assert_eq!(memberships(&stream(b"0\xB020922~1\xB017")), vec![(false, 20922), (true, 17)]);
        let u = updates(&stream(b"17\xB0Value^Part Number\xC010k^RC0402"));
        assert_eq!(u[&17], vec![("Value".into(), "10k".into()), ("Part Number".into(), "RC0402".into())]);
    }
}
