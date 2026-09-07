//! `.PrjPcb` — plain INI text with a UTF-8 BOM.
//!
//! Two things matter here: the document list, and the `[Design]` compile options.
//! Those options are the net identifier scope, so the same schematic compiles to
//! a different netlist under different settings — they are not cosmetic.

use std::collections::BTreeMap;
use std::path::Path;

/// Net identifier scope (`HierarchyMode` in `[Design]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchyMode {
    Automatic,
    Flat,
    Hierarchical,
    Global,
    StrictHierarchical,
}

impl HierarchyMode {
    fn from_i(v: i64) -> HierarchyMode {
        match v {
            1 => HierarchyMode::Flat,
            2 => HierarchyMode::Hierarchical,
            3 => HierarchyMode::Global,
            4 => HierarchyMode::StrictHierarchical,
            _ => HierarchyMode::Automatic,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HierarchyMode::Automatic => "automatic",
            HierarchyMode::Flat => "flat",
            HierarchyMode::Hierarchical => "hierarchical",
            HierarchyMode::Global => "global",
            HierarchyMode::StrictHierarchical => "strict_hierarchical",
        }
    }
}

/// The compile options that change net naming, with Altium's own defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileOptions {
    pub hierarchy_mode: HierarchyMode,
    pub allow_port_net_names: bool,
    pub allow_sheet_entry_net_names: bool,
    pub netlist_single_pin_nets: bool,
    pub append_sheet_number_to_local_nets: bool,
    pub power_port_names_take_priority: bool,
    pub name_nets_hierarchically: bool,
    pub channel_designator_format: String,
    pub channel_room_level_separator: String,
    pub default_configuration: String,
}

impl CompileOptions {
    /// Defaults for a board project (a `.PrjPcb` exists but says nothing).
    pub fn board_project() -> CompileOptions {
        CompileOptions {
            hierarchy_mode: HierarchyMode::Automatic,
            allow_port_net_names: false,
            allow_sheet_entry_net_names: true,
            netlist_single_pin_nets: false,
            append_sheet_number_to_local_nets: false,
            power_port_names_take_priority: false,
            name_nets_hierarchically: false,
            channel_designator_format: "$Component_$ChannelIndex".to_string(),
            channel_room_level_separator: "_".to_string(),
            default_configuration: String::new(),
        }
    }

    /// Defaults for a loose document with no project file. The scope is
    /// **Global**, not the board-project default — getting this backwards
    /// changes the netlist of every project-less design.
    pub fn free_document() -> CompileOptions {
        CompileOptions {
            hierarchy_mode: HierarchyMode::Global,
            ..CompileOptions::board_project()
        }
    }
}

/// A parsed `.PrjPcb`.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    /// `DocumentPath=` of every `[DocumentN]` section, in file order.
    pub documents: Vec<String>,
    pub options: CompileOptions,
    /// Every section, for anything read later (variants, configurations).
    pub sections: Vec<(String, BTreeMap<String, String>)>,
}

