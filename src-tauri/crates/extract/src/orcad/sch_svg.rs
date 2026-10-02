//! Capture pages rendered as sheet SVGs.

use std::collections::BTreeMap;

use eda_parse_orcad::capture::CaptureDoc;

use super::SheetInstance;
use crate::design::Design;

/// Capture's own schematic colours.
pub fn palette() -> BTreeMap<String, String> {
    BTreeMap::new()
}

pub fn render_sheet(_doc: &CaptureDoc, _s: &SheetInstance, _total: i64, _model: &Design) -> String {
    String::from(r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#)
}
