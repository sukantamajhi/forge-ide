//! Rope-backed text buffer with multi-cursor editing and undo/redo.
//!
//! All offsets are `char` indices into the rope.

mod buffer;
pub mod brackets;
pub mod columns;
mod editing;
pub mod search;
mod selection;
pub mod words;

pub use buffer::{Buffer, TextEdit};
pub use editing::{map_through, IndentStyle, PAIRS};
pub use selection::map_offset_through_edits;
pub use selection::{Selection, SelectionSet};
