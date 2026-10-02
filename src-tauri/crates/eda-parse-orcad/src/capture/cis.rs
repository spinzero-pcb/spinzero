//! Capture CIS variants (filled in by the variants milestone).

use ole_cfb::Cfb;

use super::doc::Folder;
use super::library::LibraryInfo;

#[derive(Debug, Clone, Default)]
pub struct Cis {}

pub fn read_cis(_cfb: &Cfb, _lib: &LibraryInfo, _folders: &[Folder]) -> Option<Cis> {
    None
}
