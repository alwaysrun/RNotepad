//! Main window, command dispatch and view management.
//!
//! The window is a native Win32 top-level window.  Text is hosted either by
//! Scintilla (editable documents) or by the self-drawn large view; hex and
//! results are always self-drawn / owner-data.  All state lives in [`App`],
//! owned by the window through `GWLP_USERDATA` and released in `WM_NCDESTROY`.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM, GlobalFree,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetDC,
    GetStockObject, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_CHARSET, FF_SWISS, FW_NORMAL,
    HBRUSH, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC,
    VARIABLE_PITCH, WHITE_BRUSH,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW, OFN_EXPLORER, OFN_FILEMUSTEXIST,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, BST_CHECKED, ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX,
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVM_INSERTCOLUMNW, LVM_SETCOLUMNW, LVM_SETITEMCOUNT,
    LVN_GETDISPINFOW, LVN_ITEMACTIVATE, LVS_OWNERDATA, LVS_REPORT, LVS_SINGLESEL, NM_DBLCLK,
    NM_RCLICK, NMLISTVIEW, NMLVDISPINFOW, NMMOUSE, NMHDR, SB_SETPARTS, SB_SETTEXTW,
    SB_SETTIPTEXTW, SBARS_SIZEGRIP, STATUSCLASSNAMEW, WC_LISTVIEWW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus, VK_F3, VK_RETURN};
use windows::Win32::UI::Shell::{
    DefSubclassProc, DragAcceptFiles, DragFinish, DragQueryFileW, SetWindowSubclass, HDROP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BM_GETCHECK, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CheckMenuItem,
    CreateAcceleratorTableW, CreateMenu, CreatePopupMenu, CreateWindowExW, CREATESTRUCTW,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, EnableMenuItem, ES_AUTOHSCROLL, FCONTROL, FVIRTKEY, GetClientRect,
    GetCursorPos, GetMenu, GetMessageW, GetParent, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowLongPtrW, GWLP_USERDATA, HMENU, IDC_ARROW, IDYES, KillTimer,
    LoadCursorW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_YESNO, MENU_ITEM_FLAGS,
    MessageBoxW, MF_BYCOMMAND, MF_CHECKED, MF_DISABLED, MF_ENABLED, MF_GRAYED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, MESSAGEBOX_STYLE, MoveWindow, PostMessageW,
    PostQuitMessage, RegisterClassExW, RegisterClassW, SW_HIDE, SW_SHOW, SendMessageW, SetParent, SetTimer,
    SetWindowLongPtrW, SetWindowTextW, ShowWindow, TrackPopupMenuEx, TranslateAcceleratorW,
    TranslateMessage, TPM_LEFTALIGN, TPM_RETURNCMD, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_COPY,
    WM_CUT, WM_DESTROY, WM_DROPFILES, WM_KEYDOWN, WM_NCCREATE, WM_NCDESTROY, WM_NOTIFY, WM_PAINT,
    WM_PASTE, WM_SIZE, WM_TIMER, WNDCLASSEXW, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW,
    WS_EX_CLIENTEDGE, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SYSMENU, WS_TABSTOP,
    WS_THICKFRAME, WS_VISIBLE, WM_SETFONT, ACCEL, ACCEL_VIRT_FLAGS,
};

use rnotepad_core::bytes::ByteSource;
use rnotepad_core::coord::{CoordMap, RawByteOffset, Utf8Pos};
use rnotepad_core::document::ViewMode;
use rnotepad_core::eol::{EolKind, EolWritePolicy};
use rnotepad_core::encoding::{decode, precheck_unmappable, BomPolicy, Charset};
use rnotepad_core::format::{Diagnostic, FormatOptions, FormatProvider, JsonFormatProvider};
use rnotepad_core::search::{MatchHit, SearchMode, SearchQuery};

use rnotepad_win::dw::{MonoFont, DEFAULT_FONT_POINTS, rgb};
use rnotepad_win::icon::WindowIcons;
use rnotepad_win::notify::ChangeWatcher;
use rnotepad_win::results::ResultsModel;
use rnotepad_win::save::{save_document, SaveRequest, SaveError};
use rnotepad_win::sci::{Scintilla, SCN_UPDATEUI_CODE};
use rnotepad_win::statusbar::{self, StatusInfo};
use rnotepad_win::view::{
    SharedDoc, SharedViewData, ViewData, ViewKind, ViewWindow, VIEW_TAG_HEX, VIEW_TAG_TEXT,
    WM_APP_VIEW_COPY, WM_APP_VIEW_SYNC,
};

use crate::commands::*;
use crate::session::{Session, ViewLayout};
use crate::tasks::SearchJob;

/// Blank lines keep the layout readable and the geometry explicit.
const WINDOW_WIDTH: i32 = 1100;
const WINDOW_HEIGHT: i32 = 760;
const BODY_BACKGROUND: COLORREF = rgb(255, 255, 255);
const FIND_BAR_BACKGROUND: COLORREF = rgb(246, 246, 246);
const MARGIN_CELLS: i32 = 5;
const CHAR_WIDTH_96DPI: i32 = 8;
const MONO_POINTS: i32 = DEFAULT_FONT_POINTS;
const FONT_FACE: &str = "Consolas";
const JSON_INDENT: usize = 2;
const NO_HIT: usize = 0;
const MAX_SCI_HIGHLIGHTS: usize = 2000;
const MAX_REPLACEMENTS: usize = 1_000_000;
const FILE_BUFFER_LEN: usize = 4096;
const FILE_FILTER: &str = "All files\0*.*\0Text files\0*.txt;*.log;*.json;*.csv;*.md\0\0";
const OPEN_TITLE: &str = "Open file";
const SAVE_TITLE: &str = "Save as";
const STATUS_FONT_FACE: &str = "Segoe UI";
const STATUS_FONT_POINTS: i32 = 9;
const POINTS_PER_INCH: i32 = 72;
const ENCODING_TIP: &str = "Double-click to change the encoding";
const EOL_TIP: &str = "Double-click to change the line endings";
const MENU_HEADER_ENCODING: &str = "Detected";
const MENU_HEADER_EOL: &str = "Line endings";
const MENU_REINTERPRET: &str = "Reinterpret (bytes unchanged)";
const MENU_CONVERT: &str = "Convert to (changes bytes on save)";
const MENU_BOM: &str = "BOM";
const MENU_KEEP_AS_IS: &str = "Keep as is (Preserve)";
const MENU_CONVERT_WINDOWS: &str = "Convert to Windows (CRLF)";
const MENU_CONVERT_UNIX: &str = "Convert to Unix (LF)";
const MENU_CONVERT_MAC: &str = "Convert to Classic Mac (CR)";
const MSG_WRITE_BLOCKED: &str = "This action needs an editable Text view";
const STATUS_WIDTHS: [i32; STATUS_FIELD_COUNT] = [64, 190, 110, 84, 120, 96, 104, 0];
/// Scale factor turning a `0.0..=1.0` confidence into a percentage.
const PERCENT_SCALE: f32 = 100.0;
/// Characters contributed per byte by the hex copy format (`AA `).
const HEX_COPY_BYTES_PER_ITEM: usize = 3;
/// Posted by the floating results frame when its client area changed.
const WM_APP_FLOAT_RESIZE: u32 = 0x8000 + 3;
/// Column headers and widths of the search-hit table.
const RESULT_COLUMNS: [(&str, i32); 3] = [("Line", 60), ("Column", 70), ("Offset", 100)];
/// Column headers and widths of the JSON problem table.
const PROBLEM_COLUMNS: [(&str, i32); 3] = [("Line", 60), ("Column", 70), ("Message", 300)];
/// Window class of the floating results frame.
const FLOAT_CLASS_NAME: &str = "RNotepadFloatFrame";
/// Caption of the floating results frame.
const FLOAT_TITLE: &str = "Results";
/// Menu items of the results context menu, kept above zero as Win32 requires.
const ID_RESULTS_SHOW_TEXT: u32 = 3001;
const ID_RESULTS_SHOW_HEX: u32 = 3002;
/// Charset commands and their menu labels, shared by both menus.
const CHARSET_MENU_ITEMS: [(u32, &str); 9] = [
    (ID_ENC_UTF8, "UTF-8"),
    (ID_ENC_UTF16LE, "UTF-16 LE"),
    (ID_ENC_UTF16BE, "UTF-16 BE"),
    (ID_ENC_GBK, "GBK"),
    (ID_ENC_GB18030, "GB18030"),
    (ID_ENC_BIG5, "Big5"),
    (ID_ENC_SHIFT_JIS, "Shift-JIS"),
    (ID_ENC_WINDOWS_1252, "Windows-1252"),
    (ID_ENC_LATIN1, "Latin-1"),
];
/// Byte-conversion commands and their menu labels.
const CONVERT_MENU_ITEMS: [(u32, &str); 9] = [
    (ID_CONVERT_UTF8, "UTF-8"),
    (ID_CONVERT_UTF16LE, "UTF-16 LE"),
    (ID_CONVERT_UTF16BE, "UTF-16 BE"),
    (ID_CONVERT_GBK, "GBK"),
    (ID_CONVERT_GB18030, "GB18030"),
    (ID_CONVERT_BIG5, "Big5"),
    (ID_CONVERT_SHIFT_JIS, "Shift-JIS"),
    (ID_CONVERT_WINDOWS_1252, "Windows-1252"),
    (ID_CONVERT_LATIN1, "Latin-1"),
];
const ACCELERATORS: [ACCEL; 5] = [
    control_accel(b'O', ID_FILE_OPEN),
    control_accel(b'S', ID_FILE_SAVE),
    control_accel(b'F', ID_EDIT_FIND),
    control_accel(b'N', ID_FILE_NEW),
    ACCEL {
        fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0),
        key: VK_F3.0,
        cmd: ID_EDIT_FIND_NEXT as u16,
    },
];

const fn control_accel(key: u8, command: u32) -> ACCEL {
    ACCEL {
        fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | FCONTROL.0),
        key: key as u16,
        cmd: command as u16,
    }
}

/// Text view backing store.
enum TextHandle {
    Scintilla(Scintilla),
    Large(ViewWindow),
}

struct FindBar {
    edit: HWND,
    replace: HWND,
    next: HWND,
    all: HWND,
    replace_all: HWND,
    case: HWND,
    close: HWND,
    cancel: HWND,
    visible: bool,
}

/// Which table the shared owner-data list currently shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultsMode {
    Hits,
    Problems,
}

struct App {
    hwnd: HWND,
    session: Session,
    font: Option<MonoFont>,
    text: Option<TextHandle>,
    text_data: Option<SharedViewData>,
    hex: Option<ViewWindow>,
    results_list: Option<HWND>,
    /// Set while the results list is reparented into a floating frame.
    float_frame: Option<HWND>,
    results_mode: ResultsMode,
    /// Diagnostics from the last failed JSON format or validation.
    problems: Vec<Diagnostic>,
    status: Option<HWND>,
    status_font: Option<HFONT>,
    icons: Option<WindowIcons>,
    find: Option<FindBar>,
    job: Option<SearchJob>,
    watcher: Option<ChangeWatcher>,
    /// Guards against text/hex link feedback loops.
    syncing: bool,
    /// Keeps a large document scrolled to its last line.
    follow_tail: bool,
    status_message: String,
    editing: bool,
    fields_snapshot: [String; STATUS_FIELD_COUNT],
    disp_text: Vec<u16>,
    hit_cursor: usize,
}

impl Default for App {
    fn default() -> Self {
        Self {
            hwnd: HWND::default(),
            session: Session::default(),
            font: None,
            text: None,
            text_data: None,
            hex: None,
            results_list: None,
            float_frame: None,
            results_mode: ResultsMode::Hits,
            problems: Vec::new(),
            status: None,
            status_font: None,
            icons: None,
            find: None,
            job: None,
            watcher: None,
            syncing: false,
            follow_tail: false,
            status_message: String::new(),
            editing: false,
            fields_snapshot: std::array::from_fn(|_| String::new()),
            disp_text: Vec::new(),
            hit_cursor: NO_HIT,
        }
    }
}

thread_local! {
    static IN_APP: Cell<bool> = const { Cell::new(false) };
}

/// Runs one message with exclusive access to [`App`].
///
/// Message boxes and modal dialogs pump messages; the guard drops re-entrant
/// `WM_TIMER` / `WM_COMMAND` messages so a single `&mut App` is alive at a
/// time and no aliasing can occur.
fn with_app(hwnd: HWND, visit: impl FnOnce(&mut App)) {
    if IN_APP.with(Cell::get) {
        return;
    }
    // SAFETY: the pointer is owned by the window and cleared on destruction.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut App;
    if pointer.is_null() {
        return;
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            IN_APP.with(|flag| flag.set(false));
        }
    }
    IN_APP.with(|flag| flag.set(true));
    let _reset = Reset;
    // SAFETY: the window owns the allocation and the re-entrancy guard
    // guarantees this is the only live reference.
    visit(unsafe { &mut *pointer });
}

