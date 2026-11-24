//! Platform app-bundle and file icon rasterization (`app_icon_rgba`).

#![allow(unsafe_code)]

use super::{Image, ImageCache, ImageError, Input};
#[cfg(not(target_arch = "wasm32"))]
use super::{DecodeFailure, Decoded, mip_chain};
#[cfg(target_os = "linux")]
use super::mip_chain_with_intrinsic;

impl ImageCache {
    /// The Finder icon for an app bundle or any file path, rasterized at 2× `size_pt`.
    /// See [`ImageCache::app_icon_scaled`].
    pub fn app_icon(&self, path: impl AsRef<std::path::Path>, size_pt: f32) -> Image {
        self.app_icon_scaled(path, size_pt, 2.0)
    }

    /// The Finder icon for `path`, rasterized at `size_pt * scale` pixels square (pass the
    /// window's display scale). Handles are shared by (path, size, scale) rather than by
    /// content, so repeated calls never re-rasterize while a handle is alive. Lookup and
    /// rasterization run on the cache worker like any decode, so this is safe to call from a view.
    /// Missing paths still get the system's generic document icon on macOS. Windows reads an
    /// executable's or icon file's first icon; Linux resolves a `.desktop` entry's `Icon=`
    /// through the active freedesktop icon theme chain (`index.theme`, `Inherits`, `hicolor`,
    /// then `pixmaps`), or decodes an image file directly. Missing icons resolve with
    /// [`Image::is_not_found`] returning `true`, so callers can draw their own fallback.
    pub fn app_icon_scaled(&self, path: impl AsRef<std::path::Path>, size_pt: f32, scale: f32) -> Image {
        let path = path.as_ref().to_path_buf();
        let (nominal_px, scale_int, raster_px) = super::freedesktop::themed_icon_dims(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-app-icon\0");
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update(&[0]);
        hash.update(&size_pt.to_bits().to_le_bytes());
        hash.update(&scale.to_bits().to_le_bytes());
        self.request(*hash.finalize().as_bytes(), Input::Icon(path, nominal_px, scale_int, raster_px))
    }
}

#[cfg(target_os = "macos")]
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

/// Rasterize the platform icon for an app bundle or file at about `px`×`px`, as straight-alpha
/// sRGB RGBA8 ready for [`ImageCache::rgba`](super::ImageCache::rgba). Most code wants
/// [`ImageCache::app_icon`](super::ImageCache::app_icon), which calls this on its worker and
/// caches the result.
///
/// macOS: `NSWorkspace iconForFile:` drawn into a thread-local `NSBitmapImageRep` context.
/// Neither needs the main thread: `NSWorkspace` is not main-actor isolated in the SDK, and
/// AppKit allows drawing an `NSImage` into a bitmap context owned by a secondary thread.
/// Windows: the file's first icon from `PrivateExtractIconsW` at `px` (at most 256).
/// Linux: resolves a `.desktop` entry's `Icon=` through the active freedesktop icon theme
/// chain (`index.theme`, `Inherits`, `hicolor`, then `pixmaps`), rasterizing PNG or SVG to
/// fit `px`; a non-square pixmap keeps its aspect ratio.
pub fn app_icon_rgba(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    if px == 0 || px > 1024 { return Err(ImageError("app icon size must be 1..=1024 pixels".into())); }
    #[cfg(target_os = "macos")]
    { macos_icon(path, px) }
    #[cfg(target_os = "windows")]
    { windows_icon(path, px).map_err(|f| f.error) }
    #[cfg(target_os = "linux")]
    { linux_icon(path, px, 1, px).map(|(w, h, _, rgba)| (w, h, rgba)).map_err(|f| f.error) }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { let _ = path; Err(ImageError("app_icon is unsupported on this platform".into())) }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn app_icon_decoded(
    path: &std::path::Path,
    nominal_px: u32,
    scale: u32,
    raster_px: u32,
) -> Result<Decoded, DecodeFailure> {
    if raster_px == 0 || raster_px > 1024 {
        return Err(DecodeFailure::from(ImageError("app icon size must be 1..=1024 pixels".into())));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (nominal_px, scale);
        let (w, h, bytes) = macos_icon(path, raster_px)?;
        mip_chain(w, h, &bytes).map_err(DecodeFailure::from)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (nominal_px, scale);
        let (w, h, bytes) = windows_icon(path, raster_px)?;
        mip_chain(w, h, &bytes).map_err(DecodeFailure::from)
    }
    #[cfg(target_os = "linux")]
    {
        let (w, h, intrinsic, bytes) = linux_icon(path, nominal_px, scale, raster_px)?;
        match intrinsic {
            Some(size) => mip_chain_with_intrinsic(w, h, &bytes, Some(size)).map_err(DecodeFailure::from),
            None => mip_chain(w, h, &bytes).map_err(DecodeFailure::from),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (path, nominal_px, scale);
        Err(DecodeFailure::from(ImageError("app_icon is unsupported on this platform".into())))
    }
}

#[cfg(target_os = "windows")]
fn windows_icon(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), DecodeFailure> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW};
    let err = |what: &str| DecodeFailure::from(ImageError(format!("{what}: {}", path.display())));
    // PrivateExtractIconsW reads a fixed MAX_PATH buffer.
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    if wide.len() > 260 { return Err(err("icon path is longer than MAX_PATH")); }
    let mut buffer = [0u16; 260];
    buffer[..wide.len()].copy_from_slice(&wide);
    let side = px.min(256) as i32;
    let mut icon: HICON = std::ptr::null_mut();
    // SAFETY: `buffer` is NUL-terminated within MAX_PATH and `&mut icon` holds the 1 requested handle.
    let extracted = unsafe { PrivateExtractIconsW(buffer.as_ptr(), 0, side, side, &mut icon, std::ptr::null_mut(), 1, 0) };
    if extracted == 0 || extracted == u32::MAX || icon.is_null() {
        return Err(DecodeFailure::not_found(format!("no icon: {}", path.display())));
    }
    // SAFETY: `ICONINFO` is a plain Win32 C struct whose all-zero representation is valid.
    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    // SAFETY: `icon` is a live `HICON` from `PrivateExtractIconsW` and `&mut info` is writable; any returned bitmaps are deleted below.
    if unsafe { GetIconInfo(icon, &mut info) } == 0 {
        // SAFETY: `icon` is owned by this function and unused after returning.
        unsafe { DestroyIcon(icon) };
        return Err(err("unreadable icon"));
    }
    let result = (|| {
        if info.hbmColor.is_null() { return Err(err("monochrome icon")); }
        // SAFETY: `BITMAP` is a plain Win32 C struct whose all-zero representation is valid.
        let mut bitmap: BITMAP = unsafe { std::mem::zeroed() };
        // SAFETY: `info.hbmColor` is a live non-null `HBITMAP` and `&raw mut bitmap` holds `size_of::<BITMAP>()` writable bytes.
        if unsafe { GetObjectW(info.hbmColor, std::mem::size_of::<BITMAP>() as i32, (&raw mut bitmap).cast()) } == 0 {
            return Err(err("unreadable icon bitmap"));
        }
        let (w, h) = (bitmap.bmWidth, bitmap.bmHeight);
        if w <= 0 || h <= 0 || w > 1024 || h > 1024 { return Err(err("icon bitmap has no pixels")); }
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: rows top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                // SAFETY: `BITMAPINFOHEADER` is a plain Win32 C struct whose remaining fields are valid when zeroed.
                ..unsafe { std::mem::zeroed() }
            },
            // SAFETY: `BITMAPINFO` is a plain Win32 C struct whose remaining color-table field is valid when zeroed.
            ..unsafe { std::mem::zeroed() }
        };
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        // SAFETY: passing null creates a screen-compatible memory DC (or null on failure), deleted below.
        let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        // SAFETY: `info.hbmColor` is a live `w`×`h` bitmap, `pixels` holds `w * h * 4` writable bytes for the 32-bpp `header`, and a null `dc` returns 0.
        let rows = unsafe { GetDIBits(dc, info.hbmColor, 0, h as u32, pixels.as_mut_ptr().cast(), &mut header, DIB_RGB_COLORS) };
        // SAFETY: `dc` came from `CreateCompatibleDC` above and is not used afterwards.
        unsafe { DeleteDC(dc) };
        if rows == 0 { return Err(err("unreadable icon pixels")); }
        // BGRA with straight alpha; icons without an alpha channel leave it zero and draw opaque.
        let opaque = pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 0);
        for p in pixels.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
            if opaque { p[3] = 255; }
        }
        Ok((w as u32, h as u32, pixels))
    })();
    // SAFETY: `hbmColor` and `hbmMask` (when non-null) were created by `GetIconInfo`, and `icon` by `PrivateExtractIconsW`; none are used again.
    unsafe {
        if !info.hbmColor.is_null() { DeleteObject(info.hbmColor); }
        if !info.hbmMask.is_null() { DeleteObject(info.hbmMask); }
        DestroyIcon(icon);
    }
    result
}

