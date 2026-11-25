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
///   (Windows 11 22H2+ Mica, Mica Alt, and Desktop Acrylic; Linux Wayland compositor blur via
///   `WindowBackdrop::Blur`).
/// - **Per-element OS backdrop ([`El::system_material`](crate::El::system_material))**: blurs
///   what is behind the window under a single rounded element (macOS `NSGlassEffectView` /
///   `NSVisualEffectView`, Linux Wayland `wl_region` blur when `WindowBackdrop::None` is
///   active, and opt-in Windows 11 `windows-system-material`; draws its fallback fill elsewhere).
/// - **In-window blur ([`El::backdrop_blur`](crate::El::backdrop_blur) / `rune-fx`)**: blurs
///   Rune's own content already drawn behind an element inside the same window; works identically
///   on every platform including web and headless renders.
///
/// # Platform notes
///
/// - **Defaults**: defaults to [`WindowBackdrop::None`]. While unchanged from `None`, Rune makes
///   zero DWM backdrop calls and existing windows, CPU-first launches, and headless renders stay
///   unchanged.
/// - **Active state and system policies**: on Windows 11, `DWMWA_SYSTEMBACKDROP_TYPE` draws Mica,
///   Mica Alt, and Desktop Acrylic only while the window is the foreground window and falls back
///   to a solid neutral colour whenever the window is unfocused, in Battery Saver, in High
///   Contrast mode, or when Transparency Effects are turned off in Windows Settings. Because this
///   is Windows' own rendering behaviour, [`Cx::window_backdrop`](crate::Cx::window_backdrop)
///   still reports the active variant (and `is_active()` stays `true`) once DWM accepts the
///   attribute—including on a non-activating window or overlay (`KeyPolicy::None`) that never
///   takes focus and therefore never shows the live material. Pair `cx.window_backdrop()` with
///   [`App::on_focus`](crate::App::on_focus) or
///   [`El::on_window_focus`](crate::El::on_window_focus) if your view needs to tell "backdrop
///   active and window focused" from "backdrop active" and supply its own fill while unfocused.
/// - **Linux Wayland compositor blur (`WindowBackdrop::Blur`)**: binds the Wayland staging
///   protocol `ext_background_effect_v1` (version 1, checking `Capability::Blur`; KWin 6.7+,
///   GNOME Mutter 51+, Hyprland 0.56+, niri 26.04+, COSMIC 1.11) with
///   `org_kde_kwin_blur_manager` (version 1) as a fallback when `ext_background_effect_v1` is
///   absent (KDE Plasma 5.x–6.6.x). Works on transparent GPU toplevel windows, toplevel-fallback
///   overlays, and `zwlr_layer_shell_v1` overlays, approximating `sctk-adwaita` rounded bottom
///   corners in surface-local logical coordinates. Neither Wayland protocol ties blur to keyboard
///   focus—the blur region is surface-attached state and compositors such as KWin blur mapped
///   unfocused windows and layer-shell overlays alike—though the blur algorithm and any focus
///   rules remain subject to compositor policy (and disabling a compositor's blur effect revokes
///   `Capability::Blur` at runtime). Unlike Windows Mica/Acrylic (which paint DWM's own tint
///   behind [`Color::TRANSPARENT`](crate::Color::TRANSPARENT)), Wayland blur is untinted: views
///   should draw a translucent tint (such as `bg.alpha(0.75)`) when
///   `cx.window_backdrop() == WindowBackdrop::Blur`. Query
///   [`Cx::window_backdrop_supported`](crate::Cx::window_backdrop_supported) to check compositor
///   support even while [`WindowBackdrop::None`] is selected.
/// - **Unsupported environments**: on Windows builds before 22621, X11/XWayland, Wayland
///   compositors without `ext_background_effect_v1` (or with blur disabled) and without
///   `org_kde_kwin_blur_manager`, opaque windows, plain HWND swap chains (`DxgiFromHwnd`), the
///   CPU painter, software/WARP adapters, macOS, web, and headless renders,
///   [`Cx::window_backdrop`](crate::Cx::window_backdrop) reports [`WindowBackdrop::None`]. On
///   Windows and Linux, if a non-overlay transparent window requests a non-`None` backdrop that
///   cannot activate, Rune clears its surface to an opaque theme-matched fallback so the window
///   never opens a transparent hole even if the view skips checking `cx.window_backdrop()`.
/// - **Live hardware verification**: Windows 11 Mica, Mica Alt, and Desktop Acrylic have been
///   verified on live Windows 11 hardware (standard, custom-titlebar, and borderless windows;
///   issue #300). Whole-surface Wayland `WindowBackdrop::Blur` on a `zwlr_layer_shell_v1`
///   overlay has been confirmed live on KDE Plasma / KWin 6.8.3; toplevel windows, other
///   compositors, and per-element `El::system_material` shaped blur are unit-tested off-screen
///   and awaiting live confirmation on issue #1381.
///
/// ```no_run
/// use rune::{App, AppSpec, Color, WindowBackdrop, WindowOptions, el};
///
/// struct State { focused: bool }
///
/// let opts = WindowOptions { transparent: true, ..Default::default() };
/// let spec = AppSpec::new("Notes", (800.0, 600.0), || State { focused: true }, |s, cx| {
///     let base = rune::tok::theme(cx.system_dark()).bg;
///     // Wayland blur is untinted (draw a translucent fill); Windows draws its own flat
///     // neutral fallback while unfocused even though `is_active()` stays `true`.
///     let bg = match cx.window_backdrop() {
///         WindowBackdrop::Blur => base.alpha(0.75),
///         b if b.is_active() && s.focused => Color::TRANSPARENT,
///         _ => base,
///     };
///     el().w_full().h_full().fill(bg)
/// });
/// App::new(spec)
///     .window_options(opts)
///     .window_backdrop(WindowBackdrop::Mica)
///     .on_focus(|s, focused, _| s.focused = focused)
///     .run();
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowBackdrop {
    /// No system backdrop (default). Zero DWM or Wayland blur surface requests are made.
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
    /// Linux Wayland compositor behind-window blur (`ext_background_effect_v1`, with
    /// `org_kde_kwin_blur_manager` fallback). Supplies untinted blur behind a translucent surface
    /// fill; maps to [`WindowBackdrop::None`] on Windows, macOS, X11, web, and unsupported
    /// Wayland compositors.
    Blur,
}

