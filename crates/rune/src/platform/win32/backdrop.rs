//! Windows 11 whole-window system backdrops (`DWMWA_SYSTEMBACKDROP_TYPE`).
//!
//! Uses only documented Win32 DWM attributes:
//! - `DWMWA_SYSTEMBACKDROP_TYPE` (38, `DWM_SYSTEMBACKDROP_TYPE`, Windows 11 Build 22621+)
//! - `DWMWA_USE_IMMERSIVE_DARK_MODE` (20, `BOOL`, Windows 11 Build 22000+, synced while a
//!   backdrop is active so DWM tints Mica / Mica Alt / Acrylic for light or dark mode)
//!
//! Never calls undocumented `SetWindowCompositionAttribute`, accent policies, or pre-22H2
//! `DWMWA_MICA_EFFECT` (1029). Does not call `DwmExtendFrameIntoClientArea(-1)` in this first
//! seam (matching winit, window-vibrancy, Windows Terminal, and GPUI so `CustomFrame` avoids
//! duplicate native caption buttons); the seam stays isolated so frame extension can be added in a
//! few lines if live Windows 11 testing shows standard or borderless windows require it.

use crate::WindowBackdrop;

/// Minimum Windows 11 build that documents `DWMWA_SYSTEMBACKDROP_TYPE` (Windows 11 22H2).
pub(crate) const MIN_BACKDROP_BUILD: u32 = 22621;

/// Documented `DWM_SYSTEMBACKDROP_TYPE` values from `dwmapi.h` (Windows 11 Build 22621+).
pub(crate) const DWMSBT_NONE: i32 = 1;
pub(crate) const DWMSBT_MAINWINDOW: i32 = 2;
pub(crate) const DWMSBT_TRANSIENTWINDOW: i32 = 3;
pub(crate) const DWMSBT_TABBEDWINDOW: i32 = 4;

/// All runtime facts that decide whether `DWMWA_SYSTEMBACKDROP_TYPE` may activate on a Windows
/// window or overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackdropInputs {
    /// Requested backdrop from app/window/overlay builders or runtime setters.
    pub requested: WindowBackdrop,
    /// Windows build number from `RtlGetVersion`, or `None` if unavailable.
    pub build: Option<u32>,
    /// Whether the window was created transparent (always `true` for overlays).
    pub transparent: bool,
    /// Active DX12 swap-chain presentation path (`Gpu::dx12_swapchain_kind()`).
    pub swapchain: wgpu::Dx12SwapchainKind,
    /// Active surface composite alpha mode, or `None` when no GPU surface is configured.
    pub alpha_mode: Option<wgpu::CompositeAlphaMode>,
    /// `true` while drawing through the CPU painter (`SurfaceMode::Cpu` / `HandingOver` /
    /// `GpuJob::SoftwareDeferred` / `CpuFirst::Only`).
    pub cpu_painter: bool,
    /// Active GPU adapter type, or `None` when no GPU device is attached.
    pub device_type: Option<wgpu::DeviceType>,
}

/// Pure eligibility rule for Windows 11 `DWMWA_SYSTEMBACKDROP_TYPE`.
///
/// Returns `Some(inputs.requested)` only when every precondition holds:
/// 1. `requested` is not `WindowBackdrop::None`;
/// 2. Windows build >= 22621;
/// 3. window is transparent;
/// 4. DX12 DirectComposition swap chain (`DxgiFromVisual`, never `DxgiFromHwnd`);
/// 5. surface composite alpha is `PreMultiplied`;
/// 6. not drawing on the CPU painter;
/// 7. a hardware/non-CPU GPU adapter is attached (`device_type != DeviceType::Cpu`, excluding WARP).
/// Whether this Windows host and surface satisfy the preconditions for DWM system backdrops
/// (`Mica`, `MicaAlt`, `Acrylic`), independent of `inputs.requested`.
pub(crate) const fn surface_supports_dwm(inputs: BackdropInputs) -> bool {
    let Some(build) = inputs.build else {
        return false;
    };
    if build < MIN_BACKDROP_BUILD {
        return false;
    }
    if !inputs.transparent {
        return false;
    }
    if !matches!(inputs.swapchain, wgpu::Dx12SwapchainKind::DxgiFromVisual) {
        return false;
    }
    if !matches!(inputs.alpha_mode, Some(wgpu::CompositeAlphaMode::PreMultiplied)) {
        return false;
    }
    if inputs.cpu_painter {
        return false;
    }
    !matches!(inputs.device_type, Some(wgpu::DeviceType::Cpu) | None)
}

