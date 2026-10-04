//! Running the rule pack, which is a separate program.
//!
//! `bom-rules` is not linked into this app. It is an executable we spawn, hand a CSV,
//! and read JSON back from. That is a deliberate boundary and it has outlived its
//! original reason twice over:
//!
//!   * it started as the GPL seam, keeping a closed server from linking a GPL crate —
//!     gone, the licences changed;
//!   * it stayed because the MCP server runs the same rules on the same BOM, and a
//!     free finding must carry the SAME fingerprint as the paid finding that refines
//!     it. Two copies of the rules drift; one program cannot.
//!
//! **Where the binary is**, in the order we look:
//!
//!   1. `SPINZERO_BOM_RULES_BIN` — an explicit path. What CI and a dev build use.
//!   2. A `bom-rules` sitting beside this executable. What the installer produces, and
//!      the only one that matters on a customer's machine.
//!   3. `cargo run -p bom-rules` inside `SPINZERO_BOM_RULES_REPO`. The source-checkout
//!      path, so `npm run tauri dev` works with nothing built or staged.
//!
//! The same three steps, in the same order, as the MCP server's own resolver. Two
//! programs looking for one binary should not disagree about where it lives.
//!
//! **A missing binary is not a crash.** It is a free tier that says it cannot run,
//! naming the variable to set. The app opens boards, renders them and takes review
//! comments without ever running a rule.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use crate::findings::{FindingsDoc, MappingPreview, MappingReport};

/// Columns the app fills from the extractor's own virtual fields rather than from a
/// BOM column. Passed to the rule pack so that a BOM column aliasing to the same
/// logical field is dropped instead of mapped a second time.
const AUTHORITATIVE: &str = "Reference,Quantity,DNP";

/// How we invoke the rule pack: a program plus the arguments that must come first.
struct Runner {
    program: PathBuf,
    leading: Vec<String>,
}

impl Runner {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.leading);
        cmd
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "bom-rules.exe"
    } else {
        "bom-rules"
    }
}

/// Find the rule pack, or say what to set.
fn runner() -> Result<Runner, String> {
    if let Some(explicit) = std::env::var("SPINZERO_BOM_RULES_BIN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        let path = PathBuf::from(&explicit);
        if !path.is_file() {
            return Err(format!(
                "SPINZERO_BOM_RULES_BIN points at a missing file: {explicit}"
            ));
        }
        return Ok(Runner { program: path, leading: Vec::new() });
    }

    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(exe_name())))
        .filter(|p| p.is_file())
    {
        return Ok(Runner { program: sibling, leading: Vec::new() });
    }

    // Source checkout. Slow (cargo re-resolves the workspace every call) and that is
    // fine: nobody ships this path, and a developer who wants it fast sets
    // SPINZERO_BOM_RULES_BIN to their own target/release build.
    if let Some(repo) = std::env::var("SPINZERO_BOM_RULES_REPO")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        let manifest = PathBuf::from(&repo).join("Cargo.toml");
        if !manifest.is_file() {
            return Err(format!("SPINZERO_BOM_RULES_REPO has no Cargo.toml: {repo}"));
        }
        return Ok(Runner {
            program: PathBuf::from("cargo"),
            leading: vec![
                "run".into(),
                "--quiet".into(),
                "--release".into(),
                "--manifest-path".into(),
                manifest.to_string_lossy().into_owned(),
                "-p".into(),
                "bom-rules".into(),
                "--bin".into(),
                "bom-rules".into(),
                "--".into(),
            ],
        });
    }

    Err(format!(
        "the BOM rule pack is not installed: no {} beside this app, and neither \
         SPINZERO_BOM_RULES_BIN nor SPINZERO_BOM_RULES_REPO is set",
        exe_name()
    ))
}