impl App {
    fn initialise(&mut self) {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        match MonoFont::new(dpi) {
            Ok(mut font) => {
                let hdc = unsafe { GetDC(Some(self.hwnd)) };
                if hdc.is_invalid() {
                    self.status_message = "GetDC failed while measuring the font".to_owned();
                } else {
                    let measured = font.measure(hdc);
                    unsafe {
                        let _ = ReleaseDC(Some(self.hwnd), hdc);
                    }
                    if let Err(err) = measured {
                        self.status_message = format!("Font measurement failed: {err}");
                    }
                }
                self.font = Some(font);
            }
            Err(err) => {
                self.status_message = format!("Font creation failed: {err}");
            }
        }
        self.ensure_find_bar();
        self.ensure_status_bar();
        // The class already carries the icons; WM_SETICON also refreshes the
        // taskbar entry after the window becomes visible.
        if let Some(icons) = self.icons.as_ref() {
            icons.apply(self.hwnd);
        }
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                TIMER_REFRESH,
                TIMER_INTERVAL_MS,
                None,
            );
        }
        self.layout();
        self.update_status();
    }

    fn ensure_find_bar(&mut self) {
        if self.find.is_some() {
            return;
        }
        let edit = match create_child(self.hwnd, w!("EDIT"), EDIT_STYLE, ID_FIND_EDIT) {
            Ok(hwnd) => hwnd,
            Err(err) => {
                self.status_message = err;
                return;
            }
        };
        let replace = match create_child(self.hwnd, w!("EDIT"), EDIT_STYLE, ID_FIND_REPLACE_EDIT) {
            Ok(hwnd) => hwnd,
            Err(err) => {
                self.status_message = err;
                return;
            }
        };
        let next = create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_NEXT_BUTTON);
        let all = create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_ALL_BUTTON);
        let replace_all =
            create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_REPLACE_BUTTON);
        let case = create_child(self.hwnd, w!("BUTTON"), CHECKBOX_STYLE, ID_FIND_CASE);
        let close = create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_CLOSE);
        let cancel = create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_CANCEL);
        let (
            Ok(next),
            Ok(all),
            Ok(replace_all),
            Ok(case),
            Ok(close),
            Ok(cancel),
        ) = (next, all, replace_all, case, close, cancel)
        else {
            self.status_message = "Creating find-bar controls failed".to_owned();
            return;
        };
        set_text(edit, "");
        set_text(replace, "");
        set_text(next, "Next");
        set_text(all, "All");
        set_text(replace_all, "Replace");
        set_text(case, "Case");
        set_text(close, "X");
        set_text(cancel, "Stop");
        unsafe {
            let _ = SetWindowSubclass(edit, Some(find_edit_proc), 1, 0);
        }
        let bar = FindBar {
            edit,
            replace,
            next,
            all,
            replace_all,
            case,
            close,
            cancel,
            visible: false,
        };
        show_bar(&bar, false);
        self.find = Some(bar);
    }

    fn ensure_results_list(&mut self) -> Option<HWND> {
        if let Some(list) = self.results_list {
            return Some(list);
        }
        let instance = match unsafe { GetModuleHandleW(None) } {
            Ok(module) => module,
            Err(err) => {
                self.status_message = format!("GetModuleHandleW failed: {err}");
                return None;
            }
        };
        // SAFETY: the common controls were initialised and the parent window
        // is live; the result is stored for the lifetime of the window.
        let list = unsafe {
            CreateWindowExW(
                Default::default(),
                WC_LISTVIEWW,
                w!(""),
                LIST_VIEW_STYLE,
                0,
                0,
                1,
                1,
                Some(self.hwnd),
                None,
                Some(HINSTANCE(instance.0)),
                None,
            )
        };
        let list = match list {
            Ok(hwnd) => hwnd,
            Err(err) => {
                self.status_message = format!("Creating the results list failed: {err}");
                return None;
            }
        };
        for (index, (title, width)) in RESULT_COLUMNS.iter().enumerate() {
            let mut wide = to_wide(title);
            let mut column = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: *width,
                pszText: PWSTR(wide.as_mut_ptr()),
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    list,
                    LVM_INSERTCOLUMNW,
                    Some(WPARAM(index)),
                    Some(LPARAM(&mut column as *mut LVCOLUMNW as isize)),
                );
            }
        }
        show_window(list, false);
        self.results_list = Some(list);
        self.results_mode = ResultsMode::Hits;
        Some(list)
    }

    /// Rewrites the list headers for the active table.
    fn configure_results_columns(&self, list: HWND, mode: ResultsMode) {
        let columns = match mode {
            ResultsMode::Hits => &RESULT_COLUMNS,
            ResultsMode::Problems => &PROBLEM_COLUMNS,
        };
        for (index, (title, width)) in columns.iter().enumerate() {
            let mut wide = to_wide(title);
            let mut column = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: *width,
                pszText: PWSTR(wide.as_mut_ptr()),
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    list,
                    LVM_SETCOLUMNW,
                    Some(WPARAM(index)),
                    Some(LPARAM(&mut column as *mut LVCOLUMNW as isize)),
                );
            }
        }
    }

    /// Switches the shared list between search hits and JSON problems.
    fn set_results_mode(&mut self, mode: ResultsMode) {
        if self.results_mode == mode {
            return;
        }
        self.results_mode = mode;
        if let Some(list) = self.results_list {
            self.configure_results_columns(list, mode);
        }
        self.refresh_results_count();
    }

    /// Drops diagnostics and returns the list to the search-hit layout.
    fn reset_results_table(&mut self) {
        self.problems.clear();
        self.results_mode = ResultsMode::Hits;
        if let Some(list) = self.results_list {
            self.configure_results_columns(list, ResultsMode::Hits);
        }
    }

    /// Number of rows in the active table.
    fn visible_row_count(&self) -> usize {
        match self.results_mode {
            ResultsMode::Hits => self.session.results.len(),
            ResultsMode::Problems => self.problems.len(),
        }
    }

    fn ensure_status_bar(&mut self) -> Option<HWND> {
        if let Some(status) = self.status {
            return Some(status);
        }
        let instance = match unsafe { GetModuleHandleW(None) } {
            Ok(module) => module,
            Err(err) => {
                self.status_message = format!("GetModuleHandleW failed: {err}");
                return None;
            }
        };
        // SAFETY: the common controls were initialised and the parent window
        // is live; the status bar owns itself until the parent is destroyed.
        let style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | SBARS_SIZEGRIP);
        let created = unsafe {
            CreateWindowExW(
                Default::default(),
                STATUSCLASSNAMEW,
                w!(""),
                style,
                0,
                0,
                1,
                1,
                Some(self.hwnd),
                None,
                Some(HINSTANCE(instance.0)),
                None,
            )
        };
        match created {
            Ok(hwnd) => {
                // The default shell font lacks U+25BE, so the actionable-field
                // marker would render as a box; use the Windows UI font.
                let dpi = unsafe { GetDpiForWindow(self.hwnd) };
                let height = -(STATUS_FONT_POINTS * dpi as i32 / POINTS_PER_INCH);
                let face = to_wide(STATUS_FONT_FACE);
                let font = unsafe {
                    CreateFontW(
                        height,
                        0,
                        0,
                        0,
                        FW_NORMAL.0 as i32,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET,
                        OUT_DEFAULT_PRECIS,
                        CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY,
                        (VARIABLE_PITCH.0 as u32) | (FF_SWISS.0 as u32),
                        PCWSTR(face.as_ptr()),
                    )
                };
                self.status_font = Some(font);
                unsafe {
                    let _ = SendMessageW(
                        hwnd,
                        WM_SETFONT,
                        Some(WPARAM(font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
                self.status = Some(hwnd);
                Some(hwnd)
            }
            Err(err) => {
                self.status_message = format!("Creating the status bar failed: {err}");
                None
            }
        }
    }

    /// Height of the status bar in physical pixels.
    fn status_height(&self) -> i32 {
        let Some(status) = self.status else {
            return STATUS_HEIGHT;
        };
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(status, &mut rect) }.is_ok() {
            (rect.bottom - rect.top).max(0)
        } else {
            STATUS_HEIGHT
        }
    }

    /// Recomputes the status bar partitions for the given client width.
    ///
    /// The last partition uses `-1`, which the control extends to its right
    /// edge; fixed widths are clamped so the parts stay ordered.
    fn apply_status_parts(&self, width: i32) {
        let Some(status) = self.status else {
            return;
        };
        let mut edges = [0i32; STATUS_FIELD_COUNT];
        let mut left = 0;
        let limit = width.max(1);
        for (index, configured) in STATUS_WIDTHS
            .iter()
            .take(STATUS_FIELD_COUNT.saturating_sub(1))
            .copied()
            .enumerate()
        {
            left = (left + configured.max(0)).min(limit);
            edges[index] = left;
        }
        edges[STATUS_FIELD_COUNT - 1] = -1;
        unsafe {
            let _ = SendMessageW(
                status,
                SB_SETPARTS,
                Some(WPARAM(STATUS_FIELD_COUNT)),
                Some(LPARAM(edges.as_ptr() as isize)),
            );
        }
        self.set_status_tip(statusbar::FIELD_ENCODING, ENCODING_TIP);
        self.set_status_tip(statusbar::FIELD_EOL, EOL_TIP);
    }

    fn set_status_tip(&self, index: usize, text: &str) {
        let Some(status) = self.status else {
            return;
        };
        let mut wide = to_wide(text);
        unsafe {
            let _ = SendMessageW(
                status,
                SB_SETTIPTEXTW,
                Some(WPARAM(index)),
                Some(LPARAM(wide.as_mut_ptr() as isize)),
            );
        }
    }

    fn set_status_part(&self, index: usize, text: &str) {
        let Some(status) = self.status else {
            return;
        };
        let mut wide = to_wide(text);
        unsafe {
            let _ = SendMessageW(
                status,
                SB_SETTEXTW,
                Some(WPARAM(index)),
                Some(LPARAM(wide.as_mut_ptr() as isize)),
            );
        }
    }

    fn layout(&mut self) {
        let mut client = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut client) }.is_err() {
            return;
        }
        let width = client.right.max(0);
        let height = client.bottom.max(0);
        if self.ensure_status_bar().is_some() {
            if let Some(status) = self.status {
                // The status bar sizes itself to the bottom of its parent.
                unsafe {
                    let _ = SendMessageW(status, WM_SIZE, Some(WPARAM(0)), Some(LPARAM(0)));
                }
            }
            self.apply_status_parts(width);
        }
        let body_height = (height - self.status_height()).max(0);
        if !self.session.has_document() {
            self.hide_text();
            self.hide_hex();
            self.hide_results();
            self.layout_find_bar(body_height, width);
            self.paint_frame();
            return;
        }
        let split = ((width as f32) * TEXT_SPLIT_RATIO) as i32;
        match self.session.layout {
            ViewLayout::Text => {
                self.position_text(0, 0, width, body_height);
                self.hide_hex();
                self.hide_results();
            }
            ViewLayout::Hex => {
                if self.ensure_hex().is_some() {
                    self.position_hex(0, 0, width, body_height);
                }
                self.hide_text();
                self.hide_results();
            }
            ViewLayout::TextAndHex => {
                let left = (split - SPLIT_GAP / 2).max(0);
                let right = (width - split - SPLIT_GAP / 2).max(0);
                self.position_text(0, 0, left, body_height);
                if self.ensure_hex().is_some() {
                    self.position_hex(split + SPLIT_GAP / 2, 0, right, body_height);
                }
                self.hide_results();
            }
            ViewLayout::TextAndResults => {
                if self.float_frame.is_some() {
                    self.position_text(0, 0, width, body_height);
                    self.hide_hex();
                } else {
                    let right = RESULTS_WIDTH.min((width / 2).max(0));
                    let left = (width - right - SPLIT_GAP).max(0);
                    self.position_text(0, 0, left, body_height);
                    self.hide_hex();
                    self.position_results(left + SPLIT_GAP, 0, right, body_height);
                }
            }
        }
        self.layout_find_bar(body_height, width);
        self.paint_frame();
    }

    fn layout_find_bar(&mut self, body_height: i32, width: i32) {
        let Some(bar) = self.find.as_ref() else {
            return;
        };
        if !bar.visible {
            return;
        }
        let row_one = body_height + FIND_ROW_GAP;
        let row_two = row_one + FIND_ROW_HEIGHT + FIND_ROW_GAP;
        let close_width = 28;
        let button_width = 72;
        let case_width = 56;
        let available = (width
            - button_width * 3
            - case_width
            - close_width
            - FIND_ROW_GAP * 5)
            .max(0);
        let edit_width = available;
        move_control(bar.edit, FIND_ROW_GAP, row_one, edit_width, FIND_ROW_HEIGHT);
        let x = FIND_ROW_GAP + edit_width + FIND_ROW_GAP;
        move_control(bar.next, x, row_one, button_width, FIND_ROW_HEIGHT);
        let x = x + button_width + FIND_ROW_GAP;
        move_control(bar.all, x, row_one, button_width, FIND_ROW_HEIGHT);
        let x = x + button_width + FIND_ROW_GAP;
        move_control(bar.case, x, row_one, case_width, FIND_ROW_HEIGHT);
        let x = x + case_width + FIND_ROW_GAP;
        move_control(bar.close, x, row_one, close_width, FIND_ROW_HEIGHT);
        let replace_width = (width - button_width - FIND_CANCEL_WIDTH - FIND_ROW_GAP * 5).max(0);
        move_control(bar.replace, FIND_ROW_GAP, row_two, replace_width, FIND_ROW_HEIGHT);
        let x = FIND_ROW_GAP + replace_width + FIND_ROW_GAP;
        move_control(bar.replace_all, x, row_two, button_width, FIND_ROW_HEIGHT);
        let x = x + button_width + FIND_ROW_GAP;
        move_control(bar.cancel, x, row_two, FIND_CANCEL_WIDTH, FIND_ROW_HEIGHT);
    }

    fn position_text(&self, x: i32, y: i32, width: i32, height: i32) {
        match &self.text {
            Some(TextHandle::Scintilla(control)) => {
                control.set_bounds(x, y, width, height);
                show_window(control.hwnd(), true);
            }
            Some(TextHandle::Large(view)) => {
                view.set_rect(x, y, width, height);
                show_window(view.hwnd(), true);
            }
            None => {}
        }
    }

    fn position_hex(&mut self, x: i32, y: i32, width: i32, height: i32) {
        if let Some(view) = &self.hex {
            view.set_rect(x, y, width, height);
            show_window(view.hwnd(), true);
        }
    }

    fn position_results(&mut self, x: i32, y: i32, width: i32, height: i32) {
        if self.float_frame.is_some() {
            return;
        }
        if let Some(list) = self.ensure_results_list() {
            unsafe {
                let _ = MoveWindow(list, x, y, width.max(0), height.max(0), true);
            }
            show_window(list, true);
        }
    }

    fn hide_text(&mut self) {
        match &self.text {
            Some(TextHandle::Scintilla(control)) => show_window(control.hwnd(), false),
            Some(TextHandle::Large(view)) => show_window(view.hwnd(), false),
            None => {}
        }
    }

    fn hide_hex(&mut self) {
        if let Some(view) = &self.hex {
            show_window(view.hwnd(), false);
        }
    }

    fn hide_results(&mut self) {
        if self.float_frame.is_some() {
            return;
        }
        if let Some(list) = self.results_list {
            show_window(list, false);
        }
    }

    fn ensure_hex(&mut self) -> Option<HWND> {
        if let Some(view) = &self.hex {
            return Some(view.hwnd());
        }
        let data = Rc::new(RefCell::new(ViewData::default()));
        if let Some(shared) = &self.session.doc {
            data.borrow_mut().doc = Some(shared.clone());
        }
        match ViewWindow::create(self.hwnd, ViewKind::Hex, data) {
            Ok(view) => {
                let hwnd = view.hwnd();
                self.hex = Some(view);
                Some(hwnd)
            }
            Err(err) => {
                self.status_message = format!("Creating the hex view failed: {err}");
                None
            }
        }
    }

    fn rebuild_text_view(&mut self, shared: &SharedDoc) {
        self.text = None;
        self.text_data = None;
        let (kind, text_available, text, eol) = {
            let guard = shared
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                guard.kind,
                guard.text_available,
                guard.decoded_text(),
                guard.eol.dominant,
            )
        };
        if !text_available {
            return;
        }
        if kind.is_editable() {
            match Scintilla::create(self.hwnd, RECT::default()) {
                Ok(control) => {
                    let margin = MARGIN_CELLS * CHAR_WIDTH_96DPI;
                    control.apply_style(FONT_FACE, MONO_POINTS, margin);
                    let document_text = text.unwrap_or_default();
                    control.set_text(&document_text);
                    control.set_read_only(true);
                    control.set_eol(eol);
                    tracing::info!(
                        text_bytes = document_text.len(),
                        buffer_bytes = control.length(),
                        "Scintilla text view ready"
                    );
                    self.text = Some(TextHandle::Scintilla(control));
                    return;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "Scintilla unavailable, using the large view");
                    self.status_message = format!("Scintilla unavailable, read-only fallback: {err}");
                }
            }
        }
        let data = Rc::new(RefCell::new(ViewData {
            doc: Some(shared.clone()),
            ..Default::default()
        }));
        match ViewWindow::create(self.hwnd, ViewKind::Text, data.clone()) {
            Ok(view) => {
                self.text = Some(TextHandle::Large(view));
                self.text_data = Some(data);
            }
            Err(err) => {
                self.status_message = format!("Creating the text view failed: {err}");
            }
        }
    }

    fn attach_document(&mut self, shared: &SharedDoc) {
        if let Some(data) = &self.text_data {
            let mut view = data.borrow_mut();
            view.doc = Some(shared.clone());
            view.first_line = 0;
            view.first_row = 0;
            view.highlights.clear();
            view.sel = None;
        }
        if let Some(view) = &self.hex {
            let data = view.data();
            let mut state = data.borrow_mut();
            state.doc = Some(shared.clone());
            state.first_line = 0;
            state.first_row = 0;
            state.highlights.clear();
            state.sel = None;
        }
    }

    fn refresh_title(&self) {
        let name = self
            .session
            .read(|doc| doc.path.clone())
            .flatten()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Untitled".to_owned());
        let title = format!("{name} - RNotepad");
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(to_wide(&title).as_ptr()));
        }
    }

    fn new_document(&mut self) {
        self.cancel_search();
        self.watcher = None;
        self.hex = None;
        self.editing = false;
        self.follow_tail = false;
        self.reset_results_table();
        let shared = self.session.new_blank();
        self.rebuild_text_view(&shared);
        self.attach_document(&shared);
        self.session.layout = ViewLayout::Text;
        self.status_message = "New document".to_owned();
        self.refresh_title();
        self.hit_cursor = NO_HIT;
        if let Some(list) = self.results_list {
            unsafe {
                SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0)));
            }
        }
        self.layout();
        self.update_status();
    }

    fn open_document(&mut self, path: PathBuf) {
        self.cancel_search();
        match self.session.open(&path) {
            Ok(shared) => {
                self.editing = false;
                self.hex = None;
                self.follow_tail = false;
                self.reset_results_table();
                self.watcher = match ChangeWatcher::new(&path) {
                    Ok(watcher) => Some(watcher),
                    Err(err) => {
                        tracing::warn!(error = %err, "file change watcher unavailable");
                        None
                    }
                };
                self.rebuild_text_view(&shared);
                self.attach_document(&shared);
                let text_available = self
                    .session
                    .read(|doc| doc.text_available)
                    .unwrap_or(false);
                self.session.layout = if text_available {
                    ViewLayout::Text
                } else {
                    ViewLayout::Hex
                };
                self.hit_cursor = NO_HIT;
                let (kind, charset, confidence) = self
                    .session
                    .read(|doc| (doc.kind, doc.encoding.charset, doc.confidence))
                    .unwrap_or((rnotepad_core::document::DocumentKind::LargeReadOnly, Charset::Utf8, 0.0));
                tracing::info!(
                    path = %path.display(),
                    kind = ?kind,
                    charset = charset.display_name(),
                    confidence,
                    "document opened"
                );
                self.status_message = format!("Opened {}", path.display());
                self.refresh_title();
                self.layout();
                self.update_status();
            }
            Err(err) => {
                tracing::error!(path = %path.display(), error = %err, "open failed");
                self.show_message(MB_ICONERROR, "Open failed", &format!("{err}"));
            }
        }
    }

    fn close_document(&mut self) {
        if self.confirm_discard() != Some(true) {
            return;
        }
        self.cancel_search();
        self.watcher = None;
        self.text = None;
        self.text_data = None;
        self.hex = None;
        self.editing = false;
        self.follow_tail = false;
        self.session.close();
        self.reset_results_table();
        self.hit_cursor = NO_HIT;
        if let Some(list) = self.results_list {
            unsafe {
                SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0)));
            }
        }
        self.status_message = "Document closed".to_owned();
        self.refresh_title();
        self.layout();
        self.update_status();
    }

    fn save(&mut self, target: Option<PathBuf>) {
        self.cancel_search();
        let text = match &self.text {
            Some(TextHandle::Scintilla(control)) => Some(control.get_text()),
            _ => None,
        };
        let Some(text) = text else {
            self.status_message = "Editing requires the Scintilla text view".to_owned();
            self.update_status();
            return;
        };
        let current = self.session.read(|doc| doc.path.clone()).flatten();
        let path = match target.or(current) {
            Some(path) => path,
            None => match pick_file(self.hwnd, true, None) {
                Some(path) => path,
                None => return,
            },
        };
        let Some(shared) = self.session.doc.clone() else {
            self.status_message = "No document".to_owned();
            return;
        };
        let request = SaveRequest {
            text: &text,
            target: Some(&path),
            backup: false,
            allow_external_overwrite: false,
        };
        let result = {
            let mut guard = shared
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            save_document(&mut guard, &request).map(|()| {
                guard.write_policy.eol.resolve(guard.eol.dominant)
            })
        };
        match result {
            Ok(eol) => {
                if let Some(TextHandle::Scintilla(control)) = &self.text {
                    control.set_eol(eol);
                    control.mark_saved();
                }
                self.status_message = format!("Saved {}", path.display());
                self.refresh_title();
            }
            Err(err) => {
                self.show_message(MB_ICONERROR, "Save failed", &describe_save_error(&err));
            }
        }
        self.update_status();
    }

    fn save_as(&mut self) {
        let initial = self.session.read(|doc| doc.path.clone()).flatten();
        if let Some(path) = pick_file(self.hwnd, true, initial.as_deref()) {
            self.save(Some(path));
        }
    }

    fn set_layout(&mut self, layout: ViewLayout) {
        if layout != ViewLayout::Hex {
            let text_available = self
                .session
                .read(|doc| doc.text_available)
                .unwrap_or(true);
            if !text_available {
                self.set_layout(ViewLayout::Hex);
                return;
            }
        }
        self.cancel_search();
        let view = if layout == ViewLayout::Hex {
            ViewMode::Hex
        } else {
            ViewMode::Text
        };
        self.session.write(|doc| doc.view = view);
        self.session.layout = layout;
        if layout == ViewLayout::TextAndResults {
            let _ = self.ensure_results_list();
        }
        self.layout();
        self.update_status();
    }

    fn toggle_editing(&mut self) {
        let enabled = !self.editing;
        let available = if let Some(TextHandle::Scintilla(control)) = &self.text {
            control.set_read_only(!enabled);
            if enabled {
                control.focus();
            }
            true
        } else {
            false
        };
        if !available {
            self.status_message = "Editing is available only in the Scintilla text view".to_owned();
            self.update_status();
            return;
        }
        self.editing = enabled;
        self.status_message = if enabled {
            "Editing enabled".to_owned()
        } else {
            "Read-only".to_owned()
        };
        self.update_status();
    }

    fn reinterpret(&mut self, charset: Charset) {
        self.cancel_search();
        let Some(shared) = self.session.doc.clone() else {
            return;
        };
        let result = {
            let mut guard = shared
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.reinterpret(charset)
        };
        match result {
            Ok(()) => {
                self.rebuild_text_view(&shared);
                self.attach_document(&shared);
                let view = self
                    .session
                    .read(|doc| doc.view)
                    .unwrap_or(ViewMode::Text);
                if view == ViewMode::Hex {
                    self.session.layout = ViewLayout::Hex;
                }
                if let Some(view) = &self.hex {
                    view.refresh_scroll();
                    view.invalidate();
                }
                self.status_message = format!("Encoding: {}", charset.display_name());
                self.layout();
            }
            Err(err) => {
                self.show_message(MB_ICONERROR, "Reinterpret failed", &format!("{err}"));
            }
        }
        self.update_status();
    }

    /// `true` when the active document accepts byte-changing commands.
    ///
    /// This is the second line of defence required by the specification: the
    /// menus grey out unavailable items, but a stale menu or an accelerator
    /// must not be able to modify a read-only or hex document.
    fn writable_text(&self) -> bool {
        let state = self
            .session
            .read(|doc| (doc.kind, doc.view, doc.text_available))
            .unwrap_or((
                rnotepad_core::document::DocumentKind::LargeReadOnly,
                ViewMode::Hex,
                false,
            ));
        let (kind, view, text_available) = state;
        kind.is_editable()
            && view == ViewMode::Text
            && text_available
            && matches!(self.text, Some(TextHandle::Scintilla(_)))
    }

    fn write_blocked(&mut self, operation: &str) {
        tracing::warn!(operation, "write command rejected: document is read-only or in hex mode");
        self.status_message = format!("{operation}: {MSG_WRITE_BLOCKED}");
        self.update_status();
    }

    fn set_bom(&mut self, policy: BomPolicy) {
        if !self.writable_text() {
            self.write_blocked("BOM");
            return;
        }
        self.cancel_search();
        self.session.write(|doc| {
            doc.write_policy.bom = policy;
            doc.mark_dirty();
        });
        self.status_message = "BOM policy updated".to_owned();
        self.update_status();
    }

    /// Changes the write policy without touching the buffer.
    fn set_eol_policy(&mut self, policy: EolWritePolicy) {
        self.session.write(|doc| doc.write_policy.eol = policy);
        self.status_message = format!("EOL policy: {}", policy.display_name());
        self.update_status();
    }

    fn set_eol(&mut self, kind: EolKind) {
        if !self.writable_text() {
            self.write_blocked("EOL conversion");
            return;
        }
        let converted = if let Some(TextHandle::Scintilla(control)) = &self.text {
            control.convert_eols(kind);
            true
        } else {
            false
        };
        if !converted {
            self.write_blocked("EOL conversion");
            return;
        }
        self.cancel_search();
        let policy = match kind {
            EolKind::CrLf => EolWritePolicy::CrLf,
            EolKind::Lf => EolWritePolicy::Lf,
            EolKind::Cr => EolWritePolicy::Cr,
        };
        self.session.write(|doc| {
            doc.write_policy.eol = policy;
            doc.mark_dirty();
        });
        self.status_message = format!("EOL: {}", kind.display_name());
        self.update_status();
    }

    /// Switches the written charset without changing the current buffer.
    ///
    /// The buffer stays UTF-8; the conversion is applied when the document is
    /// saved.  Characters that cannot be mapped are rejected up front so a
    /// save can never silently lose data.
    fn convert_to(&mut self, charset: Charset) {
        if !self.writable_text() {
            self.write_blocked("Convert encoding");
            return;
        }
        let Some(text) = self
            .text
            .as_ref()
            .and_then(|handle| match handle {
                TextHandle::Scintilla(control) => Some(control.get_text()),
                TextHandle::Large(_) => None,
            })
        else {
            self.write_blocked("Convert encoding");
            return;
        };
        let unmappable = precheck_unmappable(&text, charset);
        if !unmappable.is_empty() {
            tracing::warn!(
                charset = charset.display_name(),
                count = unmappable.len(),
                "conversion rejected: unmappable characters"
            );
            self.show_message(
                MB_ICONERROR,
                "Cannot convert",
                &format!(
                    "{} characters cannot be encoded as {}. The document was not changed.",
                    unmappable.len(),
                    charset.display_name()
                ),
            );
            self.status_message = "Conversion rejected".to_owned();
            self.update_status();
            return;
        }
        self.cancel_search();
        self.session.write(|doc| {
            doc.write_policy.charset = charset;
            doc.mark_dirty();
        });
        self.status_message = format!("Will save as {} (on save)", charset.display_name());
        self.update_status();
    }

    /// Opens the encoding menu at the cursor.
    fn open_encoding_menu(&mut self) {
        tracing::info!("encoding menu opened");
        let Some(menu) = create_menu_or_report() else {
            return;
        };
        let state = self.session.read(|doc| {
            (
                doc.encoding.charset,
                doc.encoding.bom,
                doc.write_policy.bom,
                doc.write_policy.charset,
                doc.confidence,
            )
        });
        let Some((charset, bom, bom_policy, target_charset, confidence)) = state else {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            return;
        };
        let writable = self.writable_text();
        let header = format!(
            "{MENU_HEADER_ENCODING}: {} · {} ({:.0}%)",
            charset.display_name(),
            bom.display_name(),
            f32::from(confidence) * PERCENT_SCALE
        );
        unsafe {
            append(menu, MF_STRING | MF_DISABLED, 0, &header);
            append(menu, MF_SEPARATOR, 0, "");
            append(menu, MF_STRING | MF_DISABLED, 0, MENU_REINTERPRET);
            for (id, name) in CHARSET_MENU_ITEMS {
                let flags = if name == charset.display_name() {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                append(menu, flags, id, name);
            }
            append(menu, MF_SEPARATOR, 0, "");
            append(menu, MF_STRING | MF_DISABLED, 0, MENU_CONVERT);
            for (id, name) in CONVERT_MENU_ITEMS {
                let mut flags = MF_STRING;
                if !writable {
                    flags = flags | MF_GRAYED;
                }
                if name == target_charset.display_name() {
                    flags = flags | MF_CHECKED;
                }
                append(menu, flags, id, name);
            }
            append(menu, MF_SEPARATOR, 0, "");
            append(menu, MF_STRING | MF_DISABLED, 0, MENU_BOM);
            let bom_flags = |policy: BomPolicy, current: BomPolicy| {
                let mut flags = MF_STRING;
                if !writable {
                    flags = flags | MF_GRAYED;
                }
                if policy == current {
                    flags = flags | MF_CHECKED;
                }
                flags
            };
            append(menu, bom_flags(BomPolicy::Keep, bom_policy), ID_BOM_KEEP, "Keep");
            append(menu, bom_flags(BomPolicy::Add, bom_policy), ID_BOM_ADD, "Add");
            append(menu, bom_flags(BomPolicy::Strip, bom_policy), ID_BOM_STRIP, "Strip");
        }
        let command = self.track_menu(menu);
        if command != 0 {
            self.command(command);
        }
    }

    /// Opens the end-of-line menu at the cursor.
    fn open_eol_menu(&mut self) {
        tracing::info!("eol menu opened");
        let Some(menu) = create_menu_or_report() else {
            return;
        };
        let state = self.session.read(|doc| {
            (doc.eol, doc.write_policy.eol)
        });
        let Some((eol, policy)) = state else {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            return;
        };
        let writable = self.writable_text();
        let mixed = if eol.mixed {
            format!(" · Mixed {}:{}", eol.crlf, eol.lf)
        } else {
            String::new()
        };
        let header = format!(
            "{MENU_HEADER_EOL}: CRLF {} / LF {} / CR {}{mixed}",
            eol.crlf, eol.lf, eol.cr
        );
        unsafe {
            append(menu, MF_STRING | MF_DISABLED, 0, &header);
            append(menu, MF_SEPARATOR, 0, "");
            let keep = if policy == EolWritePolicy::Preserve {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            append(menu, keep, ID_EOL_PRESERVE, MENU_KEEP_AS_IS);
            append(menu, MF_SEPARATOR, 0, "");
            let convert = |id: u32, text: &str| {
                let mut flags = MF_STRING;
                if !writable {
                    flags = flags | MF_GRAYED;
                }
                append(menu, flags, id, text);
            };
            convert(ID_EOL_CRLF, MENU_CONVERT_WINDOWS);
            convert(ID_EOL_LF, MENU_CONVERT_UNIX);
            convert(ID_EOL_CR, MENU_CONVERT_MAC);
        }
        let command = self.track_menu(menu);
        if command != 0 {
            self.command(command);
        }
    }

    /// Runs a modal popup menu and returns the chosen command id.
    ///
    /// `TrackPopupMenuEx` returns the command id because `TPM_RETURNCMD` is
    /// set; `0` means the menu was cancelled.  The menu is owned locally and
    /// destroyed on every path.
    fn track_menu(&mut self, menu: HMENU) -> u32 {
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            return 0;
        }
        let flags = (TPM_RETURNCMD | TPM_LEFTALIGN).0;
        let chosen = unsafe { TrackPopupMenuEx(menu, flags, point.x, point.y, self.hwnd, None) };
        unsafe {
            let _ = DestroyMenu(menu);
        }
        chosen.0 as u32
    }

    fn format_json(&mut self, minify: bool) {
        let text = match &self.text {
            Some(TextHandle::Scintilla(control)) => Some(control.get_text()),
            _ => None,
        };
        let Some(text) = text else {
            self.status_message = "Formatting requires the Scintilla text view".to_owned();
            self.update_status();
            return;
        };
        let provider = JsonFormatProvider;
        let result = if minify {
            provider.minify(&text)
        } else {
            provider.format(&text, &FormatOptions { indent: JSON_INDENT, lenient: false })
        };
        match result {
            Ok(output) => {
                if let Some(TextHandle::Scintilla(control)) = &self.text {
                    control.set_text(&output.text);
                }
                self.mark_buffer_dirty();
                self.status_message = if minify {
                    "JSON minified".to_owned()
                } else {
                    "JSON formatted".to_owned()
                };
            }
            Err(diagnostics) => self.show_diagnostics(&diagnostics),
        }
        self.update_status();
    }

    fn validate_json(&mut self) {
        let text = match &self.text {
            Some(TextHandle::Scintilla(control)) => Some(control.get_text()),
            _ => None,
        };
        let Some(text) = text else {
            self.status_message = "Validation requires the Scintilla text view".to_owned();
            self.update_status();
            return;
        };
        let provider = JsonFormatProvider;
        match provider.validate(&text) {
            Ok(()) => {
                self.status_message = "JSON is valid".to_owned();
            }
            Err(diagnostics) => self.show_diagnostics(&diagnostics),
        }
        self.update_status();
    }

    /// Shows JSON diagnostics in the shared panel and jumps to the first one.
    ///
    /// The original buffer is never modified here; the panel only reports the
    /// problems found before formatting is applied.
    fn show_diagnostics(&mut self, diagnostics: &[Diagnostic]) {
        let Some(first) = diagnostics.first().cloned() else {
            self.status_message = "No diagnostics".to_owned();
            return;
        };
        self.problems = diagnostics.to_vec();
        if let Some(list) = self.ensure_results_list() {
            self.configure_results_columns(list, ResultsMode::Problems);
            self.results_mode = ResultsMode::Problems;
        }
        self.refresh_results_count();
        self.session.layout = ViewLayout::TextAndResults;
        self.layout();
        self.navigate_to_problem(&first);
    }

    /// Selects the span of one diagnostic and reports its location.
    fn navigate_to_problem(&mut self, problem: &Diagnostic) {
        if let Some(TextHandle::Scintilla(control)) = &self.text {
            let start = problem.span.start.0;
            control.goto_position(start);
            control.set_selection(start, problem.span.end.0);
            control.focus();
        }
        self.status_message = format!(
            "Ln {}, Col {}: {}",
            problem.line_col.0, problem.line_col.1, problem.message
        );
        self.update_status();
    }

    /// Shows the results context menu for one hit row.
    fn show_results_context_menu(&mut self, index: usize) {
        if self.results_mode != ResultsMode::Hits {
            return;
        }
        let Some(hit) = self.session.results.hit(index).copied() else {
            return;
        };
        let Some(menu) = create_menu_or_report() else {
            return;
        };
        // SAFETY: the popup menu is owned locally and destroyed by track_menu.
        unsafe {
            append(menu, MF_STRING, ID_RESULTS_SHOW_TEXT, "Show in Text");
            append(menu, MF_STRING, ID_RESULTS_SHOW_HEX, "Show in Hex");
        }
        let command = self.track_menu(menu);
        match command {
            ID_RESULTS_SHOW_TEXT => {
                if self.session.layout == ViewLayout::Hex {
                    self.set_layout(ViewLayout::Text);
                }
                self.hit_cursor = index.saturating_add(1);
                self.navigate_to(&hit);
            }
            ID_RESULTS_SHOW_HEX => {
                if self.session.layout != ViewLayout::Hex {
                    self.set_layout(ViewLayout::Hex);
                }
                self.hit_cursor = index.saturating_add(1);
                self.navigate_to(&hit);
            }
            _ => {}
        }
    }

    fn mark_buffer_dirty(&mut self) {
        self.cancel_search();
        self.session.write(rnotepad_core::document::Document::mark_dirty);
    }

    fn find_pattern(&self) -> Option<String> {
        let bar = self.find.as_ref()?;
        let pattern = read_edit(bar.edit)?;
        if pattern.is_empty() {
            None
        } else {
            Some(pattern)
        }
    }

    fn find_replacement(&self) -> String {
        self.find
            .as_ref()
            .and_then(|bar| read_edit(bar.replace))
            .unwrap_or_default()
    }

    fn find_case_sensitive(&self) -> bool {
        let Some(bar) = self.find.as_ref() else {
            return false;
        };
        let state = unsafe { SendMessageW(bar.case, BM_GETCHECK, None, None) };
        state.0 == BST_CHECKED.0 as isize
    }

    fn toggle_find_bar(&mut self, show: bool) {
        let Some(bar) = self.find.as_mut() else {
            return;
        };
        bar.visible = show;
        show_bar(bar, show);
        show_window(bar.cancel, show && self.job.is_some());
        if show {
            unsafe {
                let _ = SetFocus(Some(bar.edit));
            }
        }
        let mut client = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut client) }.is_ok() {
            let body = (client.bottom - self.status_height()).max(0);
            self.layout_find_bar(body, client.right);
        }
        self.paint_frame();
    }

    fn find_next(&mut self) {
        let Some(pattern) = self.find_pattern() else {
            self.toggle_find_bar(true);
            self.status_message = "Enter a search pattern".to_owned();
            self.update_status();
            return;
        };
        let case = self.find_case_sensitive();
        let found = if let Some(TextHandle::Scintilla(control)) = &self.text {
            let (_, end) = control.selection();
            let from = if end == 0 { 0 } else { end };
            let hit = control.find(&pattern, case, false, false, from, true);
            if let Some((start, stop)) = hit {
                control.set_selection(start, stop);
                control.focus();
            }
            hit
        } else {
            None
        };
        match found {
            Some((start, stop)) => {
                self.status_message = format!(
                    "Match at bytes {start}..{stop} ({} occurrences in the panel)",
                    self.session.results.len()
                );
            }
            None => {
                if self.text.is_some() {
                    self.status_message = format!("No match for \"{pattern}\"");
                } else if !self.session.results.is_empty() {
                    self.jump_to_next_hit();
                    return;
                } else {
                    self.start_find_all();
                    return;
                }
            }
        }
        self.update_status();
    }

    fn start_find_all(&mut self) {
        let Some(pattern) = self.find_pattern() else {
            self.toggle_find_bar(true);
            self.status_message = "Enter a search pattern".to_owned();
            self.update_status();
            return;
        };
        let Some(shared) = self.session.doc.clone() else {
            return;
        };
        self.cancel_search();
        let (doc_id, revision) = {
            let guard = shared
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (guard.id, guard.revision)
        };
        self.problems.clear();
        self.session.results = ResultsModel::start(doc_id, revision);
        self.hit_cursor = NO_HIT;
        self.set_results_mode(ResultsMode::Hits);
        if let Some(list) = self.ensure_results_list() {
            unsafe {
                SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0)));
            }
        }
        let query = SearchQuery {
            pattern: pattern.clone(),
            mode: SearchMode::Literal,
            case_sensitive: self.find_case_sensitive(),
            whole_word: false,
        };
        self.job = Some(SearchJob::spawn(shared, query));
        self.session.layout = ViewLayout::TextAndResults;
        self.status_message = format!("Searching \"{pattern}\"...");
        if let Some(bar) = self.find.as_ref() {
            show_window(bar.cancel, bar.visible);
        }
        self.layout();
        self.update_status();
    }

    fn cancel_search(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel();
            if let Some(bar) = self.find.as_ref() {
                show_window(bar.cancel, false);
            }
        }
    }

    fn cancel_search_from_ui(&mut self) {
        if self.job.is_some() {
            self.cancel_search();
            self.session.results.finish();
            self.status_message = "Search cancelled".to_owned();
            self.update_status();
        }
    }

    /// Toggles the large-view tail lock.
    fn toggle_follow_tail(&mut self) {
        self.follow_tail = !self.follow_tail;
        if self.follow_tail {
            self.scroll_large_to_end();
            self.status_message = "Follow Tail enabled".to_owned();
        } else {
            self.status_message = "Follow Tail disabled".to_owned();
        }
        self.update_status();
    }

    /// Pins the large text view to the last page.
    ///
    /// The line index may still be partial; `line_of` forces it to scan up to
    /// the last byte, after which the scrollbar range is exact.
    fn scroll_large_to_end(&mut self) {
        let end = self.session.read(|doc| doc.raw.len()).unwrap_or(0);
        self.session
            .read(|doc| doc.line_index.line_of(RawByteOffset(end)));
        let Some(TextHandle::Large(view)) = &self.text else {
            return;
        };
        view.refresh_scroll();
        view.scroll_to_end();
        view.invalidate();
    }

    /// Docks a floating results panel or detaches the docked one.
    fn toggle_results_float(&mut self) {
        if self.float_frame.is_some() {
            self.dock_results();
        } else {
            self.float_results();
        }
        self.update_status();
    }

    /// Reparents the results list into an owned tool window.
    fn float_results(&mut self) {
        if self.session.layout != ViewLayout::TextAndResults {
            self.set_layout(ViewLayout::TextAndResults);
        }
        if self.float_frame.is_some() {
            return;
        }
        let Some(list) = self.ensure_results_list() else {
            return;
        };
        let instance = match unsafe { GetModuleHandleW(None) } {
            Ok(module) => module,
            Err(err) => {
                self.status_message = format!("GetModuleHandleW failed: {err}");
                return;
            }
        };
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_VISIBLE;
        let class = to_wide(FLOAT_CLASS_NAME);
        let title = to_wide(FLOAT_TITLE);
        // SAFETY: the frame class is registered once per process; the owner
        // keeps it above the main window and off the taskbar.
        let frame = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                FLOAT_FRAME_WIDTH,
                FLOAT_FRAME_HEIGHT,
                Some(self.hwnd),
                None,
                Some(HINSTANCE(instance.0)),
                None,
            )
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(err) => {
                tracing::warn!(error = %err, "floating results frame creation failed");
                self.status_message = format!("Floating panel failed: {err}");
                return;
            }
        };
        let _ = unsafe { SetParent(list, Some(frame)) };
        show_window(list, true);
        self.float_frame = Some(frame);
        self.resize_float_results();
        self.layout();
        self.status_message = "Results panel floating".to_owned();
    }

    /// Returns the floating results list to the main window.
    fn dock_results(&mut self) {
        let Some(frame) = self.float_frame.take() else {
            return;
        };
        if let Some(list) = self.results_list {
            let _ = unsafe { SetParent(list, Some(self.hwnd)) };
        }
        unsafe {
            let _ = DestroyWindow(frame);
        }
        self.layout();
        self.status_message = "Results panel docked".to_owned();
    }

    /// Fits the docked list to the floating frame's client area.
    fn resize_float_results(&mut self) {
        let (Some(frame), Some(list)) = (self.float_frame, self.results_list) else {
            return;
        };
        let mut client = RECT::default();
        if unsafe { GetClientRect(frame, &mut client) }.is_err() {
            return;
        }
        move_control(list, 0, 0, client.right, client.bottom);
    }

    fn poll_job(&mut self) {
        let (events, finished) = match self.job.as_mut() {
            Some(job) => (job.drain(), job.is_finished()),
            None => return,
        };
        let mut changed = false;
        let mut done = finished;
        for event in events {
            match event {
                rnotepad_core::search::TaskEvent::Progress(done_bytes, total) => {
                    self.status_message = format!("Searching… {done_bytes}/{total} bytes");
                }
                rnotepad_core::search::TaskEvent::Hits(hits) => {
                    self.session.results.push(&hits);
                    changed = true;
                }
                rnotepad_core::search::TaskEvent::Done => {
                    self.session.results.finish();
                    self.status_message =
                        format!("{} matches", self.session.results.len());
                    done = true;
                }
                rnotepad_core::search::TaskEvent::Failed(message) => {
                    self.status_message = format!("Search failed: {message}");
                    done = true;
                }
            }
        }
        if changed {
            self.refresh_results_count();
            self.apply_search_highlights();
        }
        if done {
            self.refresh_results_count();
            self.job = None;
            if let Some(bar) = self.find.as_ref() {
                show_window(bar.cancel, false);
            }
        }
        if changed || done {
            self.update_status();
        }
    }

    fn refresh_results_count(&mut self) {
        if let Some(list) = self.results_list {
            let len = self.visible_row_count();
            unsafe {
                SendMessageW(
                    list,
                    LVM_SETITEMCOUNT,
                    Some(WPARAM(len)),
                    Some(LPARAM(0)),
                );
            }
        }
    }

    fn apply_search_highlights(&mut self) {
        let ranges: Vec<std::ops::Range<u64>> = self
            .session
            .results
            .hits()
            .iter()
            .map(|hit| hit.raw_offset..hit.raw_offset.saturating_add(u64::from(hit.match_len)))
            .collect();
        if let Some(data) = &self.text_data {
            data.borrow_mut().highlights = ranges.clone();
        }
        if let Some(view) = &self.hex {
            view.data().borrow_mut().highlights = ranges;
            view.invalidate();
        }
        if let Some(TextHandle::Scintilla(control)) = &self.text {
            let shared = self.session.doc.clone();
            if let Some(shared) = shared {
                let guard = shared
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let positions: Vec<(usize, usize)> = self
                    .session
                    .results
                    .hits()
                    .iter()
                    .take(MAX_SCI_HIGHLIGHTS)
                    .filter_map(|hit| {
                        let start = guard
                            .coord
                            .raw_to_utf8(RawByteOffset(hit.raw_offset))?;
                        let end = guard.coord.raw_to_utf8(RawByteOffset(
                            hit.raw_offset.saturating_add(u64::from(hit.match_len)),
                        ))?;
                        Some((start.0, end.0))
                    })
                    .collect();
                control.clear_highlights();
                control.highlight_ranges(&positions);
            }
        }
    }

    fn jump_to_next_hit(&mut self) {
        let len = self.session.results.len();
        if len == 0 {
            self.start_find_all();
            return;
        }
        let index = self.hit_cursor % len;
        self.hit_cursor = (index + 1) % len;
        if let Some(hit) = self.session.results.hit(index).copied() {
            self.navigate_to(&hit);
        }
    }

    fn navigate_to(&mut self, hit: &MatchHit) {
        let Some(shared) = self.session.doc.clone() else {
            return;
        };
        let guard = shared
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let line_no = hit.line_no;
        let length = u64::from(hit.match_len);
        let raw = hit.raw_offset;
        if matches!(self.session.layout, ViewLayout::TextAndResults)
            || matches!(self.session.layout, ViewLayout::Text)
        {
            if let Some(TextHandle::Scintilla(control)) = &self.text {
                let start = guard.coord.raw_to_utf8(RawByteOffset(raw));
                let end = guard
                    .coord
                    .raw_to_utf8(RawByteOffset(raw.saturating_add(length)));
                if let (Some(start), Some(end)) = (start, end) {
                    control.set_selection(start.0, end.0);
                    control.focus();
                } else {
                    control.scroll_to_line(line_no);
                }
            }
            if let Some(data) = &self.text_data {
                let mut view = data.borrow_mut();
                view.first_line = line_no.saturating_sub(3);
                view.sel = Some(raw..raw.saturating_add(length));
            }
        }
        if matches!(self.session.layout, ViewLayout::TextAndHex) {
            if let Some(TextHandle::Scintilla(control)) = &self.text {
                control.scroll_to_line(line_no);
            }
            if let Some(data) = &self.text_data {
                let mut view = data.borrow_mut();
                view.first_line = line_no.saturating_sub(3);
                view.sel = Some(raw..raw.saturating_add(length));
            }
        }
        if let Some(view) = &self.hex {
            let data = view.data();
            let mut state = data.borrow_mut();
            state.first_row = raw / HEX_BYTES_PER_ROW;
            state.sel = Some(raw..raw.saturating_add(length));
            drop(state);
            view.refresh_scroll();
            view.invalidate();
        }
        drop(guard);
        self.status_message = format!(
            "Ln {}, Col {} · 0x{:08X}",
            line_no.saturating_add(1),
            hit.col.saturating_add(1),
            raw
        );
        self.update_status();
    }

    /// Mirrors a selection made in one view into the other view.
    ///
    /// Text selections move the hex viewport; hex selections move the text
    /// selection or, for the self-drawn text view, its first visible line.
    fn on_view_sync(&mut self, tag: usize) {
        if self.syncing {
            return;
        }
        let selection = match tag {
            VIEW_TAG_TEXT => self
                .text_data
                .as_ref()
                .and_then(|data| data.try_borrow().ok().and_then(|view| view.sel.clone())),
            VIEW_TAG_HEX => self
                .hex
                .as_ref()
                .and_then(|view| view.data().try_borrow().ok().and_then(|state| state.sel.clone())),
            _ => None,
        };
        let Some(selection) = selection else {
            return;
        };
        let mapped = self
            .session
            .read(|doc| {
                let start = doc.coord.raw_to_utf8(RawByteOffset(selection.start))?;
                let end = doc.coord.raw_to_utf8(RawByteOffset(selection.end))?;
                let line = doc.line_index.line_of(RawByteOffset(selection.start));
                Some((start.0, end.0, line))
            })
            .flatten();
        let Some((start, end, line)) = mapped else {
            return;
        };
        if tag == VIEW_TAG_HEX {
            if let Some(TextHandle::Scintilla(control)) = &self.text {
                if control.selection() != (start, end) {
                    self.syncing = true;
                    control.set_selection(start, end);
                    self.syncing = false;
                }
            } else if let (Some(data), Some(line)) = (&self.text_data, line) {
                data.borrow_mut().first_line = line;
                if let Some(TextHandle::Large(view)) = &self.text {
                    view.refresh_scroll();
                    view.invalidate();
                }
            }
            return;
        }
        if let Some(hex) = &self.hex {
            {
                let data = hex.data();
                let mut state = data.borrow_mut();
                state.sel = Some(selection.clone());
                state.first_row = selection.start / HEX_BYTES_PER_ROW as u64;
            }
            hex.refresh_scroll();
            hex.invalidate();
        }
    }

    /// Mirrors the Scintilla caret or selection into the hex view.
    fn sync_hex_from_text(&mut self) {
        if self.syncing {
            return;
        }
        let Some(hex) = &self.hex else {
            return;
        };
        let Some(TextHandle::Scintilla(control)) = &self.text else {
            return;
        };
        let (start, end) = control.selection();
        let (start, end) = if start == end {
            let caret = control.current_position();
            (caret, caret.saturating_add(1))
        } else {
            (start, end)
        };
        let mapped = self
            .session
            .read(|doc| {
                let start = doc.coord.utf8_to_raw(Utf8Pos(start))?;
                let end = doc.coord.utf8_to_raw(Utf8Pos(end))?;
                Some((start.0, end.0))
            })
            .flatten();
        let Some((start, end)) = mapped else {
            return;
        };
        if start >= end {
            return;
        }
        {
            let data = hex.data();
            let mut state = data.borrow_mut();
            state.sel = Some(start..end);
            state.first_row = start / HEX_BYTES_PER_ROW as u64;
        }
        hex.refresh_scroll();
        hex.invalidate();
    }

    /// Copies the selection of the focused view.
    fn copy_selection(&mut self) {
        let focus = unsafe { GetFocus() };
        if let Some(hex) = &self.hex {
            if focus == hex.hwnd() {
                let selection = hex
                    .data()
                    .try_borrow()
                    .ok()
                    .and_then(|state| state.sel.clone());
                if let Some(selection) = selection {
                    let text = self.render_hex_selection(&selection);
                    self.set_clipboard(&text);
                }
                return;
            }
        }
        if let Some(TextHandle::Large(view)) = &self.text {
            if focus == view.hwnd() {
                let selection = self
                    .text_data
                    .as_ref()
                    .and_then(|data| data.try_borrow().ok().and_then(|state| state.sel.clone()));
                if let Some(selection) = selection {
                    let text = self.render_text_selection(&selection);
                    self.set_clipboard(&text);
                }
                return;
            }
        }
        if let Some(TextHandle::Scintilla(control)) = &self.text {
            // SAFETY: the control is alive and handles WM_COPY itself.
            unsafe {
                let _ = SendMessageW(control.hwnd(), WM_COPY, None, None);
            }
        }
    }

    fn render_hex_selection(&self, selection: &std::ops::Range<u64>) -> String {
        let bytes = self
            .session
            .read(|doc| {
                doc.raw
                    .slice(selection.start..selection.end)
                    .map(<[u8]>::to_vec)
            })
            .flatten()
            .unwrap_or_default();
        let mut text = String::with_capacity(bytes.len() * HEX_COPY_BYTES_PER_ITEM);
        for (index, byte) in bytes.iter().enumerate() {
            if index > 0 {
                text.push(' ');
            }
            text.push_str(&format!("{byte:02X}"));
        }
        text
    }

    fn render_text_selection(&self, selection: &std::ops::Range<u64>) -> String {
        let span = self
            .session
            .read(|doc| {
                doc.raw
                    .slice(selection.start..selection.end)
                    .map(|raw| (raw.to_vec(), doc.encoding.charset))
            })
            .flatten();
        let Some((raw, charset)) = span else {
            return String::new();
        };
        decode(&raw, charset, 0).text
    }

    fn set_clipboard(&mut self, text: &str) {
        match write_clipboard(text) {
            Ok(()) => {
                self.status_message = format!("Copied {} character(s)", text.chars().count());
            }
            Err(err) => {
                tracing::warn!(error = %err, "clipboard write failed");
                self.status_message = format!("Copy failed: {err}");
            }
        }
        self.update_status();
    }

    fn replace_all(&mut self) {
        if !self.editing {
            self.status_message = "Enable editing before replacing".to_owned();
            self.update_status();
            return;
        }
        let Some(pattern) = self.find_pattern() else {
            self.status_message = "Enter a search pattern".to_owned();
            self.update_status();
            return;
        };
        let replacement = self.find_replacement();
        let case = self.find_case_sensitive();
        let count = match &self.text {
            Some(TextHandle::Scintilla(control)) => {
                replace_all_literal(control, &pattern, &replacement, case)
            }
            _ => 0,
        };
        if count > 0 {
            self.mark_buffer_dirty();
            self.status_message = format!("Replaced {count} occurrence(s)");
        } else {
            self.status_message = format!("No match for \"{pattern}\"");
        }
        self.update_status();
    }

    fn poll_watcher(&mut self) {
        let changed = match &self.watcher {
            Some(watcher) => watcher.take_change(),
            None => return,
        };
        if !changed {
            return;
        }
        let dirty = self.is_dirty();
        if dirty {
            self.status_message =
                "The file changed on disk; save or reopen to refresh".to_owned();
            self.update_status();
            return;
        }
        self.cancel_search();
        let Some(shared) = self.session.doc.clone() else {
            return;
        };
        let result = {
            let mut guard = shared
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.reload()
        };
        match result {
            Ok(()) => {
                self.rebuild_text_view(&shared);
                self.attach_document(&shared);
                if let Some(view) = &self.hex {
                    view.refresh_scroll();
                    view.invalidate();
                }
                self.status_message = "Reloaded after an external change".to_owned();
                self.layout();
                if self.follow_tail {
                    self.scroll_large_to_end();
                }
            }
            Err(err) => {
                self.status_message = format!("Reload failed: {err}");
            }
        }
        self.update_status();
    }

    fn is_dirty(&self) -> bool {
        let doc_dirty = self.session.read(|doc| doc.dirty).unwrap_or(false);
        let buffer_dirty = match &self.text {
            Some(TextHandle::Scintilla(control)) => control.is_modified(),
            _ => false,
        };
        doc_dirty || buffer_dirty
    }

    fn confirm_discard(&mut self) -> Option<bool> {
        if !self.session.has_document() {
            return Some(true);
        }
        if !self.is_dirty() {
            return Some(true);
        }
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(to_wide("Discard unsaved changes?").as_ptr()),
                PCWSTR(to_wide("RNotepad").as_ptr()),
                MB_YESNO | MB_ICONINFORMATION,
            )
        };
        Some(answer == IDYES)
    }

    fn caret_state(&self) -> (Option<(u32, u32)>, Option<u64>, Option<u64>) {
        let Some(shared) = &self.session.doc else {
            return (None, None, None);
        };
        let guard = shared
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.view == ViewMode::Hex {
            let first_row = self
                .hex
                .as_ref()
                .map(|view| view.data().borrow().first_row)
                .unwrap_or(0);
            let raw = first_row.saturating_mul(HEX_BYTES_PER_ROW);
            let position = guard
                .coord
                .raw_to_view(RawByteOffset(raw))
                .map(|view| (view.line.saturating_add(1), view.col.saturating_add(1)));
            return (position, None, Some(raw));
        }
        match &self.text {
            Some(TextHandle::Scintilla(control)) => {
                let position = control.current_position();
                let raw = guard.coord.utf8_to_raw(Utf8Pos(position));
                let (start, end) = control.selection();
                let selection = if start < end {
                    let start = guard.coord.utf8_to_raw(Utf8Pos(start));
                    let end = guard.coord.utf8_to_raw(Utf8Pos(end));
                    start.zip(end).map(|(start, end)| end.0.saturating_sub(start.0))
                } else {
                    None
                };
                let line_col = raw
                    .and_then(|raw| guard.coord.raw_to_view(raw))
                    .map(|view| (view.line.saturating_add(1), view.col.saturating_add(1)));
                (line_col, selection, raw.map(|raw| raw.0))
            }
            Some(TextHandle::Large(_)) => {
                let first_line = self
                    .text_data
                    .as_ref()
                    .map(|data| data.borrow().first_line)
                    .unwrap_or(0);
                let start = guard.line_index.line_start(first_line);
                let raw = start.map(|offset| offset.0);
                let line_col = start
                    .and_then(|offset| guard.coord.raw_to_view(offset))
                    .map(|view| (view.line.saturating_add(1), view.col.saturating_add(1)));
                (line_col, None, raw)
            }
            None => (None, None, None),
        }
    }

    fn compute_status_fields(&mut self) -> [String; STATUS_FIELD_COUNT] {
        let Some(shared) = &self.session.doc else {
            self.status_message.clear();
            return std::array::from_fn(|_| String::new());
        };
        let (position, selection, offset) = self.caret_state();
        let dirty = self.is_dirty();
        let view = if self.session.layout == ViewLayout::Hex {
            ViewMode::Hex
        } else {
            ViewMode::Text
        };
        let progress = if self.status_message.is_empty() {
            None
        } else {
            Some(self.status_message.clone())
        };
        let guard = shared
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let info = StatusInfo {
            doc: &guard,
            view,
            dirty,
            position,
            selection_bytes: selection,
            offset,
            progress,
        };
        let mut fields = statusbar::fields(&info);
        if self.session.results.is_stale() || !self.session.results.is_complete() {
            if !self.session.results.is_empty() {
                let count = self.session.results.len();
                fields[statusbar::FIELD_PROGRESS] = format!("{count} results");
            }
        }
        fields
    }

    /// Greys out menu commands that the current document cannot run.
    ///
    /// Menu state is advisory; [`App::writable_text`] remains the authoritative
    /// check when a command actually runs.
    fn sync_command_state(&self) {
        let menu = unsafe { GetMenu(self.hwnd) };
        if menu.0.is_null() {
            return;
        }
        let has_doc = self.session.has_document();
        let (editable, text_available) = self
            .session
            .read(|doc| (doc.kind.is_editable(), doc.text_available))
            .unwrap_or((false, false));
        let writable = self.writable_text();
        let scintilla = matches!(self.text, Some(TextHandle::Scintilla(_)));
        let set = |id: u32, enabled: bool| {
            let flags = MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED };
            // SAFETY: the menu belongs to this window and is live.
            unsafe {
                let _ = EnableMenuItem(menu, id, flags);
            }
        };

        set(ID_FILE_SAVE, editable);
        set(ID_FILE_SAVE_AS, editable);
        set(ID_FILE_CLOSE, has_doc);
        set(ID_EDIT_UNDO, scintilla);
        set(ID_EDIT_REDO, scintilla);
        set(ID_EDIT_CUT, scintilla);
        set(ID_EDIT_COPY, scintilla);
        set(ID_EDIT_PASTE, scintilla);
        set(ID_EDIT_TOGGLE, has_doc);
        set(ID_VIEW_TEXT, has_doc && text_available);
        set(ID_VIEW_HEX, has_doc);
        set(ID_VIEW_SPLIT, has_doc && text_available);
        set(ID_VIEW_RESULTS, has_doc);
        set(ID_VIEW_FOLLOW_TAIL, has_doc && matches!(self.text, Some(TextHandle::Large(_))));
        set(ID_VIEW_RESULTS_FLOAT, has_doc);
        set(ID_FIND_CANCEL, self.job.is_some());
        set(ID_EOL_PRESERVE, has_doc);
        set(ID_EOL_CRLF, writable);
        set(ID_EOL_LF, writable);
        set(ID_EOL_CR, writable);
        set(ID_BOM_KEEP, writable);
        set(ID_BOM_ADD, writable);
        set(ID_BOM_STRIP, writable);
        set(ID_FORMAT_JSON, writable);
        for (id, _) in CHARSET_MENU_ITEMS {
            set(id, has_doc);
        }
        for (id, _) in CONVERT_MENU_ITEMS {
            set(id, writable);
        }

        let follow_flags = MF_BYCOMMAND
            | if self.follow_tail {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        let float_flags = MF_BYCOMMAND
            | if self.float_frame.is_some() {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        // SAFETY: live menu owned by this window.
        unsafe {
            let _ = CheckMenuItem(menu, ID_VIEW_FOLLOW_TAIL, follow_flags.0);
            let _ = CheckMenuItem(menu, ID_VIEW_RESULTS_FLOAT, float_flags.0);
        }
    }

    fn update_status(&mut self) {
        let fields = self.compute_status_fields();
        if self.status.is_some() {
            for (index, text) in fields.iter().enumerate() {
                if self.fields_snapshot[index] != *text {
                    self.set_status_part(index, text);
                }
            }
        }
        self.fields_snapshot = fields;
        self.sync_command_state();
    }

    fn paint_frame(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn paint(&mut self) {
        // SAFETY: standard BeginPaint / EndPaint pairing.
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if !hdc.is_invalid() {
                let mut client = RECT::default();
                if GetClientRect(self.hwnd, &mut client).is_ok() {
                    let body = RECT {
                        left: 0,
                        top: 0,
                        right: client.right,
                        bottom: (client.bottom - self.status_height()).max(0),
                    };
                    let body_brush = CreateSolidBrush(BODY_BACKGROUND);
                    FillRect(hdc, &body, body_brush);
                    let _ = DeleteObject(HGDIOBJ(body_brush.0));

                    let find_brush = CreateSolidBrush(FIND_BAR_BACKGROUND);
                    if let Some(bar) = self.find.as_ref() {
                        if bar.visible {
                            let bar_rect = RECT {
                                left: 0,
                                top: body.bottom,
                                right: client.right,
                                bottom: (body.bottom + FIND_BAR_HEIGHT).min(client.bottom),
                            };
                            FillRect(hdc, &bar_rect, find_brush);
                        }
                    }
                    let _ = DeleteObject(HGDIOBJ(find_brush.0));
                }
                let _ = EndPaint(self.hwnd, &ps);
            }
        }
    }

    fn command(&mut self, id: u32) {
        match id {
            ID_FILE_NEW => self.new_document(),
            ID_FILE_OPEN => {
                if let Some(path) = pick_file(self.hwnd, false, None) {
                    self.open_document(path);
                }
            }
            ID_FILE_SAVE => self.save(None),
            ID_FILE_SAVE_AS => self.save_as(),
            ID_FILE_CLOSE => self.close_document(),
            ID_FILE_EXIT => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            ID_EDIT_UNDO => self.edit_command(rnotepad_win::sci_consts::SCI_UNDO),
            ID_EDIT_REDO => self.edit_command(rnotepad_win::sci_consts::SCI_REDO),
            ID_EDIT_CUT => self.edit_command(WM_CUT),
            ID_EDIT_COPY => self.edit_command(WM_COPY),
            ID_EDIT_PASTE => self.edit_command(WM_PASTE),
            ID_EDIT_TOGGLE => self.toggle_editing(),
            ID_EDIT_FIND => self.toggle_find_bar(true),
            ID_EDIT_FIND_NEXT => self.find_next(),
            ID_EDIT_FIND_ALL => self.start_find_all(),
            ID_VIEW_TEXT => self.set_layout(ViewLayout::Text),
            ID_VIEW_HEX => self.set_layout(ViewLayout::Hex),
            ID_VIEW_SPLIT => self.set_layout(ViewLayout::TextAndHex),
            ID_VIEW_RESULTS => self.set_layout(ViewLayout::TextAndResults),
            ID_VIEW_FOLLOW_TAIL => self.toggle_follow_tail(),
            ID_VIEW_RESULTS_FLOAT => self.toggle_results_float(),
            ID_EOL_LF => self.set_eol(EolKind::Lf),
            ID_EOL_CRLF => self.set_eol(EolKind::CrLf),
            ID_EOL_CR => self.set_eol(EolKind::Cr),
            ID_EOL_PRESERVE => self.set_eol_policy(EolWritePolicy::Preserve),
            ID_BOM_KEEP => self.set_bom(BomPolicy::Keep),
            ID_BOM_ADD => self.set_bom(BomPolicy::Add),
            ID_BOM_STRIP => self.set_bom(BomPolicy::Strip),
            ID_FORMAT_JSON => self.format_json(false),
            ID_FORMAT_MINIFY => self.format_json(true),
            ID_FORMAT_DIAGNOSTIC => self.validate_json(),
            ID_HELP_ABOUT => {
                self.show_message(
                    MB_ICONINFORMATION,
                    "RNotepad",
                    "RNotepad — a fast, low-memory text and log viewer.\n\
                     Text view uses Scintilla; large files are memory mapped.",
                );
            }
            ID_FIND_NEXT_BUTTON => self.find_next(),
            ID_FIND_ALL_BUTTON => self.start_find_all(),
            ID_FIND_REPLACE_BUTTON => self.replace_all(),
            ID_FIND_CLOSE => self.toggle_find_bar(false),
            ID_FIND_CANCEL => self.cancel_search_from_ui(),
            other => {
                if let Some(charset) = charset_for(other) {
                    self.reinterpret(charset);
                } else if let Some(charset) = convert_charset_for(other) {
                    self.convert_to(charset);
                }
            }
        }
    }

    fn edit_command(&mut self, message: u32) {
        if let Some(TextHandle::Scintilla(control)) = &self.text {
            control.send(message, 0, 0);
        }
    }

    fn on_notify(&mut self, lparam: LPARAM) {
        let header = lparam.0 as *const NMHDR;
        if header.is_null() {
            return;
        }
        // SAFETY: WM_NOTIFY always carries a valid NMHDR header.
        let header = unsafe { &*header };
        let scintilla = match &self.text {
            Some(TextHandle::Scintilla(control)) => Some(control.hwnd()),
            _ => None,
        };
        if let Some(scintilla) = scintilla {
            if header.hwndFrom == scintilla && header.code == SCN_UPDATEUI_CODE {
                self.sync_hex_from_text();
                return;
            }
        }
        if header.code == NM_DBLCLK {
            // A double click on an actionable status field opens its menu.
            if let Some(status) = self.status {
                if header.hwndFrom == status {
                    // SAFETY: status bars send NM_DBLCLK with an NMMOUSE
                    // structure whose `dwItemSpec` is the partition index.
                    let info = lparam.0 as *const NMMOUSE;
                    if !info.is_null() {
                        match unsafe { (*info).dwItemSpec } {
                            statusbar::FIELD_ENCODING => self.open_encoding_menu(),
                            statusbar::FIELD_EOL => self.open_eol_menu(),
                            _ => {}
                        }
                    }
                    return;
                }
            }
        } else if header.code == LVN_GETDISPINFOW {
            // SAFETY: the control sent LVN_GETDISPINFOW, which carries an
            // NMLVDISPINFOW structure.
            let info = lparam.0 as *mut NMLVDISPINFOW;
            if info.is_null() {
                return;
            }
            let index = unsafe { (*info).item.iItem };
            let sub = unsafe { (*info).item.iSubItem };
            let text = match self.results_mode {
                ResultsMode::Hits => self
                    .session
                    .results
                    .row(usize::try_from(index).unwrap_or(usize::MAX))
                    .map(|row| match sub {
                        0 => format!("{}", row.line),
                        1 => format!("{}", row.column),
                        _ => format!("0x{:08X}", row.offset),
                    })
                    .unwrap_or_default(),
                ResultsMode::Problems => self
                    .problems
                    .get(usize::try_from(index).unwrap_or(usize::MAX))
                    .map(|problem| match sub {
                        0 => format!("{}", problem.line_col.0),
                        1 => format!("{}", problem.line_col.1),
                        _ => match &problem.hint {
                            Some(hint) => format!("{} — {hint}", problem.message),
                            None => problem.message.clone(),
                        },
                    })
                    .unwrap_or_default(),
            };
            self.disp_text = to_wide(&text);
            let value = self.disp_text.as_mut_ptr();
            unsafe {
                (*info).item.pszText = PWSTR(value);
                (*info).item.cchTextMax = self.disp_text.len() as i32;
            }
        } else if header.code == LVN_ITEMACTIVATE {
            // SAFETY: the control sent LVN_ITEMACTIVATE with an NMLISTVIEW.
            let info = lparam.0 as *mut NMLISTVIEW;
            if info.is_null() {
                return;
            }
            let index = unsafe { (*info).iItem };
            if let Ok(index) = usize::try_from(index) {
                match self.results_mode {
                    ResultsMode::Hits => {
                        self.hit_cursor = index.saturating_add(1);
                        if let Some(hit) = self.session.results.hit(index).copied() {
                            self.navigate_to(&hit);
                        }
                    }
                    ResultsMode::Problems => {
                        if let Some(problem) = self.problems.get(index).cloned() {
                            self.navigate_to_problem(&problem);
                        }
                    }
                }
            }
        } else if header.code == NM_RCLICK {
            // SAFETY: list views send NM_RCLICK with an NMLISTVIEW-compatible
            // structure; the first fields match NMITEMACTIVATE.
            let info = lparam.0 as *mut NMLISTVIEW;
            if info.is_null() {
                return;
            }
            if let Ok(index) = usize::try_from(unsafe { (*info).iItem }) {
                self.show_results_context_menu(index);
            }
        }
    }

    fn show_message(&self, icon: MESSAGEBOX_STYLE, caption: &str, text: &str) {
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(to_wide(text).as_ptr()),
                PCWSTR(to_wide(caption).as_ptr()),
                MB_OK | icon,
            );
        }
    }

    fn shutdown(&mut self) {
        self.cancel_search();
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH);
        }
        self.watcher = None;
        self.text = None;
        self.text_data = None;
        self.hex = None;
        if let Some(frame) = self.float_frame.take() {
            // The list is a child of the frame, so destroying the frame
            // releases both windows.
            unsafe {
                let _ = DestroyWindow(frame);
            }
            self.results_list = None;
        }
        if let Some(list) = self.results_list.take() {
            unsafe {
                let _ = DestroyWindow(list);
            }
        }
        if let Some(font) = self.status_font.take() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }
        // Icon handles live until the window is destroyed because the window
        // class still references them.
        self.session.close();
    }
}

