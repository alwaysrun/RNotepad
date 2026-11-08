//! Read-only hex view over the raw byte stream.
//!
//! Rows are read directly from the shared [`rnotepad_core::bytes::RawBytes`],
//! so even a multi-gigabyte log only keeps one screen of bytes in memory.
//! Layout: `Offset(0x + 8 hex) | 8 bytes | 8 bytes | ASCII`, with a column
//! header row and a wider gap between the two byte groups.

use std::ops::Range;

use rnotepad_core::bytes::ByteSource;
use rnotepad_core::document::Document;
use rnotepad_core::encoding::{decode, unit_len, Charset};
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, FillRect, HBRUSH, HDC, HGDIOBJ, SetBkMode, SetTextColor,
    TextOutW, TRANSPARENT,
};

use crate::dw::{column_to_x, rgb, MonoFont};
use crate::view::ViewData;

const HEX_TEXT_COLOR: COLORREF = rgb(40, 40, 40);
const OFFSET_TEXT_COLOR: COLORREF = rgb(120, 120, 120);
const HEADER_BACKGROUND: COLORREF = rgb(245, 245, 245);
const HEADER_TEXT_COLOR: COLORREF = rgb(140, 140, 140);
const BOM_COLOR: COLORREF = rgb(214, 234, 255);
const EOL_COLOR: COLORREF = rgb(214, 246, 218);
const HIGHLIGHT_COLOR: COLORREF = rgb(255, 232, 140);
/// Selection follows the reference hex editor: a solid accent fill.
const SELECTION_COLOR: COLORREF = rgb(26, 115, 232);
const SELECTED_TEXT_COLOR: COLORREF = rgb(255, 255, 255);
const MULTIBYTE_COLOR: COLORREF = rgb(238, 234, 250);
const BYTES_PER_ROW: usize = 16;
/// Bytes per visual group; four columns read comfortably as `AB CD EF 01`.
const GROUP_BYTES: usize = 4;
const HEX_CELLS_PER_BYTE: usize = 3;
const OFFSET_PREFIX: &str = "0x";
const OFFSET_HEX_DIGITS: usize = 8;
const OFFSET_CELLS: usize = OFFSET_PREFIX.len() + OFFSET_HEX_DIGITS;
const OFFSET_GAP_CELLS: usize = 1;
/// Extra cells between groups. Combined with the trailing byte space this
/// renders as four blank columns, e.g. `AB CD EF 01    23 45 67 89`.
const GROUP_GAP_CELLS: usize = 3;
const ASCII_GAP_CELLS: usize = 2;
/// Extra vertical pixels per row so dense hex lines stay easy to scan.
pub const ROW_GAP_PIXELS: i32 = 2;
const HEADER_ROWS: i32 = 1;
const OFFSET_LABEL: &str = "Offset";
const ASCII_LABEL: &str = "ASCII";
const PRINTABLE_MIN: u8 = 0x20;
const PRINTABLE_MAX: u8 = 0x7E;
const CR_BYTE: u8 = 0x0D;
const LF_BYTE: u8 = 0x0A;
const CR_UNIT: u16 = 0x000D;
const LF_UNIT: u16 = 0x000A;
const PREVIEW_DOT: char = '.';
const UNKNOWN_CHAR: char = '\u{FFFD}';
const NO_ROWS: i32 = 0;

/// Column that starts the byte grid (after the offset and its gap).
const GRID_COLUMN: usize = OFFSET_CELLS + OFFSET_GAP_CELLS;
/// Column of the first byte in each row, including the intra-row group gap.
const fn byte_column(index: usize) -> u32 {
    (GRID_COLUMN + index * HEX_CELLS_PER_BYTE + (index / GROUP_BYTES) * GROUP_GAP_CELLS) as u32
}
/// Column of the ASCII preview.
const ASCII_COLUMN: u32 =
    byte_column(BYTES_PER_ROW - 1) + HEX_CELLS_PER_BYTE as u32 + ASCII_GAP_CELLS as u32;

/// Height of one painted hex row, including the extra breathing space.
fn row_height(font: &MonoFont) -> i32 {
    font.line_height + ROW_GAP_PIXELS
}

/// What the preview column shows for one byte.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewCell {
    Char(char),
    Dot,
    Blank,
}

