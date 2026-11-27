//! System text-size scale factor: Windows Settings > Accessibility > Text size
//! (`UISettings.TextScaleFactor` / `HKCU\Software\Microsoft\Accessibility\TextScaleFactor`),
//! iOS Dynamic Type (`UIFont` body text size / `UIApplication.preferredContentSizeCategory`),
//! Linux `org.gnome.desktop.interface` `text-scaling-factor` (XDG Settings portal with
//! off-UI-thread GSettings fallback), and `1.0` on macOS and headless runs.
//!
//! Headless captures and checks can override the scale with `RUNE_TEXT_SCALE=<factor>`.
//! Native windows and overlays sample `current()` on creation, focus and `WM_SETTINGCHANGE`,
//! and read `cached()` (an atomic load with no I/O) on redraws triggered by live OS changes.

#![allow(unsafe_code)]

/// Default text-size scale factor when no OS preference or preset is active.
pub(crate) const DEFAULT: f32 = 1.0;
/// Lower bound for a sanitized text-size scale factor (covers Chrome's minimum font size and iOS XS).
pub(crate) const MIN: f32 = 0.25;
/// Upper bound for a sanitized text-size scale factor (covers iOS Accessibility XXXL 53/17 ≈ 3.12
/// and Chrome's 72 px maximum 4.5).
pub(crate) const MAX: f32 = 5.0;

/// Sanitizes a positive finite scale factor into `MIN..=MAX`, rejecting zero, negative, NaN or
/// infinite values.
pub(crate) fn sanitize(scale: f32) -> Option<f32> {
    (scale.is_finite() && scale > 0.0).then(|| scale.clamp(MIN, MAX))
}

/// Converts a double-precision scale factor (Linux portal/GSettings, WinRT `TextScaleFactor`)
/// into a sanitized `f32`.
#[cfg_attr(not(any(test, target_os = "windows", target_os = "linux", target_os = "ios", target_arch = "wasm32")), allow(dead_code))]
pub(crate) fn from_f64(scale: f64) -> Option<f32> {
    (scale.is_finite() && scale > 0.0).then(|| (scale as f32).clamp(MIN, MAX))
}

/// Converts Windows' `HKCU\Software\Microsoft\Accessibility\TextScaleFactor` DWORD percentage
/// (e.g. `150` -> `1.5`, range `100..=225`) into a sanitized scale factor.
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn from_windows_percent(percent: u32) -> Option<f32> {
    (percent > 0).then(|| percent as f32 / 100.0).and_then(sanitize)
}

/// Apple's default iOS Dynamic Type body font size (`UIContentSizeCategoryLarge`), in points.
#[cfg(any(test, target_os = "ios"))]
const IOS_DEFAULT_BODY_PT: f64 = 17.0;

/// Converts an iOS Dynamic Type body point size (from `UIFont.preferredFontForTextStyle:`)
/// into a scale factor relative to the default 17 pt `Large` body size.
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn from_ios_body_points(points: f64) -> Option<f32> {
    from_f64(points / IOS_DEFAULT_BODY_PT)
}