/// Bitmask of `WindowBackdrop` variants supported by this Windows surface without issuing DWM
/// calls (`Mica | MicaAlt | Acrylic` when eligible, `0` otherwise; `WindowBackdrop::Blur` is
/// Wayland-only and never set on Windows).
pub(crate) const fn windows_supported_mask(inputs: BackdropInputs) -> u8 {
    if surface_supports_dwm(inputs) {
        WindowBackdrop::Mica.support_bit()
            | WindowBackdrop::MicaAlt.support_bit()
            | WindowBackdrop::Acrylic.support_bit()
    } else {
        0
    }
}

pub(crate) const fn eligible_backdrop(inputs: BackdropInputs) -> Option<WindowBackdrop> {
    if !matches!(
        inputs.requested,
        WindowBackdrop::Mica | WindowBackdrop::MicaAlt | WindowBackdrop::Acrylic
    ) {
        return None;
    }
    if surface_supports_dwm(inputs) {
        Some(inputs.requested)
    } else {
        None
    }
}

/// Maps a [`WindowBackdrop`] to its documented `DWM_SYSTEMBACKDROP_TYPE` (`i32`) constant.
pub(crate) const fn dwm_backdrop_value(backdrop: WindowBackdrop) -> i32 {
    match backdrop {
        WindowBackdrop::None | WindowBackdrop::Blur => DWMSBT_NONE,
        WindowBackdrop::Mica => DWMSBT_MAINWINDOW,
        WindowBackdrop::MicaAlt => DWMSBT_TABBEDWINDOW,
        WindowBackdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
    }
}

/// Whether a Windows window whose requested backdrop could not activate must clear its surface to
/// an opaque theme-matched fallback so it never opens a transparent hole.
///
/// Only applies to non-overlay transparent windows that explicitly requested a non-`None`
/// backdrop which did not activate. Overlays (floating transparent HUDs/pills) and default
/// `WindowBackdrop::None` windows keep their normal transparent clear.
pub(crate) const fn needs_opaque_fallback(
    is_overlay: bool,
    transparent: bool,
    requested: WindowBackdrop,
    active: WindowBackdrop,
) -> bool {
    !is_overlay && transparent && requested.is_active() && !active.is_active()
}

/// Narrow Win32 seam wrapping `DwmSetWindowAttribute` for off-screen unit testing.
pub(crate) trait BackdropSeam {
    /// Applies `DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &value, 4)`.
    /// Returns the Win32 `HRESULT` (`>= 0` indicates `S_OK` / success).
    fn set_system_backdrop(&mut self, hwnd: isize, value: i32) -> i32;

    /// Applies `DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark, 4)`.
    /// Returns the Win32 `HRESULT` (`>= 0` indicates `S_OK` / success).
    fn set_immersive_dark_mode(&mut self, hwnd: isize, dark: bool) -> i32;
}

/// Real Win32 seam backed by `DwmSetWindowAttribute`.
#[cfg(target_os = "windows")]
#[derive(Default)]
pub(crate) struct RealBackdropSeam;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
impl BackdropSeam for RealBackdropSeam {
    fn set_system_backdrop(&mut self, hwnd: isize, value: i32) -> i32 {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::Graphics::Dwm::{DWMWA_SYSTEMBACKDROP_TYPE, DwmSetWindowAttribute};
        // SAFETY: `hwnd` is a live top-level window handle owned by this thread; `value` is a
        // 4-byte `DWM_SYSTEMBACKDROP_TYPE` (`i32`) matching `DWMWA_SYSTEMBACKDROP_TYPE`.
        unsafe {
            DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_SYSTEMBACKDROP_TYPE as u32,
                (&raw const value).cast(),
                size_of::<i32>() as u32,
            )
        }
    }

    fn set_immersive_dark_mode(&mut self, hwnd: isize, dark: bool) -> i32 {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::core::BOOL;
        use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
        let enabled: BOOL = i32::from(dark);
        // SAFETY: `hwnd` is a live top-level window handle owned by this thread; `enabled` is a
        // 4-byte Win32 `BOOL` matching `DWMWA_USE_IMMERSIVE_DARK_MODE`.
        unsafe {
            DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&raw const enabled).cast(),
                size_of::<BOOL>() as u32,
            )
        }
    }
}