/// Owned solid brush, deleted when the paint pass ends.
struct SolidBrush(HBRUSH);

impl SolidBrush {
    fn create(color: COLORREF) -> Option<Self> {
        // SAFETY: `CreateSolidBrush` has no preconditions.
        let handle = unsafe { CreateSolidBrush(color) };
        if handle.is_invalid() {
            None
        } else {
            Some(Self(handle))
        }
    }
}

impl Drop for SolidBrush {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and deleted exactly once.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0 .0));
        }
    }
}

/// Paints the column header and the visible hex rows.
pub fn paint(hdc: HDC, rect: &RECT, doc: &Document, data: &ViewData, font: &MonoFont) {
    if font.line_height <= NO_ROWS || font.char_width <= NO_ROWS {
        return;
    }
    // SAFETY: `hdc` is valid for the duration of WM_PAINT.
    unsafe {
        let _ = SetBkMode(hdc, TRANSPARENT);
    }

    let brush_bom = SolidBrush::create(BOM_COLOR);
    let brush_eol = SolidBrush::create(EOL_COLOR);
    let brush_highlight = SolidBrush::create(HIGHLIGHT_COLOR);
    let brush_selection = SolidBrush::create(SELECTION_COLOR);
    let brush_multibyte = SolidBrush::create(MULTIBYTE_COLOR);
    let brush_header = SolidBrush::create(HEADER_BACKGROUND);

    draw_header(hdc, rect, font, brush_header.as_ref());

    let total = doc.raw.len();
    let rows = (rect.bottom / row_height(font).max(1)).max(NO_ROWS);
    let visible_rows = (rows - HEADER_ROWS).max(NO_ROWS);
    let charset = doc.encoding.charset;
    let bom_len = doc.encoding.bom.len();

    for row_index in 0..visible_rows {
        let row = data.first_row.saturating_add(row_index as u64);
        let start = row.saturating_mul(BYTES_PER_ROW as u64);
        if start >= total {
            break;
        }
        let end = start.saturating_add(BYTES_PER_ROW as u64).min(total);
        let Some(raw) = doc.raw.slice(start..end) else {
            break;
        };
        let y = (row_index + HEADER_ROWS) * row_height(font);
        let selected = selected_mask(raw.len(), start, data);

        draw_byte_backgrounds(
            hdc,
            raw,
            start,
            bom_len,
            charset,
            data,
            &selected,
            font,
            y,
            &brush_bom,
            &brush_eol,
            &brush_highlight,
            &brush_selection,
            &brush_multibyte,
        );

        let offset = format!("{OFFSET_PREFIX}{start:08X}");
        // SAFETY: valid DC and UTF-16 slices that outlive the calls.
        unsafe {
            SetTextColor(hdc, OFFSET_TEXT_COLOR);
            let _ = TextOutW(hdc, 0, y, &wide(&offset));
        }
        for (index, byte) in raw.iter().enumerate() {
            let x = column_to_x(font, byte_column(index));
            // Selected bytes invert to white on the accent background, matching
            // the reference editor.
            // SAFETY: valid DC.
            unsafe {
                SetTextColor(
                    hdc,
                    if selected[index] {
                        SELECTED_TEXT_COLOR
                    } else {
                        HEX_TEXT_COLOR
                    },
                );
                let _ = TextOutW(hdc, x, y, &wide(&format!("{byte:02X}")));
            }
        }
        // SAFETY: valid DC.
        unsafe {
            SetTextColor(hdc, HEX_TEXT_COLOR);
        }

        let preview = preview_cells(raw, start, charset);
        for (index, cell) in preview.iter().enumerate() {
            let text = match cell {
                PreviewCell::Char(ch) => ch.to_string(),
                PreviewCell::Dot => PREVIEW_DOT.to_string(),
                PreviewCell::Blank => continue,
            };
            let x = column_to_x(font, ASCII_COLUMN + index as u32);
            // SAFETY: valid DC.
            unsafe {
                let _ = TextOutW(hdc, x, y, &wide(&text));
            }
        }
    }
}

