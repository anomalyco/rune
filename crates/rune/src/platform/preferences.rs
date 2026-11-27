//! Native accessibility and appearance preferences, sampled when a host is created or focused
//! (appearance also on each redraw, which a change asks for).
//! Headless runtimes use their explicit Env instead.

#![allow(unsafe_code)]

#[cfg(target_os = "macos")]
pub(crate) fn reduced_motion() -> bool {
    use objc2::{class, msg_send};
    // SAFETY: NSWorkspace's shared instance lives for the process; these selectors return
    // an object pointer and Objective-C BOOL respectively. Native hosts call on the UI thread.
    unsafe {
        let workspace: *mut objc2::runtime::AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let reduced: objc2::runtime::Bool = msg_send![workspace, accessibilityDisplayShouldReduceMotion];
        reduced.as_bool()
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
    let mut animations: i32 = 1;
    // SAFETY: this SPI writes one BOOL to a valid, aligned local variable.
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut animations as *mut i32).cast(), 0) != 0 && animations == 0 }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) fn reduced_motion() -> bool {
    crate::platform::portal_appearance::reduced_motion()
}

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
static CACHED_REDUCED_MOTION: CachedReducedMotion = CachedReducedMotion::new();

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
static CACHED_DOUBLE_CLICK: CachedDoubleClick = CachedDoubleClick::new();

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
static FALLBACK_WINDOWS: std::sync::Mutex<Vec<std::sync::Weak<winit::window::Window>>> =
    std::sync::Mutex::new(Vec::new());

/// Registers `window` to redraw if a slow Linux fallback preference read finishes with a
/// non-default value after launch already fell back to the default.
#[cfg(all(target_os = "linux", not(feature = "dbus")))]
pub(crate) fn follow_fallback(window: &std::sync::Arc<winit::window::Window>) {
    let mut windows = FALLBACK_WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    windows.retain(|w| w.strong_count() > 0);
    windows.push(std::sync::Arc::downgrade(window));
    drop(windows);
    if CACHED_REDUCED_MOTION.late_changed() || CACHED_DOUBLE_CLICK.late_changed() {
        window.request_redraw();
    }
}

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
fn redraw_fallback_windows() {
    let live: Vec<_> = {
        let mut windows = FALLBACK_WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(|w| w.strong_count() > 0);
        windows.iter().filter_map(std::sync::Weak::upgrade).collect()
    };
    for window in live {
        window.request_redraw();
    }
}

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
pub(crate) fn reduced_motion() -> bool {
    CACHED_REDUCED_MOTION.get_or_init_notifying(read_reduced_motion_fallback, redraw_fallback_windows)
}

/// Default double-click interval (milliseconds) when the Linux desktop reports none.
#[cfg(any(test, target_os = "linux"))]
pub(crate) const DEFAULT_DOUBLE_CLICK_MS: u32 = 500;
/// Default double-click interval (seconds) when the Linux desktop reports none.
#[cfg(any(test, all(target_os = "linux", feature = "dbus")))]
pub(crate) const DEFAULT_DOUBLE_CLICK_SECS: f64 = DEFAULT_DOUBLE_CLICK_MS as f64 / 1000.0;
/// Bounds for a Linux desktop's reported double-click interval (milliseconds).
#[cfg(any(test, target_os = "linux"))]
pub(crate) const MIN_DOUBLE_CLICK_MS: u32 = 50;
#[cfg(any(test, target_os = "linux"))]
pub(crate) const MAX_DOUBLE_CLICK_MS: u32 = 5000;

/// Clamps a positive millisecond double-click interval to `MIN_DOUBLE_CLICK_MS..=MAX_DOUBLE_CLICK_MS`,
/// returning `None` for zero.
#[cfg(any(test, target_os = "linux"))]
pub(crate) fn clamp_double_click_ms(ms: u32) -> Option<u32> {
    (ms > 0).then(|| ms.clamp(MIN_DOUBLE_CLICK_MS, MAX_DOUBLE_CLICK_MS))
}