const EDIT_STYLE: WINDOW_STYLE =
    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32);
const PUSH_BUTTON_STYLE: WINDOW_STYLE =
    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32);
const CHECKBOX_STYLE: WINDOW_STYLE =
    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32);
const LIST_VIEW_STYLE: WINDOW_STYLE =
    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_BORDER.0 | LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL);

fn convert_charset_for(id: u32) -> Option<Charset> {
    match id {
        ID_CONVERT_UTF8 => Some(Charset::Utf8),
        ID_CONVERT_UTF16LE => Some(Charset::Utf16Le),
        ID_CONVERT_UTF16BE => Some(Charset::Utf16Be),
        ID_CONVERT_GBK => Some(Charset::Gbk),
        ID_CONVERT_GB18030 => Some(Charset::Gb18030),
        ID_CONVERT_BIG5 => Some(Charset::Big5),
        ID_CONVERT_SHIFT_JIS => Some(Charset::ShiftJis),
        ID_CONVERT_WINDOWS_1252 => Some(Charset::Windows1252),
        ID_CONVERT_LATIN1 => Some(Charset::Latin1),
        _ => None,
    }
}

fn charset_for(id: u32) -> Option<Charset> {
    match id {
        ID_ENC_UTF8 => Some(Charset::Utf8),
        ID_ENC_UTF16LE => Some(Charset::Utf16Le),
        ID_ENC_UTF16BE => Some(Charset::Utf16Be),
        ID_ENC_GBK => Some(Charset::Gbk),
        ID_ENC_GB18030 => Some(Charset::Gb18030),
        ID_ENC_BIG5 => Some(Charset::Big5),
        ID_ENC_SHIFT_JIS => Some(Charset::ShiftJis),
        ID_ENC_WINDOWS_1252 => Some(Charset::Windows1252),
        ID_ENC_LATIN1 => Some(Charset::Latin1),
        _ => None,
    }
}