impl Project {
    pub fn open(path: &Path) -> Result<Project, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();
        Ok(Project::parse(&name, &bytes))
    }

    pub fn parse(name: &str, bytes: &[u8]) -> Project {
        let text = strip_bom(bytes);
        let sections = parse_ini(&text);
        let mut documents = Vec::new();
        for (sec, kv) in &sections {
            if sec.to_ascii_uppercase().starts_with("DOCUMENT") {
                if let Some(p) = kv.get("DOCUMENTPATH").filter(|s| !s.is_empty()) {
                    documents.push(p.clone());
                }
            }
        }
        let design: BTreeMap<String, String> = sections
            .iter()
            .find(|(s, _)| s.eq_ignore_ascii_case("Design"))
            .map(|(_, kv)| kv.clone())
            .unwrap_or_default();

        let mut options = CompileOptions::board_project();
        let get = |k: &str| design.get(k).map(String::as_str);
        if let Some(v) = get("HIERARCHYMODE").and_then(|v| v.trim().parse::<i64>().ok()) {
            options.hierarchy_mode = HierarchyMode::from_i(v);
        }
        let flag = |k: &str, cur: bool| get(k).map(ini_bool).unwrap_or(cur);
        options.allow_port_net_names = flag("ALLOWPORTNETNAMES", options.allow_port_net_names);
        options.allow_sheet_entry_net_names =
            flag("ALLOWSHEETENTRYNETNAMES", options.allow_sheet_entry_net_names);
        options.netlist_single_pin_nets =
            flag("NETLISTSINGLEPINNETS", options.netlist_single_pin_nets);
        options.append_sheet_number_to_local_nets =
            flag("APPENDSHEETNUMBERTOLOCALNETS", options.append_sheet_number_to_local_nets);
        options.power_port_names_take_priority =
            flag("POWERPORTNAMESTAKEPRIORITY", options.power_port_names_take_priority);
        options.name_nets_hierarchically =
            flag("NAMENETSHIERARCHICALLY", options.name_nets_hierarchically);
        if let Some(v) = get("CHANNELDESIGNATORFORMATSTRING").filter(|s| !s.is_empty()) {
            options.channel_designator_format = v.to_string();
        }
        if let Some(v) = get("CHANNELROOMLEVELSEPERATOR").or_else(|| get("CHANNELROOMLEVELSEPARATOR"))
        {
            options.channel_room_level_separator = v.to_string();
        }
        if let Some(v) = get("DEFAULTCONFIGURATION") {
            options.default_configuration = v.to_string();
        }

        Project {
            name: name.to_string(),
            documents,
            options,
            sections,
        }
    }

    /// Document paths whose extension matches, resolved against the project's
    /// directory. Altium writes Windows-relative paths; both separators are
    /// normalised so the corpus reads on any host.
    pub fn documents_with_ext(&self, dir: &Path, ext: &str) -> Vec<std::path::PathBuf> {
        self.documents
            .iter()
            .filter(|d| {
                std::path::Path::new(d.as_str())
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case(ext))
                    .unwrap_or(false)
            })
            .map(|d| dir.join(d.replace('\\', "/")))
            .collect()
    }

    /// Variant names defined by the project (`[ProjectVariant…] Description=`).
    /// Until variants are applied, this is what the diagnostics block reports.
    pub fn variant_names(&self) -> Vec<String> {
        self.sections
            .iter()
            .filter(|(s, _)| s.to_ascii_uppercase().starts_with("PROJECTVARIANT"))
            .filter_map(|(_, kv)| kv.get("DESCRIPTION").cloned())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// `TRUE`/`FALSE`, `1`/`0` and `T`/`F` all occur in project files.
fn ini_bool(v: &str) -> bool {
    matches!(v.trim().to_ascii_uppercase().as_str(), "1" | "T" | "TRUE" | "Y" | "YES")
}

fn strip_bom(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    crate::record::decode(body)
}

/// Sections in file order; keys upper-cased for lookup, values left alone.
fn parse_ini(text: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut out: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    let mut cur = String::new();
    let mut kv: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if !cur.is_empty() || !kv.is_empty() {
                out.push((std::mem::take(&mut cur), std::mem::take(&mut kv)));
            }
            cur = name.to_string();
        } else if let Some(eq) = line.find('=') {
            kv.insert(line[..eq].trim().to_ascii_uppercase(), line[eq + 1..].trim().to_string());
        }
    }
    if !cur.is_empty() || !kv.is_empty() {
        out.push((cur, kv));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRJ: &str = "\u{feff}[Design]\r\nHierarchyMode=1\r\nAllowPortNetNames=TRUE\r\n\
AllowSheetEntryNetNames=0\r\nPowerPortNamesTakePriority=1\r\n\
[Document1]\r\nDocumentPath=Top_Level.SchDoc\r\n\
[Document2]\r\nDocumentPath=board\\Main.PcbDoc\r\n";

    #[test]
    fn reads_documents_and_compile_options() {
        let p = Project::parse("demo", PRJ.as_bytes());
        assert_eq!(p.documents, vec!["Top_Level.SchDoc", "board\\Main.PcbDoc"]);
        assert_eq!(p.options.hierarchy_mode, HierarchyMode::Flat);
        assert!(p.options.allow_port_net_names, "TRUE and 1 both mean true");
        assert!(!p.options.allow_sheet_entry_net_names, "0 overrides the true default");
        assert!(p.options.power_port_names_take_priority);
        let dir = Path::new("/tmp/proj");
        assert_eq!(
            p.documents_with_ext(dir, "PcbDoc"),
            vec![Path::new("/tmp/proj/board/Main.PcbDoc").to_path_buf()]
        );
    }

    /// Corner case 12: a loose document is compiled with Global scope, not the
    /// board-project default.
    #[test]
    fn free_document_defaults_differ_from_board_project() {
        assert_eq!(CompileOptions::board_project().hierarchy_mode, HierarchyMode::Automatic);
        assert_eq!(CompileOptions::free_document().hierarchy_mode, HierarchyMode::Global);
        assert!(CompileOptions::board_project().allow_sheet_entry_net_names);
        assert!(!CompileOptions::board_project().allow_port_net_names);
    }
}