/// The Linux desktop's configured double-click interval in seconds: from the XDG Settings portal
/// (`org.gnome.desktop.peripherals.mouse` `double-click`), falling back off the UI thread to
/// `kdeglobals` or GSettings, or `0.5` s when none answers.
#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) fn double_click_interval() -> f64 {
    crate::platform::portal_appearance::double_click_interval().unwrap_or(DEFAULT_DOUBLE_CLICK_SECS)
}

#[cfg(all(target_os = "linux", not(feature = "dbus")))]
pub(crate) fn double_click_interval() -> f64 {
    CACHED_DOUBLE_CLICK.get_or_init_notifying(read_double_click_fallback, redraw_fallback_windows)
}

#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CacheState<T> {
    /// No worker started yet.
    Unstarted,
    /// Worker is reading the platform setting; `timed_out` records that at least one caller
    /// returned `default` before the worker finished.
    Pending { timed_out: bool },
    /// Worker finished and stored the resolved setting; `late_changed` is true when a caller
    /// already returned `default` and the resolved setting differs from it.
    Ready { value: T, late_changed: bool },
}

/// One-shot off-UI-thread cache for Linux fallback preference reads when `dbus` is disabled:
/// spawns one short-lived worker thread per process, waits at most `FIRST_ANSWER` so launch does
/// not stall, and notifies open windows if a non-default answer arrives after that wait expires.
#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
struct CachedPref<T: Copy + Eq + Send + Sync + 'static> {
    default: T,
    state: std::sync::Mutex<CacheState<T>>,
    ready: std::sync::Condvar,
}

