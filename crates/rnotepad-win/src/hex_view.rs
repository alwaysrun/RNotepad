//! Read-only hex view over the raw byte stream.
//!
//! Rows are read directly from the shared [`rnotepad_core::bytes::RawBytes`],
//! so even a multi-gigabyte log only keeps one screen of bytes in memory.
//! Layout: `Offset(0x + 8 hex) | 4 x 4 bytes | ASCII`, with a column header
//! row and a wider gap after every group of four bytes.

use std::ops::Range;

use rnotepad_core::bytes::ByteSource;
use rnotepad_core::document::Document;
use rnotepad_core::encoding::{decode, unit_len, Charset};
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, FillRect, HBRUSH, HDC, HGDIOBJ, IntersectClipRect,
    RestoreDC, SaveDC, SetBkMode, SetTextColor, TextOutW, TRANSPARENT,
};

use crate::dw::{column_to_x, rgb, MonoFont};
use crate::view::ViewData;

const HEX_TEXT_COLOR: COLORREF = rgb(40, 40, 40);
const OFFSET_TEXT_COLOR: COLORREF = rgb(120, 120, 120);
const HEADER_BACKGROUND: COLORREF = rgb(245, 245, 245);
const HEADER_TEXT_COLOR: COLORREF = rgb(140, 140, 140);
/// Background of the pinned offset gutter.
const GUTTER_BACKGROUND: COLORREF = rgb(248, 248, 248);
/// One-pixel rule that separates the gutter from the scrolling grid.
const GUTTER_RULE_COLOR: COLORREF = rgb(222, 222, 222);
const RULE_WIDTH: i32 = 1;
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
/// Cells reserved per byte: two hex digits plus two blank cells that keep
/// neighbouring bytes visually apart.
const HEX_CELLS_PER_BYTE: usize = 4;
const OFFSET_PREFIX: &str = "0x";
const OFFSET_HEX_DIGITS: usize = 8;
const OFFSET_CELLS: usize = OFFSET_PREFIX.len() + OFFSET_HEX_DIGITS;
/// Blank cells between the offset and the first byte, so the two are easy to
/// tell apart at a glance.
const OFFSET_GAP_CELLS: usize = 4;
/// Extra cells between groups. Combined with the two blank cells after each
/// byte this renders four blank columns, e.g. `EF 8B FB 01    23 45 67 89`.
const GROUP_GAP_CELLS: usize = 2;
/// Blank cells between the hex grid and the ASCII preview.
const ASCII_GAP_CELLS: usize = 4;
/// Cells per preview character; the extra cell keeps the characters from
/// running together.
const ASCII_CELLS_PER_CHAR: usize = 2;
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
/// Cell columns occupied by the pinned offset gutter.
pub const GUTTER_CELLS: u32 = GRID_COLUMN as u32;
/// Total cell columns of the complete hex layout, gutter included.
pub const CONTENT_CELLS: u32 =
    ASCII_COLUMN + (BYTES_PER_ROW * ASCII_CELLS_PER_CHAR) as u32;

/// Pixel width of the pinned offset gutter.
#[must_use]
pub fn gutter_width(font: &MonoFont) -> i32 {
    column_to_x(font, GUTTER_CELLS)
}

/// Pixel width of the whole hex layout, gutter included.
#[must_use]
pub fn content_width(font: &MonoFont) -> i32 {
    column_to_x(font, CONTENT_CELLS)
}

/// Largest horizontal offset that still shows the right edge of the layout.
#[must_use]
pub fn max_h_offset(font: &MonoFont, client_width: i32) -> i32 {
    (content_width(font) - client_width.max(0)).max(0)
}

/// Height of one painted hex row, including the extra breathing space.
fn row_height(font: &MonoFont) -> i32 {
    font.line_height + ROW_GAP_PIXELS
}

