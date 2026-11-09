//! Thin Scintilla host.
//!
//! The official Scintilla DLL is loaded at runtime; there is no binding crate
//! and no static link, as required by the specification.  Messages are sent
//! synchronously on the UI thread.  Positions are UTF-8 byte offsets because
//! the control is configured with `SC_CP_UTF8`.

use std::path::PathBuf;
use std::sync::OnceLock;

use rnotepad_core::eol::EolKind;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::Controls::NMHDR;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetClientRect, SendMessageW, SetWindowPos,
    SWP_NOZORDER, WS_CHILD, WS_HSCROLL, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::error::{WinError, WinResult};
use crate::sci_consts::*;

const DLL_NAME: &str = "Scintilla.dll";
/// Directories above the executable searched for a development DLL.
const ANCESTOR_SEARCH_LEVELS: usize = 4;
const STYLE_DEFAULT: usize = 32;
const STYLE_LINE_NUMBER: usize = 33;
/// Longest byte length of a line that still counts as freshly opened.
///
/// Scintilla reports both `\r` and `\n` of a CRLF newline as separate
/// `SCN_CHARADDED` events, so indentation would be inserted twice without
/// requiring the caret line to be a bare new line (at most a CRLF terminator).
const MAX_FRESH_LINE_BYTES: usize = 2;
const INDICATOR_INDEX: usize = 0;
const INDIC_ROUNDBOX: usize = 7;
const SCFIND_WHOLEWORD: usize = 0x0000_0002;
const SCFIND_MATCHCASE: usize = 0x0000_0004;
const SCFIND_REGEXP: usize = 0x0020_0000;
const SCFIND_NONE: usize = 0x0000_0000;
const MAX_REPLACEMENTS: usize = 1_000_000;
const DEFAULT_FONT: &str = "Consolas";
const DEFAULT_POINTS: i32 = 10;
const NO_MARGIN: i32 = 0;
const MIN_MARGIN_WIDTH: i32 = 0;
const INT_MAX_POS: usize = i32::MAX as usize;
const NOT_FOUND: isize = -1;

/// Scintilla notification code emitted when the caret or selection changed.
///
/// Re-exported so the application does not depend on raw notification numbers.
pub const SCN_UPDATEUI_CODE: u32 = 2007;
/// Scintilla notification code emitted when the user typed a character.
pub const SCN_CHARADDED_CODE: u32 = 2001;

/// Newline as Scintilla reports it in `SCN_CHARADDED`, independent of the
/// document's EOL mode.
const NEWLINE_CHAR: u8 = b'\n';
/// Carriage return, accepted as a newline for defensive completeness.
const CARRIAGE_RETURN_CHAR: u8 = b'\r';
/// Scintilla notification code emitted when the user changed the zoom level.
pub const SCN_ZOOM_CODE: u32 = SCN_ZOOM;

/// Text colours expressed as Scintilla's `0x00BBGGRR` values.
const TEXT_COLOUR: usize = 0x0018_1818;
const CARET_LINE_COLOUR: usize = 0x00F6_F0E8;
const FIND_COLOUR: usize = 0x0040_9AFF;
/// Selection palette shared with the hex view: accent fill, white text.
const SELECTION_BACK_COLOUR: usize = sci_colour(26, 115, 232);
const SELECTION_TEXT_COLOUR: usize = sci_colour(255, 255, 255);

/// Scintilla's `CHARRANGE`, whose fields are 32-bit Windows `long` values.
#[repr(C)]
#[derive(Default)]
struct CharRange {
    cp_min: i32,
    cp_max: i32,
}

/// Scintilla's `TextToFind` structure.
#[repr(C)]
struct TextToFind {
    range: CharRange,
    text: *const u8,
    found: CharRange,
}

/// Scintilla's `TextRange` structure.
#[repr(C)]
struct TextRange {
    range: CharRange,
    text: *mut u8,
}

/// Handle to a Scintilla child window.
pub struct Scintilla {
    hwnd: HWND,
    /// Kept alive for the process lifetime: the window class lives in the
    /// DLL, so freeing the module would make the handle unusable.
    _module: isize,
}

