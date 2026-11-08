//! Application icon loaded from the bundled SVG assets.
//!
//! Windows has no GDI entry point for SVG, but Direct2D exposes an SVG
//! document renderer (`ID2D1DeviceContext5::CreateSvgDocument`).  The icons
//! are rasterised once at startup into a 32-bit DIB and wrapped in `HICON`
//! handles, so the title bar and taskbar show the same artwork that ships in
//! `crates/assets/icons`.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr;

use windows::core::{BOOL, Interface};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Device, ID2D1DeviceContext5, ID2D1Factory1, ID2D1Image,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_MAP_OPTIONS_READ,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_9_1, D3D_FEATURE_LEVEL_9_2,
    D3D_FEATURE_LEVEL_9_3,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, ID3D11Device,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, ICON_BIG, ICON_SMALL,
    SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
};

use crate::error::{WinError, WinResult};

/// Large application icon rendered at the system icon size.
const LARGE_ICON_SVG: &str = include_str!("../../assets/icons/RNotepad.svg");
/// Small application icon artwork, simplified for 16 px displays.
const SMALL_ICON_SVG: &str = include_str!("../../assets/icons/RNotepad-16.svg");
/// The SVG viewport declared by both assets; used only to strip the fixed
/// `width`/`height` attributes so the artwork scales to the target size.
const SVG_DECLARED_SIZE: &str = "256";
/// Fallback icon edge in pixels when the system metric is unavailable.
const FALLBACK_LARGE_ICON: i32 = 32;
/// Fallback small icon edge in pixels when the system metric is unavailable.
const FALLBACK_SMALL_ICON: i32 = 16;
/// Direct2D renders at screen resolution; icons are pixel-sized.
const ICON_DPI: f32 = 96.0;
/// Bytes per BGRA pixel.
const BYTES_PER_PIXEL: usize = 4;
/// Feature levels accepted by the D3D device, newest first.
const FEATURE_LEVELS: [D3D_FEATURE_LEVEL; 6] = [
    D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_9_3,
    D3D_FEATURE_LEVEL_9_2,
    D3D_FEATURE_LEVEL_9_1,
];

/// Owned large and small window icons.
pub struct WindowIcons {
    large: HICON,
    small: HICON,
}

impl WindowIcons {
    /// Rasterises both bundled SVGs; the caller keeps the result for the life
    /// of the window because `HICON` handles must stay valid while in use.
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
        let large_pixels = rasterise(LARGE_ICON_SVG, large_edge)?;
        let small_pixels = rasterise(SMALL_ICON_SVG, small_edge)?;
        let large = create_icon(&large_pixels, large_edge)?;
        let small = create_icon(&small_pixels, small_edge)?;
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

/// Renders an SVG document into straight-alpha BGRA pixels.
fn rasterise(svg: &str, edge: u32) -> WinResult<Vec<u8>> {
    // The root `width`/`height` attributes pin the artwork to 256 DIPs; the
    // viewport size passed to Direct2D must win so the icon scales down.
    let scalable = svg
        .replace(&format!(" width=\"{SVG_DECLARED_SIZE}\""), "")
        .replace(&format!(" height=\"{SVG_DECLARED_SIZE}\""), "");
    let stream = unsafe { SHCreateMemStream(Some(scalable.as_bytes())) }
        .ok_or_else(|| WinError::new("SHCreateMemStream returned a null stream"))?;

    let dxgi = create_dxgi_device()?;
    let factory: ID2D1Factory1 = unsafe {
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
    }
    .map_err(|err| WinError::from_windows("D2D1CreateFactory", err))?;
    let device: ID2D1Device =
        unsafe { factory.CreateDevice(&dxgi) }.map_err(|err| WinError::from_windows("CreateDevice", err))?;
    let context: ID2D1DeviceContext5 = unsafe {
        device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
    }
    .map_err(|err| WinError::from_windows("CreateDeviceContext", err))?
    .cast()
    .map_err(|err| WinError::from_windows("ID2D1DeviceContext5", err))?;

    let properties = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: ICON_DPI,
        dpiY: ICON_DPI,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
        colorContext: ManuallyDrop::new(None),
    };
    let target = unsafe {
        context.CreateBitmap(
            D2D_SIZE_U {
                width: edge,
                height: edge,
            },
            None,
            0,
            &properties,
        )
    }
    .map_err(|err| WinError::from_windows("CreateBitmap", err))?;
    let image: ID2D1Image = target
        .cast()
        .map_err(|err| WinError::from_windows("ID2D1Bitmap1 -> ID2D1Image", err))?;
    // SAFETY: `image` is the bitmap created above and lives until the context
    // is dropped; SetTarget has no failure mode.
    unsafe { context.SetTarget(&image) };