/// Number of byte rows that fit below the column header.
#[must_use]
pub fn visible_rows(height: i32, font: &MonoFont) -> u64 {
    let rows = (height / row_height(font).max(1) - HEADER_ROWS).max(NO_ROWS);
    u64::from(rows as u32)
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
    let brush_gutter = SolidBrush::create(GUTTER_BACKGROUND);
    let brush_rule = SolidBrush::create(GUTTER_RULE_COLOR);

    let h_offset = data.h_offset.clamp(0, max_h_offset(font, rect.right));
    let gutter = gutter_width(font);
    let header_bottom = (HEADER_ROWS * row_height(font)).min(rect.bottom.max(NO_ROWS));

    draw_header(hdc, rect, font, brush_header.as_ref(), gutter, h_offset);
    draw_gutter(
        hdc,
        rect,
        header_bottom,
        gutter,
        brush_gutter.as_ref(),
        brush_rule.as_ref(),
    );

    let total = doc.raw.len();
    let visible_rows = visible_rows(rect.bottom, font) as i32;
    let charset = doc.encoding.charset;
    let bom_len = doc.encoding.bom.len();

    // The offsets live in the pinned gutter and never scroll, so they are
    // drawn before the clipped grid pass.
    draw_offsets(hdc, data, total, font, visible_rows);

    // Clip everything right of the gutter; scrolled cells then slide under the
    // pinned offset column instead of painting over it.
    let saved = unsafe { SaveDC(hdc) };
    if saved == 0 {
        return;
    }
    let clip_left = gutter.min(rect.right);
    // SAFETY: `saved` guarantees a matching `RestoreDC` below.
    unsafe {
        let _ = IntersectClipRect(hdc, clip_left, header_bottom, rect.right, rect.bottom);
    }

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
            h_offset,
            &brush_bom,
            &brush_eol,
            &brush_highlight,
            &brush_selection,
            &brush_multibyte,
        );

        for (index, byte) in raw.iter().enumerate() {
            let x = column_to_x(font, byte_column(index)) - h_offset;
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
            let column = ASCII_COLUMN + (index as u32) * ASCII_CELLS_PER_CHAR as u32;
            let x = column_to_x(font, column) - h_offset;
            // SAFETY: valid DC.
            unsafe {
                let _ = TextOutW(hdc, x, y, &wide(&text));
            }
        }
    }

    // SAFETY: restores the DC state saved above.
    unsafe {
        let _ = RestoreDC(hdc, saved);
    }
}

/// Fills the pinned offset gutter and its separating rule.
fn draw_gutter(
    hdc: HDC,
    rect: &RECT,
    header_bottom: i32,
    gutter: i32,
    background: Option<&SolidBrush>,
    rule: Option<&SolidBrush>,
) {
    let body = RECT {
        left: rect.left,
        top: header_bottom,
        right: gutter,
        bottom: rect.bottom,
    };
    if let Some(brush) = background {
        // SAFETY: valid DC, rectangle and brush.
        unsafe {
            let _ = FillRect(hdc, &body, brush.0);
        }
    }
    if let Some(brush) = rule {
        let separator = RECT {
            left: (gutter - RULE_WIDTH).max(rect.left),
            top: rect.top,
            right: gutter,
            bottom: rect.bottom,
        };
        // SAFETY: valid DC, rectangle and brush.
        unsafe {
            let _ = FillRect(hdc, &separator, brush.0);
        }
    }
}

/// Draws the fixed row offsets inside the gutter.
fn draw_offsets(
    hdc: HDC,
    data: &ViewData,
    total: u64,
    font: &MonoFont,
    visible_rows: i32,
) {
    let height = row_height(font);
    // SAFETY: valid DC and UTF-16 slices that outlive the calls.
    unsafe {
        SetTextColor(hdc, OFFSET_TEXT_COLOR);
        for row_index in 0..visible_rows {
            let row = data.first_row.saturating_add(row_index as u64);
            let start = row.saturating_mul(BYTES_PER_ROW as u64);
            if start >= total {
                break;
            }
            let y = (row_index + HEADER_ROWS) * height;
            let offset = format!("{OFFSET_PREFIX}{start:08X}");
            let _ = TextOutW(hdc, 0, y, &wide(&offset));
        }
    }
}

/// Draws the `Offset`, byte-index and `ASCII` labels in the first row.
///
/// The `Offset` label is pinned with the gutter; the byte-index and `ASCII`
/// labels follow the grid's horizontal offset.
fn draw_header(
    hdc: HDC,
    rect: &RECT,
    font: &MonoFont,
    background: Option<&SolidBrush>,
    gutter: i32,
    h_offset: i32,
) {
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

    let saved = unsafe { SaveDC(hdc) };
    if saved == 0 {
        return;
    }
    let clip_left = gutter.min(rect.right);
    // SAFETY: `saved` guarantees a matching `RestoreDC` below.
    unsafe {
        let _ = IntersectClipRect(hdc, clip_left, header.top, rect.right, header.bottom);
        for index in 0..BYTES_PER_ROW {
            let x = column_to_x(font, byte_column(index)) - h_offset;
            let _ = TextOutW(hdc, x, rect.top, &wide(&format!("{index:02X}")));
        }
        let ascii_x = column_to_x(font, ASCII_COLUMN) - h_offset;
        let _ = TextOutW(hdc, ascii_x, rect.top, &wide(ASCII_LABEL));
        let _ = RestoreDC(hdc, saved);
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
    h_offset: i32,
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
        let x = column_to_x(font, byte_column(index)) - h_offset;
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

/// Picks the tint brush for one byte; kept explicit because the three masks
/// describe independent, mutually exclusive byte classes.
#[allow(clippy::too_many_arguments)]
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
/// The pinned offset gutter and the column header are not selectable.  Clicks
/// right of the gutter are mapped against the current horizontal offset, and
/// the x axis is clamped to the byte grid so clicks in the ASCII column still
/// select the nearest byte on that row.
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
    if x < gutter_width(font) {
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
    let index = byte_index_at(x.saturating_add(data.h_offset), font);
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
