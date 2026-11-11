//! Monospaced text rendering helpers.
//!
//! The large text view and the hex view draw character grids.  For a fixed
//! font GDI's `TextOutW` is the lowest-overhead path on Windows and keeps the
//! idle footprint small; DirectWrite is unnecessary for a monospace grid.
//! The wrapper below owns the font handle and reports its cell metrics so
//! callers never rely on magic pixel values.

use windows::core::w;
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, GetTextExtentPoint32W, HFONT, HGDIOBJ, SelectObject,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, FF_MODERN, FIXED_PITCH,
    OUT_DEFAULT_PRECIS,
};
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::HDC;

use crate::error::WinError;

/// Builds a GDI `COLORREF` from 8-bit channels.
#[must_use]
pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

/// Default point size of the editor font (Chinese size 小四, 12 pt).
pub const DEFAULT_FONT_POINTS: i32 = 12;
/// Smallest point size accepted for the self-drawn views.
pub const MIN_FONT_POINTS: i32 = 6;
/// Largest point size accepted for the self-drawn views.
pub const MAX_FONT_POINTS: i32 = 48;
/// Extra pixels between rows.
pub const ROW_GAP: i32 = 2;
/// Font weight used by the editor.
pub const FONT_WEIGHT_NORMAL: i32 = 400;
const POINTS_PER_INCH: i32 = 72;
const PITCH_AND_FAMILY: u32 = FIXED_PITCH.0 as u32 | FF_MODERN.0 as u32;

/// Owned monospaced font with cell metrics.
pub struct MonoFont {
    handle: HFONT,
    pub char_width: i32,
    pub line_height: i32,
}

impl MonoFont {
    /// Creates a `Consolas` font sized for `dpi` and `points`.
    ///
    /// `points` matches the value passed to `SCI_STYLESETSIZE`, so the
    /// self-drawn views keep the same glyph size as the text control even
    /// after the user zooms it.
    pub fn new(dpi: u32, points: i32) -> Result<Self, WinError> {
        let points = points.clamp(MIN_FONT_POINTS, MAX_FONT_POINTS);
        // A negative height asks GDI for a character cell of that many device
        // pixels, which is the point size converted through the screen DPI.
        let height = -((points * dpi as i32) / POINTS_PER_INCH).max(1);
        // SAFETY: all parameters are plain integers or a static wide string.
        let handle = unsafe {
            CreateFontW(
                height,
                0,
                0,
                0,
                FONT_WEIGHT_NORMAL,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                PITCH_AND_FAMILY,
                w!("Consolas"),
            )
        };
        if handle.is_invalid() {
            return Err(WinError::new("CreateFontW returned an invalid HFONT"));
        }
        Ok(Self {
            handle,
            char_width: 0,
            line_height: 0,
        })
    }

    /// Measures the font cell with a device context.
    pub fn measure(&mut self, hdc: HDC) -> Result<(), WinError> {
        // A single glyph reports the exact advance width.  Measuring two
        // glyphs and halving the result would round down on odd widths and
        // make the column grid drift away from the drawn text.
        let sample = [b'M' as u16];
        let mut size = windows::Win32::Foundation::SIZE::default();
        let measured = {
            let guard = self.select(hdc);
            // SAFETY: `guard` selected the font into this DC and `sample`
            // outlives the call.
            unsafe { GetTextExtentPoint32W(guard.hdc, &sample, &mut size) }
        };
        if !measured.as_bool() {
            // SAFETY: reading the thread's last-error value after a failed call.
            let code = unsafe { windows::Win32::Foundation::GetLastError() };
            return Err(WinError::new(format!(
                "GetTextExtentPoint32W failed with Win32 error {code:?}"
            )));
        }
        self.char_width = size.cx.max(1);
        self.line_height = (size.cy + ROW_GAP).max(1);
        Ok(())
    }

    /// Selects the font into `hdc`; the returned guard restores the previous
    /// object when dropped.
    #[must_use]
    pub fn select(&self, hdc: HDC) -> FontGuard<'_> {
        // SAFETY: both handles are owned by this object / the DC.
        let old = unsafe { SelectObject(hdc, HGDIOBJ(self.handle.0)) };
        FontGuard {
            hdc,
            old,
            _font: self,
        }
    }

    /// Number of full rows that fit in `height`.
    #[must_use]
    pub fn rows_in(&self, height: i32) -> i32 {
        if self.line_height <= 0 {
            0
        } else {
            (height / self.line_height).max(0)
        }
    }
}

impl Drop for MonoFont {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and deleted exactly once.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.handle.0));
        }
    }
}

/// Restores the previous GDI object on drop.
pub struct FontGuard<'a> {
    hdc: HDC,
    old: HGDIOBJ,
    _font: &'a MonoFont,
}

impl Drop for FontGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: restores an object previously returned by SelectObject.
        unsafe {
            let _ = SelectObject(self.hdc, self.old);
        }
    }
}

/// Converts a UTF-16 string position into a display column using the font's
/// fixed cell width.
#[must_use]
pub fn column_to_x(font: &MonoFont, column: u32) -> i32 {
    font.char_width.saturating_mul(column as i32)
}

/// Returns the client rectangle of `hwnd`.
#[must_use]
pub fn client_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid out parameter.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
    }
    rect
}