/// Maps an iOS `UIContentSizeCategory` string (both raw `UICTContentSizeCategory*` values and
/// symbolic `UIContentSizeCategory*` names) to Apple's HIG body scale relative to 17 pt `Large`.
#[cfg(any(test, target_os = "ios"))]
pub(crate) fn from_ios_category(category: &str) -> Option<f32> {
    let pt: f64 = match category {
        "UICTContentSizeCategoryXS" | "UIContentSizeCategoryExtraSmall" => 14.0,
        "UICTContentSizeCategoryS" | "UIContentSizeCategorySmall" => 15.0,
        "UICTContentSizeCategoryM" | "UIContentSizeCategoryMedium" => 16.0,
        "UICTContentSizeCategoryL" | "UIContentSizeCategoryLarge" => 17.0,
        "UICTContentSizeCategoryXL" | "UIContentSizeCategoryExtraLarge" => 19.0,
        "UICTContentSizeCategoryXXL" | "UIContentSizeCategoryExtraExtraLarge" => 21.0,
        "UICTContentSizeCategoryXXXL" | "UIContentSizeCategoryExtraExtraExtraLarge" => 23.0,
        "UICTContentSizeCategoryAccessibilityM" | "UIContentSizeCategoryAccessibilityMedium" => 28.0,
        "UICTContentSizeCategoryAccessibilityL" | "UIContentSizeCategoryAccessibilityLarge" => 33.0,
        "UICTContentSizeCategoryAccessibilityXL" | "UIContentSizeCategoryAccessibilityExtraLarge" => 40.0,
        "UICTContentSizeCategoryAccessibilityXXL" | "UIContentSizeCategoryAccessibilityExtraExtraLarge" => 47.0,
        "UICTContentSizeCategoryAccessibilityXXXL" | "UIContentSizeCategoryAccessibilityExtraExtraExtraLarge" => 53.0,
        _ => return None,
    };
    from_ios_body_points(pt)
}

/// Parses a `RUNE_TEXT_SCALE` value (e.g. `"1.5"`).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn parse_env(raw: &str) -> Option<f32> {
    raw.trim().parse::<f32>().ok().and_then(sanitize)
}

/// `RUNE_TEXT_SCALE=<factor>`: capture and check preset (e.g. `RUNE_TEXT_SCALE=1.5`), without
/// changing the machine's own text size.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn preset() -> Option<f32> {
    rune_core::env::var("RUNE_TEXT_SCALE").as_deref().and_then(parse_env)
}

/// Samples the system text-size scale factor freshly (on window creation, focus, or OS settings
/// broadcast). Honors test overrides and `RUNE_TEXT_SCALE` ahead of the OS.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn current() -> f32 {
    #[cfg(test)]
    if let Some(v) = test_overrides::get() { return v; }
    if let Some(p) = preset() { return p; }
    platform_current()
}

/// Reads the cached system text-size scale factor without touching disk, registry, or D-Bus on
/// the UI thread (safe to call on every redraw).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn cached() -> f32 {
    #[cfg(test)]
    if let Some(v) = test_overrides::get() { return v; }
    if let Some(p) = preset() { return p; }
    platform_cached()
}

/// Registers `window` to redraw whenever the OS notifies a live text-size change.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn follow(window: &std::sync::Arc<winit::window::Window>) {
    #[cfg(target_os = "windows")]
    win::follow(window);
    #[cfg(target_os = "ios")]
    ios::follow(window);
    #[cfg(not(any(target_os = "windows", target_os = "ios")))]
    let _ = window;
}

/// Registers an embedded host's `Waker` on iOS so `UIContentSizeCategoryDidChangeNotification`
/// wakes the host even without a winit window.
#[cfg(target_os = "ios")]
pub(crate) fn follow_waker(waker: crate::Waker) {
    ios::follow_waker(waker);
}

#[cfg(target_os = "windows")]
fn platform_current() -> f32 { win::current() }

#[cfg(target_os = "windows")]
fn platform_cached() -> f32 { win::cached() }

#[cfg(all(target_os = "linux", feature = "dbus"))]
fn platform_current() -> f32 { crate::platform::portal_appearance::text_scale() }

#[cfg(all(target_os = "linux", feature = "dbus"))]
fn platform_cached() -> f32 { crate::platform::portal_appearance::text_scale() }

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
fn platform_current() -> f32 {
    CACHED_LINUX.get_or_init_notifying(read_linux_fallback, crate::platform::preferences::redraw_fallback_windows)
}

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
fn platform_cached() -> f32 { platform_current() }

#[cfg(target_os = "ios")]
fn platform_current() -> f32 { ios::current() }

#[cfg(target_os = "ios")]
fn platform_cached() -> f32 { ios::cached() }

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "ios")))]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn platform_current() -> f32 { DEFAULT }

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "ios")))]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn platform_cached() -> f32 { DEFAULT }

