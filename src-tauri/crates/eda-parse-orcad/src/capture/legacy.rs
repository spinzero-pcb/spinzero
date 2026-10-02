//! The pre-2003 (Library version 1.x/2.x) grammar.

use ole_cfb::Cfb;

use super::doc::CaptureDoc;
use super::library::LibraryInfo;

pub fn parse(_cfb: &Cfb, lib: LibraryInfo) -> Result<CaptureDoc, String> {
    Err(format!(
        "OrCAD Capture file version {}.{} (pre-2003) is not supported yet",
        lib.version.0, lib.version.1
    ))
}
