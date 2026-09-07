//! Altium front-end for the extraction pipeline.
//!
//! Everything here builds the *same* public structs the KiCad path builds
//! (`design::Component`, `netlist::Frag`, `ir::Geometry`), so the two sources
//! cannot drift on schema: a schema change breaks both builders at once.

pub mod dump;