impl WindowBackdrop {
    /// Returns `true` when this value names an active system backdrop ([`Self::Mica`],
    /// [`Self::MicaAlt`], [`Self::Acrylic`], or [`Self::Blur`]), and `false` for [`Self::None`].
    ///
    /// Note that on Windows 11, `cx.window_backdrop().is_active()` reports that DWM accepted the
    /// backdrop attribute, not that the dynamic material is visibly drawn right now: Windows draws
    /// Mica, Mica Alt, and Desktop Acrylic only while the window is the foreground window and
    /// shows its own flat neutral fallback when unfocused (so a non-activating window or overlay
    /// that never takes focus never shows the live material). Use
    /// [`App::on_focus`](crate::App::on_focus) or
    /// [`El::on_window_focus`](crate::El::on_window_focus) alongside `is_active()` if the view
    /// needs to distinguish focused from unfocused state. On Linux Wayland, `Blur` is
    /// surface-attached protocol state and compositors such as KWin blur unfocused surfaces too,
    /// subject to compositor policy.
    ///
    /// ```
    /// use rune::WindowBackdrop;
    ///
    /// assert!(!WindowBackdrop::None.is_active());
    /// assert!(WindowBackdrop::Mica.is_active());
    /// assert!(WindowBackdrop::MicaAlt.is_active());
    /// assert!(WindowBackdrop::Acrylic.is_active());
    /// assert!(WindowBackdrop::Blur.is_active());
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
            Self::Blur => 4,
        }
    }

    /// Single-bit mask identifying this variant in `HostState::window_backdrop_supported`.
    /// `None` returns `0` because `Cx::window_backdrop_supported(WindowBackdrop::None)` is
    /// unconditionally `true`.
    #[inline]
    pub(crate) const fn support_bit(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Mica => 1 << 0,
            Self::MicaAlt => 1 << 1,
            Self::Acrylic => 1 << 2,
            Self::Blur => 1 << 3,
        }
    }

    /// Returns whether `backdrop` is reported supported under `mask`.
    #[inline]
    pub(crate) const fn is_supported_in(self, mask: u8) -> bool {
        let bit = self.support_bit();
        bit == 0 || (mask & bit) != 0
    }
}

#[cfg(test)]
#[path = "backdrop_tests.rs"]
mod tests;