impl Scintilla {
    /// Creates a child window.  Returns an error when the DLL is missing.
    pub fn create(parent: HWND, bounds: RECT) -> WinResult<Self> {
        let module = load_module()?;
        let instance = unsafe { GetModuleHandleW(None) }
            .map_err(|err| WinError::from_windows("GetModuleHandleW", err))?;
        // SAFETY: the Scintilla DLL registers its window class at load time.
        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                w!("Scintilla"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_HSCROLL | WS_TABSTOP,
                bounds.left,
                bounds.top,
                (bounds.right - bounds.left).max(0),
                (bounds.bottom - bounds.top).max(0),
                Some(parent),
                None,
                Some(HINSTANCE(instance.0)),
                None,
            )
        }
        .map_err(|err| {
            WinError::Scintilla(format!("CreateWindowExW(Scintilla) failed: {err}"))
        })?;
        let control = Self {
            hwnd,
            _module: module,
        };
        control.initialise();
        Ok(control)
    }

    /// Window handle.
    #[must_use]
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Sends a raw Scintilla message.
    pub fn send(&self, message: u32, wparam: usize, lparam: isize) -> isize {
        // SAFETY: the handle belongs to this control and the call runs on the
        // thread that created it.
        unsafe { SendMessageW(self.hwnd, message, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
    }

    fn initialise(&self) {
        self.send(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0);
        self.send(SCI_SETUNDOCOLLECTION, 1, 0);
        self.send(SCI_SETREADONLY, 1, 0);
        self.apply_style(DEFAULT_FONT, DEFAULT_POINTS, NO_MARGIN);
        self.set_eol(EolKind::CrLf);
        self.send(SCI_SETWRAPMODE, SC_WRAP_NONE as usize, 0);
        self.send(SCI_SETTABWIDTH, 4, 0);
        self.send(SCI_SETCARETLINEVISIBLE, 1, 0);
        self.send(SCI_SETCARETLINEBACK, CARET_LINE_COLOUR, 0);
        self.send(SCI_SETSELBACK, 1, SELECTION_BACK_COLOUR as isize);
        self.send(SCI_SETSELFORE, 1, SELECTION_TEXT_COLOUR as isize);
        self.send(SCI_SETADDITIONALSELBACK, 1, SELECTION_BACK_COLOUR as isize);
        self.send(SCI_SETADDITIONALSELFORE, 1, SELECTION_TEXT_COLOUR as isize);
        self.send(SCI_SETCARETFORE, TEXT_COLOUR, 0);
        self.send(SCI_INDICSETSTYLE, INDICATOR_INDEX, INDIC_ROUNDBOX as isize);
        self.send(SCI_INDICSETFORE, INDICATOR_INDEX, FIND_COLOUR as isize);
        self.send(SCI_INDICSETUNDER, INDICATOR_INDEX, 1);
    }

    /// Applies the editor font and the line-number margin width.
    pub fn apply_style(&self, font: &str, points: i32, margin_width: i32) {
        let name = c_string(font);
        self.send(SCI_STYLESETFONT, STYLE_DEFAULT, name.as_ptr() as isize);
        self.send(SCI_STYLESETSIZE, STYLE_DEFAULT, points as isize);
        self.send(SCI_STYLESETFORE, STYLE_DEFAULT, TEXT_COLOUR as isize);
        self.send(SCI_STYLECLEARALL, 0, 0);
        self.send(
            SCI_STYLESETFORE,
            STYLE_LINE_NUMBER,
            0x0084_8484,
        );
        self.send(SCI_SETMARGINTYPEN, 0, SC_MARGIN_NUMBER as isize);
        self.send(
            SCI_SETMARGINWIDTHN,
            0,
            margin_width.max(MIN_MARGIN_WIDTH) as isize,
        );
    }

    /// Replaces the whole buffer, even while the control is read-only.
    ///
    /// Scintilla refuses `SCI_SETTEXT` when read-only is set, so the flag is
    /// cleared for the duration of the call and restored afterwards.
    pub fn set_text(&self, text: &str) {
        let read_only = self.send(SCI_GETREADONLY, 0, 0) != 0;
        if read_only {
            self.set_read_only(false);
        }
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        self.send(SCI_SETTEXT, 0, bytes.as_ptr() as isize);
        self.send(SCI_EMPTYUNDOBUFFER, 0, 0);
        self.send(SCI_SETSAVEPOINT, 0, 0);
        if read_only {
            self.set_read_only(true);
        }
    }

    /// Returns the whole buffer as UTF-8.
    #[must_use]
    pub fn get_text(&self) -> String {
        let length = self.send(SCI_GETLENGTH, 0, 0).max(0) as usize;
        let mut buffer = vec![0u8; length + 1];
        self.send(SCI_GETTEXT, buffer.len(), buffer.as_mut_ptr() as isize);
        buffer.truncate(length);
        String::from_utf8(buffer).unwrap_or_default()
    }

    /// Byte length of the buffer.
    #[must_use]
    pub fn length(&self) -> usize {
        self.send(SCI_GETLENGTH, 0, 0).max(0) as usize
    }

    /// Enables or disables editing.
    pub fn set_read_only(&self, read_only: bool) {
        self.send(SCI_SETREADONLY, usize::from(read_only), 0);
    }

    /// `true` when the buffer has unsaved changes.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.send(SCI_GETMODIFY, 0, 0) != 0
    }

    /// Marks the current buffer as saved.
    pub fn mark_saved(&self) {
        self.send(SCI_SETSAVEPOINT, 0, 0);
    }

    /// Sets the newline mode used when Scintilla inserts new lines.
    pub fn set_eol(&self, kind: EolKind) {
        let mode = match kind {
            EolKind::CrLf => SC_EOL_CRLF,
            EolKind::Cr => SC_EOL_CR,
            EolKind::Lf => SC_EOL_LF,
        };
        self.send(SCI_SETEOLMODE, mode as usize, 0);
    }

    /// Converts existing line endings in a single undoable transaction.
    pub fn convert_eols(&self, kind: EolKind) {
        self.send(SCI_BEGINUNDOACTION, 0, 0);
        self.set_eol(kind);
        self.send(SCI_CONVERTEOLS, kind_mode(kind) as usize, 0);
        self.send(SCI_ENDUNDOACTION, 0, 0);
    }

    /// Caret position as a UTF-8 byte offset.
    #[must_use]
    pub fn current_position(&self) -> usize {
        self.send(SCI_GETCURRENTPOS, 0, 0).max(0) as usize
    }

    /// One-based line and character column of a UTF-8 offset.
    ///
    /// The editor buffer is authoritative here: a coordinate map built when
    /// the document was loaded no longer matches the buffer after an edit.
    #[must_use]
    pub fn line_column(&self, position: usize) -> (u32, u32) {
        let line = self.send(SCI_LINEFROMPOSITION, position, 0).max(0) as usize;
        let line_start = self.send(SCI_POSITIONFROMLINE, line, 0).max(0) as usize;
        let column = self
            .send(SCI_COUNTCHARACTERS, line_start, position as isize)
            .max(0) as usize;
        (
            u32::try_from(line).unwrap_or(u32::MAX).saturating_add(1),
            u32::try_from(column).unwrap_or(u32::MAX).saturating_add(1),
        )
    }

    /// Moves the caret and scrolls it into view.
    pub fn goto_position(&self, position: usize) {
        let position = position.min(INT_MAX_POS);
        self.send(SCI_GOTOPOS, position, 0);
        self.send(SCI_SCROLLCARET, 0, 0);
    }

    /// Selects `[start, end)` and scrolls to it.
    pub fn set_selection(&self, start: usize, end: usize) {
        self.send(SCI_SETSEL, start, end as isize);
        self.send(SCI_SCROLLCARET, 0, 0);
    }

    /// Selection range as `[start, end)`.
    #[must_use]
    pub fn selection(&self) -> (usize, usize) {
        let start = self.send(SCI_GETSELECTIONSTART, 0, 0).max(0) as usize;
        let end = self.send(SCI_GETSELECTIONEND, 0, 0).max(0) as usize;
        (start.min(end), start.max(end))
    }

    /// Returns the UTF-8 text in the given byte range.
    #[must_use]
    pub fn text_range(&self, start: usize, end: usize) -> String {
        let length = end.saturating_sub(start);
        if length == 0 {
            return String::new();
        }
        let mut buffer = vec![0u8; length + 1];
        let mut range = TextRange {
            range: CharRange {
                cp_min: start as i32,
                cp_max: end as i32,
            },
            text: buffer.as_mut_ptr(),
        };
        self.send(SCI_GETTEXTRANGE, 0, &mut range as *mut TextRange as isize);
        buffer.truncate(length);
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// Copies the leading whitespace of the line above the caret.
    ///
    /// Scintilla emits `SCN_CHARADDED` for the newline after the caret has
    /// moved onto the new line, so this must run from that notification.  Tabs
    /// and spaces are copied byte for byte, and the caret ends up after the
    /// inserted indentation.  Returns `false` when the control is read-only,
    /// the caret is on the first line, the caret line is not freshly opened,
    /// or the line above has no indentation.
    pub fn apply_previous_indent(&self) -> bool {
        if self.send(SCI_GETREADONLY, 0, 0) != 0 {
            return false;
        }
        let position = self.current_position();
        let line = self.send(SCI_LINEFROMPOSITION, position, 0).max(0) as usize;
        if line == 0 {
            return false;
        }
        // A CRLF Enter raises SCN_CHARADDED twice; only the first event sees
        // an empty line, so this guard prevents double indentation.
        let line_bytes = self.send(SCI_LINELENGTH, line, 0).max(0) as usize;
        if line_bytes > MAX_FRESH_LINE_BYTES {
            return false;
        }
        let previous = line - 1;
        let previous_start = self.send(SCI_POSITIONFROMLINE, previous, 0).max(0) as usize;
        let indent_end = self
            .send(SCI_GETLINEINDENTPOSITION, previous, 0)
            .max(0) as usize;
        if indent_end <= previous_start || indent_end > position {
            return false;
        }
        let indent = self.text_range(previous_start, indent_end);
        if indent.is_empty() {
            return false;
        }
        let value = c_string(&indent);
        self.send(SCI_REPLACESEL, 0, value.as_ptr() as isize);
        true
    }

    /// Current zoom in points relative to the base font size.
    #[must_use]
    pub fn zoom(&self) -> i32 {
        self.send(SCI_GETZOOM, 0, 0) as i32
    }

    /// Scrolls to the given logical line.
    pub fn scroll_to_line(&self, line: u32) {
        let position = self.send(SCI_POSITIONFROMLINE, line as usize, 0).max(0) as usize;
        self.goto_position(position);
    }

    /// First visible line.
    #[must_use]
    pub fn first_visible_line(&self) -> u32 {
        self.send(SCI_GETFIRSTVISIBLELINE, 0, 0).max(0) as u32
    }

    /// Adds find highlights in UTF-8 positions.
    pub fn highlight_ranges(&self, ranges: &[(usize, usize)]) {
        self.send(SCI_SETINDICATORCURRENT, INDICATOR_INDEX, 0);
        for (start, end) in ranges {
            if end > start {
                self.send(
                    SCI_INDICATORFILLRANGE,
                    (*start).min(INT_MAX_POS),
                    (end - start) as isize,
                );
            }
        }
    }

    /// Removes all find highlights.
    pub fn clear_highlights(&self) {
        self.send(SCI_INDICATORCLEARRANGE, 0, self.length() as isize);
    }

    /// Finds the next occurrence.
    ///
    /// Returns UTF-8 byte positions.  When `wrap` is true the search restarts
    /// from the beginning after reaching the end.
    #[must_use]
    pub fn find(
        &self,
        pattern: &str,
        case_sensitive: bool,
        whole_word: bool,
        regex: bool,
        from: usize,
        wrap: bool,
    ) -> Option<(usize, usize)> {
        let mut needle = c_string(pattern);
        let mut flags = SCFIND_NONE;
        if case_sensitive {
            flags |= SCFIND_MATCHCASE;
        }
        if whole_word {
            flags |= SCFIND_WHOLEWORD;
        }
        if regex {
            flags |= SCFIND_REGEXP;
        }
        let length = self.length().min(INT_MAX_POS);
        let start = from.min(length);
        if let Some(hit) = self.find_in(&mut needle, flags, start, length) {
            return Some(hit);
        }
        if wrap && start > 0 {
            return self.find_in(&mut needle, flags, 0, start);
        }
        None
    }

    fn find_in(
        &self,
        needle: &mut [u8],
        flags: usize,
        start: usize,
        end: usize,
    ) -> Option<(usize, usize)> {
        let mut query = TextToFind {
            range: CharRange {
                cp_min: start as i32,
                cp_max: end as i32,
            },
            text: needle.as_ptr(),
            found: CharRange::default(),
        };
        let result = self.send(SCI_FINDTEXT, flags, &mut query as *mut TextToFind as isize);
        if result == NOT_FOUND || query.found.cp_min < 0 {
            return None;
        }
        Some((query.found.cp_min as usize, query.found.cp_max.max(0) as usize))
    }

    /// Replaces the current selection with `text`.
    pub fn replace_selection(&self, text: &str) {
        let value = c_string(text);
        self.send(SCI_REPLACESEL, 0, value.as_ptr() as isize);
    }

    /// Replaces every occurrence in one undo group.
    ///
    /// Returns the number of replacements.  Only literal patterns are
    /// supported; regex replacement stays in the core search service.
    pub fn replace_all(&self, pattern: &str, replacement: &str) -> usize {
        let mut count = 0;
        let mut position = 0;
        self.send(SCI_BEGINUNDOACTION, 0, 0);
        while let Some((start, end)) = self.find(pattern, true, false, false, position, false) {
            self.set_selection(start, end);
            self.replace_selection(replacement);
            position = start + replacement.len();
            count += 1;
            if count >= MAX_REPLACEMENTS {
                break;
            }
        }
        self.send(SCI_ENDUNDOACTION, 0, 0);
        count
    }

    /// Resizes and moves the control.
    pub fn set_bounds(&self, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: the handle is a live child window.
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                width.max(0),
                height.max(0),
                SWP_NOZORDER,
            )
        };
    }

    /// Gives the keyboard focus to the control.
    pub fn focus(&self) {
        // SAFETY: focusing a live child window owned by this thread.
        let _ = unsafe { SetFocus(Some(self.hwnd)) };
    }

    /// Client size in physical pixels.
    #[must_use]
    pub fn client_size(&self) -> (i32, i32) {
        let mut rect = RECT::default();
        // SAFETY: valid out parameter.
        let _ = unsafe { GetClientRect(self.hwnd, &mut rect) };
        (rect.right, rect.bottom)
    }
}