fn replace_all_literal(
    control: &Scintilla,
    pattern: &str,
    replacement: &str,
    case_sensitive: bool,
) -> usize {
    let mut count = 0;
    let mut position = 0;
    while let Some((start, end)) =
        control.find(pattern, case_sensitive, false, false, position, false)
    {
        control.set_selection(start, end);
        control.replace_selection(replacement);
        position = start.saturating_add(replacement.len());
        count += 1;
        if count >= MAX_REPLACEMENTS {
            break;
        }
    }
    count
}

fn describe_save_error(err: &SaveError) -> String {
    match err {
        SaveError::NotEditable => "The document is read-only".to_owned(),
        SaveError::HexView => "Switch to the Text view before saving".to_owned(),
        SaveError::NoPath => "The document has no target path".to_owned(),
        SaveError::ExternalModified => {
            "The file changed on disk; reopen it or save with a new name".to_owned()
        }
        SaveError::Unmappable { count } => {
            format!("{count} characters cannot be encoded with the current charset")
        }
        other => format!("{other}"),
    }
}

fn create_child(
    parent: HWND,
    class: PCWSTR,
    style: WINDOW_STYLE,
    id: u32,
) -> Result<HWND, String> {
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|err| err.to_string())?;
    // SAFETY: the parent is live and the control id fits in the menu handle
    // slot used by child controls.
    unsafe {
        CreateWindowExW(
            Default::default(),
            class,
            w!(""),
            style,
            0,
            0,
            1,
            1,
            Some(parent),
            Some(HMENU(id as usize as *mut std::ffi::c_void)),
            Some(HINSTANCE(instance.0)),
            None,
        )
    }
    .map_err(|err| err.to_string())
}

