//! Application icon loaded from the bundled `.ico` asset.
//!
//! `build.rs` embeds the same file into the executable resources, and this
//! module turns it into live `HICON` handles for the window class, title bar,
//! taskbar and Alt+Tab list.  Only the `.ico` asset is used so every surface
//! shows exactly the artwork that ships with the executable.

use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, GetSystemMetrics, SendMessageW, HICON, ICON_BIG,
    ICON_SMALL, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, WM_SETICON,
};

use crate::error::{WinError, WinResult};

/// The single application icon shared by the executable and the window.
const ICON_FILE: &[u8] = include_bytes!("../../assets/icons/RNotepad.ico");
/// `ICONDIR` header: reserved, type and image count, all 16-bit.
const ICONDIR_BYTES: usize = 6;
/// `ICONDIRENTRY` record describing one stored image.
const ICONDIRENTRY_BYTES: usize = 16;
/// `ICONDIR.idType` value for icon files; `2` denotes a cursor.
const ICONDIR_TYPE_ICON: u16 = 1;
/// Byte offset of `ICONDIRENTRY.dwBytesInRes`.
const ENTRY_LENGTH_OFFSET: usize = 8;
/// Byte offset of `ICONDIRENTRY.dwImageOffset`.
const ENTRY_OFFSET_OFFSET: usize = 12;
/// A zero width or height encodes 256 pixels in an `ICONDIRENTRY`.
const ENTRY_EDGE_256: u32 = 256;
/// `dwVer` for version 3.00 icons, the format shipped since Windows 95.
const ICON_RESOURCE_VERSION: u32 = 0x0003_0000;
/// Fallback icon edge in pixels when the system metric is unavailable.
const FALLBACK_LARGE_ICON: i32 = 32;
/// Fallback small icon edge in pixels when the system metric is unavailable.
const FALLBACK_SMALL_ICON: i32 = 16;

/// One image stored inside the icon file.
struct IconImage<'a> {
    bytes: &'a [u8],
    edge: u32,
}

/// Owned large and small window icons.
pub struct WindowIcons {
    large: HICON,
    small: HICON,
}

impl WindowIcons {
    /// Creates both icon sizes from the bundled `.ico`; the caller keeps the
    /// result for the life of the window because `HICON` handles must stay
    /// valid while in use.
    pub fn load() -> WinResult<Self> {
        let large_edge = icon_edge(
            unsafe { GetSystemMetrics(SM_CXICON) },
            unsafe { GetSystemMetrics(SM_CYICON) },
            FALLBACK_LARGE_ICON,
        );
        let small_edge = icon_edge(
            unsafe { GetSystemMetrics(SM_CXSMICON) },
            unsafe { GetSystemMetrics(SM_CYSMICON) },
            FALLBACK_SMALL_ICON,
        );
        let large = create_icon(large_edge)?;
        let small = match create_icon(small_edge) {
            Ok(small) => small,
            Err(err) => {
                // SAFETY: the large handle was created above and is not shared.
                unsafe {
                    let _ = DestroyIcon(large);
                }
                return Err(err);
            }
        };
        Ok(Self { large, small })
    }

    /// Large icon handle, suitable for `WNDCLASSEXW::hIcon`.
    pub fn large(&self) -> HICON {
        self.large
    }

    /// Small icon handle, suitable for `WNDCLASSEXW::hIconSm`.
    pub fn small(&self) -> HICON {
        self.small
    }

    /// Applies the icons to the window title bar, taskbar and Alt+Tab list.
    pub fn apply(&self, hwnd: HWND) {
        // SAFETY: `hwnd` belongs to this thread; the icons outlive the calls
        // because `Self` owns them until the window is destroyed.
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(self.large.0 as isize)),
            );
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(self.small.0 as isize)),
            );
        }
    }
}

impl Drop for WindowIcons {
    fn drop(&mut self) {
        // SAFETY: each handle is owned and destroyed exactly once.
        unsafe {
            let _ = DestroyIcon(self.large);
            let _ = DestroyIcon(self.small);
        }
    }
}

fn icon_edge(primary: i32, secondary: i32, fallback: i32) -> u32 {
    let edge = primary.min(secondary);
    u32::try_from(if edge > 0 { edge } else { fallback }).unwrap_or(fallback as u32)
}

