//! The app's icon from image bytes (the Dock's on macOS, its windows' and the taskbar's on
//! Windows, its windows' on Linux), so an app needs no platform glue of its own.

#[cfg(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
use std::sync::Mutex;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};

/// The icon waiting for the main thread, newest wins.
#[cfg(target_os = "macos")]
static PENDING: Mutex<Option<Vec<u8>>> = Mutex::new(None);
/// The event loop has started, so AppKit is ours to call.
#[cfg(target_os = "macos")]
static LAUNCHED: AtomicBool = AtomicBool::new(false);

/// The app's icon while it runs, in place of the bundle's or executable's. Call it from any
/// thread. A headless run sets nothing. Fails when the bytes aren't an image.
///
/// - **macOS:** `bytes` is an `.icns`, PNG, TIFF or anything else `NSImage` reads, shown as the
///   Dock and app-switcher icon. Before `run`, Rune sets it once the app finishes launching; a
///   later call replaces it. (`AppHandle::set_dock_icon` shows or hides the Dock icon instead.)
/// - **Windows:** `bytes` is a PNG or JPEG, best square and 256 px or more. Each window opened
///   after the call shows it in its title bar, on the taskbar and in Alt+Tab, so call it before
///   `run`.
/// - **Linux:** `bytes` is a PNG or JPEG, best square and 256 px or more. Each window opened
///   after the call carries it, so call it before `run`: on Wayland through
///   `xdg-toplevel-icon-v1` where the compositor has it (KWin, Labwc, Muffin; elsewhere the
///   compositor keeps using the desktop entry), on X11 as `_NET_WM_ICON`. Either way the window
///   shows it in title bars and window switchers when no desktop entry matches the app id.
/// - Does nothing elsewhere.
pub fn set_app_icon(bytes: impl Into<Vec<u8>>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let bytes = bytes.into();
        if decode(&bytes).is_none() { return Err("set_app_icon: the bytes aren't an image AppKit reads".into()); }
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(bytes);
        if LAUNCHED.load(Ordering::Acquire) { crate::platform::main_queue::on_main(apply); }
    }
    #[cfg(target_os = "windows")]
    {
        let image = image::load_from_memory(&bytes.into()).map_err(|e| format!("set_app_icon: {e}"))?.into_rgba8();
        let size = |px: u32| image::imageops::resize(&image, px, px, image::imageops::FilterType::Lanczos3).into_raw();
        *WINDOWS.lock().unwrap_or_else(|e| e.into_inner()) = Some(WindowsIcon::Pixels { small: size(SMALL), big: size(BIG) });
    }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    {
        let image = image::load_from_memory(&bytes.into()).map_err(|e| format!("set_app_icon: {e}"))?.into_rgba8();
        *LINUX.lock().unwrap_or_else(|e| e.into_inner()) = Some(linux_pixels(&image));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
    let _ = bytes;
    Ok(())
}

/// Windows: the app's icon from the executable's own icon resource `id` (the one Explorer shows),
/// in place of `set_app_icon`'s bytes: each window loads it at the size it shows with
/// `LoadImageW`, so nothing is decoded or resized at launch. Give the resource bitmap images up to
/// 128 px (a PNG-compressed image larger than that is never asked for). Call it before `run`.
#[cfg(target_os = "windows")]
pub fn set_app_icon_resource(id: u16) {
    *WINDOWS.lock().unwrap_or_else(|e| e.into_inner()) = Some(WindowsIcon::Resource(id));
}

/// Windows: where each new window's `HICON`s come from. The small one draws in the title bar and
/// the taskbar's small mode; the big one on the taskbar and in Alt+Tab, where Windows scales it to
/// the display's DPI.
#[cfg(target_os = "windows")]
enum WindowsIcon {
    /// Pixels at the two sizes, from `set_app_icon`.
    Pixels { small: Vec<u8>, big: Vec<u8> },
    /// The executable's icon resource, from `set_app_icon_resource`.
    Resource(u16),
}

