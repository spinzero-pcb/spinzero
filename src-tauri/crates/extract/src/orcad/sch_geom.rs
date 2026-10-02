//! Schematic geometry for the viewer's hit-testing.

use eda_parse_orcad::capture::CaptureDoc;

use super::SheetInstance;

pub fn build(_doc: &CaptureDoc, _sheets: &[SheetInstance]) -> serde_json::Value {
    serde_json::json!({})
}