fn show_window(hwnd: HWND, visible: bool) {
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    }
}

fn move_control(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        let _ = MoveWindow(hwnd, x, y, width.max(0), height.max(0), true);
    }
}

fn show_bar(bar: &FindBar, visible: bool) {
    for hwnd in [
        bar.edit,
        bar.replace,
        bar.next,
        bar.all,
        bar.replace_all,
        bar.case,
        bar.close,
        bar.cancel,
    ] {
        show_window(hwnd, visible);
    }
}

fn read_edit(hwnd: HWND) -> Option<String> {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return Some(String::new());
    }
    let mut buffer = vec![0u16; usize::try_from(length).ok()? + 1];
    let read = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    buffer.truncate(usize::try_from(read).unwrap_or(0));
    Some(String::from_utf16_lossy(&buffer))
}

fn set_text(hwnd: HWND, text: &str) {
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(to_wide(text).as_ptr()));
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    wide
}

/// Replaces the clipboard contents with `text`.
///
/// The buffer is allocated with `GMEM_MOVEABLE` as required by the clipboard
/// API; the system takes ownership of the handle once `SetClipboardData`
/// succeeds, and it is freed when the call fails.
fn write_clipboard(text: &str) -> Result<(), String> {
    let wide = to_wide(text);
    let bytes = wide.len() * size_of::<u16>();
    // SAFETY: the clipboard and the global heap are process-wide resources.
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|err| err.to_string())?;
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err("GlobalLock failed".to_owned());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer as *mut u16, wide.len());
        let _ = GlobalUnlock(handle);
        OpenClipboard(None).map_err(|err| err.to_string())?;
        EmptyClipboard().map_err(|err| err.to_string())?;
        let copied = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(handle.0)));
        let _ = CloseClipboard();
        if copied.is_err() {
            let _ = GlobalFree(Some(handle));
        }
        copied.map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Registers the tool window that hosts a floating results list.
