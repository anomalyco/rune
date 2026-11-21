//! Platform app-bundle and file icon rasterization (`app_icon_rgba`).

#![allow(unsafe_code)]

use super::ImageError;

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
/// Linux: the closest theme size at or above `px`, or an SVG rasterized to fit `px`; a
/// non-square pixmap keeps its aspect ratio.
pub fn app_icon_rgba(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    if px == 0 || px > 1024 { return Err(ImageError("app icon size must be 1..=1024 pixels".into())); }
    #[cfg(target_os = "macos")]
    { macos_icon(path, px) }
    #[cfg(target_os = "windows")]
    { windows_icon(path, px) }
    #[cfg(target_os = "linux")]
    { linux_icon(path, px) }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { let _ = path; Err(ImageError("app_icon is unsupported on this platform".into())) }
}

#[cfg(target_os = "windows")]
fn windows_icon(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW};
    let err = |what: &str| ImageError(format!("{what}: {}", path.display()));
    // PrivateExtractIconsW reads a fixed MAX_PATH buffer.
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    if wide.len() > 260 { return Err(err("icon path is longer than MAX_PATH")); }
    let mut buffer = [0u16; 260];
    buffer[..wide.len()].copy_from_slice(&wide);
    let side = px.min(256) as i32;
    let mut icon: HICON = std::ptr::null_mut();
    // SAFETY: `buffer` is NUL-terminated within MAX_PATH and `&mut icon` holds the 1 requested handle.
    let extracted = unsafe { PrivateExtractIconsW(buffer.as_ptr(), 0, side, side, &mut icon, std::ptr::null_mut(), 1, 0) };
    if extracted == 0 || extracted == u32::MAX || icon.is_null() { return Err(err("no icon")); }
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
fn linux_icon(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let file = if path.extension().is_some_and(|e| e == "desktop") {
        freedesktop::icon_file(path, px, &freedesktop::data_dirs())
            .ok_or_else(|| ImageError(format!("no icon for {}", path.display())))?
    } else { path.to_path_buf() };
    let bytes = std::fs::read(&file).map_err(|e| ImageError(format!("{}: {e}", file.display())))?;
    if bytes.len() > 16 << 20 { return Err(ImageError(format!("{} is too large for an icon", file.display()))); }
    if super::has_svg_extension(&file) { return super::svg_rgba(&bytes, px); }
    let image = image::load_from_memory(&bytes).map_err(|e| ImageError(format!("{}: {e}", file.display())))?.into_rgba8();
    if image.width() > 1024 || image.height() > 1024 { return Err(ImageError(format!("{} is too large for an icon", file.display()))); }
    Ok((image.width(), image.height(), image.into_raw()))
}

/// Freedesktop icon lookup: a `.desktop` entry's `Icon=` as an absolute path, or by name in the
/// hicolor theme (every size directory, `@2` included) and then `pixmaps`.
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod freedesktop {
    use std::path::{Path, PathBuf};

    pub(crate) fn data_dirs() -> Vec<PathBuf> {
        let home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
        let system = rune_core::env::var("XDG_DATA_DIRS").filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        home.into_iter().chain(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from)).collect()
    }

    /// The `Icon=` key of the `[Desktop Entry]` group.
    pub(crate) fn icon_name(entry: &str) -> Option<&str> {
        let mut group = "";
        for line in entry.lines().map(str::trim) {
            if line.starts_with('[') { group = line; continue; }
            if group != "[Desktop Entry]" { continue; }
            if let Some((key, value)) = line.split_once('=') && key.trim() == "Icon" {
                return Some(value.trim()).filter(|v| !v.is_empty());
            }
        }
        None
    }

    pub(crate) fn icon_file(entry: &Path, px: u32, data_dirs: &[PathBuf]) -> Option<PathBuf> {
        let text = std::fs::read_to_string(entry).ok()?;
        let name = icon_name(&text)?;
        if Path::new(name).is_absolute() { return Path::new(name).is_file().then(|| name.into()); }
        let name = name.strip_suffix(".png").or_else(|| name.strip_suffix(".svg")).unwrap_or(name);
        // (effective pixel size, path); scalable sorts as "any size".
        let mut found: Vec<(Option<u32>, PathBuf)> = Vec::new();
        for dir in data_dirs {
            let Ok(sizes) = std::fs::read_dir(dir.join("icons/hicolor")) else { continue };
            for size in sizes.flatten() {
                let label = size.file_name().to_string_lossy().into_owned();
                let effective = if label == "scalable" { None } else {
                    let (dims, scale) = label.split_once('@').unwrap_or((&label, "1"));
                    let size = dims.split_once('x').and_then(|(w, _)| w.parse::<u32>().ok());
                    let Some(n) = size.zip(scale.parse::<u32>().ok()).map(|(w, s)| w * s.max(1)) else { continue };
                    Some(n)
                };
                for ext in ["png", "svg"] {
                    let file = size.path().join("apps").join(format!("{name}.{ext}"));
                    if file.is_file() { found.push((effective, file)); }
                }
            }
        }
        // Smallest bitmap that covers `px`, else scalable, else the largest bitmap.
        let covering = found.iter().filter(|(n, _)| n.is_some_and(|n| n >= px)).min_by_key(|(n, _)| *n);
        let scalable = found.iter().find(|(n, _)| n.is_none());
        let largest = found.iter().max_by_key(|(n, _)| *n);
        if let Some((_, file)) = covering.or(scalable).or(largest) { return Some(file.clone()); }
        data_dirs.iter().flat_map(|dir| ["png", "svg"].map(|ext| dir.join(format!("pixmaps/{name}.{ext}")))).find(|f| f.is_file())
    }
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
