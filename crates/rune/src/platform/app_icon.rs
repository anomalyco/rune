//! The app's icon: the bundle's own, image bytes, or a light/dark pair (the Dock's on macOS, its
//! windows' and the taskbar's on Windows, its windows' on Linux), so an app needs no platform glue
//! of its own.

#[cfg(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
use std::sync::Mutex;

/// What the app's icon is: the bundle's (or executable's) own, one image, or a light/dark pair
/// that follows the app's Appearance. Pass it to [`set_app_icon_choice`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppIcon {
    /// The icon the app was built with. On macOS this hands the Dock back the bundle's compiled
    /// icon (`setApplicationIconImage(nil)`), so a layered icon keeps the system's dark and tinted
    /// styles.
    Bundle,
    /// One image, whatever the Appearance.
    Image(Vec<u8>),
    /// `light` under a light Appearance and `dark` under a dark one.
    Pair { light: Vec<u8>, dark: Vec<u8> },
}

impl AppIcon {
    /// The image shown when the app's Appearance is dark (`dark`) or light: `None` for the
    /// bundle's own icon. Pure, so a test can check the choice without AppKit.
    pub fn shown(&self, dark: bool) -> Option<&[u8]> {
        match self {
            AppIcon::Bundle => None,
            AppIcon::Image(bytes) => Some(bytes.as_slice()),
            AppIcon::Pair { light, dark: night } => Some(if dark { night.as_slice() } else { light.as_slice() }),
        }
    }

    /// Whether the shown icon changes with the Appearance.
    pub fn follows_appearance(&self) -> bool {
        matches!(self, AppIcon::Pair { .. })
    }

    /// The images it holds, to check before showing any.
    #[cfg(any(target_os = "macos", test))]
    fn images(&self) -> impl Iterator<Item = &[u8]> {
        let (a, b): (Option<&[u8]>, Option<&[u8]>) = match self {
            AppIcon::Bundle => (None, None),
            AppIcon::Image(bytes) => (Some(bytes.as_slice()), None),
            AppIcon::Pair { light, dark } => (Some(light.as_slice()), Some(dark.as_slice())),
        };
        a.into_iter().chain(b)
    }
}

/// The icon the app asked for and how many times it has asked, newest wins. `None` until it
/// asks, so Rune leaves the Dock alone.
#[cfg(target_os = "macos")]
static CURRENT: Mutex<Option<(u64, AppIcon)>> = Mutex::new(None);
/// The event loop has started, so AppKit is ours to call.
#[cfg(target_os = "macos")]
static LAUNCHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app's icon while it runs, in place of the bundle's or executable's. Call it from any
/// thread. A headless run sets nothing. Fails when the bytes aren't an image. The same as
/// `set_app_icon_choice(AppIcon::Image(bytes))`.
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
    set_app_icon_choice(AppIcon::Image(bytes.into())).map_err(|e| e.replacen("set_app_icon_choice", "set_app_icon", 1))
}

/// The app's icon: the bundle's own, one image, or a light/dark pair. Call it from any thread,
/// before `run` or later; a later call replaces the earlier one. A headless run sets nothing.
/// Fails when any of the bytes aren't an image.
///
/// - **macOS:** the Dock and app-switcher icon, set once the app finishes launching (AppKit
///   paints the bundle's icon over an earlier one). `AppIcon::Bundle` gives back the bundle's
///   compiled icon exactly as macOS shows it. `AppIcon::Pair` follows the app's
///   `effectiveAppearance`, with or without a window open, so an app that forces Light or Dark
///   gets the matching side. Finder's icon for the bundle doesn't change.
/// - **Windows:** as [`set_app_icon`] for `Image`. `Pair` checks both images and shows `light`
///   (Rune doesn't follow the taskbar's theme). `Bundle` drops the image for windows opened
///   afterwards: they show the icon resource given to `set_app_icon_resource` if the app gave
///   one, else Windows' default window icon (Rune doesn't load the executable's icon by itself).
/// - **Linux:** as [`set_app_icon`] for `Image`. `Pair` checks both images and shows `light`.
///   `Bundle` drops the image, so windows opened afterwards carry none and the compositor or
///   window manager finds one through the app's desktop entry.
/// - Does nothing elsewhere.
pub fn set_app_icon_choice(icon: AppIcon) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::Ordering;
        if icon.images().any(|bytes| decode(bytes).is_none()) {
            return Err("set_app_icon_choice: the bytes aren't an image AppKit reads".into());
        }
        {
            let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
            let asked = current.as_ref().map_or(0, |(n, _)| n + 1);
            *current = Some((asked, icon));
        }
        if LAUNCHED.load(Ordering::Acquire) { crate::platform::main_queue::on_main(apply); }
    }
    #[cfg(target_os = "windows")]
    {
        let bytes = match icon {
            AppIcon::Bundle => {
                let resource = *RESOURCE.lock().unwrap_or_else(|e| e.into_inner());
                *WINDOWS.lock().unwrap_or_else(|e| e.into_inner()) = resource.map(WindowsIcon::Resource);
                return Ok(());
            }
            AppIcon::Image(bytes) => bytes,
            AppIcon::Pair { light, dark } => {
                image::load_from_memory(&dark).map_err(|e| format!("set_app_icon_choice: {e}"))?;
                light
            }
        };
        let image = image::load_from_memory(&bytes).map_err(|e| format!("set_app_icon_choice: {e}"))?.into_rgba8();
        let size = |px: u32| image::imageops::resize(&image, px, px, image::imageops::FilterType::Lanczos3).into_raw();
        *WINDOWS.lock().unwrap_or_else(|e| e.into_inner()) = Some(WindowsIcon::Pixels { small: size(SMALL), big: size(BIG) });
    }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    {
        let bytes = match icon {
            AppIcon::Bundle => {
                *LINUX.lock().unwrap_or_else(|e| e.into_inner()) = None;
                return Ok(());
            }
            AppIcon::Image(bytes) => bytes,
            AppIcon::Pair { light, dark } => {
                image::load_from_memory(&dark).map_err(|e| format!("set_app_icon_choice: {e}"))?;
                light
            }
        };
        let image = image::load_from_memory(&bytes).map_err(|e| format!("set_app_icon_choice: {e}"))?.into_rgba8();
        *LINUX.lock().unwrap_or_else(|e| e.into_inner()) = Some(linux_pixels(&image));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
    let _ = icon;
    Ok(())
}

/// Windows: the app's icon from the executable's own icon resource `id` (the one Explorer shows),
/// in place of `set_app_icon`'s bytes: each window loads it at the size it shows with
/// `LoadImageW`, so nothing is decoded or resized at launch. Give the resource bitmap images up to
/// 128 px (a PNG-compressed image larger than that is never asked for). Call it before `run`.
#[cfg(target_os = "windows")]
pub fn set_app_icon_resource(id: u16) {
    *RESOURCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
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
/// The icon resource `set_app_icon_resource` named, which `AppIcon::Bundle` goes back to.
#[cfg(target_os = "windows")]
static RESOURCE: Mutex<Option<u16>> = Mutex::new(None);
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
    LAUNCHED.store(true, std::sync::atomic::Ordering::Release);
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
mod mac;
#[cfg(target_os = "macos")]
use mac::apply;

#[cfg(test)]
#[path = "app_icon_tests.rs"]
mod tests;