/// DWM backdrop state cached for one live native `HWND`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AppliedBackdrop {
    /// No DWM backdrop attribute has ever been written to this `HWND`.
    #[default]
    Untouched,
    /// A previously active backdrop was cleared via `DWMSBT_NONE`.
    Cleared,
    /// DWM accepted `backdrop` (`HRESULT >= 0`).
    Active(WindowBackdrop),
}

/// Result of syncing [`BackdropTracker`] for a window or overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackdropSync {
    /// What [`Cx::window_backdrop`](crate::Cx::window_backdrop) should report.
    pub active: WindowBackdrop,
    /// Whether a non-overlay transparent window should clear to an opaque theme-matched fallback.
    pub opaque_fallback: bool,
}

/// Per-HWND deduplicating tracker for `DWMWA_SYSTEMBACKDROP_TYPE` and
/// `DWMWA_USE_IMMERSIVE_DARK_MODE`.
///
/// Guarantees:
/// - Zero DWM calls while `WindowBackdrop::None` is unchanged from default.
/// - Zero redundant DWM calls on steady-state frames.
/// - Re-application on demand (`force_reapply: true`) across hide/show, maximize/restore,
///   recreation, reattach, and theme updates.
/// - Clearing via `DWMSBT_NONE` when an active backdrop is turned off or becomes ineligible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BackdropTracker {
    applied: AppliedBackdrop,
    applied_dark: Option<bool>,
}

impl BackdropTracker {
    /// Synchronizes DWM backdrop state for `hwnd` against `inputs`.
    ///
    /// Pass `dark: Some(is_dark)` to sync `DWMWA_USE_IMMERSIVE_DARK_MODE` whenever a backdrop is
    /// active (ensuring Mica / Mica Alt / Acrylic tint follows light/dark mode on both windows and
    /// overlays without touching DWM while no backdrop is active).
    pub(crate) fn sync(
        &mut self,
        hwnd: isize,
        inputs: BackdropInputs,
        is_overlay: bool,
        force_reapply: bool,
        dark: Option<bool>,
        seam: &mut impl BackdropSeam,
    ) -> BackdropSync {
        let target = eligible_backdrop(inputs);
        let active = match target {
            None => {
                if matches!(self.applied, AppliedBackdrop::Active(_)) {
                    let _ = seam.set_system_backdrop(hwnd, DWMSBT_NONE);
                    self.applied = AppliedBackdrop::Cleared;
                    self.applied_dark = None;
                }
                WindowBackdrop::None
            }
            Some(backdrop) => {
                if let Some(is_dark) = dark
                    && (force_reapply || self.applied_dark != Some(is_dark))
                {
                    let _ = seam.set_immersive_dark_mode(hwnd, is_dark);
                    self.applied_dark = Some(is_dark);
                }
                let needs_call = force_reapply || self.applied != AppliedBackdrop::Active(backdrop);
                if !needs_call {
                    backdrop
                } else {
                    let hr = seam.set_system_backdrop(hwnd, dwm_backdrop_value(backdrop));
                    if hr >= 0 {
                        self.applied = AppliedBackdrop::Active(backdrop);
                        backdrop
                    } else {
                        if matches!(self.applied, AppliedBackdrop::Active(_)) {
                            let _ = seam.set_system_backdrop(hwnd, DWMSBT_NONE);
                            self.applied = AppliedBackdrop::Cleared;
                        }
                        self.applied_dark = None;
                        WindowBackdrop::None
                    }
                }
            }
        };
        let opaque_fallback =
            needs_opaque_fallback(is_overlay, inputs.transparent, inputs.requested, active);
        BackdropSync { active, opaque_fallback }
    }
}

#[cfg(test)]
#[path = "backdrop_tests.rs"]
mod tests;