fn register_float_class(instance: HINSTANCE) -> Result<(), String> {
    let class = to_wide(FLOAT_CLASS_NAME);
    // SAFETY: registered once per process with a valid wndproc.
    unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(float_frame_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: Default::default(),
            hCursor: LoadCursorW(None, IDC_ARROW).map_err(|err| err.to_string())?,
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            lpszMenuName: w!(""),
            lpszClassName: PCWSTR(class.as_ptr()),
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW(float frame) failed".to_owned());
        }
    }
    Ok(())
}

/// Frame procedure for the floating results panel.
///
/// Closing the frame asks the owner to dock the list again; resizing forwards
/// the client size so the list keeps filling the frame.
unsafe extern "system" fn float_frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            if let Ok(owner) = unsafe { GetParent(hwnd) } {
                unsafe {
                    let _ = PostMessageW(
                        Some(owner),
                        WM_COMMAND,
                        WPARAM(ID_VIEW_RESULTS_FLOAT as usize),
                        LPARAM(0),
                    );
                }
            }
            LRESULT(0)
        }
        WM_SIZE => {
            if let Ok(owner) = unsafe { GetParent(hwnd) } {
                unsafe {
                    let _ = PostMessageW(Some(owner), WM_APP_FLOAT_RESIZE, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn register_class(instance: HINSTANCE, icons: Option<&WindowIcons>) -> Result<(), String> {
    let large = icons.map(|icons| icons.large()).unwrap_or_default();
    let small = icons.map(|icons| icons.small()).unwrap_or_default();
    // SAFETY: the class is registered once per process with a valid wndproc.
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: large,
            hCursor: LoadCursorW(None, IDC_ARROW).map_err(|err| err.to_string())?,
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            lpszMenuName: w!(""),
            lpszClassName: w!("RNotepadMainWindow"),
            hIconSm: small,
        };
        if RegisterClassExW(&wc) == 0 {
            return Err("RegisterClassExW failed".to_owned());
        }
    }
    Ok(())
}

fn build_menu() -> Result<HMENU, String> {
    // SAFETY: menu handles are created and owned by this window.
    unsafe {
        let menu = CreateMenu().map_err(|err| err.to_string())?;
        let file = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(file, MF_STRING, ID_FILE_NEW, "New\tCtrl+N");
        append(file, MF_STRING, ID_FILE_OPEN, "Open...\tCtrl+O");
        append(file, MF_STRING, ID_FILE_SAVE, "Save\tCtrl+S");
        append(file, MF_STRING, ID_FILE_SAVE_AS, "Save As...");
        append(file, MF_SEPARATOR, 0, "");
        append(file, MF_STRING, ID_FILE_CLOSE, "Close");
        append(file, MF_SEPARATOR, 0, "");
        append(file, MF_STRING, ID_FILE_EXIT, "Exit");
        menu_popup(menu, file, "File");

        let edit = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(edit, MF_STRING, ID_EDIT_UNDO, "Undo\tCtrl+Z");
        append(edit, MF_STRING, ID_EDIT_REDO, "Redo\tCtrl+Y");
        append(edit, MF_SEPARATOR, 0, "");
        append(edit, MF_STRING, ID_EDIT_CUT, "Cut\tCtrl+X");
        append(edit, MF_STRING, ID_EDIT_COPY, "Copy\tCtrl+C");
        append(edit, MF_STRING, ID_EDIT_PASTE, "Paste\tCtrl+V");
        append(edit, MF_SEPARATOR, 0, "");
        append(edit, MF_STRING, ID_EDIT_TOGGLE, "Enable Editing");
        append(edit, MF_SEPARATOR, 0, "");
        append(edit, MF_STRING, ID_EDIT_FIND, "Find\tCtrl+F");
        append(edit, MF_STRING, ID_EDIT_FIND_NEXT, "Find Next\tF3");
        append(edit, MF_STRING, ID_EDIT_FIND_ALL, "Find All");
        append(edit, MF_STRING, ID_FIND_CANCEL, "Cancel Search");
        menu_popup(menu, edit, "Edit");

        let view = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(view, MF_STRING, ID_VIEW_TEXT, "Text");
        append(view, MF_STRING, ID_VIEW_HEX, "Hex");
        append(view, MF_STRING, ID_VIEW_SPLIT, "Text + Hex");
        append(view, MF_STRING, ID_VIEW_RESULTS, "Text + Results");
        append(view, MF_SEPARATOR, 0, "");
        append(view, MF_STRING, ID_VIEW_FOLLOW_TAIL, "Follow Tail");
        append(view, MF_STRING, ID_VIEW_RESULTS_FLOAT, "Float Results Panel");
        menu_popup(menu, view, "View");

        let encoding = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(encoding, MF_STRING, ID_ENC_UTF8, "UTF-8");
        append(encoding, MF_STRING, ID_ENC_UTF16LE, "UTF-16 LE");
        append(encoding, MF_STRING, ID_ENC_UTF16BE, "UTF-16 BE");
        append(encoding, MF_STRING, ID_ENC_GBK, "GBK");
        append(encoding, MF_STRING, ID_ENC_GB18030, "GB18030");
        append(encoding, MF_STRING, ID_ENC_BIG5, "Big5");
        append(encoding, MF_STRING, ID_ENC_SHIFT_JIS, "Shift-JIS");
        append(encoding, MF_STRING, ID_ENC_WINDOWS_1252, "Windows-1252");
        append(encoding, MF_STRING, ID_ENC_LATIN1, "Latin-1");
        append(encoding, MF_SEPARATOR, 0, "");
        append(encoding, MF_STRING, ID_BOM_KEEP, "Keep BOM");
        append(encoding, MF_STRING, ID_BOM_ADD, "Add BOM");
        append(encoding, MF_STRING, ID_BOM_STRIP, "Strip BOM");
        menu_popup(menu, encoding, "Encoding");

        let eol = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(eol, MF_STRING, ID_EOL_CRLF, "CRLF");
        append(eol, MF_STRING, ID_EOL_LF, "LF");
        append(eol, MF_STRING, ID_EOL_CR, "CR");
        menu_popup(menu, eol, "EOL");

        let format = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(format, MF_STRING, ID_FORMAT_JSON, "Format JSON");
        append(format, MF_STRING, ID_FORMAT_MINIFY, "Minify JSON");
        append(format, MF_STRING, ID_FORMAT_DIAGNOSTIC, "Validate JSON");
        menu_popup(menu, format, "Format");

        let help = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(help, MF_STRING, ID_HELP_ABOUT, "About RNotepad");
        menu_popup(menu, help, "Help");
        Ok(menu)
    }
}

/// Creates a popup menu, returning `None` when the system refuses.
fn create_menu_or_report() -> Option<HMENU> {
    match unsafe { CreatePopupMenu() } {
        Ok(menu) => Some(menu),
        Err(err) => {
            tracing::warn!(error = %err, "CreatePopupMenu failed");
            None
        }
    }
}

unsafe fn append(menu: HMENU, flags: MENU_ITEM_FLAGS, id: u32, text: &str) {
    let wide = to_wide(text);
    let _ = unsafe { AppendMenuW(menu, flags, id as usize, PCWSTR(wide.as_ptr())) };
}

unsafe fn menu_popup(parent: HMENU, popup: HMENU, title: &str) {
    let wide = to_wide(title);
    let _ = unsafe {
        AppendMenuW(
            parent,
            MF_POPUP,
            popup.0 as usize,
            PCWSTR(wide.as_ptr()),
        )
    };
}

fn pick_file(hwnd: HWND, save: bool, initial: Option<&Path>) -> Option<PathBuf> {
    let mut buffer = vec![0u16; FILE_BUFFER_LEN];
    if let Some(path) = initial {
        if let Some(name) = path.file_name() {
            let wide = to_wide(&name.to_string_lossy());
            let length = wide.len().min(buffer.len());
            buffer[..length].copy_from_slice(&wide[..length]);
        }
    }
    let filter = to_wide(FILE_FILTER);
    let title = to_wide(if save { SAVE_TITLE } else { OPEN_TITLE });
    let directory = initial
        .and_then(Path::parent)
        .map(|path| to_wide(&path.to_string_lossy()));
    let mut flags = OFN_EXPLORER | OFN_PATHMUSTEXIST;
    flags = if save {
        flags | OFN_OVERWRITEPROMPT
    } else {
        flags | OFN_FILEMUSTEXIST
    };
    let mut options = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrInitialDir: directory
            .as_ref()
            .map_or(PCWSTR::null(), |wide| PCWSTR(wide.as_ptr())),
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: flags,
        ..Default::default()
    };
    let accepted = unsafe {
        if save {
            GetSaveFileNameW(&mut options)
        } else {
            GetOpenFileNameW(&mut options)
        }
    };
    if !accepted.as_bool() {
        return None;
    }
    let end = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buffer[..end])))
}

