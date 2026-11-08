//! Shared host for the self-drawn text and hex views.
//!
//! Both views are plain Win32 child windows.  The window procedure keeps its
//! state in `GWLP_USERDATA`; the state is UI-thread local (`Rc<RefCell<...>>`)
//! and is dropped on `WM_NCDESTROY`.  Painting itself lives in
//! [`crate::large_view`] and [`crate::hex_view`].

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, Once, RwLock};

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetStockObject, InvalidateRect, SetBkMode, UpdateWindow, HBRUSH,
    PAINTSTRUCT, TRANSPARENT, WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_DOWN, VK_END, VK_HOME,
    VK_NEXT, VK_PRIOR, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetParent, GetScrollInfo,
    GetWindowLongPtrW, LoadCursorW, MoveWindow, PostMessageW, RegisterClassW, SetWindowLongPtrW,
    ShowWindow, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_ARROW, SB_BOTTOM, SB_ENDSCROLL,
    SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP, SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP,
    SB_VERT, SCROLLINFO, SIF_ALL, SIF_DISABLENOSCROLL, SIF_PAGE, SIF_POS, SIF_RANGE, SW_SHOW,
    WNDCLASSW, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SETFOCUS, WM_SIZE, WM_VSCROLL,
    WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_HSCROLL, WS_VISIBLE, WS_VSCROLL,
};

use rnotepad_core::bytes::ByteSource;
use rnotepad_core::coord::RawByteOffset;
use rnotepad_core::document::Document;
use rnotepad_core::encoding::unit_len;

use crate::dw::MonoFont;
use crate::hex_view::ROW_GAP_PIXELS;
use crate::error::{WinError, WinResult};

/// Row height used before the first text measurement.
const FALLBACK_ROW_PIXELS: i32 = 16;
/// Document shared by every view of the same file.
pub type SharedDoc = Arc<RwLock<Document>>;

/// Posted to the parent when a view selection changes.
///
/// `wParam` is [`VIEW_TAG_TEXT`] or [`VIEW_TAG_HEX`]; the parent maps the raw
/// selection back into the other view.
pub const WM_APP_VIEW_SYNC: u32 = 0x8000 + 1;
/// Posted to the parent when Ctrl+C is pressed inside a self-drawn view.
pub const WM_APP_VIEW_COPY: u32 = 0x8000 + 2;
/// `wParam` tag identifying the read-only text view.
pub const VIEW_TAG_TEXT: usize = 0;
/// `wParam` tag identifying the hex view.
pub const VIEW_TAG_HEX: usize = 1;

/// Which painter a [`ViewWindow`] uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Text,
    Hex,
}

/// Viewport state shared between the application and the window procedure.
pub struct ViewData {
    pub doc: Option<SharedDoc>,
    /// First visible logical line (text view).
    pub first_line: u32,
    /// First visible 16-byte row (hex view).
    pub first_row: u64,
    /// Sorted raw byte ranges highlighted by the current search.
    pub highlights: Vec<Range<u64>>,
    /// Selected raw byte range, if any.
    pub sel: Option<Range<u64>>,
}

impl Default for ViewData {
    fn default() -> Self {
        Self {
            doc: None,
            first_line: 0,
            first_row: 0,
            highlights: Vec::new(),
            sel: None,
        }
    }
}

/// UI-thread handle to the viewport state.
pub type SharedViewData = Rc<RefCell<ViewData>>;

const CLASS_NAME: windows::core::PCWSTR = w!("RNotepadViewWindow");
const MAX_LINE_COUNT_HINT: u32 = 1_000_000;
const WHEEL_LINES: i32 = 3;
const KEY_C: u32 = 0x43;
/// Win32 `MK_LBUTTON` flag carried in `WM_MOUSEMOVE`'s `wParam`.
const MK_LBUTTON_BIT: u32 = 0x0001;

struct ViewSlot {
    kind: ViewKind,
    data: SharedViewData,
    font: Option<MonoFont>,
    /// Raw offset where the current mouse selection started.
    anchor: Option<u64>,
}

/// A self-drawn read-only view window.
pub struct ViewWindow {
    hwnd: HWND,
    data: SharedViewData,
    kind: ViewKind,
}