/// Draws the `Offset`, byte-index and `ASCII` labels in the first row.
fn draw_header(hdc: HDC, rect: &RECT, font: &MonoFont, background: Option<&SolidBrush>) {
    let bottom = (HEADER_ROWS * row_height(font)).min(rect.bottom.max(NO_ROWS));
    let header = RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom,
    };
    if let Some(brush) = background {
        // SAFETY: valid DC, rectangle and brush.
        unsafe {
            FillRect(hdc, &header, brush.0);
        }
    }
    let label_cells = OFFSET_LABEL.len() as u32;
    let offset_x = column_to_x(font, OFFSET_CELLS as u32 - label_cells);
    // SAFETY: valid DC and UTF-16 slices that outlive the calls.
    unsafe {
        SetTextColor(hdc, HEADER_TEXT_COLOR);
        let _ = TextOutW(hdc, offset_x, rect.top, &wide(OFFSET_LABEL));
    }
    for index in 0..BYTES_PER_ROW {
        let x = column_to_x(font, byte_column(index));
        // SAFETY: valid DC.
        unsafe {
            let _ = TextOutW(hdc, x, rect.top, &wide(&format!("{index:02X}")));
        }
    }
    let ascii_x = column_to_x(font, ASCII_COLUMN);
    // SAFETY: valid DC.
    unsafe {
        let _ = TextOutW(hdc, ascii_x, rect.top, &wide(ASCII_LABEL));
    }
}

/// Whether the byte in each visible cell is covered by the hex selection.
fn selected_mask(len: usize, row_start: u64, data: &ViewData) -> Vec<bool> {
    let selection = data.sel.as_ref();
    (0..len)
        .map(|index| {
            selection.is_some_and(|range| range.contains(&(row_start + index as u64)))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn draw_byte_backgrounds(
    hdc: HDC,
    raw: &[u8],
    row_start: u64,
    bom_len: u64,
    charset: Charset,
    data: &ViewData,
    selected: &[bool],
    font: &MonoFont,
    y: i32,
    brush_bom: &Option<SolidBrush>,
    brush_eol: &Option<SolidBrush>,
    brush_highlight: &Option<SolidBrush>,
    brush_selection: &Option<SolidBrush>,
    brush_multibyte: &Option<SolidBrush>,
) {
    let eol = eol_byte_mask(raw, row_start, charset);
    let multibyte = multibyte_byte_mask(raw, charset);
    for index in 0..raw.len() {
        let offset = row_start + index as u64;
        let selected = selected.get(index).copied().unwrap_or(false);
        // Selection wins so the white selected text always sits on the accent
        // fill, even when the byte is also inside a search highlight.
        let brush: Option<&SolidBrush> = if selected {
            brush_selection.as_ref()
        } else if contains(&data.highlights, offset) {
            brush_highlight.as_ref()
        } else {
            class_brush(
                offset,
                bom_len,
                index,
                &eol,
                &multibyte,
                brush_bom,
                brush_eol,
                brush_multibyte,
            )
        };
        let Some(brush) = brush else { continue };
        let x = column_to_x(font, byte_column(index));
        let rect = RECT {
            left: x,
            top: y,
            right: x + column_to_x(font, HEX_CELLS_PER_BYTE as u32),
            bottom: y + row_height(font),
        };
        // SAFETY: valid DC, rectangle and brush.
        unsafe {
            FillRect(hdc, &rect, brush.0);
        }
    }
}

fn class_brush<'a>(
    offset: u64,
    bom_len: u64,
    index: usize,
    eol: &[bool],
    multibyte: &[bool],
    brush_bom: &'a Option<SolidBrush>,
    brush_eol: &'a Option<SolidBrush>,
    brush_multibyte: &'a Option<SolidBrush>,
) -> Option<&'a SolidBrush> {
    if offset < bom_len {
        brush_bom.as_ref()
    } else if eol.get(index).copied().unwrap_or(false) {
        brush_eol.as_ref()
    } else if multibyte.get(index).copied().unwrap_or(false) {
        brush_multibyte.as_ref()
    } else {
        None
    }
}

/// Marks bytes that belong to a newline sequence.
fn eol_byte_mask(raw: &[u8], row_start: u64, charset: Charset) -> Vec<bool> {
    let mut mask = vec![false; raw.len()];
    if charset.is_utf16() {
        let little = charset == Charset::Utf16Le;
        let mut index = if row_start % 2 == 0 { 0 } else { 1 };
        while index + 1 < raw.len() {
            let unit = if little {
                u16::from_le_bytes([raw[index], raw[index + 1]])
            } else {
                u16::from_be_bytes([raw[index], raw[index + 1]])
            };
            if unit == CR_UNIT || unit == LF_UNIT {
                mask[index] = true;
                mask[index + 1] = true;
            }
            index += 2;
        }
    } else {
        for (index, byte) in raw.iter().enumerate() {
            if *byte == CR_BYTE || *byte == LF_BYTE {
                mask[index] = true;
            }
        }
    }
    mask
}

/// Marks continuation bytes of multi-byte sequences in legacy charsets.
fn multibyte_byte_mask(raw: &[u8], charset: Charset) -> Vec<bool> {
    let mut mask = vec![false; raw.len()];
    if charset.is_utf8() || charset.is_utf16() || charset.is_single_byte() {
        return mask;
    }
    let mut index = 0;
    while index < raw.len() {
        let unit = unit_len(raw, index, charset).max(1);
        if unit > 1 {
            for cell in mask.iter_mut().skip(index).take(unit) {
                *cell = true;
            }
        }
        index += unit;
    }
    mask
}

/// Builds the character preview cells for one row.
fn preview_cells(raw: &[u8], row_start: u64, charset: Charset) -> Vec<PreviewCell> {
    let mut cells = vec![PreviewCell::Blank; raw.len()];
    if charset.is_utf16() {
        let mut index = if row_start % 2 == 0 { 0 } else { 1 };
        while index + 1 < raw.len() {
            let unit = unit_len(raw, index, charset).max(2);
            let text = decode(&raw[index..(index + unit).min(raw.len())], charset, 0).text;
            cells[index] = preview_for(&text);
            index += unit;
        }
        cells
    } else {
        let mut index = 0;
        while index < raw.len() {
            let byte = raw[index];
            if (PRINTABLE_MIN..=PRINTABLE_MAX).contains(&byte) {
                cells[index] = PreviewCell::Char(byte as char);
                index += 1;
                continue;
            }
            let unit = unit_len(raw, index, charset).max(1);
            let text = decode(&raw[index..(index + unit).min(raw.len())], charset, 0).text;
            cells[index] = preview_for(&text);
            index += unit;
        }
        cells
    }
}

fn preview_for(text: &str) -> PreviewCell {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if !ch.is_control() && ch != UNKNOWN_CHAR => PreviewCell::Char(ch),
        _ => PreviewCell::Dot,
    }
}

