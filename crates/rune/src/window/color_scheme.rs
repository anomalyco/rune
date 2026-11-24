//! Native window and application light/dark appearance overrides ([`ColorScheme`]).

use std::sync::atomic::{AtomicU8, Ordering};

/// Preferred native light or dark appearance for a window or the whole application.
///
/// Follows CSS `color-scheme`, Electron's `nativeTheme.themeSource`, winit's `Option<Theme>`,
/// and AppKit's `NSAppearance` (`nil`, `NSAppearanceNameAqua`, `NSAppearanceNameDarkAqua`).
///
/// Set per-window at launch with [`App::color_scheme`](crate::App::color_scheme) or
/// [`WindowSpec::color_scheme`](crate::WindowSpec::color_scheme), update a window at runtime
/// with [`Tx::set_color_scheme`](crate::Tx::set_color_scheme) or
/// [`WindowHandle::set_color_scheme`](crate::WindowHandle::set_color_scheme), or override
/// the whole application with
/// [`AppHandle::set_color_scheme`](crate::AppHandle::set_color_scheme).
///
/// ```no_run
/// use rune::{App, AppSpec, ColorScheme, el};
///
/// App::new(AppSpec::new("Notes", (640.0, 480.0), || (), |_, _| el()))
///     .color_scheme(ColorScheme::Dark)
///     .run();
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorScheme {
    /// Follow the operating system's appearance (default). For a per-window setting,
    /// inherits the app-wide override set via
    /// [`AppHandle::set_color_scheme`](crate::AppHandle::set_color_scheme) when one is active.
    #[default]
    System,
    /// Force light native chrome and appearance.
    Light,
    /// Force dark native chrome and appearance.
    Dark,
}

impl ColorScheme {
    /// Maps this scheme to winit's `Option<Theme>` (`None` follows the system).
    pub(crate) const fn winit_theme(self) -> Option<winit::window::Theme> {
        match self {
            ColorScheme::System => None,
            ColorScheme::Light => Some(winit::window::Theme::Light),
            ColorScheme::Dark => Some(winit::window::Theme::Dark),
        }
    }

    /// Resolves a window's effective scheme against the app-wide scheme: an explicit
    /// per-window `Light` or `Dark` overrides the app-wide scheme, while `System`
    /// inherits `app`.
    pub(crate) const fn effective_for_window(self, app: ColorScheme) -> ColorScheme {
        match self {
            ColorScheme::System => app,
            explicit => explicit,
        }
    }

    /// AppKit `NSAppearanceName` corresponding to this scheme, or `None` for `System`
    /// (passing `nil` to `setAppearance:` so AppKit inherits the enclosing or OS appearance).
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(crate) const fn macos_appearance_name(self) -> Option<&'static str> {
        match self {
            ColorScheme::System => None,
            ColorScheme::Light => Some("NSAppearanceNameAqua"),
            ColorScheme::Dark => Some("NSAppearanceNameDarkAqua"),
        }
    }

    pub(crate) const fn to_u8(self) -> u8 {
        match self {
            ColorScheme::System => 0,
            ColorScheme::Light => 1,
            ColorScheme::Dark => 2,
        }
    }

    pub(crate) const fn from_u8(value: u8) -> Self {
        match value {
            1 => ColorScheme::Light,
            2 => ColorScheme::Dark,
            _ => ColorScheme::System,
        }
    }
}

/// Process-wide app-level color scheme set via `AppHandle::set_color_scheme`, retained so
/// windows created or recreated later on Windows and Linux inherit it when their own
/// per-window override is `ColorScheme::System`.
static APP_COLOR_SCHEME: AtomicU8 = AtomicU8::new(0);

/// Returns the current process-wide app-level color scheme.
pub(crate) fn app_color_scheme() -> ColorScheme {
    ColorScheme::from_u8(APP_COLOR_SCHEME.load(Ordering::Acquire))
}

/// Stores the process-wide app-level color scheme.
pub(crate) fn set_app_color_scheme_state(scheme: ColorScheme) {
    APP_COLOR_SCHEME.store(scheme.to_u8(), Ordering::Release);
}

/// Resolves the winit `Option<Theme>` to apply to a native window on `is_macos` or other
/// desktop platforms.
///
/// On macOS, AppKit natively inherits `NSApplication.appearance` for any window whose own
/// `NSWindow.appearance` is `nil`, so `ColorScheme::System` maps to `None` regardless of
/// `app`. On Windows and Linux there is no process-wide appearance property, so a window
/// set to `ColorScheme::System` falls back to `app`.
pub(crate) const fn window_theme_for_platform(
    window: ColorScheme,
    app: ColorScheme,
    is_macos: bool,
) -> Option<winit::window::Theme> {
    if is_macos {
        window.winit_theme()
    } else {
        window.effective_for_window(app).winit_theme()
    }
}

/// Deduplicating tracker for applying a window's effective winit theme only when it changes.
/// Used by native hosts and unit-tested behind a fake sink.
pub(crate) fn sync_window_theme(
    applied: &mut Option<winit::window::Theme>,
    target: Option<winit::window::Theme>,
    apply: impl FnOnce(Option<winit::window::Theme>),
) -> bool {
    if *applied == target {
        return false;
    }
    *applied = target;
    apply(target);
    true
}

#[cfg(test)]
#[path = "color_scheme_tests.rs"]
mod tests;
