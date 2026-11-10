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
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::SystemTime;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM, GlobalFree,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetDC,
    GetStockObject, ScreenToClient, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_CHARSET,
    FF_SWISS, FW_NORMAL, HBRUSH, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    ReleaseDC, VARIABLE_PITCH, WHITE_BRUSH,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW, OFN_EXPLORER, OFN_FILEMUSTEXIST,
    OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
};
use windows::Win32::UI::Controls::{
    GetComboBoxInfo, InitCommonControlsEx, BST_CHECKED, COMBOBOXINFO, EM_SETSEL, ICC_BAR_CLASSES,
    ICC_LISTVIEW_CLASSES, ICC_WIN95_CLASSES,
    INITCOMMONCONTROLSEX, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW,
    LVM_INSERTCOLUMNW, LVM_SETCOLUMNW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT,
    LVN_GETDISPINFOW, LVN_ITEMACTIVATE, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_OWNERDATA,
    LVS_REPORT, LVS_SINGLESEL, NMLISTVIEW, NMLVDISPINFOW, NM_RCLICK, NMHDR, SB_SETPARTS,
    SB_SETTEXTW, STATUSCLASSNAMEW, WC_LISTVIEWW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE, VK_F3, VK_RETURN,
};
use windows::Win32::UI::Shell::{
    DefSubclassProc, DragAcceptFiles, DragFinish, DragQueryFileW, SetWindowSubclass, HDROP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CB_ADDSTRING,
    CB_RESETCONTENT,
    CB_SETDROPPEDWIDTH, CBS_AUTOHSCROLL, CBS_DROPDOWN, CheckMenuItem,
    CreateAcceleratorTableW, CreateMenu, CreatePopupMenu, CreateWindowExW, CREATESTRUCTW,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, DrawMenuBar, EnableMenuItem, FCONTROL, FVIRTKEY, GA_ROOT,
    GetAncestor, GetClientRect,
    GetCursorPos, GetMenu, GetMessageW, GetParent, GetSystemMetrics, GetWindowRect,
    GetWindowTextLengthW,
    GetWindowTextW, GetWindowLongPtrW, GWLP_USERDATA, HMENU, HTCLIENT, IDC_ARROW, IDC_SIZENS,
    IDC_SIZEWE, IDNO,
    IDYES, KillTimer, LoadCursorW, MB_DEFBUTTON3, MB_ICONERROR, MB_ICONINFORMATION,
    MB_ICONWARNING, MB_OK,
    MB_YESNOCANCEL, MB_YESNO, MENU_ITEM_FLAGS,
    MessageBoxW, MF_BYCOMMAND, MF_CHECKED, MF_ENABLED, MF_GRAYED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, MESSAGEBOX_STYLE, MoveWindow, PostMessageW,
    PostQuitMessage, RegisterClassExW, RegisterClassW, SM_CXVSCROLL, SW_HIDE, SW_SHOW,
    SendMessageW, SetCursor,
    SetMenu, SetParent, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TrackPopupMenuEx,
    TranslateAcceleratorW, TranslateMessage, TPM_LEFTALIGN, TPM_RETURNCMD, WINDOW_STYLE,
    WM_CAPTURECHANGED, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_COPY, WM_CUT, WM_DESTROY,
    WM_DROPFILES, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE,
    WM_NCDESTROY, WM_NOTIFY, WM_PAINT, WM_PASTE, WM_SETCURSOR, WM_SETTEXT, WM_SIZE, WM_TIMER,
    WNDCLASSEXW, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN,
    WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW,
    WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL, WM_SETFONT, ACCEL,
    ACCEL_VIRT_FLAGS,
};

use rnotepad_core::bytes::ByteSource;
use rnotepad_core::coord::{CoordMap, RawByteOffset, Utf8Pos};
use rnotepad_core::document::{Document, DocumentKind, ViewMode};
use rnotepad_core::eol::{EolKind, EolWritePolicy};
use rnotepad_core::encoding::{decode, encode, precheck_unmappable, BomPolicy, Charset};
use rnotepad_core::format::{Diagnostic, FormatOptions, FormatProvider, JsonFormatProvider};
use rnotepad_core::search::{MatchHit, SearchMode, SearchQuery};

use rnotepad_win::dw::{MonoFont, DEFAULT_FONT_POINTS, rgb};
use rnotepad_win::icon::WindowIcons;
use rnotepad_win::notify::ChangeWatcher;
use rnotepad_win::results::ResultsModel;
use rnotepad_win::save::{save_document, SaveRequest, SaveError};
use rnotepad_win::sci::{
    is_newline_char_added, Scintilla, SCN_CHARADDED_CODE, SCN_UPDATEUI_CODE, SCN_ZOOM_CODE,
};
use rnotepad_win::statusbar::{self, StatusInfo};
use rnotepad_win::view::{
    SharedDoc, SharedViewData, ViewData, ViewKind, ViewWindow, VIEW_TAG_HEX, VIEW_TAG_TEXT,
    WM_APP_VIEW_COPY, WM_APP_VIEW_SYNC,
};

use crate::commands::*;
use crate::recent::{RecentFiles, RECENT_LIMIT};
use crate::search_history::{SearchHistory, REPLACE_STORAGE_FILE, SEARCH_HISTORY_LIMIT};
use crate::session::{Session, ViewLayout};
use crate::tasks::SearchJob;

/// Blank lines keep the layout readable and the geometry explicit.
const WINDOW_WIDTH: i32 = 1100;
const WINDOW_HEIGHT: i32 = 760;
const BODY_BACKGROUND: COLORREF = rgb(255, 255, 255);
const FIND_BAR_BACKGROUND: COLORREF = rgb(246, 246, 246);
/// Colour of the draggable divider between the text and hex panes.
const SPLITTER_COLOR: COLORREF = rgb(206, 206, 206);
/// Ratios the divider drag may not cross, so both panes stay usable.
const MIN_SPLIT_RATIO: f32 = 0.10;
const MAX_SPLIT_RATIO: f32 = 0.90;
/// Extra pixels on each side of the divider that also start a drag.
const SPLITTER_HIT_SLOP: i32 = 3;
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
/// File name shown for a document that has never been saved.
const UNTITLED_NAME: &str = "Untitled";
/// Title suffix shown while the editor holds unsaved changes.
const TITLE_LOCAL_CHANGES: &str = " [local changes]";
/// Title suffix shown while the disk copy is newer than the loaded document.
const TITLE_EXTERNAL_CHANGE: &str = " [changed on disk]";
/// Title suffix shown when both sides hold different content.
const TITLE_BOTH_CHANGES: &str = " [local changes, changed on disk]";
/// Caption of prompts that resolve an external change.
const EXTERNAL_CHANGE_CAPTION: &str = "File changed on disk";
/// Caption of the prompt shown when the window closes.
const EXIT_CAPTION: &str = "Exit RNotepad";
/// Status hint after an external change; the user decides whether to reload.
const EXTERNAL_CHANGE_HINT: &str =
    "File changed on disk; use File > Reload to load the disk copy";
/// Extension appended when a Save As name has none of its own.
const SAVE_DEFAULT_EXTENSION: &str = "txt";
const STATUS_FONT_FACE: &str = "Segoe UI";
const STATUS_FONT_POINTS: i32 = 9;
const POINTS_PER_INCH: i32 = 72;
const MSG_WRITE_BLOCKED: &str = "This action needs an editable Text view";
const STATUS_WIDTHS: [i32; STATUS_FIELD_COUNT] = [64, 190, 110, 84, 120, 120, 104, 0];
/// Characters contributed per byte by the hex copy format (`AA `).
const HEX_COPY_BYTES_PER_ITEM: usize = 3;
/// Posted by the floating results frame when its client area changed.
const WM_APP_FLOAT_RESIZE: u32 = 0x8000 + 3;
/// Posted when a results display request was dropped during a re-entrant paint.
const WM_APP_RESULTS_REPAINT: u32 = 0x8000 + 4;
/// Subclass identifiers of the two find-bar edit controls.
const SUBCLASS_FIND_EDIT: usize = 1;
const SUBCLASS_REPLACE_EDIT: usize = 2;
/// Subclass identifier of the Scintilla editor, which intercepts right-clicks.
const SUBCLASS_EDITOR: usize = 3;

/// Column headers and widths of the search-hit table.
const RESULT_COLUMNS: [(&str, i32); 3] = [("Line", 60), ("Column", 70), ("Text", 400)];
/// Characters shown in the hit preview before ellipses are added.
const RESULT_PREVIEW_CHARS: usize = 100;
/// Characters of context kept before the match inside a hit preview.
const RESULT_PREVIEW_LEAD_CHARS: usize = 20;
/// Raw bytes read before a match when the line is longer than the preview.
const RESULT_PREVIEW_PREFIX_BYTES: u64 = 512;
/// Raw bytes read from a match onwards when the line is longer than the preview.
const RESULT_PREVIEW_SUFFIX_BYTES: u64 = 1024;
/// Marker placed where a hit preview omits text.
const RESULT_PREVIEW_ELLIPSIS: char = '…';
/// Column headers and widths of the JSON problem table.
const PROBLEM_COLUMNS: [(&str, i32); 3] = [("Line", 60), ("Column", 70), ("Message", 300)];
/// Extended styles of the results list.
///
/// Full-row selection makes a click select the whole hit, and double buffering
/// keeps owner-data repaints from flashing while the hover tip appears.
const LIST_VIEW_EX_STYLE: u32 = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
/// Smallest width the preview column may shrink to, in 96-DPI pixels.
const RESULTS_PREVIEW_MIN_WIDTH: i32 = 120;
/// Space reserved right of the preview column so the vertical scrollbar never
/// covers its text, in 96-DPI pixels.
const RESULTS_PREVIEW_GUTTER: i32 = 4;
/// Window class of the floating results frame.
const FLOAT_CLASS_NAME: &str = "RNotepadFloatFrame";
/// Caption of the floating results frame.
const FLOAT_TITLE: &str = "Results";
/// Menu items of the results context menu, kept above zero as Win32 requires.
const ID_RESULTS_SHOW_TEXT: u32 = 3001;
/// Removes every hit from the results panel.
const ID_RESULTS_CLEAR: u32 = 3002;
/// Closes the results panel.
const ID_RESULTS_CLOSE: u32 = 3003;
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
/// EOL commands and their menu labels; the current kind is checked.
const EOL_MENU_ITEMS: [(u32, EolKind, &str); 3] = [
    (ID_EOL_CRLF, EolKind::CrLf, "CRLF"),
    (ID_EOL_LF, EolKind::Lf, "LF"),
    (ID_EOL_CR, EolKind::Cr, "CR"),
];
const ACCELERATORS: [ACCEL; 8] = [
    control_accel(b'O', ID_FILE_OPEN),
    control_accel(b'S', ID_FILE_SAVE),
    control_accel(b'R', ID_FILE_RELOAD),
    control_accel(b'F', ID_EDIT_FIND),
    control_accel(b'H', ID_EDIT_REPLACE),
    control_accel(b'N', ID_FILE_NEW),
    // Copy is a window-level accelerator so it works no matter which child
    // currently owns focus; the handler then dispatches by focus.
    control_accel(b'C', ID_EDIT_COPY),
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

/// Result of an explicit save attempt, used to decide whether a document
/// transition may continue.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveOutcome {
    Saved,
    Cancelled,
    Failed,
}