/// Maps a client point to the raw byte cell under the cursor.
///
/// The x axis is clamped to the byte grid, so clicks in the offset or ASCII
/// columns still select the nearest byte on that row. Clicks on the column
/// header select nothing.
pub fn hit_test(
    doc: &Document,
    data: &ViewData,
    font: &MonoFont,
    x: i32,
    y: i32,
) -> Option<u64> {
    if font.line_height <= NO_ROWS || font.char_width <= NO_ROWS {
        return None;
    }
    let height = row_height(font);
    let header_height = HEADER_ROWS * height;
    if y < header_height {
        return None;
    }
    let row_index = ((y - header_height).max(0) as u32).checked_div(height as u32)? as u64;
    let row = data.first_row.saturating_add(row_index);
    let start = row.saturating_mul(BYTES_PER_ROW as u64);
    let total = doc.raw.len();
    if start >= total {
        return None;
    }
    let index = byte_index_at(x, font);
    let available = usize::try_from(total - start).ok()?;
    let index = index.min(available.saturating_sub(1));
    Some(start + index as u64)
}

/// Maps a horizontal position to a byte index, honouring the group gap.
fn byte_index_at(x: i32, font: &MonoFont) -> usize {
    for index in 0..BYTES_PER_ROW {
        let start = column_to_x(font, byte_column(index));
        let end = start + font.char_width * HEX_CELLS_PER_BYTE as i32;
        if x < end {
            return index;
        }
    }
    BYTES_PER_ROW - 1
}

fn contains(ranges: &[Range<u64>], offset: u64) -> bool {
    let index = ranges.partition_point(|range| range.start <= offset);
    index > 0 && ranges[index - 1].end > offset
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
