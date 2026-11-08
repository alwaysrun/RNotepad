//! Status bar field formatting.
//!
//! The formatting is pure so it can be unit-tested without a window.  The
//! application maps each field to a status bar part and handles double clicks
//! on the encoding and EOL fields.

use rnotepad_core::document::{Document, DocumentKind, ViewMode};

/// Number of status bar parts defined by the specification.
pub const FIELD_COUNT: usize = 8;
/// Index of the view-mode field.
pub const FIELD_VIEW: usize = 0;
/// Index of the encoding field (double click opens the encoding menu).
pub const FIELD_ENCODING: usize = 1;
/// Index of the EOL field (double click opens the EOL menu).
pub const FIELD_EOL: usize = 2;
/// Index of the size field.
pub const FIELD_SIZE: usize = 3;
/// Index of the caret/selection field.
pub const FIELD_POSITION: usize = 4;
/// Index of the flags field.
pub const FIELD_FLAGS: usize = 5;
/// Index of the byte offset field.
pub const FIELD_OFFSET: usize = 6;
/// Index of the progress field.
pub const FIELD_PROGRESS: usize = 7;

const BOM_MARKER: &str = " · BOM";
/// Marks a field that opens a menu on double click.
const ACTION_MARKER: &str = " ▾";
const FLAG_READ_ONLY: &str = "RO";
const FLAG_EDITING: &str = "EDIT";
const FLAG_DIRTY: &str = "*";
const FLAG_STALE_HEX: &str = "磁盘版本（有未保存修改）";
const MIXED_PREFIX: &str = "Mixed";
const CONFIDENCE_THRESHOLD: f32 = 0.90;
const PERCENT_BASE: f32 = 100.0;
const SIZE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const SIZE_STEP: f64 = 1024.0;
const LARGE_SIZE_DECIMALS: usize = 2;
const BYTE_GROUP: u64 = 1000;

/// Inputs for the status bar fields.
pub struct StatusInfo<'a> {
    pub doc: &'a Document,
    pub view: ViewMode,
    pub dirty: bool,
    /// One-based line and column of the caret or selection anchor.
    pub position: Option<(u32, u32)>,
    /// Number of selected bytes, when the selection is not empty.
    pub selection_bytes: Option<u64>,
    /// Current raw byte offset (hex view or split mode).
    pub offset: Option<u64>,
    /// Background task progress text.
    pub progress: Option<String>,
}

/// Formats every status field.
#[must_use]
pub fn fields(info: &StatusInfo<'_>) -> [String; FIELD_COUNT] {
    [
        view_field(info.view),
        encoding_field(info),
        eol_field(info),
        format_size(info.doc.size()),
        position_field(info),
        flags_field(info),
        offset_field(info),
        info.progress.clone().unwrap_or_default(),
    ]
}

fn view_field(view: ViewMode) -> String {
    match view {
        ViewMode::Text => "TEXT".to_owned(),
        ViewMode::Hex => "HEX".to_owned(),
    }
}

fn encoding_field(info: &StatusInfo<'_>) -> String {
    let encoding = info.doc.encoding;
    let mut text = encoding.charset.display_name().to_owned();
    if encoding.bom != rnotepad_core::encoding::BomState::None {
        text.push_str(BOM_MARKER);
    }
    if info.doc.confidence < CONFIDENCE_THRESHOLD {
        text.push_str(&format!(
            " ({:.0}%)",
            f32::from(info.doc.confidence) * PERCENT_BASE
        ));
    }
    text.push_str(ACTION_MARKER);
    text
}

fn eol_field(info: &StatusInfo<'_>) -> String {
    let eol = info.doc.eol;
    let mut text = if eol.mixed {
        format!("{MIXED_PREFIX} {}:{}", eol.crlf, eol.lf)
    } else {
        eol.dominant.display_name().to_owned()
    };
    text.push_str(ACTION_MARKER);
    text
}

fn position_field(info: &StatusInfo<'_>) -> String {
    if let Some(bytes) = info.selection_bytes {
        if bytes > 0 {
            return format!("Sel {bytes} B");
        }
    }
    match info.position {
        Some((line, column)) => format!("Ln {line}, Col {column}"),
        None => String::new(),
    }
}

fn flags_field(info: &StatusInfo<'_>) -> String {
    let mut flags = String::new();
    match info.doc.kind {
        DocumentKind::LargeReadOnly => flags.push_str(FLAG_READ_ONLY),
        DocumentKind::Editable => {
            flags.push_str(FLAG_EDITING);
            if info.dirty {
                flags.push(' ');
                flags.push_str(FLAG_DIRTY);
            }
        }
    }
    if info.doc.kind.is_editable() && info.dirty && info.view == ViewMode::Hex {
        if !flags.is_empty() {
            flags.push_str(" · ");
        }
        flags.push_str(FLAG_STALE_HEX);
    }
    flags
}

fn offset_field(info: &StatusInfo<'_>) -> String {
    info.offset
        .map(|offset| format!("0x{offset:08X}"))
        .unwrap_or_default()
}

/// Formats a byte count for the size field.
#[must_use]
pub fn format_size(bytes: u64) -> String {
    if bytes < BYTE_GROUP {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= SIZE_STEP && unit + 1 < SIZE_UNITS.len() {
        value /= SIZE_STEP;
        unit += 1;
    }
    format!(
        "{value:.decimals$} {}",
        SIZE_UNITS[unit],
        decimals = LARGE_SIZE_DECIMALS
    )
    .replace('.', ".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rnotepad_core::document::Document;

    fn sample() -> Document {
        Document::from_bytes(1, b"hello\nworld".to_vec())
    }

    #[test]
    fn size_units_switch_at_kibibyte() {
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
    }

    #[test]
    fn fields_have_fixed_arity() {
        let doc = sample();
        let info = StatusInfo {
            doc: &doc,
            view: ViewMode::Text,
            dirty: false,
            position: Some((1, 5)),
            selection_bytes: None,
            offset: None,
            progress: None,
        };
        let fields = fields(&info);
        assert_eq!(fields.len(), FIELD_COUNT);
        assert_eq!(fields[FIELD_VIEW], "TEXT");
        assert_eq!(fields[FIELD_SIZE], "11 B");
        assert_eq!(fields[FIELD_POSITION], "Ln 1, Col 5");
    }
}
