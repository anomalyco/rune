//! Whole-window operating-system backdrop materials ([`WindowBackdrop`]).

/// Opt-in system-drawn backdrop material behind an entire window or overlay.
///
/// On Windows 11 Build 22621 (22H2) and later, Rune applies this through the documented Win32
/// attribute `DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, ...)` when the window is
/// transparent (`WindowOptions { transparent: true, .. }` or an overlay) and presenting on a
/// hardware GPU through DirectComposition (`Dx12SwapchainKind::DxgiFromVisual`,
/// `CompositeAlphaMode::PreMultiplied`). Rune never calls undocumented APIs such as
/// `SetWindowCompositionAttribute`, accent policies, or pre-22H2 `DWMWA_MICA_EFFECT`.
///
/// Configure at creation with [`App::window_backdrop`](crate::App::window_backdrop),
/// [`WindowSpec::window_backdrop`](crate::WindowSpec::window_backdrop), or
/// [`ConfiguredOverlay::window_backdrop`](crate::ConfiguredOverlay::window_backdrop), update at
/// runtime with [`Tx::set_window_backdrop`](crate::Tx::set_window_backdrop),
/// [`WindowHandle::set_window_backdrop`](crate::WindowHandle::set_window_backdrop), or
/// [`OverlayHandle::set_window_backdrop`](crate::OverlayHandle::set_window_backdrop), and read
/// what the system actually activated with [`Cx::window_backdrop`](crate::Cx::window_backdrop).
///
/// # Choosing between `WindowBackdrop`, `El::system_material`, and `El::backdrop_blur`
///
/// - **Whole-window OS backdrop (this type)**: covers the entire window behind Rune's surface
///   (Windows 11 22H2+ Mica, Mica Alt, and Desktop Acrylic today).
/// - **Per-element OS backdrop ([`El::system_material`](crate::El::system_material))**: blurs
///   what is behind the window under a single rounded element (macOS `NSGlassEffectView` /
///   `NSVisualEffectView` today; draws its fallback fill elsewhere).
/// - **In-window blur ([`El::backdrop_blur`](crate::El::backdrop_blur) / `rune-fx`)**: blurs
///   Rune's own content already drawn behind an element inside the same window; works identically
///   on every platform including web and headless renders.
///
/// # Platform notes
///
/// - **Defaults**: defaults to [`WindowBackdrop::None`]. While unchanged from `None`, Rune makes
///   zero DWM backdrop calls and existing windows, CPU-first launches, and headless renders stay
///   unchanged.
/// - **Active state and system policies**: by Microsoft design, Mica, Mica Alt, and Desktop
///   Acrylic follow window activation and system settings, falling back to a solid neutral colour
///   when the window is inactive, on `WS_EX_NOACTIVATE` overlays, in Battery Saver, in High
///   Contrast mode, or when Transparency Effects are turned off in Windows Settings.
/// - **Unsupported environments**: on Windows builds before 22621, opaque windows, plain HWND
///   swap chains (`DxgiFromHwnd`), the CPU painter, software/WARP adapters, macOS, Linux, web,
///   and headless renders, [`Cx::window_backdrop`](crate::Cx::window_backdrop) reports
///   [`WindowBackdrop::None`]. On Windows, if a non-overlay transparent window requests a
///   non-`None` backdrop that cannot activate, Rune clears its surface to an opaque theme-matched
///   fallback so the window never opens a transparent hole even if the view skips checking
///   `cx.window_backdrop()`.
/// - **Live hardware verification**: the eligibility policy, DWM attribute values, and lifecycle
///   re-application are unit-tested off-screen behind a Win32 seam, but live visual composition on
///   real Windows 11 hardware has not yet been watched by the team and will be confirmed once
///   reported on issue #300.
///
/// ```no_run
/// use rune::{App, AppSpec, Color, WindowBackdrop, WindowOptions, el};
///
/// let opts = WindowOptions { transparent: true, ..Default::default() };
/// let spec = AppSpec::new("Notes", (800.0, 600.0), || (), |_, cx| {
///     let bg = if cx.window_backdrop().is_active() {
///         Color::TRANSPARENT
///     } else {
///         rune::tok::theme(cx.system_dark()).bg
///     };
///     el().w_full().h_full().fill(bg)
/// });
/// App::new(spec)
///     .window_options(opts)
///     .window_backdrop(WindowBackdrop::Mica)
///     .run();
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowBackdrop {
    /// No system backdrop (default). Zero DWM backdrop calls are made.
    #[default]
    None,
    /// Windows 11 Mica (`DWMSBT_MAINWINDOW`): wallpaper-tinted backdrop for long-lived windows.
    Mica,
    /// Windows 11 Mica Alt (`DWMSBT_TABBEDWINDOW`): stronger wallpaper tint for windows with
    /// tabbed titlebars or deeper visual hierarchy.
    MicaAlt,
    /// Windows 11 Desktop Acrylic (`DWMSBT_TRANSIENTWINDOW`): blurred translucent backdrop for
    /// transient surfaces and frosted windows.
    Acrylic,
}

impl WindowBackdrop {
    /// Returns `true` when this value names an active system backdrop ([`Self::Mica`],
    /// [`Self::MicaAlt`], or [`Self::Acrylic`]), and `false` for [`Self::None`].
    ///
    /// ```
    /// use rune::WindowBackdrop;
    ///
    /// assert!(!WindowBackdrop::None.is_active());
    /// assert!(WindowBackdrop::Mica.is_active());
    /// assert!(WindowBackdrop::MicaAlt.is_active());
    /// assert!(WindowBackdrop::Acrylic.is_active());
    /// ```
    #[inline]
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }

    #[inline]
    pub(crate) const fn to_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Mica => 1,
            Self::MicaAlt => 2,
            Self::Acrylic => 3,
        }
    }
}

#[cfg(test)]
#[path = "backdrop_tests.rs"]
mod tests;
