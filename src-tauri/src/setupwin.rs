//! The review setup window: `SpinZero --setup <dir>`.
//!
//! A review started from an MCP client (Claude Code, for one) stops before it runs and
//! asks two things: the board's end application, and which BOM column holds which
//! field. The review server writes that question to `<dir>/request.json` and starts
//! this app with `--setup <dir>`. The app then opens one small window and nothing else.
//! When the user presses Confirm, it writes `<dir>/answer.json` and exits.
//!
//! The file shapes are `schemas/mcp-setup-1.0.json`. That schema is the whole contract:
//! this app does not know which server wrote the request.
//!
//! A review started from the app never comes here. The app asks both questions in its
//! own review setup sheet before the review starts.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub const SCHEMA: &str = "mcp-setup-1.0";
const REQUEST: &str = "request.json";
const ANSWER: &str = "answer.json";

/// The setup directory, when this process was started as the setup window.
/// Managed as Tauri state so the commands can reach it.
pub struct SetupMode(pub Option<PathBuf>);

/// The value after `--setup`, if there is one.
pub fn dir_from_args<I: IntoIterator<Item = String>>(args: I) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--setup" {
            return it.next().filter(|d| !d.trim().is_empty()).map(PathBuf::from);
        }
    }
    None
}

/// Read the question. Returned as JSON, untyped: the frontend validates the fields it
/// renders, and a newer server adding a field must not break an older app.
pub fn read_request(dir: &Path) -> Result<serde_json::Value, String> {
    let path = dir.join(REQUEST);
    let text = fs::read_to_string(&path).map_err(|e| format!("could not read the setup request: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("the setup request is not valid JSON: {e}"))?;
    if v.get("schema_version").and_then(|s| s.as_str()) != Some(SCHEMA) {
        return Err("the setup request is in a format this version of SpinZero does not read".into());
    }
    Ok(v)
}

#[derive(Serialize)]
struct Answer<'a> {
    schema_version: &'a str,
    review_id: &'a str,
    profile: Option<&'a str>,
    mapping: &'a BTreeMap<String, String>,
    confirmed_ts: String,
}

/// Write the answer. The review id is copied from the request, so an answer can never
/// be read as another review's. Written to a temporary name and renamed, so the server
/// never reads half a file.
pub fn write_answer(
    dir: &Path,
    profile: Option<&str>,
    mapping: &BTreeMap<String, String>,
) -> Result<(), String> {
    let request = read_request(dir)?;
    let review_id = request
        .get("review_id")
        .and_then(|s| s.as_str())
        .ok_or("the setup request has no review id")?;
    let answer = Answer {
        schema_version: SCHEMA,
        review_id,
        profile: profile.map(str::trim).filter(|p| !p.is_empty()),
        mapping,
        confirmed_ts: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| e.to_string())?,
    };
    let text = serde_json::to_string_pretty(&answer).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{ANSWER}.tmp"));
    fs::write(&tmp, text).map_err(|e| format!("could not save the answer: {e}"))?;
    fs::rename(&tmp, dir.join(ANSWER)).map_err(|e| format!("could not save the answer: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory per test, under the system temp dir.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spinzero_setupwin_{}_{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(dir: &Path) {
        fs::write(
            dir.join(REQUEST),
            r#"{"schema_version":"mcp-setup-1.0","review_id":"r-1","board":"B","row_count":1,
                "profile":{"value":null,"options":[]},"fields":[],"columns":[],"attention":[]}"#,
        )
        .unwrap();
    }

    #[test]
    fn finds_the_setup_dir_after_the_flag() {
        let args = ["SpinZero.exe", "--setup", "C:\\runs\\x\\setup"].map(String::from);
        assert_eq!(dir_from_args(args), Some(PathBuf::from("C:\\runs\\x\\setup")));
        assert_eq!(dir_from_args(["SpinZero.exe".to_string()]), None);
        assert_eq!(dir_from_args(["SpinZero.exe", "--setup"].map(String::from)), None);
    }

    #[test]
    fn writes_the_answer_under_the_requests_review_id() {
        let dir = scratch("answer");
        request(dir.as_path());
        let mapping = BTreeMap::from([("mpn".to_string(), "Mfr Part #".to_string())]);
        write_answer(dir.as_path(), Some("automotive-safety"), &mapping).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.as_path().join(ANSWER)).unwrap()).unwrap();
        assert_eq!(v["review_id"], "r-1");
        assert_eq!(v["schema_version"], SCHEMA);
        assert_eq!(v["profile"], "automotive-safety");
        assert_eq!(v["mapping"]["mpn"], "Mfr Part #");
        assert!(!dir.as_path().join("answer.json.tmp").exists());
    }

    #[test]
    fn an_empty_profile_is_recorded_as_not_stated() {
        let dir = scratch("empty");
        request(dir.as_path());
        write_answer(dir.as_path(), Some("  "), &BTreeMap::new()).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.as_path().join(ANSWER)).unwrap()).unwrap();
        assert!(v["profile"].is_null());
    }

    #[test]
    fn refuses_a_request_in_another_format() {
        let dir = scratch("format");
        fs::write(dir.as_path().join(REQUEST), r#"{"schema_version":"mcp-setup-9.0"}"#).unwrap();
        assert!(read_request(dir.as_path()).is_err());
        assert!(write_answer(dir.as_path(), None, &BTreeMap::new()).is_err());
    }
}