#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
impl<T: Copy + Eq + Send + Sync + 'static> CachedPref<T> {
    const FIRST_ANSWER: std::time::Duration = std::time::Duration::from_millis(100);

    const fn new(default: T) -> Self {
        Self {
            default,
            state: std::sync::Mutex::new(CacheState::Unstarted),
            ready: std::sync::Condvar::new(),
        }
    }

    fn get_or_init_with(
        &'static self,
        name: &'static str,
        wait: std::time::Duration,
        read: impl FnOnce() -> T + Send + 'static,
        on_late: impl FnOnce() + Send + 'static,
    ) -> T {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match *state {
            CacheState::Ready { value, .. } => return value,
            CacheState::Pending { ref mut timed_out } => {
                *timed_out = true;
                return self.default;
            }
            CacheState::Unstarted => *state = CacheState::Pending { timed_out: false },
        }
        let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
            let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).unwrap_or(self.default);
            let notify_late = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let was_timed_out = matches!(*state, CacheState::Pending { timed_out: true });
                let late_changed = was_timed_out && value != self.default;
                *state = CacheState::Ready { value, late_changed };
                self.ready.notify_all();
                late_changed
            };
            if notify_late {
                on_late();
            }
        });
        if spawned.is_err() {
            *state = CacheState::Ready { value: self.default, late_changed: false };
            self.ready.notify_all();
            return self.default;
        }
        let (mut state, _) = self
            .ready
            .wait_timeout_while(state, wait, |s| matches!(*s, CacheState::Pending { .. }))
            .unwrap_or_else(|e| e.into_inner());
        match *state {
            CacheState::Ready { value, .. } => value,
            CacheState::Pending { ref mut timed_out } => {
                *timed_out = true;
                self.default
            }
            CacheState::Unstarted => self.default,
        }
    }

    fn late_changed(&self) -> bool {
        matches!(
            *self.state.lock().unwrap_or_else(|e| e.into_inner()),
            CacheState::Ready { late_changed: true, .. }
        )
    }

    #[cfg(test)]
    fn wait_ready(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !matches!(*state, CacheState::Ready { .. }) {
            state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// One-shot off-UI-thread cache for the Linux fallback reduced-motion read when the `dbus`
/// feature is disabled: spawns one short-lived worker thread per process so focus changes never
/// touch the filesystem or `dlopen` libgio on the UI thread.
#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
struct CachedReducedMotion(CachedPref<bool>);

#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
impl CachedReducedMotion {
    const fn new() -> Self {
        Self(CachedPref::new(false))
    }

    fn get_or_init_notifying(&'static self, read: fn() -> bool, on_late: fn()) -> bool {
        self.0.get_or_init_with("rune-reduced-motion", CachedPref::<bool>::FIRST_ANSWER, read, on_late)
    }

    fn late_changed(&self) -> bool {
        self.0.late_changed()
    }

    #[cfg(test)]
    fn get_or_init(&'static self, read: fn() -> bool) -> bool {
        self.get_or_init_notifying(read, || {})
    }

    #[cfg(test)]
    fn get_or_init_with(
        &'static self,
        wait: std::time::Duration,
        read: impl FnOnce() -> bool + Send + 'static,
        on_late: impl FnOnce() + Send + 'static,
    ) -> bool {
        self.0.get_or_init_with("rune-reduced-motion", wait, read, on_late)
    }

    #[cfg(test)]
    fn wait_ready(&self) {
        self.0.wait_ready();
    }
}

/// One-shot off-UI-thread cache for the Linux fallback double-click interval when the `dbus`
/// feature is disabled: spawns one short-lived worker thread per process so window creation or
/// redraws never touch the filesystem or `dlopen` libgio on the UI thread.
#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
struct CachedDoubleClick(CachedPref<u32>);

#[cfg(any(test, all(target_os = "linux", not(feature = "dbus"))))]
impl CachedDoubleClick {
    const fn new() -> Self {
        Self(CachedPref::new(DEFAULT_DOUBLE_CLICK_MS))
    }

    fn get_or_init_notifying(&'static self, read: fn() -> Option<u32>, on_late: fn()) -> f64 {
        let ms = self.0.get_or_init_with(
            "rune-double-click",
            CachedPref::<u32>::FIRST_ANSWER,
            move || read().and_then(clamp_double_click_ms).unwrap_or(DEFAULT_DOUBLE_CLICK_MS),
            on_late,
        );
        f64::from(ms) / 1000.0
    }

    fn late_changed(&self) -> bool {
        self.0.late_changed()
    }

    #[cfg(test)]
    fn get_or_init(&'static self, read: fn() -> Option<u32>) -> f64 {
        self.get_or_init_notifying(read, || {})
    }

    #[cfg(test)]
    fn get_or_init_with(
        &'static self,
        wait: std::time::Duration,
        read: impl FnOnce() -> Option<u32> + Send + 'static,
        on_late: impl FnOnce() + Send + 'static,
    ) -> f64 {
        let ms = self.0.get_or_init_with(
            "rune-double-click",
            wait,
            move || read().and_then(clamp_double_click_ms).unwrap_or(DEFAULT_DOUBLE_CLICK_MS),
            on_late,
        );
        f64::from(ms) / 1000.0
    }

    #[cfg(test)]
    fn wait_ready(&self) {
        self.0.wait_ready();
    }
}

#[cfg(any(test, target_os = "linux"))]
fn parse_kdeglobals_reduced_motion(contents: &str) -> Option<bool> {
    contents.lines()
        .filter_map(|line| line.trim().split_once('='))
        .find_map(|(k, v)| (k.trim() == "AnimationDurationFactor").then(|| v.trim().parse::<f64>().ok()).flatten())
        .map(|factor| factor == 0.0)
}

#[cfg(any(test, target_os = "linux"))]
fn parse_kdeglobals_double_click_ms(contents: &str) -> Option<u32> {
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let mut in_kde = false;
    for line in contents.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_kde = section.trim() == "KDE";
            continue;
        }
        if !in_kde { continue; }
        if let Some((k, v)) = line.split_once('=')
            && k.trim() == "DoubleClickInterval"
        {
            let ms = v.trim().parse::<i64>().ok()?;
            if ms <= 0 { return None; }
            return clamp_double_click_ms(ms.min(i64::from(u32::MAX)) as u32);
        }
    }
    None
}

#[cfg(any(test, target_os = "linux"))]
fn resolve_double_click_fallback(
    is_kde: bool,
    kdeglobals: impl FnOnce() -> Option<u32>,
    gsettings: impl FnOnce() -> Option<u32>,
) -> Option<u32> {
    if is_kde { kdeglobals() } else { gsettings().or_else(kdeglobals) }
}