// ── Windows ───────────────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod win {
    use std::sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicU32, Ordering}};
    use winit::window::Window;

    use super::{DEFAULT, from_f64, from_windows_percent};

    /// Cached scale factor stored as `f32::to_bits()`; `0` means not yet sampled.
    static CACHED: AtomicU32 = AtomicU32::new(0);
    static WINDOWS: Mutex<Vec<Weak<Window>>> = Mutex::new(Vec::new());
    static WATCH_STARTED: OnceLock<()> = OnceLock::new();
    /// Keeps the WinRT `UISettings` instance alive for the lifetime of the process so its
    /// `TextScaleFactorChanged` subscription stays registered.
    static UI_SETTINGS: OnceLock<windows::UI::ViewManagement::UISettings> = OnceLock::new();

    fn store(scale: f32) -> bool {
        let bits = scale.to_bits();
        let prev = CACHED.swap(bits, Ordering::AcqRel);
        prev != 0 && prev != bits
    }

    fn read_registry() -> Option<f32> {
        let key = windows_registry::CURRENT_USER.open("Software\\Microsoft\\Accessibility").ok()?;
        let percent = key.get_u32("TextScaleFactor").ok()?;
        from_windows_percent(percent)
    }

    pub(super) fn current() -> f32 {
        let scale = read_registry().unwrap_or(DEFAULT);
        CACHED.store(scale.to_bits(), Ordering::Release);
        scale
    }

    pub(super) fn cached() -> f32 {
        let bits = CACHED.load(Ordering::Acquire);
        if bits == 0 { DEFAULT } else { f32::from_bits(bits) }
    }

    fn redraw_windows() {
        let live: Vec<_> = {
            let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
            windows.retain(|w| w.strong_count() > 0);
            windows.iter().filter_map(Weak::upgrade).collect()
        };
        for window in live {
            window.request_redraw();
        }
    }

    /// Reads `Windows.UI.ViewManagement.UISettings::TextScaleFactor` directly.
    #[cfg(test)]
    pub(super) fn read_ui_settings() -> Option<f32> {
        let settings = windows::UI::ViewManagement::UISettings::new().ok()?;
        settings.TextScaleFactor().ok().and_then(from_f64)
    }

    pub(super) fn follow(window: &Arc<Window>) {
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(|w| w.strong_count() > 0);
        windows.push(Arc::downgrade(window));
        drop(windows);
        // Subscribe off the UI thread so cold launch never waits on WinRT activation.
        WATCH_STARTED.get_or_init(|| {
            std::thread::Builder::new()
                .name("rune-text-scale".into())
                .spawn(subscribe_ui_settings)
                .ok();
        });
    }

    fn subscribe_ui_settings() {
        use windows::Foundation::TypedEventHandler;
        use windows::UI::ViewManagement::UISettings;
        use windows::core::IInspectable;

        let Ok(settings) = UISettings::new() else { return };
        if let Ok(factor) = settings.TextScaleFactor()
            && let Some(scale) = from_f64(factor)
            && store(scale)
        {
            redraw_windows();
        }
        let handler = TypedEventHandler::<UISettings, IInspectable>::new(|sender, _| {
            if let Some(s) = sender.as_ref()
                && let Ok(factor) = s.TextScaleFactor()
                && let Some(scale) = from_f64(factor)
                && store(scale)
            {
                redraw_windows();
            }
            Ok(())
        });
        if settings.TextScaleFactorChanged(&handler).is_ok() {
            UI_SETTINGS.set(settings).ok();
        }
    }
}

// ── Linux off-UI-thread fallback (when dbus is disabled or portal does not answer) ────────────────

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
static CACHED_LINUX: CachedTextScale = CachedTextScale::new();

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
pub(crate) fn linux_fallback_late_changed() -> bool {
    CACHED_LINUX.late_changed()
}

/// One-shot off-UI-thread cache for the Linux fallback `text-scaling-factor` read when the
/// `dbus` feature is disabled.
#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
struct CachedTextScale(crate::platform::preferences::CachedPref<u32>);