#[cfg(target_os = "linux")]
fn linux_icon(
    path: &std::path::Path,
    nominal_px: u32,
    scale: u32,
    raster_px: u32,
) -> Result<(u32, u32, Option<[f32; 2]>, Vec<u8>), DecodeFailure> {
    let file = if path.extension().is_some_and(|e| e == "desktop") {
        let dirs = super::freedesktop::data_dirs();
        let home = super::freedesktop::home_dir();
        super::freedesktop::icon_file_scaled(path, nominal_px, scale, None, &dirs, home.as_deref())
            .ok_or_else(|| DecodeFailure::not_found(format!("no icon for {}", path.display())))?
    } else {
        path.to_path_buf()
    };
    super::freedesktop::decode_icon_file(&file, raster_px)
}

#[cfg(target_os = "macos")]
fn macos_icon(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::msg_send;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    let path = path.to_str().ok_or_else(|| ImageError("app icon path is not UTF-8".into()))?;
    let workspace = macos_class(c"NSWorkspace")?;
    // SAFETY: AppKit `NSWorkspace`, `NSImage`, and offscreen bitmap drawing run inside this
    // autorelease pool (`macos_bitmap`'s precondition); `small` is a `px`×`px` RGBA8 bitmap whose
    // `bitmapData` is non-null with `stride >= side * 4`, so every row slice is in bounds.
    objc2::rc::autoreleasepool(|_| unsafe {
        let workspace: *mut AnyObject = msg_send![workspace, sharedWorkspace];
        let icon: *mut AnyObject = msg_send![workspace, iconForFile: &*NSString::from_str(path)];
        if icon.is_null() { return Err(ImageError("NSWorkspace returned no icon".into())); }
        let zero = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
        // IconServices picks its rendition by the size asked for, and on macOS 26 and later a
        // small size of an app without a modern icon is the "legacy" one: a shrunken tile in a
        // grey frame. Draw at 256 pixels or more, then scale down.
        let big = px.max(256);
        // NSCompositingOperationCopy (1); a zero fromRect draws the whole best-fit representation.
        let large = macos_bitmap(big, |_, rect| { let _: () = msg_send![icon, drawInRect: rect, fromRect: zero, operation: 1usize, fraction: 1.0f64]; })?;
        let small = if big == px { large } else {
            macos_bitmap(px, |ctx, rect| {
                // NSImageInterpolationHigh.
                let _: () = msg_send![ctx, setImageInterpolation: 3usize];
                let none: *mut AnyObject = std::ptr::null_mut();
                let _: Bool = msg_send![large, drawInRect: rect, fromRect: zero, operation: 1usize, fraction: 1.0f64, respectFlipped: Bool::NO, hints: none];
            })?
        };
        let side = px as isize;
        let data: *const u8 = msg_send![small, bitmapData];
        let stride: isize = msg_send![small, bytesPerRow];
        if data.is_null() || stride < side * 4 { return Err(ImageError("icon bitmap has no pixel data".into())); }
        let mut out = Vec::with_capacity((px * px * 4) as usize);
        for y in 0..side {
            let row = std::slice::from_raw_parts(data.offset(y * stride), (side * 4) as usize);
            for p in row.chunks_exact(4) {
                let a = p[3];
                let straight = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
                out.extend_from_slice(&[straight(p[0]), straight(p[1]), straight(p[2]), a]);
            }
        }
        Ok((px, px, out))
    })
}