#[cfg(target_os = "linux")]
fn is_kde_desktop() -> bool {
    rune_core::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').any(|d| d.eq_ignore_ascii_case("KDE"))
}

#[cfg(target_os = "linux")]
fn read_kdeglobals() -> Option<String> {
    let config = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| std::path::PathBuf::from(h).join(".config")))?;
    std::fs::read_to_string(config.join("kdeglobals")).ok()
}

/// Fallback reduced-motion read for Linux desktops whose Settings portal answers neither
/// `org.freedesktop.appearance` `reduced-motion` nor `org.gnome.desktop.interface`
/// `enable-animations`, or builds without the `dbus` feature. Called at most once per process,
/// on a background worker thread.
#[cfg(target_os = "linux")]
pub(crate) fn read_reduced_motion_fallback() -> bool {
    // KDE exposes animation duration in kdeglobals; zero disables animation.
    if is_kde_desktop()
        && let Some(contents) = read_kdeglobals()
        && let Some(reduced) = parse_kdeglobals_reduced_motion(&contents)
    { return reduced; }
    // Optional GNOME GSettings, with schema lookup before constructing settings (missing
    // schemas otherwise abort GLib). No subprocess or required GNOME dependency.
    // SAFETY: `gnome_animations` dynamically resolves GIO symbols, checks every pointer for null,
    // and releases each GLib object before returning.
    unsafe { gnome_animations().is_some_and(|enabled| !enabled) }
}

/// Fallback double-click interval read (in milliseconds) for Linux desktops whose Settings portal
/// does not answer `org.gnome.desktop.peripherals.mouse` `double-click`, or builds without the
/// `dbus` feature. Called at most once per process, on a background worker thread.
#[cfg(target_os = "linux")]
pub(crate) fn read_double_click_fallback() -> Option<u32> {
    resolve_double_click_fallback(
        is_kde_desktop(),
        || read_kdeglobals().as_deref().and_then(parse_kdeglobals_double_click_ms),
        // SAFETY: `gnome_double_click_ms` dynamically resolves GIO symbols, checks every pointer
        // for null, and releases each GLib object before returning.
        || unsafe { gnome_double_click_ms() },
    )
}

#[cfg(target_os = "linux")]
unsafe fn with_gsettings<T>(
    schema_id: &std::ffi::CStr,
    key: &std::ffi::CStr,
    read: impl FnOnce(&libloading::Library, *mut std::ffi::c_void, *const std::ffi::c_char) -> Option<T>,
) -> Option<T> {
    use std::ffi::{c_char, c_void};
    type Ptr = *mut c_void;
    // SAFETY: signatures match GIO/GLib's public C API; every reference is released while
    // the library is loaded. Null source/schema/settings are handled before use.
    unsafe {
        static GIO: std::sync::OnceLock<Option<libloading::Library>> = std::sync::OnceLock::new();
        let gio = GIO.get_or_init(|| libloading::Library::new("libgio-2.0.so.0").ok()).as_ref()?;
        let default = gio.get::<unsafe extern "C" fn() -> Ptr>(b"g_settings_schema_source_get_default\0").ok()?;
        let lookup = gio.get::<unsafe extern "C" fn(Ptr, *const c_char, i32) -> Ptr>(b"g_settings_schema_source_lookup\0").ok()?;
        let has_key = gio.get::<unsafe extern "C" fn(Ptr, *const c_char) -> i32>(b"g_settings_schema_has_key\0").ok()?;
        let new = gio.get::<unsafe extern "C" fn(Ptr, Ptr, *const c_char) -> Ptr>(b"g_settings_new_full\0").ok()?;
        let schema_unref = gio.get::<unsafe extern "C" fn(Ptr)>(b"g_settings_schema_unref\0").ok()?;
        let unref = gio.get::<unsafe extern "C" fn(Ptr)>(b"g_object_unref\0").ok()?;
        let source = default();
        if source.is_null() { return None; }
        let schema = lookup(source, schema_id.as_ptr(), 1);
        if schema.is_null() { return None; }
        if has_key(schema, key.as_ptr()) == 0 { schema_unref(schema); return None; }
        let settings = new(schema, std::ptr::null_mut(), std::ptr::null());
        schema_unref(schema);
        if settings.is_null() { return None; }
        let value = read(gio, settings, key.as_ptr());
        unref(settings);
        // The static library stays loaded for GLib's retained backend callbacks.
        value
    }
}

