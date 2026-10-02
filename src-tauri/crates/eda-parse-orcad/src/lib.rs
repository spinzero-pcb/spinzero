//! Read-only reader for Cadence OrCAD design files.
//!
//! Two unrelated formats live here: OrCAD Capture's compound-file schematics
//! (`.DSN`, `.OLB`) under [`capture`], and the OrCAD / Allegro PCB Editor board
//! database (`.brd`) under [`allegro`]. Like the Altium reader, this crate knows
//! nothing about the review bundle — the projection lives in `extract`.

pub mod allegro;
pub mod capture;