    let document = unsafe {
        context.CreateSvgDocument(
            &stream,
            D2D_SIZE_F {
                width: edge as f32,
                height: edge as f32,
            },
        )
    }
    .map_err(|err| WinError::from_windows("CreateSvgDocument", err))?;
    // SAFETY: the target is set and every drawing call below is paired with
    // EndDraw; BeginDraw returns `()` on the render-target interface.
    unsafe {
        context.BeginDraw();
        context.DrawSvgDocument(&document);
        context.EndDraw(None, None)
    }
    .map_err(|err| WinError::from_windows("EndDraw", err))?;

    // Render-target bitmaps are not CPU readable; copy into a staging bitmap
    // created with CANNOT_DRAW | CPU_READ, which `Map` accepts.
    let staging_properties = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: ICON_DPI,
        dpiY: ICON_DPI,
        bitmapOptions: D2D1_BITMAP_OPTIONS_CANNOT_DRAW | D2D1_BITMAP_OPTIONS_CPU_READ,
        colorContext: ManuallyDrop::new(None),
    };
    let staging = unsafe {
        context.CreateBitmap(
            D2D_SIZE_U {
                width: edge,
                height: edge,
            },
            None,
            0,
            &staging_properties,
        )
    }
    .map_err(|err| WinError::from_windows("CreateBitmap (staging)", err))?;
    let source: ID2D1Bitmap = target
        .cast()
        .map_err(|err| WinError::from_windows("ID2D1Bitmap1 -> ID2D1Bitmap", err))?;
    unsafe { staging.CopyFromBitmap(None, &source, None) }
        .map_err(|err| WinError::from_windows("CopyFromBitmap", err))?;
    let mapped = unsafe { staging.Map(D2D1_MAP_OPTIONS_READ) }
        .map_err(|err| WinError::from_windows("ID2D1Bitmap1::Map", err))?;
    let pixels = copy_straight_alpha(&mapped, edge);
    // SAFETY: the mapping was created above and is released exactly once.
    let _ = unsafe { staging.Unmap() };
    Ok(pixels)
}

/// Copies premultiplied Direct2D pixels into straight-alpha BGRA for `HICON`,
/// which interprets the alpha channel without premultiplication.
fn copy_straight_alpha(mapped: &windows::Win32::Graphics::Direct2D::D2D1_MAPPED_RECT, edge: u32) -> Vec<u8> {
    let row_bytes = edge as usize * BYTES_PER_PIXEL;
    let mut pixels = vec![0u8; row_bytes * edge as usize];
    for y in 0..edge as usize {
        // SAFETY: the mapping covers `edge` rows of at least `pitch` bytes.
        let source_row = unsafe { mapped.bits.add(y * mapped.pitch as usize) };
        let destination_row = y * row_bytes;
        for x in 0..edge as usize {
            // SAFETY: `x` stays inside the mapped row.
            let source = unsafe { source_row.add(x * BYTES_PER_PIXEL) };
            let alpha = unsafe { *source.add(3) };
            if alpha == 0 {
                continue;
            }
            for channel in 0..3 {
                // SAFETY: BGRA channels live at offsets 0..=2.
                let value = unsafe { *source.add(channel) };
                let straight = (value as u32 * 255 / alpha as u32).min(255) as u8;
                pixels[destination_row + x * BYTES_PER_PIXEL + channel] = straight;
            }
            pixels[destination_row + x * BYTES_PER_PIXEL + 3] = alpha;
        }
    }
    pixels
}