#[cfg(target_os = "linux")]
unsafe fn gnome_animations() -> Option<bool> {
    use std::ffi::{c_char, c_void};
    // SAFETY: `g_settings_get_boolean` matches GIO's C ABI and `settings` is valid for the closure.
    unsafe {
        with_gsettings(c"org.gnome.desktop.interface", c"enable-animations", |gio, settings, key| {
            let get = gio.get::<unsafe extern "C" fn(*mut c_void, *const c_char) -> i32>(b"g_settings_get_boolean\0").ok()?;
            Some(get(settings, key) != 0)
        })
    }
}

#[cfg(target_os = "linux")]
unsafe fn gnome_double_click_ms() -> Option<u32> {
    use std::ffi::{c_char, c_void};
    // SAFETY: `g_settings_get_int` matches GIO's C ABI and `settings` is valid for the closure.
    unsafe {
        with_gsettings(c"org.gnome.desktop.peripherals.mouse", c"double-click", |gio, settings, key| {
            let get = gio.get::<unsafe extern "C" fn(*mut c_void, *const c_char) -> i32>(b"g_settings_get_int\0").ok()?;
            let ms = get(settings, key);
            u32::try_from(ms).ok().and_then(clamp_double_click_ms)
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub(crate) fn reduced_motion() -> bool { false }

/// `RUNE_INCREASE_CONTRAST=1`: [`increased_contrast_with`] on, for captures and checks.
pub(crate) fn increased_contrast_preset() -> bool {
    std::env::var_os("RUNE_INCREASE_CONTRAST").is_some_and(|v| v == "1")
}

/// Whether the system asks apps for more contrast: macOS's Increase Contrast (Accessibility >
/// Display), or a Windows contrast theme. `RUNE_INCREASE_CONTRAST=1` reports it on, for captures
/// and checks, without turning it on for the whole machine. Read as [`reduced_motion`] is, and on
/// macOS again when the display options change (`a11y/macos.rs`).
///
/// Reuses an already-sampled `contrast().is_some()` on Windows so callers that read both avoid
/// querying `SPI_GETHIGHCONTRAST` twice and always see a consistent pair.
pub(crate) fn increased_contrast_with(has_contrast: bool) -> bool {
    #[cfg(test)]
    if let Some(v) = test_overrides::increased_contrast() { return v; }
    if increased_contrast_preset() { return true; }
    #[cfg(target_os = "macos")]
    {
        let _ = has_contrast;
        use objc2::{class, msg_send};
        // SAFETY: NSWorkspace's shared instance lives for the process; the selector returns an
        // Objective-C BOOL. Native hosts call on the UI thread.
        unsafe {
            let workspace: *mut objc2::runtime::AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let more: objc2::runtime::Bool = msg_send![workspace, accessibilityDisplayShouldIncreaseContrast];
            more.as_bool()
        }
    }
    #[cfg(target_os = "windows")]
    { has_contrast }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = has_contrast;
        false
    }
}

/// The colours a system contrast theme asks every app to paint with (Windows' Contrast themes,
/// CSS `forced-colors`' system colours), in linear sRGB as Rune's colours are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemContrast {
    /// A window's ground (`Canvas`, `COLOR_WINDOW`).
    pub window: crate::Color,
    /// Text on it (`CanvasText`, `COLOR_WINDOWTEXT`).
    pub text: crate::Color,
    /// A selection, a focus ring, a checked control (`Highlight`, `COLOR_HIGHLIGHT`).
    pub highlight: crate::Color,
    /// Text on [`Self::highlight`] (`HighlightText`, `COLOR_HIGHLIGHTTEXT`).
    pub highlight_text: crate::Color,
    /// Disabled text (`GrayText`, `COLOR_GRAYTEXT`).
    pub disabled: crate::Color,
    /// A button's face and text (`ButtonFace`, `ButtonText`).
    pub button: crate::Color,
    pub button_text: crate::Color,
    /// A link (`LinkText`, `COLOR_HOTLIGHT`).
    pub link: crate::Color,
}

/// The system's contrast theme, when one is on (Windows: Settings > Accessibility > Contrast
/// themes). Read when a window or overlay opens and when Windows says its settings or colours changed.
#[cfg(target_os = "windows")]
pub(crate) fn contrast() -> Option<SystemContrast> {
    #[cfg(test)]
    if let Some(v) = test_overrides::contrast() { return v; }
    if let Some(preset) = contrast_preset() { return Some(preset); }
    use windows_sys::Win32::Graphics::Gdi::GetSysColor;
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, dwFlags: 0, lpszDefaultScheme: std::ptr::null_mut() };
    // SAFETY: SPI_GETHIGHCONTRAST fills a HIGHCONTRASTW whose size it is told.
    let read = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, (&mut hc as *mut HIGHCONTRASTW).cast(), 0) } != 0;
    if !read || hc.dwFlags & HCF_HIGHCONTRASTON == 0 { return None; }
    let sys = |index: i32| {
        // SAFETY: `GetSysColor` is a read-only Win32 query; callers pass standard `COLOR_*` indices.
        let c = unsafe { GetSysColor(index) };
        let d = |v: u32| crate::color::srgb_decode((v & 0xff) as f32 / 255.0);
        crate::Color { r: d(c), g: d(c >> 8), b: d(c >> 16), a: 1.0 }
    };
    // COLOR_WINDOW 5, WINDOWTEXT 8, HIGHLIGHT 13, HIGHLIGHTTEXT 14, BTNFACE 15, GRAYTEXT 17,
    // BTNTEXT 18, HOTLIGHT 26.
    Some(SystemContrast { window: sys(5), text: sys(8), highlight: sys(13), highlight_text: sys(14), disabled: sys(17), button: sys(15), button_text: sys(18), link: sys(26) })
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn contrast() -> Option<SystemContrast> {
    #[cfg(test)]
    if let Some(v) = test_overrides::contrast() { return v; }
    contrast_preset()
}