impl Drop for Scintilla {
    fn drop(&mut self) {
        // SAFETY: destroying a window created by this object.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

/// `SCNotification` prefix needed to read the typed character.
///
/// The real structure continues with `modifiers` and further fields, but the
/// notification only needs the leading members to answer this question.
/// `Sci_Position` is `ptrdiff_t`, so it occupies 8 bytes on 64-bit Windows;
/// treating it as `i32` shifts `ch` by four bytes and hides every newline.
#[repr(C)]
struct ScNotificationHead {
    nmhdr: NMHDR,
    _position: isize,
    ch: i32,
}

/// `true` when an `SCN_CHARADDED` notification reports a line break.
///
/// # Safety contract
///
/// `lparam` must be the `WM_NOTIFY` value delivered for `SCN_CHARADDED`.
#[must_use]
pub fn is_newline_char_added(lparam: isize) -> bool {
    if lparam == 0 {
        return false;
    }
    // SAFETY: the caller passes the notification payload of SCN_CHARADDED.
    let notification = unsafe { &*(lparam as *const ScNotificationHead) };
    notification.ch == i32::from(NEWLINE_CHAR)
        || notification.ch == i32::from(CARRIAGE_RETURN_CHAR)
}

fn kind_mode(kind: EolKind) -> i32 {
    match kind {
        EolKind::CrLf => SC_EOL_CRLF,
        EolKind::Cr => SC_EOL_CR,
        EolKind::Lf => SC_EOL_LF,
    }
}

fn c_string(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

static MODULE: OnceLock<Option<isize>> = OnceLock::new();

fn load_module() -> WinResult<isize> {
    MODULE
        .get_or_init(load_module_once)
        .ok_or_else(|| WinError::Scintilla(format!("{DLL_NAME} not found next to the executable")))
}

fn load_module_once() -> Option<isize> {
    // SAFETY: loading a DLL by name; the module is intentionally never freed.
    if let Ok(module) = unsafe { LoadLibraryW(w!("Scintilla.dll")) } {
        return Some(module.0 as isize);
    }
    let directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))?;
    let mut current = Some(directory.as_path());
    let mut levels = 0;
    while let Some(path) = current {
        if levels > ANCESTOR_SEARCH_LEVELS {
            break;
        }
        for candidate in [
            path.join(DLL_NAME),
            path.join("third_party").join(DLL_NAME),
        ] {
            let wide = wide_path(&candidate);
            // SAFETY: `wide` is a valid null-terminated path.
            if let Ok(module) = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) } {
                return Some(module.0 as isize);
            }
        }
        current = path.parent();
        levels += 1;
    }
    None
}

fn wide_path(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Builds a Scintilla colour from RGB channels.
#[must_use]
pub const fn sci_colour(r: u8, g: u8, b: u8) -> usize {
    ((b as usize) << 16) | ((g as usize) << 8) | (r as usize)
}