fn create_dxgi_device() -> WinResult<IDXGIDevice> {
    match create_d3d_device(D3D_DRIVER_TYPE_HARDWARE) {
        Ok(dxgi) => Ok(dxgi),
        Err(hardware_error) => {
            tracing::warn!(error = %hardware_error, "hardware device unavailable, trying WARP");
            create_d3d_device(D3D_DRIVER_TYPE_WARP)
        }
    }
}

fn create_d3d_device(driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE) -> WinResult<IDXGIDevice> {
    let mut device: Option<ID3D11Device> = None;
    // SAFETY: all out parameters are valid for the duration of the call.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&FEATURE_LEVELS),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }
    .map_err(|err| WinError::from_windows("D3D11CreateDevice", err))?;
    let device = device.ok_or_else(|| WinError::new("D3D11CreateDevice returned no device"))?;
    device
        .cast()
        .map_err(|err| WinError::from_windows("ID3D11Device -> IDXGIDevice", err))
}

fn create_icon(pixels: &[u8], edge: u32) -> WinResult<HICON> {
    let size = i32::try_from(edge).map_err(|_| WinError::new("icon size out of range"))?;
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: size,
        biHeight: -size,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: (edge * edge) as u32 * BYTES_PER_PIXEL as u32,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let mut bits: *mut c_void = ptr::null_mut();
    // SAFETY: `info` is a valid bitmap description; `bits` receives the buffer.
    let color = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|err| WinError::from_windows("CreateDIBSection", err))?;
    if bits.is_null() {
        // SAFETY: the bitmap was created above and is not used further.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(color.0));
        }
        return Err(WinError::new("CreateDIBSection returned a null pixel buffer"));
    }
    // SAFETY: the DIB is top-down and tightly packed, matching `pixels`.
    unsafe {
        ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
    }

    let mask_stride = (edge as usize).div_ceil(32) * (std::mem::size_of::<u32>());
    let mask_bits = vec![0u8; mask_stride * edge as usize];
    // SAFETY: `mask_bits` outlives the call; all-zero means "use the alpha channel".
    let mask = unsafe {
        CreateBitmap(
            size,
            size,
            1,
            1,
            Some(mask_bits.as_ptr() as *const c_void),
        )
    };
    if mask.is_invalid() {
        // SAFETY: the colour bitmap was created above.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(color.0));
        }
        return Err(WinError::new("CreateBitmap returned an invalid mask bitmap"));
    }

    let icon_info = ICONINFO {
        fIcon: BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    // SAFETY: both bitmaps are valid and `CreateIconIndirect` copies them.
    let icon = unsafe { CreateIconIndirect(&icon_info) }
        .map_err(|err| WinError::from_windows("CreateIconIndirect", err));
    // SAFETY: the system copied the bitmaps; the originals are deleted once.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(mask.0));
        let _ = DeleteObject(HGDIOBJ(color.0));
    }
    icon
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the Direct2D SVG pipeline produces a visible icon, which also
    /// catches a mis-sized viewport (fully clipped artwork is transparent).
    #[test]
    fn bundled_svg_renders_opaque_pixels() {
        let edge = FALLBACK_LARGE_ICON as u32;
        let pixels = rasterise(LARGE_ICON_SVG, edge).expect("SVG rasterisation");
        assert_eq!(pixels.len(), (edge * edge) as usize * BYTES_PER_PIXEL);
        let centre = ((edge / 2) * edge + edge / 2) as usize * BYTES_PER_PIXEL;
        assert!(pixels[centre + 3] > 0, "centre pixel is transparent");
        // The artwork has rounded corners, so the last pixel must fall outside
        // the plate. A non-transparent corner means the SVG was clipped instead
        // of scaled into the requested viewport.
        let corner = ((edge - 1) * edge + edge - 1) as usize * BYTES_PER_PIXEL;
        assert!(
            pixels[corner + 3] < 32,
            "corner pixel is opaque: SVG was clipped rather than scaled"
        );
        assert!(pixels.iter().any(|channel| *channel > 0), "icon is blank");
    }
}