#[cfg(target_os = "windows")]
static WINDOWS: Mutex<Option<WindowsIcon>> = Mutex::new(None);
#[cfg(target_os = "windows")]
const SMALL: u32 = 32;
#[cfg(target_os = "windows")]
const BIG: u32 = 256;
/// The big icon taken from the exe's resources: the largest size an icon resource keeps as a
/// bitmap. Its 256 px image is a PNG, which `LoadImageW` decodes through WIC (`WindowsCodecs.dll`
/// and `IconCodecService.dll` loaded on the UI thread before the first frame); 128 px still
/// covers the taskbar and Alt+Tab at 400%.
#[cfg(target_os = "windows")]
const BIG_RESOURCE: u32 = 128;

/// Windows: `attrs` with the app's icon, once `set_app_icon` has one.
#[cfg(target_os = "windows")]
pub(crate) fn with_icon(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
    use winit::window::Icon;
    let icon = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    let (small, big) = match icon.as_ref() {
        None => return attrs,
        Some(WindowsIcon::Pixels { small, big }) => (Icon::from_rgba(small.clone(), SMALL, SMALL).ok(), Icon::from_rgba(big.clone(), BIG, BIG).ok()),
        Some(WindowsIcon::Resource(id)) => {
            let at = |px: u32| Icon::from_resource(*id, Some(winit::dpi::PhysicalSize::new(px, px))).ok();
            (at(SMALL), at(BIG_RESOURCE))
        }
    };
    attrs.with_window_icon(small).with_taskbar_icon(big)
}

/// Linux: the icon's square RGBA pixels, `LINUX_SIZE` on a side.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
static LINUX: Mutex<Option<Vec<u8>>> = Mutex::new(None);
/// Linux: one size serves both protocols; the compositor or window manager scales it down.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
const LINUX_SIZE: u32 = 256;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn linux_pixels(image: &image::RgbaImage) -> Vec<u8> {
    image::imageops::resize(image, LINUX_SIZE, LINUX_SIZE, image::imageops::FilterType::Lanczos3).into_raw()
}

/// Linux: `attrs` with the app's icon, once `set_app_icon` has one. winit sends it as
/// `xdg-toplevel-icon-v1` on Wayland (before the window's first commit) and `_NET_WM_ICON` on X11.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) fn with_icon(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    let icon = LINUX.lock().unwrap_or_else(|e| e.into_inner());
    match icon.as_ref() {
        None => attrs,
        Some(rgba) => attrs.with_window_icon(winit::window::Icon::from_rgba(rgba.clone(), LINUX_SIZE, LINUX_SIZE).ok()),
    }
}

/// The event loop exists: sets an icon asked for before `run` once the main queue drains, which
/// is after AppKit finishes launching (it would show the bundle's icon over an earlier one).
#[cfg(target_os = "macos")]
pub(crate) fn launched() {
    LAUNCHED.store(true, Ordering::Release);
    crate::platform::main_queue::on_main(apply);
}

#[cfg(target_os = "macos")]
fn decode(bytes: &[u8]) -> Option<objc2::rc::Retained<objc2_app_kit::NSImage>> {
    use objc2::AnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSData;
    // NSImage may be created and read on any thread; it's nil for data it can't read.
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes))?;
    // An image with no readable representation has no size.
    let size = image.size();
    (size.width > 0.0 && size.height > 0.0).then_some(image)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn apply() {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    let Some(bytes) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    let Some(image) = decode(&bytes) else { return };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    // SAFETY: on the main thread, with an image AppKit decoded.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(16, 16, image::Rgba([40, 120, 220, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn an_icon_must_be_an_image_and_waits_for_launch() {
        assert!(set_app_icon(b"not an image".to_vec()).is_err());
        assert!(set_app_icon(png()).is_ok());
        // Without an event loop nothing touches NSApplication, so a headless run stays windowless.
        assert!(!LAUNCHED.load(Ordering::Acquire));
        assert!(PENDING.lock().unwrap().is_some());
    }
}

#[cfg(all(test, unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod linux_tests {
    use super::*;

    fn png(px: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(px, px, image::Rgba([40, 120, 220, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn an_icon_must_be_an_image_and_reaches_each_new_window() {
        assert!(set_app_icon(b"not an image".to_vec()).is_err());
        assert!(set_app_icon(png(16)).is_ok());
        assert_eq!(LINUX.lock().unwrap().as_ref().map(Vec::len), Some((LINUX_SIZE * LINUX_SIZE * 4) as usize));
        assert!(with_icon(winit::window::WindowAttributes::default()).window_icon.is_some());
    }
}
