//! Read-only reader for Altium Designer design files.
//!
//! Scope is deliberately narrow: containers, record framing, decoded records and
//! unit conversion. It knows nothing about the review bundle — the projection
//! into the bundle lives in the `extract` crate, so both design sources share
//! one set of output structs.

pub mod cfb;
pub mod doc;
pub mod image;
pub mod pcb;
pub mod prj;
pub mod record;
pub mod sch;
pub mod units;

#[cfg(test)]
mod test_support;

pub use doc::{Doc, Kind};
pub use pcb::PcbDoc;
pub use prj::{CompileOptions, HierarchyMode, Project};
pub use record::TextRecord;
pub use sch::SchDoc;
