//! Turns a parsed KiCad design into the review bundle the viewer app and the AI
//! review skills consume: a structured design model (`*_design.json`), a
//! manifest, lean enriched SVGs, a BOM, and 3D placement data.
//!
//! The crate is consumed two ways from the same code: in-process by the Tauri
//! app (replacing the external importer subprocess) and via the `pcb-extract`
//! binary used by the review skills.

pub mod bom;
pub mod design;
pub mod geom;
pub mod ir;
pub mod netclass;
pub mod netlist;
pub mod pcb;
pub mod pipeline;
pub mod sch_geom;
pub mod svg;
pub mod theme;

/// Schema id stamped into the emitted design model. Deliberately our own
/// namespace — not the identifier used by the tool this replaces.
pub const DESIGN_SCHEMA: &str = "extract.design.a0";

/// Schema id stamped into the bundle manifest. Keeps the `design_review_manifest`
/// substring the app's manifest check looks for, under our own namespace.
pub const MANIFEST_SCHEMA: &str = "extract.design_review_manifest.a0";

/// Value written to the design model's `generator` field.
pub const GENERATOR: &str = "extract";

/// Version string reported by `pcb-extract --version`.
pub fn version() -> String {
    format!("{} {}", GENERATOR, env!("CARGO_PKG_VERSION"))
}

/// Bump on ANY change to extractor output (parser, netlister, SVG renderer, BOM,
/// schema) so the app's runtime extraction cache auto-invalidates and every open
/// re-extracts. `CARGO_PKG_VERSION` does NOT move on a logic-only edit, so this
/// explicit epoch is what makes "update the extractor later" safe — it is folded
/// into the cache key alongside the source-file hashes.
pub const EXTRACTOR_CACHE_EPOCH: u32 = 23;

/// Cache-key component identifying the extractor's output contract: the epoch plus
/// the crate version. The app combines this with the design's source hashes to key
/// the runtime extraction cache (see `cache::cache_key`).
pub fn cache_version() -> String {
    format!("{}.{}", EXTRACTOR_CACHE_EPOCH, env!("CARGO_PKG_VERSION"))
}