/// Creates a system-sized `HICON` from the image closest to `edge`.
fn create_icon(edge: u32) -> WinResult<HICON> {
    let image = select_image(ICON_FILE, edge)?;
    let size = i32::try_from(edge).map_err(|_| WinError::new("icon size out of range"))?;
    // SAFETY: `image.bytes` is a validated, self-contained icon image; the
    // returned handle is owned by the caller.
    unsafe {
        CreateIconFromResourceEx(
            image.bytes,
            true,
            ICON_RESOURCE_VERSION,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
    }
    .map_err(|err| WinError::from_windows("CreateIconFromResourceEx", err))
}

/// Picks the smallest stored image that still covers `desired`; when every
/// image is smaller, the largest one is used so scaling never invents detail.
fn select_image(data: &[u8], desired: u32) -> WinResult<IconImage<'_>> {
    if data.len() < ICONDIR_BYTES {
        return Err(WinError::new("icon file is shorter than the ICONDIR header"));
    }
    let reserved = read_u16(data, 0)?;
    let kind = read_u16(data, 2)?;
    let count = read_u16(data, 4)?;
    if reserved != 0 || kind != ICONDIR_TYPE_ICON || count == 0 {
        return Err(WinError::new(format!(
            "not an icon file: reserved={reserved}, type={kind}, images={count}"
        )));
    }

    let mut smallest_fit: Option<IconImage<'_>> = None;
    let mut largest: Option<IconImage<'_>> = None;
    for index in 0..count as usize {
        let entry = ICONDIR_BYTES + index * ICONDIRENTRY_BYTES;
        if entry + ICONDIRENTRY_BYTES > data.len() {
            return Err(WinError::new(format!(
                "icon directory entry {index} lies outside the file"
            )));
        }
        let width = entry_edge(data[entry]);
        let height = entry_edge(data[entry + 1]);
        let length = read_u32(data, entry + ENTRY_LENGTH_OFFSET)? as usize;
        let offset = read_u32(data, entry + ENTRY_OFFSET_OFFSET)? as usize;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| WinError::new(format!("icon image {index} overflows the file")))?;
        if length == 0 || end > data.len() {
            return Err(WinError::new(format!(
                "icon image {index} lies outside the file"
            )));
        }
        let image = IconImage {
            bytes: &data[offset..end],
            edge: width.max(height),
        };
        if image.edge >= desired {
            if smallest_fit
                .as_ref()
                .is_none_or(|best| image.edge < best.edge)
            {
                smallest_fit = Some(image);
            }
        } else if largest.as_ref().is_none_or(|best| image.edge > best.edge) {
            largest = Some(image);
        }
    }

    smallest_fit
        .or(largest)
        .ok_or_else(|| WinError::new("icon file contains no usable image"))
}

/// Converts an `ICONDIRENTRY` dimension byte into pixels.
fn entry_edge(value: u8) -> u32 {
    if value == 0 {
        ENTRY_EDGE_256
    } else {
        u32::from(value)
    }
}

/// Reads a little-endian `u16` at `offset`.
fn read_u16(data: &[u8], offset: usize) -> WinResult<u16> {
    let bytes = data
        .get(offset..offset + size_of::<u16>())
        .ok_or_else(|| WinError::new(format!("icon field at {offset} lies outside the file")))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// Reads a little-endian `u32` at `offset`.
fn read_u32(data: &[u8], offset: usize) -> WinResult<u32> {
    let bytes = data
        .get(offset..offset + size_of::<u32>())
        .ok_or_else(|| WinError::new(format!("icon field at {offset} lies outside the file")))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the bundled icon can serve both system metric sizes.
    #[test]
    fn bundled_ico_contains_images_for_both_metrics() {
        for edge in [FALLBACK_SMALL_ICON as u32, FALLBACK_LARGE_ICON as u32] {
            let image = select_image(ICON_FILE, edge).expect("icon directory");
            assert!(!image.bytes.is_empty(), "missing image for {edge}px");
        }
    }

    /// The selection must prefer an image that covers the requested size.
    #[test]
    fn selection_prefers_the_smallest_covering_image() {
        let image = select_image(ICON_FILE, 64).expect("icon directory");
        let largest = select_image(ICON_FILE, ENTRY_EDGE_256).expect("icon directory");
        assert!(image.edge <= largest.edge);
    }
}