unsafe extern "system" fn find_edit_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass: usize,
    _refdata: usize,
) -> LRESULT {
    if message == WM_KEYDOWN && wparam.0 as u32 == VK_RETURN.0 as u32 {
        if let Ok(parent) = unsafe { GetParent(hwnd) } {
            unsafe {
                let _ = PostMessageW(
                    Some(parent),
                    WM_COMMAND,
                    WPARAM(ID_FIND_NEXT_BUTTON as usize),
                    LPARAM(0),
                );
            }
            return LRESULT(0);
        }
    }
    // SAFETY: forwarding to the original window procedure.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_NCCREATE => {
            // SAFETY: WM_NCCREATE always carries a CREATESTRUCTW pointer.
            let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            let pointer = create.lpCreateParams as *mut App;
            if !pointer.is_null() {
                // SAFETY: the pointer was produced by Box::into_raw in run().
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
                    (*pointer).hwnd = hwnd;
                }
            }
            LRESULT(1)
        }
        WM_NCDESTROY => {
            // SAFETY: reclaims the box stored at creation; the App was already
            // emptied by WM_DESTROY.
            let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut App;
            if !pointer.is_null() {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(pointer));
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_SIZE => {
            with_app(hwnd, |app| app.layout());
            LRESULT(0)
        }
        WM_PAINT => {
            with_app(hwnd, |app| app.paint());
            LRESULT(0)
        }
        WM_TIMER => {
            with_app(hwnd, |app| {
                app.poll_job();
                app.poll_watcher();
                app.update_status();
            });
            LRESULT(0)
        }
        WM_APP_FLOAT_RESIZE => {
            with_app(hwnd, App::resize_float_results);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 as u32) & 0xFFFF;
            with_app(hwnd, |app| app.command(id));
            LRESULT(0)
        }
        WM_NOTIFY => {
            let code = lparam;
            with_app(hwnd, |app| app.on_notify(code));
            LRESULT(0)
        }
        WM_APP_VIEW_SYNC => {
            let tag = wparam.0;
            with_app(hwnd, |app| app.on_view_sync(tag));
            LRESULT(0)
        }
        WM_APP_VIEW_COPY => {
            with_app(hwnd, |app| app.copy_selection());
            LRESULT(0)
        }
        WM_DROPFILES => {
            let path = unsafe { dropped_path(HDROP(wparam.0 as *mut std::ffi::c_void)) };
            with_app(hwnd, |app| match path {
                Some(path) => app.open_document(path),
                None => {
                    app.status_message = "The dropped item contains no file path".to_owned();
                    app.update_status();
                }
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            let proceed = with_app_result(hwnd, |app| app.confirm_discard());
            if matches!(proceed, Some(Some(true))) {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            with_app(hwnd, |app| app.shutdown());
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Reads the first path from a shell drop and releases the drop handle.
///
/// The editor is single-document, so additional files are reported and skipped.
unsafe fn dropped_path(hdrop: HDROP) -> Option<PathBuf> {
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    if count == 0 {
        unsafe { DragFinish(hdrop) };
        return None;
    }
    if count > 1 {
        tracing::warn!(count, "multiple files dropped; opening the first one");
    }
    let length = unsafe { DragQueryFileW(hdrop, 0, None) } as usize;
    let mut buffer = vec![0u16; length.saturating_add(1)];
    let read = unsafe { DragQueryFileW(hdrop, 0, Some(buffer.as_mut_slice())) } as usize;
    unsafe { DragFinish(hdrop) };
    if read == 0 {
        return None;
    }
    buffer.truncate(read);
    Some(PathBuf::from(OsString::from_wide(&buffer)))
}

fn with_app_result<R>(hwnd: HWND, visit: impl FnOnce(&mut App) -> R) -> Option<R> {
    if IN_APP.with(Cell::get) {
        return None;
    }
    // SAFETY: same invariants as with_app.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut App;
    if pointer.is_null() {
        return None;
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            IN_APP.with(|flag| flag.set(false));
        }
    }
    IN_APP.with(|flag| flag.set(true));
    let _reset = Reset;
    // SAFETY: exclusive access is guaranteed by the guard.
    Some(visit(unsafe { &mut *pointer }))
}

/// Creates the main window and runs the message loop.
pub fn run() -> Result<(), String> {
    // SAFETY: initialising common controls multiple times is allowed.
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES,
        };
        let _ = InitCommonControlsEx(&controls);
    }
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|err| err.to_string())?;
    // Icons must exist before class registration so the title bar, taskbar and
    // Alt+Tab pick them up from the window class as well.
    let icons = match WindowIcons::load() {
        Ok(icons) => {
            tracing::info!("application icons loaded");
            Some(icons)
        }
        Err(err) => {
            tracing::warn!(error = %err, "application icons unavailable");
            None
        }
    };
    register_class(HINSTANCE(instance.0), icons.as_ref())?;
    register_float_class(HINSTANCE(instance.0))?;
    let menu = build_menu()?;
    let mut app = Box::<App>::default();
    app.icons = icons;
    let app = Box::into_raw(app);
    // SAFETY: the pointer is handed to the window; WM_NCCREATE stores it and
    // WM_NCDESTROY reclaims it.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW | WS_EX_CLIENTEDGE,
            w!("RNotepadMainWindow"),
            w!("RNotepad"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            None,
            Some(menu),
            Some(HINSTANCE(instance.0)),
            Some(app as *const std::ffi::c_void),
        )
    };
    let hwnd = match hwnd {
        Ok(hwnd) => hwnd,
        Err(err) => {
            // SAFETY: the window never took ownership.
            drop(unsafe { Box::from_raw(app) });
            return Err(format!("CreateWindowExW failed: {err}"));
        }
    };
    // Accept shell file drops on the frame; child views fall back to it.
    unsafe {
        DragAcceptFiles(hwnd, true);
    }
    with_app(hwnd, App::initialise);
    with_app(hwnd, |app| {
        if let Some(path) = std::env::args_os().nth(1) {
            app.open_document(PathBuf::from(path));
        } else {
            app.refresh_title();
        }
    });
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
    }
    let accelerators = unsafe { CreateAcceleratorTableW(&ACCELERATORS) }
        .map_err(|err| err.to_string())?;
    let mut message = MSG::default();
    loop {
        // SAFETY: standard message loop; `message` is a valid out parameter.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if !result.as_bool() {
            break;
        }
        let translated = unsafe { TranslateAcceleratorW(hwnd, accelerators, &message) };
        if translated == 0 {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    Ok(())
}