impl ViewWindow {
    /// Creates the child window.
    pub fn create(parent: HWND, kind: ViewKind, data: SharedViewData) -> WinResult<Self> {
        register_class();
        let slot = Box::new(ViewSlot {
            kind,
            data: data.clone(),
            font: None,
            anchor: None,
        });
        let slot_ptr = Box::into_raw(slot);
        // SAFETY: the window class is registered and `slot_ptr` is handed to
        // the new window, which takes ownership in `WM_NCDESTROY`.
        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                CLASS_NAME,
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_HSCROLL | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                Some(HINSTANCE(GetModuleHandleW(None)
                    .map_err(|err| WinError::from_windows("GetModuleHandleW", err))?
                    .0)),
                Some(slot_ptr as *const std::ffi::c_void),
            )
        };
        match hwnd {
            Ok(hwnd) => {
                // SAFETY: showing a window created by this thread.
                let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
                let _ = unsafe { UpdateWindow(hwnd) };
                Ok(Self { hwnd, data, kind })
            }
            Err(err) => {
                // SAFETY: `slot_ptr` was not consumed because creation failed.
                drop(unsafe { Box::from_raw(slot_ptr) });
                Err(WinError::from_windows("CreateWindowExW", err))
            }
        }
    }

    #[must_use]
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    #[must_use]
    pub fn data(&self) -> SharedViewData {
        self.data.clone()
    }

    #[must_use]
    pub fn kind(&self) -> ViewKind {
        self.kind
    }

    /// Moves and resizes the window.
    pub fn set_rect(&self, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: `self.hwnd` is a live child window.
        let _ = unsafe { MoveWindow(self.hwnd, x, y, width.max(0), height.max(0), true) };
    }

    /// Recomputes the scrollbar range after the document or viewport changed.
    pub fn refresh_scroll(&self) {
        // SAFETY: the window is live and the slot belongs to it.
        let line_height = unsafe { slot(self.hwnd) }
            .and_then(|state| state.font.as_ref())
            .map(|font| font.line_height);
        // SAFETY: live window; the scroll info struct is fully initialized.
        unsafe {
            update_scroll(self.hwnd, self.kind, &self.data, line_height);
        }
    }

    /// Forces a repaint.
    pub fn invalidate(&self) {
        // SAFETY: live window.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    /// Scrolls to the last page; call [`Self::refresh_scroll`] first so the
    /// scroll range reflects the current document.
    pub fn scroll_to_end(&self) {
        // SAFETY: live window owned by this handle.
        unsafe {
            if let Some(slot) = slot(self.hwnd) {
                scroll_to(self.hwnd, slot, false);
            }
        }
    }
}