#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
impl CachedTextScale {
    const fn new() -> Self {
        Self(crate::platform::preferences::CachedPref::new(DEFAULT.to_bits()))
    }

    fn get_or_init_notifying(&'static self, read: fn() -> Option<f32>, on_late: fn()) -> f32 {
        let bits = self.0.get_or_init_with(
            "rune-text-scale",
            crate::platform::preferences::CachedPref::<u32>::FIRST_ANSWER,
            move || read().and_then(sanitize).unwrap_or(DEFAULT).to_bits(),
            on_late,
        );
        f32::from_bits(bits)
    }

    fn late_changed(&self) -> bool {
        self.0.late_changed()
    }

    #[cfg(test)]
    fn get_or_init(&'static self, read: fn() -> Option<f32>) -> f32 {
        self.get_or_init_notifying(read, || {})
    }

    #[cfg(test)]
    fn get_or_init_with(
        &'static self,
        wait: std::time::Duration,
        read: impl FnOnce() -> Option<f32> + Send + 'static,
        on_late: impl FnOnce() + Send + 'static,
    ) -> f32 {
        let bits = self.0.get_or_init_with(
            "rune-text-scale",
            wait,
            move || read().and_then(sanitize).unwrap_or(DEFAULT).to_bits(),
            on_late,
        );
        f32::from_bits(bits)
    }

    #[cfg(test)]
    fn wait_ready(&self) {
        self.0.wait_ready();
    }
}

/// Fallback `text-scaling-factor` read for Linux desktops whose Settings portal does not answer
/// `org.gnome.desktop.interface` `text-scaling-factor`, or builds without the `dbus` feature.
/// Called at most once per process, on a background worker thread.
#[cfg(target_os = "linux")]
pub(crate) fn read_linux_fallback() -> Option<f32> {
    // SAFETY: `gnome_text_scaling_factor` dynamically resolves GIO symbols, checks every pointer
    // for null, and releases each GLib object before returning.
    unsafe { gnome_text_scaling_factor() }
}

#[cfg(target_os = "linux")]
unsafe fn gnome_text_scaling_factor() -> Option<f32> {
    use std::ffi::{c_char, c_void};
    // SAFETY: `g_settings_get_double` matches GIO's C ABI and `settings` is valid for the closure.
    unsafe {
        crate::platform::preferences::with_gsettings(
            c"org.gnome.desktop.interface",
            c"text-scaling-factor",
            |gio, settings, key| {
                let get = gio
                    .get::<unsafe extern "C" fn(*mut c_void, *const c_char) -> f64>(b"g_settings_get_double\0")
                    .ok()?;
                from_f64(get(settings, key))
            },
        )
    }
}

// ── iOS Dynamic Type ──────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "ios")]
mod ios {
    use std::ffi::{CStr, c_char};
    use std::sync::{Arc, Mutex, Weak, atomic::{AtomicU32, Ordering}};

    use objc2::{MainThreadMarker, msg_send, rc::Retained, runtime::{AnyClass, AnyObject}};
    use objc2_foundation::NSString;

    #[cfg(target_pointer_width = "64")]
    type CgFloat = f64;
    #[cfg(not(target_pointer_width = "64"))]
    type CgFloat = f32;
    use winit::window::Window;

    use super::{DEFAULT, from_ios_body_points, from_ios_category};

    static CACHED: AtomicU32 = AtomicU32::new(0);
    static WINDOWS: Mutex<Vec<Weak<Window>>> = Mutex::new(Vec::new());
    static WAKERS: Mutex<Vec<crate::Waker>> = Mutex::new(Vec::new());

