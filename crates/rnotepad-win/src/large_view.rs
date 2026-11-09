//! Virtualized whole-line painter for read-only text documents.
//!
//! Only the visible line range is decoded and drawn; a 1 GB log therefore
//! costs the same as a 1 KB file.  The line index and the raw byte source are
//! shared with every other view of the document.

use rnotepad_core::bytes::ByteSource;
use rnotepad_core::document::Document;
use rnotepad_core::encoding::{decode, unit_len, Charset};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, FillRect, HBRUSH, HDC, HGDIOBJ, IntersectClipRect, RestoreDC,
    SaveDC, SetBkMode, SetTextColor, TextOutW, TRANSPARENT,
};

use crate::dw::{column_to_x, rgb, MonoFont};
use crate::view::{highlight_ranges_for_line, ViewData};

const TEXT_COLOR: COLORREF = rgb(24, 24, 24);
const GUTTER_TEXT_COLOR: COLORREF = rgb(132, 132, 132);
const GUTTER_RULE_COLOR: COLORREF = rgb(216, 216, 216);
const HIGHLIGHT_COLOR: COLORREF = rgb(255, 232, 140);
/// Selection palette shared with the hex view: accent fill, white text.
const SELECTION_COLOR: COLORREF = rgb(26, 115, 232);
const SELECTED_TEXT_COLOR: COLORREF = rgb(255, 255, 255);
const MIN_GUTTER_DIGITS: u32 = 3;
const GUTTER_PADDING_CELLS: i32 = 1;
const RULE_WIDTH: i32 = 1;
const DECIMAL_BASE: u32 = 10;
const NO_ROWS: i32 = 0;

/// Owned solid brush, deleted when the paint pass ends.
struct SolidBrush(HBRUSH);