impl Drop for ViewWindow {
    fn drop(&mut self) {
        // SAFETY: destroying a live child window owned by this object.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

static CLASS_REGISTERED: Once = Once::new();

fn register_class() {
    CLASS_REGISTERED.call_once(|| {
        // SAFETY: registering a class with a static name and a valid wndproc.
        unsafe {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                hIcon: Default::default(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                lpszMenuName: w!(""),
                lpszClassName: CLASS_NAME,
            };
            let _ = RegisterClassW(&wc);
        }
    });
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let create = lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW;
            if !create.is_null() {
                // SAFETY: WM_NCCREATE always carries a CREATESTRUCTW pointer.
                let slot = unsafe { (*create).lpCreateParams } as *mut ViewSlot;
                // SAFETY: the pointer was produced by `Box::into_raw` in
                // `ViewWindow::create` and is transferred to the window here.
                let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, slot as isize) };
            }
            LRESULT(1)
        }
        WM_NCDESTROY => {
            // SAFETY: reclaims the box that was stored at creation.
            let slot_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ViewSlot;
            if !slot_ptr.is_null() {
                let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
                drop(unsafe { Box::from_raw(slot_ptr) });
            }
            // SAFETY: default processing for the destroy notification.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SIZE => {
            // SAFETY: live window and slot pointer.
            unsafe {
                let slot = slot(hwnd);
                if let Some(slot) = slot {
                    let line_height = slot.font.as_ref().map(|font| font.line_height);
                    update_scroll(hwnd, slot.kind, &slot.data, line_height);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            // SAFETY: standard paint sequence paired with EndPaint.
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                if let Some(slot) = slot(hwnd) {
                    paint(hwnd, hdc, slot);
                }
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_VSCROLL => {
            // SAFETY: live window.
            unsafe {
                if let Some(slot) = slot(hwnd) {
                    apply_vscroll(hwnd, slot, (wparam.0 as u32) as i32);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            // SAFETY: live window.
            unsafe {
                if let Some(slot) = slot(hwnd) {
                    let lines = if delta >= 0 { WHEEL_LINES } else { -WHEEL_LINES };
                    scroll_lines(hwnd, slot, lines);
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let key = wparam.0 as u32;
            // SAFETY: live window.
            unsafe {
                if let Some(slot) = slot(hwnd) {
                    if key == KEY_C && GetKeyState(VK_CONTROL.0 as i32) < 0 {
                        if let Some(parent) = GetParent(hwnd).ok() {
                            let _ = PostMessageW(Some(parent), WM_APP_VIEW_COPY, WPARAM(0), LPARAM(0));
                        }
                    }
                    match key {
                        k if k == VK_UP.0 as u32 => scroll_lines(hwnd, slot, -1),
                        k if k == VK_DOWN.0 as u32 => scroll_lines(hwnd, slot, 1),
                        k if k == VK_PRIOR.0 as u32 => scroll_pages(hwnd, slot, -1),
                        k if k == VK_NEXT.0 as u32 => scroll_pages(hwnd, slot, 1),
                        k if k == VK_HOME.0 as u32 => scroll_to(hwnd, slot, true),
                        k if k == VK_END.0 as u32 => scroll_to(hwnd, slot, false),
                        _ => {}
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // SAFETY: focusing our own child window and starting a selection.
            unsafe {
                let _ = SetFocus(Some(hwnd));
                if let Some(slot) = slot(hwnd) {
                    if let Some(offset) = hit_test(hwnd, slot, lparam) {
                        let end = selection_end(slot, offset);
                        slot.anchor = Some(offset);
                        if let Ok(mut data) = slot.data.try_borrow_mut() {
                            data.sel = Some(offset..end);
                        }
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        SetCapture(hwnd);
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // SAFETY: live window; dragging extends the selection.
            unsafe {
                if let Some(slot) = slot(hwnd) {
                    if slot.anchor.is_some() && (wparam.0 as u32 & MK_LBUTTON_BIT) != 0 {
                        if let Some(offset) = hit_test(hwnd, slot, lparam) {
                            if let Some(anchor) = slot.anchor {
                                let (first, second) = ordered(anchor, offset);
                                let end = selection_end(slot, second);
                                if let Ok(mut data) = slot.data.try_borrow_mut() {
                                    data.sel = Some(first..end);
                                }
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // SAFETY: live window; ends the drag and tells the parent to sync.
            unsafe {
                let _ = ReleaseCapture();
                if let Some(slot) = slot(hwnd) {
                    if slot.anchor.take().is_some() {
                        if let Ok(parent) = GetParent(hwnd) {
                            let tag = match slot.kind {
                                ViewKind::Text => VIEW_TAG_TEXT,
                                ViewKind::Hex => VIEW_TAG_HEX,
                            };
                            let _ = PostMessageW(
                                Some(parent),
                                WM_APP_VIEW_SYNC,
                                WPARAM(tag),
                                LPARAM(0),
                            );
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_SETFOCUS => {
            // SAFETY: live window.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: forwards unhandled messages to the default proc.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// Returns the unit-complete end offset for a selection that starts at
/// `offset`, so a UTF-8 or multi-byte character is never split.
fn selection_end(slot: &ViewSlot, offset: u64) -> u64 {
    let Ok(data) = slot.data.try_borrow() else {
        return offset.saturating_add(1);
    };
    let Some(doc) = data.doc.as_ref() else {
        return offset.saturating_add(1);
    };
    let Ok(doc) = doc.read() else {
        return offset.saturating_add(1);
    };
    match slot.kind {
        ViewKind::Hex => offset.saturating_add(1).min(doc.raw.len()),
        ViewKind::Text => {
            let Some(line) = doc.line_index.line_of(RawByteOffset(offset)) else {
                return offset.saturating_add(1);
            };
            let Some(range) = doc.line_index.line_range(line) else {
                return offset.saturating_add(1);
            };
            let Some(raw) = doc.raw.slice(range.start..range.end) else {
                return offset.saturating_add(1);
            };
            let local = usize::try_from(offset.saturating_sub(range.start)).unwrap_or(0);
            let unit = unit_len(&raw, local, doc.encoding.charset).max(1) as u64;
            offset.saturating_add(unit).min(range.end)
        }
    }
}

/// Maps a mouse position inside a view to a raw byte offset.
unsafe fn hit_test(hwnd: HWND, slot: &ViewSlot, lparam: LPARAM) -> Option<u64> {
    let font = slot.font.as_ref()?;
    let data = slot.data.try_borrow().ok()?;
    let doc = data.doc.as_ref()?.read().ok()?;
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    let mut rect = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    let _ = rect;
    match slot.kind {
        ViewKind::Text => crate::large_view::hit_test(&doc, &data, font, x, y),
        ViewKind::Hex => crate::hex_view::hit_test(&doc, &data, font, x, y),
    }
}

fn ordered(first: u64, second: u64) -> (u64, u64) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

/// # Safety
/// `hwnd` must be a live window whose `GWLP_USERDATA` holds a `ViewSlot`.
unsafe fn slot(hwnd: HWND) -> Option<&'static mut ViewSlot> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ViewSlot;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: the pointer is owned by this window until WM_NCDESTROY.
        Some(unsafe { &mut *ptr })
    }
}

unsafe fn paint(hwnd: HWND, hdc: windows::Win32::Graphics::Gdi::HDC, slot: &mut ViewSlot) {
    let mut rect = RECT::default();
    // SAFETY: valid out parameter.
    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    // SAFETY: valid HDC during WM_PAINT.
    unsafe {
        let brush = HBRUSH(GetStockObject(WHITE_BRUSH).0);
        windows::Win32::Graphics::Gdi::FillRect(hdc, &rect, brush);
        let _ = SetBkMode(hdc, TRANSPARENT);
    }
    if slot.font.is_none() {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        match MonoFont::new(dpi) {
            Ok(mut font) => {
                let _ = font.measure(hdc);
                slot.font = Some(font);
            }
            Err(_) => return,
        }
    }
    let Some(font) = slot.font.as_ref() else {
        return;
    };
    let data = match slot.data.try_borrow() {
        Ok(data) => data,
        Err(_) => return, // Re-entrant paint; skip instead of panicking.
    };
    let Some(doc) = data.doc.as_ref() else {
        return;
    };
    let Ok(doc) = doc.read() else {
        return;
    };
    match slot.kind {
        ViewKind::Text => crate::large_view::paint(hdc, &rect, &doc, &data, font),
        ViewKind::Hex => crate::hex_view::paint(hdc, &rect, &doc, &data, font),
    }
}

unsafe fn apply_vscroll(hwnd: HWND, slot: &mut ViewSlot, code: i32) {
    let mut info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        ..Default::default()
    };
    if unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) }.is_err() {
        return;
    }
    let pos = info.nPos;
    match code {
        c if c == SB_LINEUP.0 => scroll_lines(hwnd, slot, -1),
        c if c == SB_LINEDOWN.0 => scroll_lines(hwnd, slot, 1),
        c if c == SB_PAGEUP.0 => scroll_pages(hwnd, slot, -1),
        c if c == SB_PAGEDOWN.0 => scroll_pages(hwnd, slot, 1),
        c if c == SB_TOP.0 => set_pos(hwnd, slot, info.nMin as u64),
        c if c == SB_BOTTOM.0 => set_pos(
            hwnd,
            slot,
            (info.nMax as i64 - info.nPage as i64 + 1).max(0) as u64,
        ),
        c if c == SB_THUMBTRACK.0 || c == SB_THUMBPOSITION.0 => {
            let _ = pos;
            set_pos(hwnd, slot, info.nTrackPos as u64);
        }
        c if c == SB_ENDSCROLL.0 => {}
        _ => {}
    }
}

unsafe fn scroll_lines(hwnd: HWND, slot: &mut ViewSlot, delta: i32) {
    let current = current_pos(slot);
    let next = (current as i64 + delta as i64).max(0) as u64;
    set_pos(hwnd, slot, next);
}

unsafe fn scroll_pages(hwnd: HWND, slot: &mut ViewSlot, direction: i32) {
    let mut info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_PAGE,
        ..Default::default()
    };
    // SAFETY: live window.
    let _ = unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) };
    let page = info.nPage.max(1) as i32;
    scroll_lines(hwnd, slot, direction * page);
}

unsafe fn scroll_to(hwnd: HWND, slot: &mut ViewSlot, top: bool) {
    let mut info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE,
        ..Default::default()
    };
    // SAFETY: live window.
    let _ = unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) };
    let target = if top {
        info.nMin as u64
    } else {
        (info.nMax as i64 - info.nPage as i64 + 1).max(0) as u64
    };
    set_pos(hwnd, slot, target);
}

fn current_pos(slot: &ViewSlot) -> u64 {
    match slot.kind {
        ViewKind::Text => slot.data.borrow().first_line as u64,
        ViewKind::Hex => slot.data.borrow().first_row,
    }
}

unsafe fn set_pos(hwnd: HWND, slot: &mut ViewSlot, value: u64) {
    {
        let mut data = slot.data.borrow_mut();
        match slot.kind {
            ViewKind::Text => data.first_line = value.min(u32::MAX as u64) as u32,
            ViewKind::Hex => data.first_row = value,
        }
    }
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_POS,
        nPos: value.min(i32::MAX as u64) as i32,
        ..Default::default()
    };
    // SAFETY: live window.
    unsafe {
        SetScrollInfo(hwnd, SB_VERT, &info, true);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    let _ = info;
}

/// # Safety
/// `hwnd` must be a live view window.
unsafe fn update_scroll(
    hwnd: HWND,
    kind: ViewKind,
    data: &SharedViewData,
    line_height: Option<i32>,
) {
    let mut rect = RECT::default();
    // SAFETY: valid out parameter.
    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    let data = data.borrow();
    let (total, page) = match data.doc.as_ref() {
        Some(doc) => {
            let Ok(doc) = doc.read() else { return };
            let row_height = line_height.unwrap_or(FALLBACK_ROW_PIXELS);
            match kind {
                ViewKind::Text => {
                    let lines = doc
                        .line_index
                        .total_lines()
                        .unwrap_or_else(|| doc.line_index.scanned_line_hint())
                        .min(MAX_LINE_COUNT_HINT);
                    (lines as u64, estimate_page(rect.bottom, row_height, 0))
                }
                ViewKind::Hex => {
                    let rows = doc.raw.len() / 16 + 1;
                    (
                        rows,
                        estimate_page(rect.bottom, row_height + ROW_GAP_PIXELS, 1),
                    )
                }
            }
        }
        None => (0, 1),
    };
    let mut info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
        nMin: 0,
        nMax: total.min(i32::MAX as u64) as i32,
        nPage: page.max(1),
        nPos: match kind {
            ViewKind::Text => data.first_line as i32,
            ViewKind::Hex => data.first_row.min(i32::MAX as u64) as i32,
        },
        ..Default::default()
    };
    // SAFETY: live window.
    unsafe {
        SetScrollInfo(hwnd, SB_VERT, &info, true);
    }
    let _ = &mut info;
}

/// Positive row count that fits the viewport below the column header.
fn estimate_page(height: i32, row_height: i32, header_rows: i32) -> u32 {
    let rows = height.max(1) / row_height.max(1);
    (rows - header_rows).max(1) as u32
}

/// Applies a highlight colour to a range, used by both painters.
pub fn highlight_ranges_for_line(
    highlights: &[Range<u64>],
    line_start: u64,
    line_end: u64,
) -> &[Range<u64>] {
    if highlights.is_empty() {
        return &[];
    }
    let start = highlights.partition_point(|range| range.end <= line_start);
    let end = highlights.partition_point(|range| range.start < line_end);
    &highlights[start..end]
}