#[cfg(target_os = "macos")]
fn macos_class(name: &std::ffi::CStr) -> Result<&'static objc2::runtime::AnyClass, ImageError> {
    objc2::runtime::AnyClass::get(name).ok_or_else(|| ImageError(format!("{} is unavailable", name.to_string_lossy())))
}

/// A `side`×`side` sRGB bitmap with `draw(context, rect)` run into it as the current context.
/// Autoreleased: call inside an autorelease pool.
#[cfg(target_os = "macos")]
unsafe fn macos_bitmap(
    side: u32,
    draw: impl FnOnce(*mut objc2::runtime::AnyObject, objc2_foundation::NSRect),
) -> Result<*mut objc2::runtime::AnyObject, ImageError> {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::rc::{Allocated, Retained};
    use objc2::msg_send;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    let (bitmap, context, color_space) = (macos_class(c"NSBitmapImageRep")?, macos_class(c"NSGraphicsContext")?, macos_class(c"NSColorSpace")?);
    let n = side as isize;
    // SAFETY: AppKit bitmap and context calls on objects this function retains or that live in
    // the caller's autorelease pool.
    unsafe {
        let rep: Allocated<AnyObject> = msg_send![bitmap, alloc];
        let null: *mut *mut u8 = std::ptr::null_mut();
        // Default bitmapFormat is premultiplied alpha-last RGBA, the only layout AppKit draws into.
        let rep: Option<Retained<AnyObject>> = msg_send![rep,
            initWithBitmapDataPlanes: null, pixelsWide: n, pixelsHigh: n, bitsPerSample: 8isize,
            samplesPerPixel: 4isize, hasAlpha: Bool::YES, isPlanar: Bool::NO,
            colorSpaceName: &*NSString::from_str("NSDeviceRGBColorSpace"), bitmapFormat: 0usize,
            bytesPerRow: n * 4, bitsPerPixel: 32isize];
        let rep = rep.ok_or_else(|| ImageError("could not allocate an icon bitmap".into()))?;
        let srgb: *mut AnyObject = msg_send![color_space, sRGBColorSpace];
        let tagged: *mut AnyObject = msg_send![&*rep, bitmapImageRepByRetaggingWithColorSpace: srgb];
        let tagged = if tagged.is_null() { Retained::autorelease_return(rep) } else { tagged };
        let ctx: *mut AnyObject = msg_send![context, graphicsContextWithBitmapImageRep: tagged];
        if ctx.is_null() { return Err(ImageError("could not create an icon context".into())); }
        let _: () = msg_send![context, saveGraphicsState];
        let _: () = msg_send![context, setCurrentContext: ctx];
        draw(ctx, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(side as f64, side as f64)));
        let _: () = msg_send![ctx, flushGraphics];
        let _: () = msg_send![context, restoreGraphicsState];
        Ok(tagged)
    }
}