impl SolidBrush {
    fn create(color: COLORREF) -> Option<Self> {
        // SAFETY: `CreateSolidBrush` has no preconditions; the handle is
        // owned by the returned wrapper.
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

/// Paints the visible text lines.
pub fn paint(hdc: HDC, rect: &RECT, doc: &Document, data: &ViewData, font: &MonoFont) {
    if font.line_height <= NO_ROWS || font.char_width <= NO_ROWS {
        return;
    }
    // SAFETY: `hdc` is valid for the duration of WM_PAINT; the mode is
    // process-global per DC and does not need to be restored.
    unsafe {
        let _ = SetBkMode(hdc, TRANSPARENT);
    }

    let total_lines = doc
        .line_index
        .total_lines()
        .unwrap_or_else(|| doc.line_index.scanned_line_hint());
    let gutter_width = gutter_width(doc, font);
    let visible_rows = font.rows_in(rect.bottom);
    let charset = doc.encoding.charset;

    let selection_brush = SolidBrush::create(SELECTION_COLOR);
    let highlight_brush = SolidBrush::create(HIGHLIGHT_COLOR);
    let rule_brush = SolidBrush::create(GUTTER_RULE_COLOR);
    if let Some(brush) = &rule_brush {
        let rule = RECT {
            left: (gutter_width - RULE_WIDTH).max(rect.left),
            top: rect.top,
            right: gutter_width,
            bottom: rect.bottom,
        };
        // SAFETY: valid DC and rectangle.
        unsafe {
            FillRect(hdc, &rule, brush.0);
        }
    }

    for row in 0..visible_rows {
        let line = data.first_line.saturating_add(row as u32);
        if line >= total_lines {
            break;
        }
        let Some(range) = doc.line_index.line_range(line) else {
            break;
        };
        let Some(raw) = doc.raw.slice(range.start..range.end) else {
            break;
        };
        let y = row * font.line_height;

        // Highlights are painted first so the selection fill wins where the
        // two ranges overlap, matching the hex view.
        for highlight in highlight_ranges_for_line(&data.highlights, range.start, range.end) {
            draw_span(
                hdc,
                raw,
                charset,
                range.start,
                highlight.start,
                highlight.end,
                font,
                gutter_width,
                y,
                highlight_brush.as_ref(),
            );
        }
        if let Some(selection) = &data.sel {
            draw_span(
                hdc,
                raw,
                charset,
                range.start,
                selection.start,
                selection.end,
                font,
                gutter_width,
                y,
                selection_brush.as_ref(),
            );
        }

        let text = decode(raw, charset, 0).text;
        let visible = text.trim_end_matches(['\r', '\n']);
        let number = line.saturating_add(1).to_string();
        let number_x = gutter_width
            - GUTTER_PADDING_CELLS * font.char_width
            - number.len() as i32 * font.char_width;
        let selection_rect = data.sel.as_ref().and_then(|selection| {
            selection_rect(
                raw,
                charset,
                range.start,
                selection.start,
                selection.end,
                font,
                gutter_width,
                y,
            )
        });
        // SAFETY: valid DC and UTF-16 slices that outlive the call.
        unsafe {
            SetTextColor(hdc, GUTTER_TEXT_COLOR);
            let _ = TextOutW(hdc, number_x, y, &wide(&number));
            SetTextColor(hdc, TEXT_COLOR);
            let _ = TextOutW(hdc, gutter_width, y, &wide(visible));
            // Redraw the glyphs white inside the selection clip so the result
            // is the same accent-on-white pairing used by the hex view.
            if let Some(rect) = selection_rect {
                let saved = SaveDC(hdc);
                if saved != 0 {
                    let _ = IntersectClipRect(hdc, rect.left, rect.top, rect.right, rect.bottom);
                    SetTextColor(hdc, SELECTED_TEXT_COLOR);
                    let _ = TextOutW(hdc, gutter_width, y, &wide(visible));
                    let _ = RestoreDC(hdc, saved);
                }
            }
        }
    }
}

/// Fills the background of a raw span inside one line.
#[allow(clippy::too_many_arguments)]
fn draw_span(
    hdc: HDC,
    raw: &[u8],
    charset: Charset,
    line_start: u64,
    span_start: u64,
    span_end: u64,
    font: &MonoFont,
    gutter_width: i32,
    y: i32,
    brush: Option<&SolidBrush>,
) {
    let Some(brush) = brush else { return };
    let Some((start_col, width_cols)) =
        span_columns(raw, charset, span_start.saturating_sub(line_start), span_end.saturating_sub(line_start))
    else {
        return;
    };
    let left = gutter_width + column_to_x(font, start_col);
    let width = column_to_x(font, width_cols).max(1);
    let rect = RECT {
        left,
        top: y,
        right: left + width,
        bottom: y + font.line_height,
    };
    // SAFETY: valid DC, rectangle and brush.
    unsafe {
        FillRect(hdc, &rect, brush.0);
    }
}

/// Returns the client rectangle of a raw byte span inside one line.
#[allow(clippy::too_many_arguments)]
fn selection_rect(
    raw: &[u8],
    charset: Charset,
    line_start: u64,
    span_start: u64,
    span_end: u64,
    font: &MonoFont,
    gutter_width: i32,
    y: i32,
) -> Option<RECT> {
    let (start_col, width_cols) = span_columns(
        raw,
        charset,
        span_start.saturating_sub(line_start),
        span_end.saturating_sub(line_start),
    )?;
    let left = gutter_width + column_to_x(font, start_col);
    let width = column_to_x(font, width_cols).max(1);
    Some(RECT {
        left,
        top: y,
        right: left + width,
        bottom: y + font.line_height,
    })
}

/// Returns the display column and width of a raw byte span inside a line.
fn span_columns(raw: &[u8], charset: Charset, from: u64, to: u64) -> Option<(u32, u32)> {
    let len = raw.len() as u64;
    let mut from = from.min(len);
    let mut to = to.min(len);
    if charset.is_utf16() {
        from -= from % 2;
        to -= to % 2;
    }
    if from >= to {
        return None;
    }
    let prefix = decode(&raw[..from as usize], charset, 0).text;
    let span = decode(&raw[from as usize..to as usize], charset, 0).text;
    Some((prefix.width() as u32, span.width().max(1) as u32))
}

fn gutter_width(doc: &Document, font: &MonoFont) -> i32 {
    let total_lines = doc
        .line_index
        .total_lines()
        .unwrap_or_else(|| doc.line_index.scanned_line_hint());
    let digits = decimal_digits(total_lines).max(MIN_GUTTER_DIGITS);
    (digits as i32 + GUTTER_PADDING_CELLS) * font.char_width
}

/// Maps a client point to the nearest raw offset inside a line.
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
    let total_lines = doc
        .line_index
        .total_lines()
        .unwrap_or_else(|| doc.line_index.scanned_line_hint());
    let row = data
        .first_line
        .saturating_add((y.max(0) as u32).checked_div(font.line_height as u32)?);
    if row >= total_lines {
        return None;
    }
    let range = doc.line_index.line_range(row)?;
    let raw = doc.raw.slice(range.start..range.end)?;
    let target = ((x - gutter_width(doc, font)).max(0) / font.char_width) as usize;
    let charset = doc.encoding.charset;
    let mut offset = 0usize;
    let mut column = 0usize;
    while offset < raw.len() {
        let unit = unit_len(raw, offset, charset).max(1);
        let end = (offset + unit).min(raw.len());
        let text = decode(&raw[offset..end], charset, 0).text;
        let mut width = 0usize;
        for ch in text.chars() {
            if ch == '\r' || ch == '\n' {
                break;
            }
            width = width.saturating_add(UnicodeWidthChar::width(ch).unwrap_or(1).max(1));
        }
        if target < column.saturating_add(width.max(1)) {
            return Some(range.start + offset as u64);
        }
        column = column.saturating_add(width.max(1));
        offset = end;
    }
    Some(range.end)
}

fn decimal_digits(value: u32) -> u32 {
    let mut digits = 1;
    let mut current = value;
    while current >= DECIMAL_BASE {
        current /= DECIMAL_BASE;
        digits += 1;
    }
    digits
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