/// A scratch directory for one invocation, removed when it drops.
///
/// Under the OS temp dir, never the project folder: these files are regenerable and
/// the project folder syncs to git, SharePoint or OneDrive. Removal is best-effort on
/// purpose — a leaked temp file is a tidiness problem, and failing a BOM check over
/// one would be a correctness problem.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("spinzero_rules_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create a scratch directory for the rule pack: {e}"))?;
        Ok(Scratch(dir))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write the inputs every invocation shares: the BOM, and the approved mapping.
fn write_inputs(
    scratch: &Scratch,
    csv: &str,
    overrides: &BTreeMap<String, String>,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let bom = scratch.join("bom.csv");
    std::fs::write(&bom, csv).map_err(|e| format!("cannot stage the BOM for the rule pack: {e}"))?;
    if overrides.is_empty() {
        return Ok((bom, None));
    }
    let map = scratch.join("overrides.json");
    let json = serde_json::to_string(overrides).map_err(|e| e.to_string())?;
    std::fs::write(&map, json)
        .map_err(|e| format!("cannot stage the column mapping for the rule pack: {e}"))?;
    Ok((bom, Some(map)))
}

/// Run one invocation and read a JSON file back.
fn invoke<T: serde::de::DeserializeOwned>(
    mut cmd: Command,
    produced: &PathBuf,
    what: &str,
) -> Result<T, String> {
    let output = cmd
        .output()
        .map_err(|e| format!("could not start the BOM rule pack: {e}"))?;
    if !output.status.success() {
        // The pack writes its diagnosis to stderr. Last line, because the first is
        // usually usage text and the last is the reason.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.trim().lines().last().unwrap_or("no output").to_string();
        return Err(format!("the BOM rule pack failed: {reason}"));
    }
    let text = std::fs::read_to_string(produced)
        .map_err(|e| format!("the BOM rule pack produced no {what}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("the {what} could not be read: {e}"))
}

/// Run the deterministic checks over a BOM, and report which column fed which field.
///
/// `overrides` is the user's approved column mapping (`project::BomMapping`). It is
/// applied here rather than left to the aliases because a mis-mapped column is
/// indistinguishable from missing data in every finding downstream.
pub fn run(
    csv: &str,
    profile: &str,
    overrides: &BTreeMap<String, String>,
) -> Result<(FindingsDoc, MappingReport), String> {
    let runner = runner()?;
    let scratch = Scratch::new()?;
    let (bom, map) = write_inputs(&scratch, csv, overrides)?;
    let findings_path = scratch.join("findings.json");
    let mapping_path = scratch.join("mapping.json");

    let mut cmd = runner.command();
    cmd.arg("--bom")
        .arg(&bom)
        .arg("--profile")
        .arg(profile)
        .arg("--authoritative")
        .arg(AUTHORITATIVE)
        .arg("--out")
        .arg(&findings_path)
        .arg("--mapping")
        .arg(&mapping_path);
    if let Some(map) = &map {
        cmd.arg("--overrides").arg(map);
    }

    let doc: FindingsDoc = invoke(cmd, &findings_path, "findings document")?;
    // The mapping report is read separately and its absence is not fatal: a report we
    // cannot read costs the "which column did we use" note, while the findings the
    // user actually asked for are already in hand.
    let mapping: MappingReport = std::fs::read_to_string(&mapping_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    Ok((doc, mapping))
}

/// The approval dialog's view of the mapping, for the same BOM.
pub fn mapping(
    csv: &str,
    profile: &str,
    overrides: &BTreeMap<String, String>,
) -> Result<MappingPreview, String> {
    let runner = runner()?;
    let scratch = Scratch::new()?;
    let (bom, map) = write_inputs(&scratch, csv, overrides)?;
    let out = scratch.join("preview.json");

    let mut cmd = runner.command();
    cmd.arg("mapping")
        .arg("--bom")
        .arg(&bom)
        .arg("--profile")
        .arg(profile)
        .arg("--authoritative")
        .arg(AUTHORITATIVE)
        .arg("--out")
        .arg(&out);
    if let Some(map) = &map {
        cmd.arg("--overrides").arg(map);
    }

    invoke(cmd, &out, "column mapping")
}
