//! The Capture project file (`.OPJ`): a parenthesised text tree listing the
//! design, its libraries, and Capture's per-project settings.
//!
//! Only what the extraction needs is kept: the schematic design files, the
//! libraries, and the board file the project's netlist step writes to.

use std::path::{Path, PathBuf};

/// One `(File "<path>" (Type "<type>"))` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// As written: often relative with Windows separators (`.\design.dsn`).
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub files: Vec<ProjectFile>,
    /// Every quoted `("Key" "Value")` setting, in file order.
    pub settings: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Open,
    Close,
    Atom(String),
}

fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '(' => out.push(Tok::Open),
            ')' => out.push(Tok::Close),
            '"' => {
                // Capture writes Windows paths with bare backslashes and never
                // escapes a quote inside a value, so a quote always ends it.
                let mut v = String::new();
                for d in it.by_ref() {
                    if d == '"' {
                        break;
                    }
                    v.push(d);
                }
                out.push(Tok::Atom(v));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut v = String::from(c);
                while let Some(&d) = it.peek() {
                    if d.is_whitespace() || d == '(' || d == ')' || d == '"' {
                        break;
                    }
                    v.push(d);
                    it.next();
                }
                out.push(Tok::Atom(v));
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
enum Node {
    Atom(String),
    List(Vec<Node>),
}

fn parse_nodes(toks: &[Tok], i: &mut usize) -> Vec<Node> {
    let mut out = Vec::new();
    while *i < toks.len() {
        match &toks[*i] {
            Tok::Open => {
                *i += 1;
                out.push(Node::List(parse_nodes(toks, i)));
            }
            Tok::Close => {
                *i += 1;
                return out;
            }
            Tok::Atom(a) => {
                *i += 1;
                out.push(Node::Atom(a.clone()));
            }
        }
    }
    out
}

fn atom(n: &Node) -> Option<&str> {
    match n {
        Node::Atom(a) => Some(a),
        _ => None,
    }
}

fn walk(nodes: &[Node], p: &mut Project) {
    for n in nodes {
        let Node::List(items) = n else { continue };
        let head = items.first().and_then(atom).unwrap_or("");
        match head {
            "ExpressProject" => {
                p.name = items.get(1).and_then(atom).unwrap_or("").to_string();
                walk(&items[1..], p);
            }
            "File" => {
                let path = items.get(1).and_then(atom).unwrap_or("").to_string();
                let kind = items[1..]
                    .iter()
                    .find_map(|x| match x {
                        Node::List(l) if l.first().and_then(atom) == Some("Type") => {
                            l.get(1).and_then(atom).map(str::to_string)
                        }
                        _ => None,
                    })
                    .unwrap_or_default();
                p.files.push(ProjectFile { path, kind });
            }
            "Folder" => walk(&items[1..], p),
            _ => {
                if items.len() == 2 {
                    if let (Some(k), Some(v)) = (atom(&items[0]), atom(&items[1])) {
                        p.settings.push((k.to_string(), v.to_string()));
                        continue;
                    }
                }
                walk(&items[1..], p);
            }
        }
    }
}

impl Project {
    pub fn parse(text: &str) -> Project {
        let toks = tokenize(text);
        let mut i = 0;
        let nodes = parse_nodes(&toks, &mut i);
        let mut p = Project::default();
        walk(&nodes, &mut p);
        p
    }

    pub fn open(path: &Path) -> Result<Project, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Project::parse(&super::bytes::decode_1252(&bytes)))
    }

    pub fn setting(&self, key: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// The schematic design files the project names, in order.
    pub fn designs(&self) -> impl Iterator<Item = &ProjectFile> {
        self.files.iter().filter(|f| f.kind.eq_ignore_ascii_case("Schematic Design"))
    }

    /// Resolve a path the project wrote (relative to the project's own folder,
    /// Windows separators, any case) against the file system.
    pub fn resolve(project_dir: &Path, written: &str) -> Option<PathBuf> {
        let rel = written.replace('\\', "/");
        let rel = rel.trim_start_matches("./");
        let direct = project_dir.join(rel);
        if direct.exists() {
            return Some(direct);
        }
        // Case-insensitive, component by component: Capture is a Windows tool
        // and projects move to case-sensitive file systems.
        let mut cur = project_dir.to_path_buf();
        for part in rel.split('/').filter(|s| !s.is_empty() && *s != ".") {
            if part == ".." {
                cur = cur.parent()?.to_path_buf();
                continue;
            }
            let found = std::fs::read_dir(&cur).ok()?.flatten().find(|e| {
                e.file_name().to_string_lossy().eq_ignore_ascii_case(part)
            })?;
            cur = found.path();
        }
        cur.exists().then_some(cur)
    }

    /// The board the project's netlist step writes to, as a bare file name.
    /// The setting is an absolute path on the designer's machine, so only its
    /// last component is meaningful here.
    pub fn board_name(&self) -> Option<String> {
        let v = self.setting("Allegro Netlist Output Board File")?;
        let name = v.rsplit(['\\', '/']).next()?.trim();
        (!name.is_empty()).then(|| name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPJ: &str = r#"(ExpressProject "project1"
  (ProjectVersion "19981106")
  (Folder "Design Resources"
    (Folder "Library"
      (File ".\lib\library1.olb"
        (Type "Schematic Library")))
    (File ".\project1.dsn"
      (Type "Schematic Design"))
    ("Allegro Netlist Output Board File"
       "C:\Users\x\PCBdesign\mainFinal.brd")
    (DRC_Scope "0")))"#;

    #[test]
    fn reads_designs_libraries_and_the_board() {
        let p = Project::parse(OPJ);
        assert_eq!(p.name, "project1");
        let d: Vec<_> = p.designs().map(|f| f.path.as_str()).collect();
        assert_eq!(d, vec![".\\project1.dsn"]);
        assert_eq!(p.files.len(), 2);
        assert_eq!(p.board_name().as_deref(), Some("mainFinal.brd"));
        assert_eq!(p.setting("DRC_Scope"), Some("0"));
    }
}