/// `RUNE_CONTRAST=aquatic|desert|dusk|night-sky`: Windows 11's contrast theme of that name, as
/// its system colours are, without turning it on for the whole machine: for captures and checks
/// of an app in each.
pub(crate) fn contrast_preset() -> Option<SystemContrast> {
    let name = rune_core::env::var("RUNE_CONTRAST")?;
    let [window, text, highlight, highlight_text, disabled, link] = match name.to_ascii_lowercase().as_str() {
        "aquatic" => ["#202020", "#ffffff", "#8ee3f0", "#263b50", "#a6a6a6", "#75e9fc"],
        "desert" => ["#fffaef", "#3d3d3d", "#903909", "#fff5e3", "#6d6d6d", "#1c5e75"],
        "dusk" => ["#2d3236", "#ffffff", "#a1bfde", "#212d3b", "#a6a6a6", "#70ebde"],
        "night-sky" | "nightsky" => ["#000000", "#ffffff", "#d6b4fd", "#2b2b2b", "#a6a6a6", "#8080ff"],
        _ => return None,
    }.map(|h| crate::color::hex(h).unwrap_or(crate::Color::BLACK));
    Some(SystemContrast { window, text, highlight, highlight_text, disabled, button: window, button_text: text, link })
}
/// The desktop's appearance as an app keeps it from one run to the next (`last_appearance`):
/// light or dark (None: the desktop says neither), and whether it asks for reduced motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub dark: Option<bool>,
    pub reduced_motion: bool,
}

