//! Parser and in-memory model for KiCad design files.
//!
//! This crate reads `.kicad_pro` (JSON), `.kicad_sch` and `.kicad_pcb`
//! (S-expressions) into faithful typed structures — a superset of what any one
//! consumer needs — so downstream emitters can project exactly the views they
//! want. It is a clean-room implementation driven by the on-disk file formats.

pub mod pcb;
pub mod schematic;
pub mod sexpr;

pub use pcb::Pcb;
pub use schematic::Schematic;
pub use sexpr::{parse as parse_sexpr, Node, ParseError};