/// Disk snapshot used to tell a new external revision apart from repeated
/// notifications for the revision already reported to the user.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiskState {
    /// The file exists and its size and modification time were read.
    Readable { size: u64, mtime: Option<SystemTime> },
    /// The file is missing or unreadable; any notification is significant.
    Unreadable,
}

/// How the editor buffer and the file on disk differ from each other.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChangeState {
    /// Both sides match the revision that was loaded.
    Clean,
    /// Only the editor buffer holds unsaved changes.
    Local,
    /// Only the disk file is newer than the loaded revision.
    External,
    /// Both sides hold changes relative to the loaded revision.
    Both,
}

impl ChangeState {
    /// Title suffix that names the state, so all three cases stay distinct.
    fn title_marker(self) -> &'static str {
        match self {
            Self::Clean => "",
            Self::Local => TITLE_LOCAL_CHANGES,
            Self::External => TITLE_EXTERNAL_CHANGE,
            Self::Both => TITLE_BOTH_CHANGES,
        }
    }
}

struct FindBar {
    /// Editable combo box holding the pattern; its drop list shows the
    /// recent-search history.
    edit: HWND,
    /// Edit child of `edit`, which receives keystrokes and focus.
    input: HWND,
    /// Editable combo box holding the replacement; its drop list shows the
    /// recent-replacement history.
    replace: HWND,
    /// Edit child of `replace`, which receives keystrokes and focus.
    replace_input: HWND,
    next: HWND,
    all: HWND,
    replace_one: HWND,
    replace_all: HWND,
    case: HWND,
    wrap: HWND,
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

/// Axis along which a draggable divider separates two panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitAxis {
    /// Panes sit side by side; the divider moves horizontally.
    Columns,
    /// Panes sit one above the other; the divider moves vertically.
    Rows,
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
    /// `true` while the file on disk holds a revision newer than the loaded
    /// document that has not been reconciled yet.
    external_change: bool,
    /// Disk revision already reported for the pending external change, so a
    /// burst of notifications does not reopen the prompt.
    reported_disk_state: Option<DiskState>,
    /// Change state last reflected in the title bar.
    change_state: ChangeState,
    /// Recently opened files shown in the File menu.
    recent: RecentFiles,
    /// Recently used search patterns shown in the find-bar dropdown.
    history: SearchHistory,
    /// Recently used replacement text shown in the replace-field dropdown.
    replace_history: SearchHistory,
    /// Selection range last copied into the find field.  A repeated Ctrl+F
    /// with the same selection must not overwrite a freshly typed pattern.
    last_find_seed: Option<(usize, usize)>,
    /// Guards against text/hex link feedback loops.
    syncing: bool,
    /// Last text selection applied to the hex view, so Scintilla's deferred
    /// `SCN_UPDATEUI` notifications for pure scrolling do not clobber it.
    last_text_selection: Option<(usize, usize)>,
    /// Keeps a large document scrolled to its last line.
    follow_tail: bool,
    /// Ratio of the text pane in the text + hex split; dragged by the divider.
    split_ratio: f32,
    /// Ratio of the text pane above the docked results panel.
    results_split_ratio: f32,
    /// Width last applied to the preview column, so layout passes triggered by
    /// unrelated geometry do not rewrite it and repaint the header.
    results_preview_width: i32,
    /// Axis of the divider being dragged, if any.
    split_axis: Option<SplitAxis>,
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
            external_change: false,
            reported_disk_state: None,
            change_state: ChangeState::Clean,
            recent: RecentFiles::default(),
            history: SearchHistory::default(),
            replace_history: SearchHistory::default(),
            last_find_seed: None,
            syncing: false,
            last_text_selection: None,
            follow_tail: false,
            split_ratio: TEXT_SPLIT_RATIO,
            results_split_ratio: TEXT_SPLIT_RATIO,
            results_preview_width: 0,
            split_axis: None,
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
    static REPAINT_POSTED: Cell<bool> = const { Cell::new(false) };
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
        match MonoFont::new(dpi, MONO_POINTS) {
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
        // Every launch starts with an untitled editable buffer so typing works
        // without first creating a document.
        self.new_document();
    }

