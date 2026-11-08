//! Pure-Rust data engine for RNotepad.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod bytes;
pub mod coord;
pub mod document;
pub mod encoding;
pub mod eol;
pub mod format;
pub mod line_index;
pub mod search;