    fn read_dynamic_type() -> Option<f32> {
        if MainThreadMarker::new().is_none() { return None; }
        // Prefer UIFont's live body point size (follows Apple's Dynamic Type curve automatically),
        // falling back to UIApplication.preferredContentSizeCategory.
        // SAFETY: main thread checked above; runtime class lookup returns None if UIKit is not
        // loaded; selectors match UIKit's public Objective-C API.
        unsafe {
            if let Some(font_cls) = AnyClass::get(c"UIFont") {
                let style = NSString::from_str("UICTFontTextStyleBody");
                let font: *mut AnyObject = msg_send![font_cls, preferredFontForTextStyle: &*style];
                if !font.is_null() {
                    let pt: CgFloat = msg_send![font, pointSize];
                    if let Some(scale) = from_ios_body_points(pt as f64) {
                        return Some(scale);
                    }
                }
            }
            if let Some(app_cls) = AnyClass::get(c"UIApplication") {
                let app: *mut AnyObject = msg_send![app_cls, sharedApplication];
                if !app.is_null() {
                    let cat: *mut AnyObject = msg_send![app, preferredContentSizeCategory];
                    if !cat.is_null() {
                        let utf8: *const c_char = msg_send![cat, UTF8String];
                        if !utf8.is_null()
                            && let Ok(s) = CStr::from_ptr(utf8).to_str()
                            && let Some(scale) = from_ios_category(s)
                        {
                            return Some(scale);
                        }
                    }
                }
            }
        }
        None
    }

    pub(super) fn current() -> f32 {
        let scale = read_dynamic_type().unwrap_or(DEFAULT);
        CACHED.store(scale.to_bits(), Ordering::Release);
        scale
    }

    pub(super) fn cached() -> f32 {
        let bits = CACHED.load(Ordering::Acquire);
        if bits == 0 { DEFAULT } else { f32::from_bits(bits) }
    }

    fn notify_all() {
        let live_windows: Vec<_> = {
            let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
            windows.retain(|w| w.strong_count() > 0);
            windows.iter().filter_map(Weak::upgrade).collect()
        };
        for window in live_windows {
            window.request_redraw();
        }
        let wakers: Vec<_> = WAKERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for wake in wakers {
            wake();
        }
    }

    fn ensure_observer() {
        std::thread_local! {
            static WATCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        if MainThreadMarker::new().is_none() || WATCHING.with(|c| c.replace(true)) { return; }
        let _ = current();
        let block = block2::RcBlock::new(|_: *mut AnyObject| {
            let scale = read_dynamic_type().unwrap_or(DEFAULT);
            let bits = scale.to_bits();
            let prev = CACHED.swap(bits, Ordering::AcqRel);
            if prev != bits {
                notify_all();
            }
        });
        // SAFETY: main thread checked above; NSNotificationCenter.defaultCenter lives for the
        // process, and the process-lifetime observer token is intentionally leaked like accent.rs.
        unsafe {
            let Some(center_cls) = AnyClass::get(c"NSNotificationCenter") else { return };
            let center: Retained<AnyObject> = msg_send![center_cls, defaultCenter];
            let name = NSString::from_str("UIContentSizeCategoryDidChangeNotification");
            let token: Option<Retained<AnyObject>> = msg_send![&*center, addObserverForName: &*name,
                object: std::ptr::null::<AnyObject>(), queue: std::ptr::null::<AnyObject>(), usingBlock: &*block];
            std::mem::forget(token);
        }
    }

    pub(super) fn follow(window: &Arc<Window>) {
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(|w| w.strong_count() > 0);
        windows.push(Arc::downgrade(window));
        drop(windows);
        ensure_observer();
    }

    pub(super) fn follow_waker(waker: crate::Waker) {
        WAKERS.lock().unwrap_or_else(|e| e.into_inner()).push(waker);
        ensure_observer();
    }
}

// ── Test overrides ────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_overrides {
    use std::cell::Cell;

    thread_local! {
        static SCALE: Cell<Option<f32>> = const { Cell::new(None) };
    }

    pub(super) fn get() -> Option<f32> { SCALE.with(Cell::get) }

    pub(crate) struct Guard {
        prev: Option<f32>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            SCALE.with(|c| c.set(self.prev));
        }
    }

    pub(crate) fn set(scale: f32) -> Guard {
        let prev = SCALE.with(|c| c.replace(Some(scale)));
        Guard { prev }
    }
}

#[cfg(test)]
#[path = "text_scale_tests.rs"]
mod tests;