    fn ensure_find_bar(&mut self) {
        if self.find.is_some() {
            return;
        }
        let edit = match create_child(self.hwnd, w!("COMBOBOX"), FIND_PATTERN_STYLE, ID_FIND_EDIT) {
            Ok(hwnd) => hwnd,
            Err(err) => {
                self.status_message = err;
                return;
            }
        };
        let Some(input) = combo_edit_handle(edit) else {
            self.status_message = "Creating the pattern field failed".to_owned();
            return;
        };
        let replace = match create_child(self.hwnd, w!("COMBOBOX"), FIND_PATTERN_STYLE, ID_FIND_REPLACE_EDIT) {
            Ok(hwnd) => hwnd,
            Err(err) => {
                self.status_message = err;
                return;
            }
        };
        let Some(replace_input) = combo_edit_handle(replace) else {
            self.status_message = "Creating the replacement field failed".to_owned();
            return;
        };
        let next = create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_NEXT_BUTTON);
        let all = create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_ALL_BUTTON);
        let replace_one =
            create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_REPLACE_ONE);
        let replace_all =
            create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_REPLACE_BUTTON);
        let case = create_child(self.hwnd, w!("BUTTON"), CHECKBOX_STYLE, ID_FIND_CASE);
        let wrap = create_child(self.hwnd, w!("BUTTON"), CHECKBOX_STYLE, ID_FIND_WRAP);
        let close = create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_CLOSE);
        let cancel = create_child(self.hwnd, w!("BUTTON"), PUSH_BUTTON_STYLE, ID_FIND_CANCEL);
        let (
            Ok(next),
            Ok(all),
            Ok(replace_one),
            Ok(replace_all),
            Ok(case),
            Ok(wrap),
            Ok(close),
            Ok(cancel),
        ) = (next, all, replace_one, replace_all, case, wrap, close, cancel)
        else {
            self.status_message = "Creating find-bar controls failed".to_owned();
            return;
        };
        set_text(edit, "");
        set_text(replace, "");
        set_text(next, "Next");
        set_text(all, "Find All");
        set_text(replace_one, "Replace");
        set_text(replace_all, "Replace All");
        set_text(case, "Case");
        set_text(wrap, "Wrap");
        set_text(close, "X");
        set_text(cancel, "Stop");
        unsafe {
            // Wrapping is the historical behaviour, so keep it enabled until
            // the user unchecks the box.
            let _ = SendMessageW(
                wrap,
                BM_SETCHECK,
                Some(WPARAM(BST_CHECKED.0 as usize)),
                None,
            );
            // Return searches from the pattern field and replaces from the
            // replacement field; Escape closes the bar from either field.
            let _ = SetWindowSubclass(
                input,
                Some(find_edit_proc),
                SUBCLASS_FIND_EDIT,
                ID_FIND_NEXT_BUTTON as usize,
            );
            let _ = SetWindowSubclass(
                replace_input,
                Some(find_edit_proc),
                SUBCLASS_REPLACE_EDIT,
                ID_FIND_REPLACE_ONE as usize,
            );
        }
        let bar = FindBar {
            edit,
            input,
            replace,
            replace_input,
            next,
            all,
            replace_one,
            replace_all,
            case,
            wrap,
            close,
            cancel,
            visible: false,
        };
        show_bar(&bar, false);
        self.find = Some(bar);
        self.refresh_find_history();
        self.refresh_replace_history();
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
        // Full-row selection and double buffering are extended styles, so they
        // are applied after creation rather than through the window style.
        unsafe {
            SendMessageW(
                list,
                LVM_SETEXTENDEDLISTVIEWSTYLE,
                Some(WPARAM(LIST_VIEW_EX_STYLE as usize)),
                Some(LPARAM(LIST_VIEW_EX_STYLE as isize)),
            );
        }
        show_window(list, false);
        self.results_list = Some(list);
        self.results_mode = ResultsMode::Hits;
        self.fit_results_last_column(ResultsMode::Hits);
        Some(list)
    }

    /// Stretches the preview column so the table fills the list client width.
    ///
    /// Line and Column keep their fixed widths so digits stay aligned, while
    /// the Text/Message column absorbs whatever space the panel has, including
    /// after the user drags the divider or resizes the floating frame.
    fn fit_results_last_column(&mut self, mode: ResultsMode) {
        let Some(list) = self.results_list else {
            return;
        };
        let columns = match mode {
            ResultsMode::Hits => &RESULT_COLUMNS,
            ResultsMode::Problems => &PROBLEM_COLUMNS,
        };
        let last = columns.len().saturating_sub(1);
        if last == 0 {
            return;
        }
        let mut client = RECT::default();
        if unsafe { GetClientRect(list, &mut client) }.is_err() {
            return;
        }
        let fixed: i32 = columns[..last].iter().map(|(_, width)| *width).sum();
        // The vertical scrollbar and a small gutter would otherwise cover the
        // tail of long previews.
        let reserved = unsafe { GetSystemMetrics(SM_CXVSCROLL) } + RESULTS_PREVIEW_GUTTER;
        let width = client
            .right
            .saturating_sub(fixed)
            .saturating_sub(reserved)
            .max(RESULTS_PREVIEW_MIN_WIDTH);
        if width == self.results_preview_width {
            return;
        }
        self.results_preview_width = width;
        let mut column = LVCOLUMNW {
            mask: LVCF_WIDTH,
            cx: width,
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                list,
                LVM_SETCOLUMNW,
                Some(WPARAM(last)),
                Some(LPARAM(&mut column as *mut LVCOLUMNW as isize)),
            );
        }
    }

    /// Rewrites the list headers for the active table.
    fn configure_results_columns(&mut self, list: HWND, mode: ResultsMode) {
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
        // The loop just restored the declaration width, so the cached width is
        // stale and the fit below must run again.
        self.results_preview_width = 0;
        self.fit_results_last_column(mode);
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

    /// Clears the JSON problem rows after a successful format or validation.
    ///
    /// The panel itself stays visible; only stale diagnostics are removed so
    /// the UI does not keep reporting errors that no longer exist.
    fn clear_problems(&mut self) {
        self.problems.clear();
        if self.results_mode == ResultsMode::Problems {
            self.results_mode = ResultsMode::Hits;
            if let Some(list) = self.results_list {
                self.configure_results_columns(list, ResultsMode::Hits);
            }
        }
        self.refresh_results_count();
    }

    /// Line preview shown in the Text column for one hit.
    fn hit_preview_text(&self, index: usize) -> String {
        let Some(hit) = self.session.results.hit(index) else {
            return String::new();
        };
        self.session
            .read(|document| line_preview(document, hit))
            .unwrap_or_default()
    }

    /// Repaints the results list after a dropped display request.
    fn repaint_results(&mut self) {
        let Some(list) = self.results_list else {
            return;
        };
        let _ = unsafe { InvalidateRect(Some(list), None, true) };
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
        let style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0);
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

    /// Space the find bar reserves at the bottom of the view area.
    fn find_bar_height(&self) -> i32 {
        match &self.find {
            Some(bar) if bar.visible => FIND_BAR_HEIGHT,
            _ => 0,
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
        let body_height = (height - self.status_height() - self.find_bar_height()).max(0);
        if !self.session.has_document() {
            self.hide_text();
            self.hide_hex();
            self.hide_results();
            self.layout_find_bar(body_height, width);
            self.paint_frame();
            return;
        }
        let split = self.split_x(width);
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
                    let split = self.results_split_y(body_height);
                    let top = (split - SPLIT_GAP / 2).max(0);
                    let bottom = (body_height - split - SPLIT_GAP / 2).max(0);
                    self.position_text(0, 0, width, top);
                    self.hide_hex();
                    self.position_results(0, split + SPLIT_GAP / 2, width, bottom);
                }
            }
        }
        self.layout_find_bar(body_height, width);
        self.paint_frame();
    }

    /// Horizontal client position of the text/hex divider.
    fn split_x(&self, width: i32) -> i32 {
        ((width as f32) * self.split_ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)) as i32
    }

    /// Vertical client position of the text/results divider.
    fn results_split_y(&self, body_height: i32) -> i32 {
        ((body_height as f32) * self.results_split_ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO))
            as i32
    }

    /// Axis of the draggable divider under `(x, y)`, if any.
    fn splitter_at(&self, x: i32, y: i32, width: i32, height: i32) -> Option<SplitAxis> {
        let body_height = (height - self.status_height() - self.find_bar_height()).max(0);
        let half = SPLIT_GAP / 2;
        let slop = SPLITTER_HIT_SLOP;
        match self.session.layout {
            ViewLayout::TextAndHex => {
                if y < 0 || y >= body_height {
                    return None;
                }
                let split = self.split_x(width);
                (split - half - slop..=split + half + slop)
                    .contains(&x)
                    .then_some(SplitAxis::Columns)
            }
            ViewLayout::TextAndResults if self.float_frame.is_none() => {
                let split = self.results_split_y(body_height);
                (split - half - slop..=split + half + slop)
                    .contains(&y)
                    .then_some(SplitAxis::Rows)
            }
            _ => None,
        }
    }

    fn client_size(&self) -> (i32, i32) {
        let mut rect = RECT::default();
        match unsafe { GetClientRect(self.hwnd, &mut rect) } {
            Ok(()) => (rect.right.max(0), rect.bottom.max(0)),
            Err(_) => (0, 0),
        }
    }

    /// Starts a divider drag when the left button goes down on it.
    fn begin_split_drag(&mut self, x: i32, y: i32) -> bool {
        let (width, height) = self.client_size();
        let Some(axis) = self.splitter_at(x, y, width, height) else {
            return false;
        };
        self.split_axis = Some(axis);
        unsafe {
            SetCapture(self.hwnd);
        }
        true
    }

    /// Moves the divider while the button is held down.
    fn drag_split(&mut self, x: i32, y: i32) -> bool {
        let Some(axis) = self.split_axis else {
            return false;
        };
        let (width, height) = self.client_size();
        match axis {
            SplitAxis::Columns => {
                let width = width.max(1);
                let ratio = (x as f32 / width as f32).clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
                if (ratio - self.split_ratio).abs() > f32::EPSILON {
                    self.split_ratio = ratio;
                    self.layout();
                }
            }
            SplitAxis::Rows => {
                let body_height =
                    (height - self.status_height() - self.find_bar_height()).max(1);
                let ratio =
                    (y as f32 / body_height as f32).clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
                if (ratio - self.results_split_ratio).abs() > f32::EPSILON {
                    self.results_split_ratio = ratio;
                    self.layout();
                }
            }
        }
        true
    }

    /// Ends a divider drag and releases the mouse capture.
    fn end_split_drag(&mut self) -> bool {
        if self.split_axis.is_none() {
            return false;
        }
        self.split_axis = None;
        unsafe {
            let _ = ReleaseCapture();
        }
        true
    }

    /// Shows the resize cursor while the pointer is over the divider.
    fn splitter_cursor(&self) -> bool {
        let axis = match self.split_axis {
            Some(axis) => axis,
            None => {
                let mut point = POINT::default();
                if unsafe { GetCursorPos(&mut point) }.is_err() {
                    return false;
                }
                let converted = unsafe { ScreenToClient(self.hwnd, &mut point) };
                if !converted.as_bool() {
                    return false;
                }
                let (width, height) = self.client_size();
                let Some(axis) = self.splitter_at(point.x, point.y, width, height) else {
                    return false;
                };
                axis
            }
        };
        let resource = match axis {
            SplitAxis::Columns => IDC_SIZEWE,
            SplitAxis::Rows => IDC_SIZENS,
        };
        match unsafe { LoadCursorW(None, resource) } {
            Ok(cursor) => {
                unsafe {
                    SetCursor(Some(cursor));
                }
                true
            }
            Err(_) => false,
        }
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
        let available = (width
            - FIND_BUTTON_WIDTH * 2
            - FIND_CASE_WIDTH
            - FIND_WRAP_WIDTH
            - FIND_CLOSE_WIDTH
            - FIND_ROW_GAP * 6)
            .max(0);
        let edit_width = available;
        // The combo height covers its closed field plus the drop list; the
        // list height is what caps the history panel.
        move_control(bar.edit, FIND_ROW_GAP, row_one, edit_width, FIND_DROPDOWN_HEIGHT);
        unsafe {
            let _ = SendMessageW(
                bar.edit,
                CB_SETDROPPEDWIDTH,
                Some(WPARAM(edit_width.max(0) as usize)),
                None,
            );
        }
        let x = FIND_ROW_GAP + edit_width + FIND_ROW_GAP;
        move_control(bar.next, x, row_one, FIND_BUTTON_WIDTH, FIND_ROW_HEIGHT);
        let x = x + FIND_BUTTON_WIDTH + FIND_ROW_GAP;
        move_control(bar.all, x, row_one, FIND_BUTTON_WIDTH, FIND_ROW_HEIGHT);
        let x = x + FIND_BUTTON_WIDTH + FIND_ROW_GAP;
        move_control(bar.case, x, row_one, FIND_CASE_WIDTH, FIND_ROW_HEIGHT);
        let x = x + FIND_CASE_WIDTH + FIND_ROW_GAP;
        move_control(bar.wrap, x, row_one, FIND_WRAP_WIDTH, FIND_ROW_HEIGHT);
        let x = x + FIND_WRAP_WIDTH + FIND_ROW_GAP;
        move_control(bar.close, x, row_one, FIND_CLOSE_WIDTH, FIND_ROW_HEIGHT);
        let replace_width = (width
            - FIND_BUTTON_WIDTH * 2
            - FIND_CANCEL_WIDTH
            - FIND_ROW_GAP * 5)
            .max(0);
        move_control(bar.replace, FIND_ROW_GAP, row_two, replace_width, FIND_DROPDOWN_HEIGHT);
        unsafe {
            let _ = SendMessageW(
                bar.replace,
                CB_SETDROPPEDWIDTH,
                Some(WPARAM(replace_width.max(0) as usize)),
                None,
            );
        }
        let x = FIND_ROW_GAP + replace_width + FIND_ROW_GAP;
        move_control(bar.replace_one, x, row_two, FIND_BUTTON_WIDTH, FIND_ROW_HEIGHT);
        let x = x + FIND_BUTTON_WIDTH + FIND_ROW_GAP;
        move_control(bar.replace_all, x, row_two, FIND_BUTTON_WIDTH, FIND_ROW_HEIGHT);
        let x = x + FIND_BUTTON_WIDTH + FIND_ROW_GAP;
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
            // The panel width drives the preview column, so the columns must
            // follow every resize instead of keeping the creation-time widths.
            self.fit_results_last_column(self.results_mode);
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

    /// Moves keyboard focus into the view the current layout shows.
    ///
    /// Without this the window keeps focus on itself after launch, document
    /// changes and dialogs, so typing would not reach the editor until the
    /// user clicked inside it.
    fn focus_active_view(&self) {
        if self.session.layout != ViewLayout::Hex {
            match &self.text {
                Some(TextHandle::Scintilla(control)) => {
                    control.focus();
                    return;
                }
                Some(TextHandle::Large(view)) => {
                    let _ = unsafe { SetFocus(Some(view.hwnd())) };
                    return;
                }
                None => {}
            }
        }
        if let Some(view) = &self.hex {
            let _ = unsafe { SetFocus(Some(view.hwnd())) };
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
        let points = self.text_font_points();
        match ViewWindow::create(self.hwnd, ViewKind::Hex, data, points) {
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

    /// Font size in points the hex view must use so its glyphs match the text
    /// view, including any zoom the user applied to the Scintilla control.
    fn text_font_points(&self) -> i32 {
        match &self.text {
            Some(TextHandle::Scintilla(control)) => MONO_POINTS.saturating_add(control.zoom()),
            _ => MONO_POINTS,
        }
    }

    /// Keeps the hex cell size in step with Scintilla's Ctrl+wheel zoom.
    fn apply_text_zoom(&mut self) {
        let points = self.text_font_points();
        if let Some(hex) = &self.hex {
            hex.set_font_points(points);
        }
    }

    fn rebuild_text_view(&mut self, shared: &SharedDoc) {
        self.text = None;
        self.text_data = None;
        self.last_text_selection = None;
        // Reset until the new document proves it can accept edits.
        self.editing = false;
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
                    // Opened files start editable; the toggle can revoke it.
                    self.editing = true;
                    control.set_read_only(false);
                    control.set_eol(eol);
                    // Scintilla draws its own context menu, which cannot carry
                    // application commands; the subclass routes right-clicks
                    // to the app-owned menu instead.
                    let _ = unsafe {
                        SetWindowSubclass(
                            control.hwnd(),
                            Some(editor_context_proc),
                            SUBCLASS_EDITOR,
                            0,
                        )
                    };
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
        match ViewWindow::create(self.hwnd, ViewKind::Text, data.clone(), MONO_POINTS) {
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
            view.h_offset = 0;
            view.highlights.clear();
            view.sel = None;
        }
        if let Some(view) = &self.hex {
            let data = view.data();
            let mut state = data.borrow_mut();
            state.doc = Some(shared.clone());
            state.first_line = 0;
            state.first_row = 0;
            state.h_offset = 0;
            state.highlights.clear();
            state.sel = None;
        }
    }

    fn refresh_title(&self) {
        let name = self.document_name();
        let marker = self.change_state().title_marker();
        let title = format!("{name}{marker} - RNotepad");
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(to_wide(&title).as_ptr()));
        }
    }

    /// Compares the editor buffer with the disk file.
    fn change_state(&self) -> ChangeState {
        match (self.is_dirty(), self.external_change) {
            (false, false) => ChangeState::Clean,
            (true, false) => ChangeState::Local,
            (false, true) => ChangeState::External,
            (true, true) => ChangeState::Both,
        }
    }

    /// File name shown in the title bar and prompts.
    fn document_name(&self) -> String {
        self.session
            .read(|doc| {
                doc.path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .flatten()
            .unwrap_or_else(|| UNTITLED_NAME.to_owned())
    }

    fn new_document(&mut self) {
        if !self.confirm_transition() {
            return;
        }
        self.cancel_search();
        self.watcher = None;
        self.reset_external_change();
        self.hex = None;
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
        self.focus_active_view();
    }

    fn open_document(&mut self, path: PathBuf) {
        if !self.confirm_transition() {
            return;
        }
        self.cancel_search();
        match self.session.open(&path) {
            Ok(shared) => {
                self.hex = None;
                self.follow_tail = false;
                self.reset_external_change();
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
                self.record_recent(&path);
                self.status_message = if kind.is_editable() {
                    format!("Opened {}", path.display())
                } else {
                    format!("Opened {} (read-only)", path.display())
                };
                self.refresh_title();
                self.layout();
                self.update_status();
                self.focus_active_view();
            }
            Err(err) => {
                tracing::error!(path = %path.display(), error = %err, "open failed");
                self.show_message(MB_ICONERROR, "Open failed", &format!("{err}"));
            }
        }
    }

    /// Opens the recent-file entry selected from the File menu.
    fn open_recent(&mut self, command: u32) {
        let index = (command - ID_FILE_RECENT_FIRST) as usize;
        let path = self.recent.paths().get(index).cloned();
        match path {
            Some(path) => self.open_document(path),
            None => {
                self.status_message = "The recent-file entry is no longer available".to_owned();
                self.update_status();
            }
        }
    }

    /// Records an opened path and refreshes the menu when the list changed.
    fn record_recent(&mut self, path: &Path) {
        if self.recent.record(path) {
            self.refresh_menu();
        }
    }

    /// Rebuilds the whole menu so File > Recent Files matches [`RecentFiles`].
    fn refresh_menu(&mut self) {
        let menu = match build_menu(&self.recent) {
            Ok(menu) => menu,
            Err(err) => {
                tracing::warn!(error = %err, "menu rebuild failed");
                return;
            }
        };
        let previous = unsafe { GetMenu(self.hwnd) };
        if unsafe { SetMenu(self.hwnd, Some(menu)) }.is_err() {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            tracing::warn!("menu installation failed");
            return;
        }
        unsafe {
            let _ = DrawMenuBar(self.hwnd);
        }
        if !previous.0.is_null() {
            unsafe {
                let _ = DestroyMenu(previous);
            }
        }
        self.sync_command_state();
    }

    fn close_document(&mut self) {
        if !self.confirm_transition() {
            return;
        }
        self.cancel_search();
        self.watcher = None;
        self.reset_external_change();
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
        // A window without a document has no editor and most commands stay
        // disabled; closing the last document therefore opens a blank one so
        // the app remains immediately usable.
        if !self.session.has_document() {
            self.new_document();
        }
        self.status_message = "Document closed".to_owned();
        self.refresh_title();
        self.layout();
        self.update_status();
    }

    fn save(&mut self, target: Option<PathBuf>) -> SaveOutcome {
        self.save_with(target, false)
    }

    /// Saves the buffer.
    ///
    /// A disk copy that changed after the document was loaded is never
    /// replaced silently: the user is asked first, so keeping local edits is
    /// always an explicit decision.
    fn save_with(
        &mut self,
        target: Option<PathBuf>,
        allow_external_overwrite: bool,
    ) -> SaveOutcome {
        self.cancel_search();
        let text = match &self.text {
            Some(TextHandle::Scintilla(control)) => Some(control.get_text()),
            _ => None,
        };
        let Some(text) = text else {
            self.status_message = "Editing requires the Scintilla text view".to_owned();
            self.update_status();
            return SaveOutcome::Failed;
        };
        let current = self.session.read(|doc| doc.path.clone()).flatten();
        let previous_path = current.clone();
        let path = match target.or(current) {
            Some(path) => path,
            None => match pick_file(self.hwnd, true, None) {
                Some(path) => path,
                None => return SaveOutcome::Cancelled,
            },
        };
        let Some(shared) = self.session.doc.clone() else {
            self.status_message = "No document".to_owned();
            self.update_status();
            return SaveOutcome::Failed;
        };
        let mut allowed = allow_external_overwrite;
        let result = loop {
            let request = SaveRequest {
                text: &text,
                target: Some(&path),
                backup: false,
                allow_external_overwrite: allowed,
            };
            let outcome = {
                let mut guard = shared
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                save_document(&mut guard, &request).map(|()| {
                    guard.write_policy.eol.resolve(guard.eol.dominant)
                })
            };
            match outcome {
                Err(SaveError::ExternalModified) if !allowed => {
                    if !self.confirm_external_overwrite() {
                        self.status_message =
                            "Save cancelled: the file changed on disk".to_owned();
                        self.update_status();
                        return SaveOutcome::Cancelled;
                    }
                    allowed = true;
                }
                other => break other,
            }
        };
        match result {
            Ok(eol) => {
                if let Some(TextHandle::Scintilla(control)) = &self.text {
                    control.set_eol(eol);
                    control.mark_saved();
                }
                // Save As moves the document to a new path, so external
                // changes must be watched there instead of the old file.
                if previous_path.as_deref() != Some(path.as_path()) {
                    self.watcher = match ChangeWatcher::new(&path) {
                        Ok(watcher) => Some(watcher),
                        Err(err) => {
                            tracing::warn!(error = %err, "file change watcher unavailable");
                            None
                        }
                    };
                }
                self.record_recent(&path);
                // The file on disk now matches the editor again.
                self.clear_external_change();
                self.status_message = format!("Saved {}", path.display());
                self.refresh_title();
                self.update_status();
                SaveOutcome::Saved
            }
            Err(err) => {
                self.show_message(MB_ICONERROR, "Save failed", &describe_save_error(&err));
                self.update_status();
                SaveOutcome::Failed
            }
        }
    }

    fn save_as(&mut self) {
        let initial = self.session.read(|doc| doc.path.clone()).flatten();
        if let Some(path) = pick_file(self.hwnd, true, initial.as_deref()) {
            let _ = self.save(Some(path));
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
    /// must not be able to modify a read-only or hex document.  "Enable
    /// Editing" is part of that contract, so a document the user switched to
    /// read-only rejects Format, EOL, BOM and encoding changes as well.
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
            && self.editing
            && view == ViewMode::Text
            && text_available
            && matches!(self.text, Some(TextHandle::Scintilla(_)))
    }

    /// `true` when the buffer should be treated as a JSON document.
    ///
    /// Strict validation would hide the JSON entries as soon as an edit breaks
    /// the syntax, which is exactly when Format and Validate are needed.  The
    /// leading object or array marker and the file extension are used instead,
    /// so the entries stay available for malformed documents.
    fn is_json_document(&self, text: &str) -> bool {
        if matches!(text.trim_start().chars().next(), Some('{') | Some('[')) {
            return true;
        }
        self.session
            .read(|doc| {
                doc.path
                    .as_ref()
                    .and_then(|path| path.extension())
                    .map(|ext| ext.eq_ignore_ascii_case("json"))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
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

    /// Shows the editor context menu at the cursor.
    ///
    /// JSON entries are only added when the buffer parses as JSON, so the menu
    /// never offers an action that would fail straight away.
    fn show_editor_context_menu(&mut self) {
        let writable = self.writable_text();
        let Some(TextHandle::Scintilla(control)) = &self.text else {
            return;
        };
        let (start, end) = control.selection();
        let has_selection = start < end;
        let can_undo = control.can_undo();
        let can_redo = control.can_redo();
        let is_json = self.is_json_document(&control.get_text());
        // Edit commands dispatch by keyboard focus, so the editor must own it
        // before the menu opens.
        control.focus();
        let Some(menu) = create_menu_or_report() else {
            return;
        };
        // SAFETY: the menu is owned locally and destroyed by track_menu.
        unsafe {
            append(menu, MF_STRING, ID_EDIT_UNDO, "Undo");
            append(menu, MF_STRING, ID_EDIT_REDO, "Redo");
            append(menu, MF_SEPARATOR, 0, "");
            append(menu, MF_STRING, ID_EDIT_CUT, "Cut");
            append(menu, MF_STRING, ID_EDIT_COPY, "Copy");
            append(menu, MF_STRING, ID_EDIT_PASTE, "Paste");
            append(menu, MF_STRING, ID_EDIT_DELETE, "Delete");
            append(menu, MF_SEPARATOR, 0, "");
            append(menu, MF_STRING, ID_EDIT_SELECT_ALL, "Select All");
            if is_json {
                append(menu, MF_SEPARATOR, 0, "");
                append(menu, MF_STRING, ID_FORMAT_JSON, "Format JSON");
                append(menu, MF_STRING, ID_FORMAT_MINIFY, "Minify JSON");
                append(menu, MF_STRING, ID_FORMAT_DIAGNOSTIC, "Validate JSON");
                set_menu_enabled(menu, ID_FORMAT_JSON, writable);
                set_menu_enabled(menu, ID_FORMAT_MINIFY, writable);
            }
        }
        set_menu_enabled(menu, ID_EDIT_UNDO, can_undo);
        set_menu_enabled(menu, ID_EDIT_REDO, can_redo);
        set_menu_enabled(menu, ID_EDIT_CUT, writable && has_selection);
        set_menu_enabled(menu, ID_EDIT_COPY, has_selection);
        set_menu_enabled(menu, ID_EDIT_PASTE, writable);
        set_menu_enabled(menu, ID_EDIT_DELETE, writable && has_selection);
        let chosen = self.track_menu(menu);
        if chosen != 0 {
            self.command(chosen);
        }
    }

    fn format_json(&mut self, minify: bool) {
        if !self.writable_text() {
            self.write_blocked(if minify { "Minify JSON" } else { "Format JSON" });
            return;
        }
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
                self.clear_problems();
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
                self.clear_problems();
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

    /// Shows the results context menu for one hit row, or for the empty area
    /// that clears the whole result set.
    fn show_results_context_menu(&mut self, index: Option<usize>) {
        let hits_mode = self.results_mode == ResultsMode::Hits;
        let hit = if hits_mode {
            index.and_then(|index| self.session.results.hit(index).copied())
        } else {
            None
        };
        let Some(menu) = create_menu_or_report() else {
            return;
        };
        // SAFETY: the popup menu is owned locally and destroyed by track_menu.
        unsafe {
            if hit.is_some() {
                append(menu, MF_STRING, ID_RESULTS_SHOW_TEXT, "Show in Text");
            }
            if hits_mode {
                append(menu, MF_STRING, ID_RESULTS_CLEAR, "Clear");
            }
            if hit.is_some() || hits_mode {
                append(menu, MF_SEPARATOR, 0, "");
            }
            append(menu, MF_STRING, ID_RESULTS_CLOSE, "Close");
        }
        let command = self.track_menu(menu);
        match command {
            ID_RESULTS_SHOW_TEXT => {
                let (Some(index), Some(hit)) = (index, hit) else {
                    return;
                };
                if self.session.layout == ViewLayout::Hex {
                    self.set_layout(ViewLayout::Text);
                }
                self.hit_cursor = index.saturating_add(1);
                self.navigate_to(&hit, true);
            }
            ID_RESULTS_CLEAR => {
                self.clear_results();
                self.status_message = "Search results cleared".to_owned();
                self.update_status();
            }
            ID_RESULTS_CLOSE => self.close_results_panel(),
            _ => {}
        }
    }

    /// Closes the results panel and returns to the plain text layout.
    ///
    /// Collected hits are kept, so reopening the panel through View > Results
    /// shows them again; use Clear to drop them.
    fn close_results_panel(&mut self) {
        if self.float_frame.is_some() {
            self.dock_results();
        }
        if self.session.layout == ViewLayout::TextAndResults {
            self.set_layout(ViewLayout::Text);
        }
        self.status_message = "Results panel closed".to_owned();
        self.update_status();
    }

    fn mark_buffer_dirty(&mut self) {
        // Hits collected before this edit point at offsets that may no longer
        // exist, and the text they described is gone, so drop them instead of
        // leaving a stale list the next Find All would appear to confirm.
        self.clear_results();
        self.session.write(|document| {
            rnotepad_core::document::Document::mark_dirty(document);
        });
    }

    /// Drops every hit and its highlights, leaving the results panel empty.
    fn clear_results(&mut self) {
        self.cancel_search();
        self.session.results.clear();
        self.hit_cursor = NO_HIT;
        self.clear_search_highlights();
        if self.results_mode == ResultsMode::Hits {
            self.refresh_results_count();
        }
    }

    /// Removes search highlights from every view without touching the hits.
    fn clear_search_highlights(&mut self) {
        if let Some(data) = &self.text_data {
            data.borrow_mut().highlights.clear();
        }
        if let Some(view) = &self.hex {
            view.data().borrow_mut().highlights.clear();
            view.invalidate();
        }
        if let Some(TextHandle::Scintilla(control)) = &self.text {
            control.clear_highlights();
        }
    }

    /// Selected text that may seed the find field, with the range it came from.
    ///
    /// Only the first non-blank line is used: the find field is a single-line
    /// edit control, while a whole-line selection includes the line break at
    /// its end, which would never match as a literal pattern.
    fn selection_for_find(&self) -> Option<((usize, usize), String)> {
        let (range, text) = match &self.text {
            Some(TextHandle::Scintilla(control)) => {
                let (start, end) = control.selection();
                if start >= end {
                    return None;
                }
                ((start, end), control.text_range(start, end))
            }
            Some(TextHandle::Large(_)) => {
                let selection = self
                    .text_data
                    .as_ref()
                    .and_then(|data| data.try_borrow().ok().and_then(|view| view.sel.clone()))?;
                let start = usize::try_from(selection.start).ok()?;
                let end = usize::try_from(selection.end).ok()?;
                let text = self.render_text_selection(&selection);
                ((start, end), text)
            }
            None => return None,
        };
        let pattern = text.lines().map(str::trim).find(|line| !line.is_empty())?;
        Some((range, pattern.to_owned()))
    }

    /// Seeds the find field with the current selection when it is usable.
    ///
    /// The field is only left untouched when the selection has not changed
    /// since the previous seed and already holds a pattern, so a repeated
    /// Ctrl+F keeps freshly typed text while any new selection still seeds.
    fn prefill_find_pattern(&mut self) {
        let Some((range, pattern)) = self.selection_for_find() else {
            return;
        };
        let Some(bar) = self.find.as_ref() else {
            return;
        };
        let already_seeded = self.last_find_seed == Some(range)
            && read_edit(bar.edit).is_some_and(|text| !text.is_empty());
        if already_seeded {
            return;
        }
        self.last_find_seed = Some(range);
        set_text(bar.edit, &pattern);
        // Selecting the seeded text lets the user either type a fresh pattern
        // or press Enter immediately to search for it.
        unsafe {
            let _ = SendMessageW(bar.input, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
    }

    /// Adds a pattern to the persistent recent-search list.
    fn remember_search(&mut self, pattern: &str) {
        if self.history.record(pattern) {
            self.refresh_find_history();
        }
    }

    /// Adds a replacement to the persistent recent-replacement list.
    fn remember_replacement(&mut self, replacement: &str) {
        if self.replace_history.record(replacement) {
            self.refresh_replace_history();
        }
    }

    /// Rebuilds the pattern dropdown from the persisted history.
    fn refresh_find_history(&mut self) {
        let Some(bar) = self.find.as_ref() else {
            return;
        };
        fill_history_combo(bar.edit, bar.input, self.history.patterns());
    }

    /// Rebuilds the replacement dropdown from the persisted history.
    fn refresh_replace_history(&mut self) {
        let Some(bar) = self.find.as_ref() else {
            return;
        };
        fill_history_combo(bar.replace, bar.replace_input, self.replace_history.patterns());
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

    /// `true` when Find Next may continue from the other end of the buffer.
    fn find_wrap_enabled(&self) -> bool {
        let Some(bar) = self.find.as_ref() else {
            return false;
        };
        let state = unsafe { SendMessageW(bar.wrap, BM_GETCHECK, None, None) };
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
        } else {
            // Returning to the editor keeps typing seamless after Escape or
            // the close button.
            self.focus_active_view();
        }
        // The bar reserves space above the status bar, so the panes must be
        // re-laid out whenever it appears or disappears.
        self.layout();
    }

    /// Shows the find bar, seeding the pattern field from the selection.
    fn open_find(&mut self) {
        self.prefill_find_pattern();
        self.toggle_find_bar(true);
    }

    /// Shows the find bar and puts the caret in the replacement field.
    fn open_replace(&mut self) {
        self.prefill_find_pattern();
        self.toggle_find_bar(true);
        if let Some(bar) = self.find.as_ref() {
            unsafe {
                let _ = SetFocus(Some(bar.replace));
            }
        }
    }

    fn find_next(&mut self) {
        let Some(pattern) = self.find_pattern() else {
            self.toggle_find_bar(true);
            self.status_message = "Enter a search pattern".to_owned();
            self.update_status();
            return;
        };
        let case = self.find_case_sensitive();
        let wrap = self.find_wrap_enabled();
        self.remember_search(&pattern);
        let found = if let Some(TextHandle::Scintilla(control)) = &self.text {
            let (_, end) = control.selection();
            let from = if end == 0 { 0 } else { end };
            let hit = control.find(&pattern, case, false, false, from, wrap);
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

    /// Rewrites the shared document from the editor buffer.
    ///
    /// Search and result previews read the shared document, which otherwise
    /// only changes when the file is saved.  The editor text excludes the BOM
    /// and keeps its own EOLs, so the rebuilt byte stream stays aligned with
    /// the caret offsets the editor reports.  Returns `false` when the buffer
    /// cannot be encoded, so the caller can stop instead of searching stale
    /// bytes.
    fn sync_document_from_editor(&mut self) -> bool {
        let Some(TextHandle::Scintilla(control)) = &self.text else {
            return true;
        };
        let text = control.get_text();
        let Some(shared) = self.session.doc.clone() else {
            return true;
        };
        let encoded = {
            let mut guard = shared
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !guard.kind.is_editable() {
                return true;
            }
            let charset = guard.encoding.charset;
            let bom = guard.encoding.bom;
            let encoded = encode(&text, charset);
            match encoded {
                Ok(body) => {
                    let bom_len = bom.len() as usize;
                    let mut bytes = Vec::with_capacity(bom_len + body.len());
                    // The document's interpretation layers skip the BOM, so
                    // the rebuilt stream has to carry the same prefix.
                    bytes.extend_from_slice(bom.bytes());
                    bytes.extend_from_slice(&body);
                    guard.replace_content(bytes, bom_len as u64);
                    Ok(())
                }
                Err(unmappable) => Err((charset, unmappable.len())),
            }
        };
        match encoded {
            Ok(()) => true,
            Err((charset, count)) => {
                self.status_message = format!(
                    "Search needs an encodable buffer: {count} character(s) cannot be encoded as {}",
                    charset.display_name()
                );
                self.update_status();
                false
            }
        }
    }

    fn start_find_all(&mut self) {
        let Some(pattern) = self.find_pattern() else {
            self.toggle_find_bar(true);
            self.status_message = "Enter a search pattern".to_owned();
            self.update_status();
            return;
        };
        // Stop any job still holding the document before it is rewritten.
        self.cancel_search();
        // The search reads the shared document, which only changes on save;
        // without this refresh Find All would answer for the bytes that were
        // loaded rather than the text the user sees.
        if !self.sync_document_from_editor() {
            return;
        }
        let Some(shared) = self.session.doc.clone() else {
            return;
        };
        self.remember_search(&pattern);
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

    /// Enables or disables the optional file log from the Help menu.
    fn toggle_logging(&mut self) {
        if crate::logging_enabled() {
            crate::disable_logging();
            self.status_message = "Logging disabled".to_owned();
        } else {
            match crate::enable_logging() {
                Ok(()) => {
                    self.status_message =
                        "Logging enabled: logs/rnotepad.<date>.log".to_owned();
                    tracing::info!("file logging enabled");
                }
                Err(err) => {
                    self.status_message = format!("Logging unavailable: {err}");
                    self.show_message(MB_ICONERROR, "Logging", &err);
                }
            }
        }
        self.update_status();
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
        self.fit_results_last_column(self.results_mode);
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
            self.navigate_to(&hit, true);
        }
    }

    /// Moves the active view to one hit.
    ///
    /// `select_match` selects the whole match (Find Next, context-menu jumps,
    /// double-clicking a results row); when false the caret is parked at the
    /// hit start instead.
    fn navigate_to(&mut self, hit: &MatchHit, select_match: bool) {
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
                    let stop = if select_match { end.0 } else { start.0 };
                    control.set_selection(start.0, stop);
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
                self.last_text_selection = Some((start, end));
            } else if let Some(TextHandle::Large(view)) = &self.text {
                if let Some(line) = line {
                    view.reveal_line(line);
                    view.refresh_scroll();
                    view.invalidate();
                }
            }
            return;
        }
        if let Some(hex) = &self.hex {
            let row = selection.start / HEX_BYTES_PER_ROW;
            {
                let data = hex.data();
                let mut state = data.borrow_mut();
                state.sel = Some(selection.clone());
            }
            hex.reveal_row(row);
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
        // `SCN_UPDATEUI` is also sent after a repaint that only scrolled the
        // text pane.  Without this check the hex viewport would jump to the
        // caret and the byte selection would be replaced.
        if self.last_text_selection == Some((start, end)) {
            return;
        }
        self.last_text_selection = Some((start, end));
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
        let row = start / HEX_BYTES_PER_ROW;
        {
            let data = hex.data();
            let mut state = data.borrow_mut();
            state.sel = Some(start..end);
        }
        hex.reveal_row(row);
        hex.refresh_scroll();
        hex.invalidate();
    }

    /// The find-bar edit control that owns keyboard focus, if any.
    ///
    /// Ctrl+C is a window-level accelerator, so the copy handler must consult
    /// the focused control instead of assuming the editor always owns keys.
    fn focused_find_edit(&self) -> Option<HWND> {
        let focus = unsafe { GetFocus() };
        let bar = self.find.as_ref()?;
        // The pattern combo forwards focus to its embedded edit child.
        if focus == bar.input || focus == bar.edit {
            Some(bar.input)
        } else if focus == bar.replace {
            Some(bar.replace)
        } else {
            None
        }
    }

    /// Copies the selection of the focused control.
    fn copy_selection(&mut self) {
        // The find fields are ordinary edit controls and copy their own
        // selection when the accelerator fires while they own focus.
        if let Some(edit) = self.focused_find_edit() {
            // SAFETY: the edit control is alive and handles WM_COPY itself.
            unsafe {
                let _ = SendMessageW(edit, WM_COPY, None, None);
            }
            return;
        }
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
            let (start, end) = control.selection();
            if start >= end {
                // Never fail silently: an empty selection would otherwise look
                // like the clipboard shortcut did nothing.
                self.status_message = "Nothing selected to copy".to_owned();
                self.update_status();
                return;
            }
            // SAFETY: the control is alive and handles WM_COPY itself.
            unsafe {
                let _ = SendMessageW(control.hwnd(), WM_COPY, None, None);
            }
            self.status_message = "Copied selection".to_owned();
            self.update_status();
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

    fn replace_one(&mut self) {
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
        self.remember_replacement(&replacement);
        let case = self.find_case_sensitive();
        let wrap = self.find_wrap_enabled();
        let Some(TextHandle::Scintilla(control)) = &self.text else {
            self.status_message = "Replace needs the Scintilla text view".to_owned();
            self.update_status();
            return;
        };
        let (start, end) = control.selection();
        // Replace the current match when the selection already is one; otherwise
        // advance to the next match, which matches Find Next semantics.
        let target = if start < end
            && text_matches(&control.text_range(start, end), &pattern, case)
        {
            Some((start, end))
        } else {
            control.find(&pattern, case, false, false, end, wrap)
        };
        let Some((start, end)) = target else {
            self.status_message = format!("No match for \"{pattern}\"");
            self.update_status();
            return;
        };
        control.set_selection(start, end);
        control.replace_selection(&replacement);
        control.focus();
        // Continue after the inserted text so a replacement that contains the
        // pattern cannot be matched again by the same click.  Selecting the next
        // hit mirrors Find Next, letting the user repeat Replace to walk through
        // the document.
        let resume_at = start.saturating_add(replacement.len());
        let next = control.find(&pattern, case, false, false, resume_at, wrap);
        if let Some((next_start, next_end)) = next {
            control.set_selection(next_start, next_end);
        }
        // The editor borrow ends here, so the buffer can be marked dirty.
        self.mark_buffer_dirty();
        self.status_message = match next {
            Some((next_start, next_end)) => format!(
                "Replaced 1 occurrence at bytes {start}..{end}; next match at bytes {next_start}..{next_end}"
            ),
            None => format!("Replaced 1 occurrence at bytes {start}..{end}; no further match"),
        };
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
        self.remember_replacement(&replacement);
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

    /// Reports external file changes without touching the document.
    ///
    /// Reloading is always an explicit user decision (`File > Reload`), so the
    /// timer only maintains the change indicator and a status hint.
    fn poll_watcher(&mut self) {
        let changed = match &self.watcher {
            Some(watcher) => watcher.take_change(),
            None => return,
        };
        if !changed {
            return;
        }
        // A burst of notifications for a file that already matches the loaded
        // revision must not rebuild the indicator on every timer tick.
        if !self.file_changed_on_disk() {
            self.clear_external_change();
            return;
        }
        let Some(state) = self.disk_state() else {
            return;
        };
        // One external save may deliver several notifications; only a new
        // revision updates the indicator and the hint.
        if self.external_change && self.reported_disk_state == Some(state) {
            return;
        }
        self.mark_external_change(state);
        self.status_message = EXTERNAL_CHANGE_HINT.to_owned();
        self.update_status();
    }

    /// Records that the disk copy is newer and shows the title indicator.
    fn mark_external_change(&mut self, state: DiskState) {
        self.reported_disk_state = Some(state);
        if !self.external_change {
            self.external_change = true;
            self.refresh_title();
        }
    }

    /// Drops the pending indicator and every recorded disk snapshot.
    fn clear_external_change(&mut self) {
        if !self.external_change && self.reported_disk_state.is_none() {
            return;
        }
        self.reset_external_change();
        self.refresh_title();
    }

    /// Clears the pending indicator without touching the window title, for
    /// callers that replace the whole document and refresh the title anyway.
    fn reset_external_change(&mut self) {
        self.external_change = false;
        self.reported_disk_state = None;
    }

    /// Current size and modification time of the backing file.
    fn disk_state(&self) -> Option<DiskState> {
        let path = self.session.read(|doc| doc.path.clone()).flatten()?;
        Some(match std::fs::metadata(&path) {
            Ok(metadata) => DiskState::Readable {
                size: metadata.len(),
                mtime: metadata.modified().ok(),
            },
            // A missing or unreadable file should reach the reload error path.
            Err(_) => DiskState::Unreadable,
        })
    }

    /// Explicit File > Reload command.
    ///
    /// Unsaved edits are confirmed first, so the command can never discard
    /// them silently; a document without a backing file has nothing to reload.
    fn reload_from_disk(&mut self) {
        let has_path = self
            .session
            .read(|doc| doc.path.is_some())
            .unwrap_or(false);
        if !has_path {
            self.status_message = "Nothing to reload: the document has no file on disk".to_owned();
            self.update_status();
            return;
        }
        if !self.confirm_transition() {
            return;
        }
        match self.reload_document() {
            Ok(()) => {
                self.clear_external_change();
                self.status_message = "Reloaded from disk".to_owned();
            }
            Err(err) => self.status_message = format!("Reload failed: {err}"),
        }
        self.update_status();
    }

    /// File > Open Containing Folder.
    ///
    /// Explorer is started with `/select` so the folder opens with the
    /// document highlighted.  A document that has never been saved has no
    /// folder, so the command reports that instead of silently doing nothing.
    fn open_containing_folder(&mut self) {
        let path = self.session.read(|doc| doc.path.clone()).flatten();
        let Some(path) = path else {
            self.status_message =
                "Nothing to reveal: the document has no file on disk".to_owned();
            self.update_status();
            return;
        };
        // `raw_arg` passes the switch and the path as one literal argument;
        // the path is quoted for Explorer's own command-line parser because it
        // may contain spaces.
        let mut argument = OsString::from("/select,\"");
        argument.push(path.as_os_str());
        argument.push("\"");
        match Command::new("explorer.exe").raw_arg(argument).spawn() {
            Ok(_) => self.status_message = "Opened containing folder".to_owned(),
            Err(err) => {
                tracing::error!(
                    path = %path.display(),
                    error = %err,
                    "opening the containing folder failed"
                );
                self.status_message = format!("Cannot open the containing folder: {err}");
            }
        }
        self.update_status();
    }

    /// Reloads the document from disk and refreshes the attached views.
    ///
    /// Returns a user-facing error description; the caller owns the status
    /// message because the reason for reloading differs (timer vs. command).
    fn reload_document(&mut self) -> Result<(), String> {
        self.cancel_search();
        let Some(shared) = self.session.doc.clone() else {
            return Err("No document".to_owned());
        };
        let previous = self
            .session
            .read(|doc| (doc.kind, doc.text_available))
            .unwrap_or((DocumentKind::LargeReadOnly, false));
        {
            let mut guard = shared
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.reload().map_err(|err| err.to_string())?;
        }
        let current = self
            .session
            .read(|doc| (doc.kind, doc.text_available))
            .unwrap_or(previous);
        if current == previous {
            // Reloading a live log must not recreate the view; that flashes
            // the pane and drops the scroll position.
            self.refresh_text_view();
        } else {
            self.rebuild_text_view(&shared);
            self.attach_document(&shared);
            self.layout();
        }
        if let Some(view) = &self.hex {
            view.refresh_scroll();
            view.invalidate();
        }
        if self.follow_tail {
            self.scroll_large_to_end();
        }
        Ok(())
    }


    /// `true` when the file on disk differs from the opened revision.
    fn file_changed_on_disk(&self) -> bool {
        let Some(path) = self.session.read(|doc| doc.path.clone()).flatten() else {
            return true;
        };
        let (opened_size, opened_mtime) = self
            .session
            .read(|doc| (doc.opened_size, doc.opened_mtime))
            .unwrap_or((0, None));
        match std::fs::metadata(&path) {
            Ok(metadata) => {
                metadata.len() != opened_size || metadata.modified().ok() != opened_mtime
            }
            // A missing or unreadable file should reach the reload error path.
            Err(_) => true,
        }
    }

    /// Refreshes an existing text view after the document changed in place.
    fn refresh_text_view(&mut self) {
        match &self.text {
            Some(TextHandle::Large(view)) => {
                view.refresh_scroll();
                view.invalidate();
            }
            Some(TextHandle::Scintilla(control)) => {
                let text = self.session.read(|doc| doc.decoded_text()).flatten();
                if let Some(text) = text {
                    control.set_text(&text);
                    // Replacing the buffer marks the control modified; the
                    // reload just confirmed it matches the file again.
                    control.mark_saved();
                }
            }
            None => {}
        }
    }

    fn is_dirty(&self) -> bool {
        let doc_dirty = self.session.read(|doc| doc.dirty).unwrap_or(false);
        let buffer_dirty = match &self.text {
            Some(TextHandle::Scintilla(control)) => control.is_modified(),
            _ => false,
        };
        doc_dirty || buffer_dirty
    }

    /// Prompts to save unsaved changes before the document is replaced.
    ///
    /// Returns `true` when the caller may continue: nothing was dirty, the
    /// save succeeded, or the user chose to discard.  `false` means the user
    /// cancelled or the save failed, so the requested operation must be
    /// abandoned and the current document kept as-is.
    fn confirm_transition(&mut self) -> bool {
        if !self.session.has_document() || !self.is_dirty() {
            return true;
        }
        let name = self.document_name();
        let message = format!("Save changes to {name}?");
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(to_wide(&message).as_ptr()),
                PCWSTR(to_wide("RNotepad").as_ptr()),
                MB_YESNOCANCEL | MB_ICONWARNING,
            )
        };
        match answer {
            IDYES => matches!(self.save(None), SaveOutcome::Saved),
            IDNO => true,
            // Cancel keeps the document untouched and aborts the transition.
            _ => false,
        }
    }

    /// Confirms leaving the application.
    ///
    /// Only unsaved edits need a prompt: exiting without them leaves the disk
    /// copy untouched, so a newer disk revision is not a reason to interrupt
    /// the user.  When local edits do exist, a newer disk revision must be
    /// surfaced as well, because saving would otherwise overwrite it silently.
    fn confirm_exit(&mut self) -> bool {
        if !self.session.has_document() || !self.is_dirty() {
            return true;
        }
        let external = self.external_change_pending();
        if !external {
            return self.confirm_transition();
        }
        // A watcher notification may still be queued when the window closes,
        // so the title indicator is refreshed from the metadata comparison
        // before the user decides.
        if !self.external_change {
            if let Some(state) = self.disk_state() {
                self.mark_external_change(state);
            }
            self.status_message = EXTERNAL_CHANGE_HINT.to_owned();
            self.update_status();
        }
        self.confirm_exit_both()
    }

    /// `true` when the editable document has a newer revision on disk.
    fn external_change_pending(&self) -> bool {
        let editable = self
            .session
            .read(|doc| doc.kind.is_editable() && doc.path.is_some())
            .unwrap_or(false);
        editable && (self.external_change || self.file_changed_on_disk())
    }

    /// Exit prompt when both sides changed, so saving cannot overwrite the
    /// disk copy without the user knowing.
    fn confirm_exit_both(&mut self) -> bool {
        let name = self.document_name();
        let message = format!(
            "{name} has unsaved edits and has also changed on disk since it was opened.\n\n\
             Yes: save the editor contents and exit (overwrites the disk copy)\n\
             No: exit and discard the unsaved edits (the disk copy is kept)\n\
             Cancel: stay open (use File > Reload to load the disk copy)"
        );
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(to_wide(&message).as_ptr()),
                PCWSTR(to_wide(EXIT_CAPTION).as_ptr()),
                MB_YESNOCANCEL | MB_ICONWARNING | MB_DEFBUTTON3,
            )
        };
        match answer {
            // The prompt names the overwrite explicitly, so the save may pass
            // the external-change guard it would otherwise trip again.
            IDYES => matches!(self.save_with(None, true), SaveOutcome::Saved),
            IDNO => true,
            _ => false,
        }
    }

    /// Asks whether the editor contents may replace a newer disk copy.
    fn confirm_external_overwrite(&self) -> bool {
        let name = self.document_name();
        let message = format!(
            "{name} has changed on disk since it was opened.\n\n\
             Overwrite the disk file with the editor contents?"
        );
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(to_wide(&message).as_ptr()),
                PCWSTR(to_wide(EXTERNAL_CHANGE_CAPTION).as_ptr()),
                MB_YESNO | MB_ICONWARNING,
            )
        };
        answer == IDYES
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
                let line_col = Some(control.line_column(position));
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
            editing: self.editing,
            position,
            selection_bytes: selection,
            offset,
            progress,
            external_change: self.external_change,
        };
        let mut fields = statusbar::fields(&info);
        let results = &self.session.results;
        if !results.is_empty() && (results.is_stale() || !results.is_complete()) {
            let count = results.len();
            let label = if results.is_stale() {
                format!("{count} results (stale)")
            } else {
                format!("{count} results")
            };
            fields[statusbar::FIELD_PROGRESS] = label;
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
        let has_path = self
            .session
            .read(|doc| doc.path.is_some())
            .unwrap_or(false);
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
        set(ID_FILE_RELOAD, has_path);
        set(ID_FILE_OPEN_FOLDER, has_path);
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
        set(ID_FORMAT_MINIFY, writable);
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
        let logging_flags = MF_BYCOMMAND
            | if crate::logging_enabled() {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        let editing_flags = MF_BYCOMMAND
            | if self.editing {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        // Selecting the encoding and EOL menus marks the option in effect,
        // so the user does not have to infer it from the status bar.
        let (current_charset, current_eol) = self
            .session
            .read(|doc| (doc.encoding.charset, doc.eol.dominant))
            .unwrap_or((Charset::Utf8, EolKind::CrLf));
        // SAFETY: live menu owned by this window.
        unsafe {
            let _ = CheckMenuItem(menu, ID_VIEW_FOLLOW_TAIL, follow_flags.0);
            let _ = CheckMenuItem(menu, ID_VIEW_RESULTS_FLOAT, float_flags.0);
            let _ = CheckMenuItem(menu, ID_HELP_ENABLE_LOG, logging_flags.0);
            let _ = CheckMenuItem(menu, ID_EDIT_TOGGLE, editing_flags.0);
            for (id, _) in CHARSET_MENU_ITEMS {
                let checked = has_doc && charset_for(id) == Some(current_charset);
                let flags = MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED };
                let _ = CheckMenuItem(menu, id, flags.0);
            }
            for (id, kind, _) in EOL_MENU_ITEMS {
                let checked = has_doc && kind == current_eol;
                let flags = MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED };
                let _ = CheckMenuItem(menu, id, flags.0);
            }
        }
    }

    fn update_status(&mut self) {
        // Local edits start and stop without a document transition, so the
        // title marker must follow the change state on every refresh.
        let state = self.change_state();
        if state != self.change_state {
            self.change_state = state;
            self.refresh_title();
        }
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
                    let find_height = self.find_bar_height();
                    let body = RECT {
                        left: 0,
                        top: 0,
                        right: client.right,
                        bottom: (client.bottom - self.status_height() - find_height).max(0),
                    };
                    let body_brush = CreateSolidBrush(BODY_BACKGROUND);
                    FillRect(hdc, &body, body_brush);
                    let _ = DeleteObject(HGDIOBJ(body_brush.0));

                    // The divider sits in the gap the panes leave for it.
                    let divider = if self.session.has_document() {
                        let half = SPLIT_GAP / 2;
                        match self.session.layout {
                            ViewLayout::TextAndHex => {
                                let split = self.split_x(client.right);
                                Some(RECT {
                                    left: split - half,
                                    top: body.top,
                                    right: split + half,
                                    bottom: body.bottom,
                                })
                            }
                            ViewLayout::TextAndResults if self.float_frame.is_none() => {
                                let split = self.results_split_y(body.bottom);
                                Some(RECT {
                                    left: body.left,
                                    top: split - half,
                                    right: body.right,
                                    bottom: split + half,
                                })
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    if let Some(divider) = divider {
                        let split_brush = CreateSolidBrush(SPLITTER_COLOR);
                        FillRect(hdc, &divider, split_brush);
                        let _ = DeleteObject(HGDIOBJ(split_brush.0));
                    }

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
            ID_FILE_SAVE => {
                self.save(None);
            }
            ID_FILE_SAVE_AS => self.save_as(),
            ID_FILE_RELOAD => self.reload_from_disk(),
            ID_FILE_OPEN_FOLDER => self.open_containing_folder(),
            ID_FILE_CLOSE => self.close_document(),
            ID_FILE_EXIT => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            ID_EDIT_UNDO => self.edit_command(rnotepad_win::sci_consts::SCI_UNDO),
            ID_EDIT_REDO => self.edit_command(rnotepad_win::sci_consts::SCI_REDO),
            ID_EDIT_CUT => self.edit_command(WM_CUT),
            ID_EDIT_COPY => self.copy_selection(),
            ID_EDIT_PASTE => self.edit_command(WM_PASTE),
            ID_EDIT_TOGGLE => self.toggle_editing(),
            ID_EDIT_FIND => self.open_find(),
            ID_EDIT_REPLACE => self.open_replace(),
            ID_EDIT_FIND_NEXT => self.find_next(),
            ID_EDIT_FIND_ALL => self.start_find_all(),
            ID_EDIT_SELECT_ALL => self.edit_command(rnotepad_win::sci_consts::SCI_SELECTALL),
            ID_EDIT_DELETE => self.edit_command(rnotepad_win::sci_consts::SCI_CLEAR),
            ID_EDIT_CONTEXT_MENU => self.show_editor_context_menu(),
            ID_VIEW_TEXT => self.set_layout(ViewLayout::Text),
            ID_VIEW_HEX => self.set_layout(ViewLayout::Hex),
            ID_VIEW_SPLIT => self.set_layout(ViewLayout::TextAndHex),
            ID_VIEW_RESULTS => self.set_layout(ViewLayout::TextAndResults),
            ID_VIEW_FOLLOW_TAIL => self.toggle_follow_tail(),
            ID_VIEW_RESULTS_FLOAT => self.toggle_results_float(),
            ID_HELP_ENABLE_LOG => self.toggle_logging(),
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
            ID_FIND_REPLACE_ONE => self.replace_one(),
            ID_FIND_REPLACE_BUTTON => self.replace_all(),
            ID_FIND_CLOSE => self.toggle_find_bar(false),
            ID_FIND_CANCEL => self.cancel_search_from_ui(),
            other => {
                if (ID_FILE_RECENT_FIRST..=ID_FILE_RECENT_LAST).contains(&other) {
                    self.open_recent(other);
                } else if let Some(charset) = charset_for(other) {
                    self.reinterpret(charset);
                } else if let Some(charset) = convert_charset_for(other) {
                    self.convert_to(charset);
                }
            }
        }
    }

    fn edit_command(&mut self, message: u32) {
        // Cut and paste must act on the control that owns focus; the find
        // fields are ordinary edit controls.  Scintilla-only messages such as
        // undo and redo keep going to the editor.
        if matches!(message, WM_CUT | WM_PASTE) {
            if let Some(edit) = self.focused_find_edit() {
                // SAFETY: the edit control is alive and handles these messages.
                unsafe {
                    let _ = SendMessageW(edit, message, None, None);
                }
                return;
            }
        }
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
            if header.hwndFrom == scintilla {
                match header.code {
                    SCN_CHARADDED_CODE => {
                        // Keep the indentation of the line above when the user
                        // presses Enter, matching config-file editing habits.
                        if self.editing && is_newline_char_added(lparam.0) {
                            if let Some(TextHandle::Scintilla(control)) = &self.text {
                                control.apply_previous_indent();
                            }
                        }
                        return;
                    }
                    SCN_UPDATEUI_CODE => {
                        self.sync_hex_from_text();
                        return;
                    }
                    SCN_ZOOM_CODE => {
                        self.apply_text_zoom();
                        return;
                    }
                    _ => {}
                }
            }
        }
        if header.code == LVN_GETDISPINFOW {
            // SAFETY: the control sent LVN_GETDISPINFOW, which carries an
            // NMLVDISPINFOW structure.
            let info = lparam.0 as *mut NMLVDISPINFOW;
            if info.is_null() {
                return;
            }
            let index = unsafe { (*info).item.iItem };
            let sub = unsafe { (*info).item.iSubItem };
            let row_index = usize::try_from(index).unwrap_or(usize::MAX);
            let text = match self.results_mode {
                ResultsMode::Hits => match self.session.results.row(row_index) {
                    Some(row) if sub == 0 => format!("{}", row.line),
                    Some(row) if sub == 1 => format!("{}", row.column),
                    Some(_) => self.hit_preview_text(row_index),
                    None => String::new(),
                },
                ResultsMode::Problems => self
                    .problems
                    .get(row_index)
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
                            // A double-click returns focus to the text view and
                            // selects the whole match, ready for editing.
                            self.navigate_to(&hit, true);
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
                self.show_results_context_menu(Some(index));
            } else {
                // Right-clicking the empty area still offers "Clear".
                self.show_results_context_menu(None);
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
        // Flush buffered records before the process exits.
        crate::shutdown_logging();
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

/// Editable combo box with a drop list and a scrollbar for long histories.
const FIND_PATTERN_STYLE: WINDOW_STYLE = WINDOW_STYLE(
    WS_CHILD.0
        | WS_VISIBLE.0
        | WS_TABSTOP.0
        | WS_BORDER.0
        | WS_VSCROLL.0
        | CBS_DROPDOWN as u32
        | CBS_AUTOHSCROLL as u32,
);
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

fn text_matches(selected: &str, pattern: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        selected == pattern
    } else {
        selected.eq_ignore_ascii_case(pattern)
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

/// Builds the preview shown in the Text column for one search hit.
///
/// The matched text stays inside the preview and the whole preview is capped
/// at [`RESULT_PREVIEW_CHARS`] characters, so a long line cannot push the
/// match out of the cell.  Only bytes around the hit are read, keeping
/// owner-data painting cheap even for very long lines.
fn line_preview(document: &Document, hit: &MatchHit) -> String {
    if document.id != hit.doc_id {
        return String::new();
    }
    let Some(line_range) = document.line_index.line_range(hit.line_no) else {
        return String::new();
    };
    let match_start = hit.raw_offset;
    if match_start < line_range.start || match_start > line_range.end {
        return String::new();
    }
    let match_end = match_start
        .saturating_add(u64::from(hit.match_len))
        .min(line_range.end);
    let prefix_start = document
        .coord
        .char_boundary(RawByteOffset(
            match_start.saturating_sub(RESULT_PREVIEW_PREFIX_BYTES),
        ))
        .0
        .max(line_range.start);
    let suffix_end = match_end
        .saturating_add(RESULT_PREVIEW_SUFFIX_BYTES)
        .min(line_range.end);

    let prefix = decode_span(document, prefix_start..match_start);
    let mut body = decode_span(document, match_start..suffix_end);
    if suffix_end < line_range.end {
        // A bounded window may end inside a character; drop the artifact.
        let len = body.trim_end_matches('\u{FFFD}').len();
        body.truncate(len);
    }
    let body = body.trim_end_matches(|ch| ch == '\r' || ch == '\n');

    let prefix_chars = prefix.chars().count();
    let lead_chars = prefix_chars.min(RESULT_PREVIEW_LEAD_CHARS);
    let lead: String = prefix
        .chars()
        .skip(prefix_chars.saturating_sub(lead_chars))
        .collect();
    let available = RESULT_PREVIEW_CHARS.saturating_sub(lead_chars);
    let body_chars = body.chars().count();
    let taken: String = body.chars().take(available).collect();

    let mut preview = String::new();
    if prefix_start > line_range.start || prefix_chars > lead_chars {
        preview.push(RESULT_PREVIEW_ELLIPSIS);
    }
    preview.push_str(&lead);
    preview.push_str(&taken);
    if body_chars > available || suffix_end < line_range.end {
        preview.push(RESULT_PREVIEW_ELLIPSIS);
    }
    preview
}

/// Decodes a raw byte span with the document charset.
fn decode_span(document: &Document, range: std::ops::Range<u64>) -> String {
    document
        .raw
        .slice(range)
        .map(|bytes| decode(bytes, document.encoding.charset, 0).text)
        .unwrap_or_default()
}

/// Queues at most one results repaint when a display request is dropped.
///
/// Owner-data list views ask for cell text synchronously while painting.  A
/// paint started from inside [`with_app`] (focus changes on navigation) is
/// re-entrant, so the request is discarded and the control keeps the blank
/// cell.  Re-posting a paint request once per cycle lets the list ask again
/// outside the guard and restore the text.
fn request_results_repaint(hwnd: HWND, lparam: LPARAM) {
    let header = lparam.0 as *const NMHDR;
    if header.is_null() {
        return;
    }
    // SAFETY: the sender passes a live NMHDR for the duration of the message.
    if unsafe { (*header).code } != LVN_GETDISPINFOW {
        return;
    }
    if REPAINT_POSTED.with(Cell::get) {
        return;
    }
    REPAINT_POSTED.with(|flag| flag.set(true));
    // SAFETY: posting to the live top-level window satisfies the message loop.
    if unsafe { PostMessageW(Some(hwnd), WM_APP_RESULTS_REPAINT, WPARAM(0), LPARAM(0)) }.is_err() {
        // A failed post must not keep the flag latched.
        REPAINT_POSTED.with(|flag| flag.set(false));
    }
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
        bar.replace_one,
        bar.replace_all,
        bar.case,
        bar.wrap,
        bar.close,
        bar.cancel,
    ] {
        show_window(hwnd, visible);
    }
}

/// Rebuilds an editable combo box from a history list.
///
/// `CB_RESETCONTENT` also empties the edit field of an editable combo, so the
/// text currently in the field is saved and restored; a search must never lose
/// what the user typed unless the user deletes it.
fn fill_history_combo(combo: HWND, input: HWND, patterns: &[String]) {
    let current = read_edit(combo).unwrap_or_default();
    unsafe {
        let _ = SendMessageW(combo, CB_RESETCONTENT, None, None);
        for pattern in patterns.iter().take(SEARCH_HISTORY_LIMIT) {
            let wide = to_wide(pattern);
            let _ = SendMessageW(
                combo,
                CB_ADDSTRING,
                None,
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
        if !current.is_empty() {
            // WM_SETTEXT fills the embedded edit without touching the list.
            let wide = to_wide(&current);
            let _ = SendMessageW(combo, WM_SETTEXT, None, Some(LPARAM(wide.as_ptr() as isize)));
            // Leave the caret after the restored text, ready for editing.
            let _ = SendMessageW(
                input,
                EM_SETSEL,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(-1)),
            );
        }
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

/// Returns the edit control embedded in an editable combo box.
fn combo_edit_handle(combo: HWND) -> Option<HWND> {
    let mut info = COMBOBOXINFO {
        cbSize: u32::try_from(size_of::<COMBOBOXINFO>()).ok()?,
        ..Default::default()
    };
    let _ = unsafe { GetComboBoxInfo(combo, &mut info) };
    (!info.hwndItem.is_invalid()).then_some(info.hwndItem)
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
        if let Err(err) = OpenClipboard(None) {
            // Ownership never transferred, so the block must be released.
            let _ = GlobalFree(Some(handle));
            return Err(err.to_string());
        }
        if let Err(err) = EmptyClipboard() {
            let _ = CloseClipboard();
            let _ = GlobalFree(Some(handle));
            return Err(err.to_string());
        }
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

fn build_menu(recent: &RecentFiles) -> Result<HMENU, String> {
    // SAFETY: menu handles are created and owned by this window.
    unsafe {
        let menu = CreateMenu().map_err(|err| err.to_string())?;
        let file = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(file, MF_STRING, ID_FILE_NEW, "New\tCtrl+N");
        append(file, MF_STRING, ID_FILE_OPEN, "Open...\tCtrl+O");
        append(file, MF_SEPARATOR, 0, "");
        let recent_menu = CreatePopupMenu().map_err(|err| err.to_string())?;
        if recent.paths().is_empty() {
            append(recent_menu, MF_STRING | MF_GRAYED, 0, "(no recent files)");
        } else {
            for (index, path) in recent.paths().iter().take(RECENT_LIMIT).enumerate() {
                let label = format!("{}. {}", index + 1, path.display());
                let id = ID_FILE_RECENT_FIRST + index as u32;
                append(recent_menu, MF_STRING, id, &label);
            }
        }
        menu_popup(file, recent_menu, "Recent Files");
        append(file, MF_SEPARATOR, 0, "");
        append(file, MF_STRING, ID_FILE_SAVE, "Save\tCtrl+S");
        append(file, MF_STRING, ID_FILE_SAVE_AS, "Save As...");
        append(file, MF_STRING, ID_FILE_RELOAD, "Reload\tCtrl+R");
        append(file, MF_STRING, ID_FILE_OPEN_FOLDER, "Open Containing Folder");
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
        append(edit, MF_STRING, ID_EDIT_REPLACE, "Replace\tCtrl+H");
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
        for (id, label) in CHARSET_MENU_ITEMS {
            append(encoding, MF_STRING, id, label);
        }
        append(encoding, MF_SEPARATOR, 0, "");
        append(encoding, MF_STRING, ID_BOM_KEEP, "Keep BOM");
        append(encoding, MF_STRING, ID_BOM_ADD, "Add BOM");
        append(encoding, MF_STRING, ID_BOM_STRIP, "Strip BOM");
        menu_popup(menu, encoding, "Encoding");

        let eol = CreatePopupMenu().map_err(|err| err.to_string())?;
        for (id, _, label) in EOL_MENU_ITEMS {
            append(eol, MF_STRING, id, label);
        }
        menu_popup(menu, eol, "EOL");

        let format = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(format, MF_STRING, ID_FORMAT_JSON, "Format JSON");
        append(format, MF_STRING, ID_FORMAT_MINIFY, "Minify JSON");
        append(format, MF_STRING, ID_FORMAT_DIAGNOSTIC, "Validate JSON");
        menu_popup(menu, format, "Format");

        let help = CreatePopupMenu().map_err(|err| err.to_string())?;
        append(help, MF_STRING, ID_HELP_ENABLE_LOG, "Enable Logging");
        append(help, MF_SEPARATOR, 0, "");
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

/// Enables or greys out one menu entry by command id.
fn set_menu_enabled(menu: HMENU, id: u32, enabled: bool) {
    let flags = MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED };
    // SAFETY: the menu handle is alive until track_menu destroys it.
    let _ = unsafe { EnableMenuItem(menu, id, flags) };
}

/// Routes editor right-clicks to the app-owned context menu.
///
/// Scintilla would otherwise show its built-in popup, which cannot host
/// application commands such as JSON formatting.
unsafe extern "system" fn editor_context_proc(
    hwnd: HWND,
    message: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
    _subclass: usize,
    _refdata: usize,
) -> LRESULT {
    if message == WM_CONTEXTMENU {
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        if !root.is_invalid() {
            unsafe {
                let _ = PostMessageW(
                    Some(root),
                    WM_COMMAND,
                    WPARAM(ID_EDIT_CONTEXT_MENU as usize),
                    LPARAM(0),
                );
            }
            return LRESULT(0);
        }
    }
    // SAFETY: forwarding to the original window procedure.
    unsafe { DefSubclassProc(hwnd, message, _wparam, _lparam) }
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
    // Save As appends this extension when the typed name has none, reusing the
    // extension of the file being edited whenever possible.
    let default_ext = to_wide(if save {
        initial
            .and_then(Path::extension)
            .and_then(|ext| ext.to_str())
            .unwrap_or(SAVE_DEFAULT_EXTENSION)
    } else {
        ""
    });
    let directory = initial
        .and_then(Path::parent)
        .map(|path| to_wide(&path.to_string_lossy()));
    // OFN_NOCHANGEDIR keeps the process working directory stable, which
    // matters for relative paths used elsewhere in the process.
    let mut flags = OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
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
        lpstrDefExt: if save {
            PCWSTR(default_ext.as_ptr())
        } else {
            PCWSTR::null()
        },
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
    refdata: usize,
) -> LRESULT {
    if message == WM_KEYDOWN {
        let key = wparam.0 as u32;
        // `refdata` carries the command Enter should run for this field.
        let command = if key == VK_RETURN.0 as u32 {
            Some(refdata)
        } else if key == VK_ESCAPE.0 as u32 {
            Some(ID_FIND_CLOSE as usize)
        } else {
            None
        };
        if let Some(command) = command {
            // The pattern field is the embedded edit of a combo box, so the
            // owning window is the top-level window, not the direct parent.
            let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
            if !root.is_invalid() {
                unsafe {
                    let _ = PostMessageW(
                        Some(root),
                        WM_COMMAND,
                        WPARAM(command),
                        LPARAM(0),
                    );
                }
                return LRESULT(0);
            }
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
        WM_SETCURSOR => {
            let hit_test = (lparam.0 & 0xFFFF) as u32;
            let resizing = with_app_result(hwnd, |app| app.splitter_cursor());
            if hit_test == HTCLIENT && resizing == Some(true) {
                LRESULT(1)
            } else {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_LBUTTONDOWN => {
            let handled = with_app_result(hwnd, |app| {
                app.begin_split_drag(lparam_x(lparam), lparam_y(lparam))
            });
            if handled == Some(true) {
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_MOUSEMOVE => {
            let handled = with_app_result(hwnd, |app| {
                app.drag_split(lparam_x(lparam), lparam_y(lparam))
            });
            if handled == Some(true) {
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_LBUTTONUP => {
            let handled = with_app_result(hwnd, |app| app.end_split_drag());
            if handled == Some(true) {
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_CAPTURECHANGED => {
            with_app(hwnd, |app| app.split_axis = None);
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
        WM_APP_RESULTS_REPAINT => {
            REPAINT_POSTED.with(|flag| flag.set(false));
            with_app(hwnd, App::repaint_results);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 as u32) & 0xFFFF;
            with_app(hwnd, |app| app.command(id));
            LRESULT(0)
        }
        WM_NOTIFY => {
            if IN_APP.with(Cell::get) {
                request_results_repaint(hwnd, lparam);
            }
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
            let proceed = with_app_result(hwnd, |app| app.confirm_exit());
            if matches!(proceed, Some(true)) {
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

/// Signed x coordinate of a client-area mouse message.
fn lparam_x(lparam: LPARAM) -> i32 {
    (lparam.0 & 0xFFFF) as i16 as i32
}

/// Signed y coordinate of a client-area mouse message.
fn lparam_y(lparam: LPARAM) -> i32 {
    ((lparam.0 >> 16) & 0xFFFF) as i16 as i32
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
            dwICC: ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES | ICC_WIN95_CLASSES,
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
    let recent = RecentFiles::load();
    let menu = build_menu(&recent)?;
    let mut app = Box::<App>::default();
    app.icons = icons;
    app.recent = recent;
    app.history = SearchHistory::load();
    app.replace_history = SearchHistory::load_named(REPLACE_STORAGE_FILE);
    let app = Box::into_raw(app);
    // SAFETY: the pointer is handed to the window; WM_NCCREATE stores it and
    // WM_NCDESTROY reclaims it.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW | WS_EX_CLIENTEDGE,
            w!("RNotepadMainWindow"),
            w!("RNotepad"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
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
    // The window owns focus right after being shown, so hand it to the editor.
    with_app(hwnd, |app| app.focus_active_view());
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