/// Gives Rune the desktop's appearance as the app's last run saw it, before the first window:
/// `last` is what `keep` was last given (None on a first run). Linux asks the XDG desktop portal
/// for the appearance, over D-Bus, and a window's first frame can't wait for an answer that may
/// take a tenth of a second (a portal that is starting) without the window coming that much
/// later. With `last`, the first frame draws with it at once, and the portal's answer redraws
/// the windows only when it differs (the desktop's setting changed while the app was closed).
/// Without it, the first window waits for the portal as it did.
///
/// `keep` runs on a worker thread with each answer that isn't what it last had: save it where
/// the app keeps its launch state. Windows and macOS report their appearance without a wait, so
/// there this does nothing.
pub fn last_appearance(last: Option<Appearance>, keep: impl FnMut(Appearance) + Send + 'static) {
    #[cfg(all(target_os = "linux", feature = "dbus"))]
    crate::platform::portal_appearance::last(last, Box::new(keep));
    #[cfg(not(all(target_os = "linux", feature = "dbus")))]
    let _ = (last, keep);
}

/// The desktop's light or dark preference where winit reports none (Linux: the XDG desktop
/// portal's `color-scheme`). None leaves the host's own default.
#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) fn dark() -> Option<bool> { crate::platform::portal_appearance::dark() }

#[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_arch = "wasm32")))]
pub(crate) fn dark() -> Option<bool> { None }

/// [`dark`] for a window winit gives no theme; None leaves the window's appearance as it is.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn desktop_dark(window: &winit::window::Window) -> Option<bool> {
    dark().filter(|_| window.theme().is_none())
}

/// A new window's appearance: `--theme` when given, else winit's theme, else [`desktop_dark`],
/// else dark. On Linux a change of the desktop's preference redraws the window, which rereads it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn window_dark(window: &std::sync::Arc<winit::window::Window>, theme: Option<&str>) -> bool {
    #[cfg(all(target_os = "linux", feature = "dbus"))]
    crate::platform::portal_appearance::follow(window);
    #[cfg(all(target_os = "linux", not(feature = "dbus")))]
    follow_fallback(window);
    match theme {
        Some("light") => false,
        Some("dark") => true,
        _ => window.theme().map_or_else(|| desktop_dark(window).unwrap_or(true), |t| t == winit::window::Theme::Dark),
    }
}

/// macOS "Keyboard navigation" (Full Keyboard Access): when off, Tab visits only text fields
/// and lists, as in AppKit. Windows and Linux always Tab through every control.
#[cfg(target_os = "macos")]
pub(crate) fn full_keyboard_access() -> bool {
    use objc2::{class, msg_send};
    // SAFETY: NSApplication's shared instance lives for the process; the selector returns an
    // Objective-C BOOL. Native hosts call on the UI thread.
    unsafe {
        let app: *mut objc2::runtime::AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let enabled: objc2::runtime::Bool = msg_send![app, isFullKeyboardAccessEnabled];
        enabled.as_bool()
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn full_keyboard_access() -> bool { true }

#[cfg(test)]
pub(crate) mod test_overrides {
    use std::cell::Cell;
    use super::SystemContrast;

    thread_local! {
        static CONTRAST: Cell<Option<Option<SystemContrast>>> = const { Cell::new(None) };
        static INCREASED_CONTRAST: Cell<Option<bool>> = const { Cell::new(None) };
    }

    pub(super) fn contrast() -> Option<Option<SystemContrast>> { CONTRAST.with(Cell::get) }
    pub(super) fn increased_contrast() -> Option<bool> { INCREASED_CONTRAST.with(Cell::get) }

    pub(crate) struct Guard {
        prev_contrast: Option<Option<SystemContrast>>,
        prev_increased: Option<bool>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            CONTRAST.with(|c| c.set(self.prev_contrast));
            INCREASED_CONTRAST.with(|c| c.set(self.prev_increased));
        }
    }

    pub(crate) fn set_contrast(contrast: Option<SystemContrast>, increased_contrast: bool) -> Guard {
        let prev_contrast = CONTRAST.with(|c| c.replace(Some(contrast)));
        let prev_increased = INCREASED_CONTRAST.with(|c| c.replace(Some(increased_contrast)));
        Guard { prev_contrast, prev_increased }
    }
}

#[cfg(test)]
#[path = "preferences_tests.rs"]
mod tests;
