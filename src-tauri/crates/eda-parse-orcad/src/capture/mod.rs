//! OrCAD Capture (`.DSN` designs, `.OLB` libraries).

pub mod bytes;
pub mod cache;
pub mod cis;
pub mod doc;
pub mod framing;
pub mod hierarchy;
pub mod legacy;
pub mod library;
pub mod page;
pub mod symbol;

pub use doc::{is_capture, open, parse, CaptureDoc, Folder};
