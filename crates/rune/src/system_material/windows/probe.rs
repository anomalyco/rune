//! Single host-availability probe and bounded Acrylic brush cache for Windows 11
//! `El::system_material`, shared by production (`real.rs`) and the live COM tests.
//!
//! Each OS step is isolated behind `HostProbeSeam` / `AcrylicFactorySeam` so every failure mode
//! (unsupported Windows build, disabled DWM composition in Session 0 / headless runners,
//! `CoreMessaging.dll` `CreateDispatcherQueueController`, `Compositor::new`,
//! `CreateHostBackdropBrush`, and partial Acrylic brush compilation failure) is unit-tested on
//! every platform with zero handle leaks.

use std::collections::HashMap;

use super::MIN_HOST_BACKDROP_BUILD;
use crate::SystemMaterial;

/// Host-level availability of the Windows 11 `HostBackdropBrush` compositor stack (independent
/// of any specific window or HWND).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HostAvailability {
    /// Host is Windows 11 Build 22000+ with DWM composition enabled and a working
    /// `CoreMessaging.dll` `DispatcherQueue`, `Windows.UI.Composition::Compositor`, and
    /// `HostBackdropBrush`.
    Available { build: u32 },
    /// Host cannot run Windows 11 `HostBackdropBrush` materials; production paints fallback fills
    /// and remembers this result so later frames perform zero probes or COM calls.
    Unavailable { build: Option<u32>, reason: HostUnavailableReason },
}

/// Specific step in the single host probe that reported the host cannot run Windows 11 materials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HostUnavailableReason {
    /// `RtlGetVersion` returned no build or a build below `MIN_HOST_BACKDROP_BUILD` (22000,
    /// e.g. Windows 10 where `DWMWA_USE_HOSTBACKDROPBRUSH` does not exist).
    UnsupportedBuild,
    /// `DwmIsCompositionEnabled` returned `FALSE` or an `HRESULT` error (e.g. Session 0 /
    /// non-interactive runner without a running Desktop Window Manager).
    DwmCompositionDisabled(String),
    /// `CoreMessaging.dll` / `CreateDispatcherQueueController` failed on the calling thread.
    DispatcherQueue(String),
    /// `Windows.UI.Composition::Compositor::new` failed (e.g. no desktop compositor session).
    Compositor(String),
    /// `Compositor::CreateHostBackdropBrush` failed.
    HostBackdropBrush(String),
}

impl HostAvailability {
    /// Human-readable one-line explanation naming the host's build number and which call said no.
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::Available { build } => format!("Windows build {build}: compositor available"),
            Self::Unavailable { build, reason } => {
                let build_str = match build {
                    Some(b) => format!("Windows build {b}"),
                    None => "Windows build unknown".into(),
                };
                match reason {
                    HostUnavailableReason::UnsupportedBuild => format!(
                        "{build_str}: requires build {MIN_HOST_BACKDROP_BUILD}+ for DWMWA_USE_HOSTBACKDROPBRUSH"
                    ),
                    HostUnavailableReason::DwmCompositionDisabled(err) => {
                        format!("{build_str}: DwmIsCompositionEnabled unavailable ({err})")
                    }
                    HostUnavailableReason::DispatcherQueue(err) => {
                        format!("{build_str}: CreateDispatcherQueueController failed ({err})")
                    }
                    HostUnavailableReason::Compositor(err) => {
                        format!("{build_str}: Compositor::new failed ({err})")
                    }
                    HostUnavailableReason::HostBackdropBrush(err) => {
                        format!("{build_str}: CreateHostBackdropBrush failed ({err})")
                    }
                }
            }
        }
    }
}

/// Narrow seam isolating every OS call of the single Windows 11 host-availability probe so
/// production and tests execute the exact same decision logic and verify zero leaks on failure.
pub(crate) trait HostProbeSeam {
    type Compositor;
    type BackdropBrush;

    fn windows_build(&mut self) -> Option<u32>;
    fn dwm_composition_enabled(&mut self) -> Result<bool, String>;
    fn ensure_dispatcher_queue(&mut self) -> Result<(), String>;
    fn create_compositor(&mut self) -> Result<Self::Compositor, String>;
    fn create_host_backdrop_brush(
        &mut self,
        compositor: &Self::Compositor,
    ) -> Result<Self::BackdropBrush, String>;
    fn close_backdrop_brush(&mut self, brush: Self::BackdropBrush);
    fn close_compositor(&mut self, compositor: Self::Compositor);
}

/// The single host-availability probe used by both production and the live COM tests.
///
/// Checks cheapest / least invasive preconditions first (zero `CoreMessaging.dll` loads or WinRT
/// activation on Windows 10 or Session 0), closes every COM object it creates on both success and
/// error paths, and never panics or hangs:
/// 1. `windows_build() >= 22000` (`MIN_HOST_BACKDROP_BUILD`);
/// 2. `dwm_composition_enabled()` (`DwmIsCompositionEnabled`);
/// 3. `ensure_dispatcher_queue()` (`CoreMessaging.dll` `CreateDispatcherQueueController`);
/// 4. `create_compositor()` (`Windows.UI.Composition::Compositor::new`);
/// 5. `create_host_backdrop_brush()` (`Compositor::CreateHostBackdropBrush`).
pub(crate) fn probe_host_with<S: HostProbeSeam>(seam: &mut S) -> HostAvailability {
    let build = seam.windows_build();
    let Some(build_num) = build.filter(|&b| b >= MIN_HOST_BACKDROP_BUILD) else {
        return HostAvailability::Unavailable {
            build,
            reason: HostUnavailableReason::UnsupportedBuild,
        };
    };
    match seam.dwm_composition_enabled() {
        Ok(true) => {}
        Ok(false) => {
            return HostAvailability::Unavailable {
                build: Some(build_num),
                reason: HostUnavailableReason::DwmCompositionDisabled(
                    "DwmIsCompositionEnabled returned FALSE".into(),
                ),
            };
        }
        Err(err) => {
            return HostAvailability::Unavailable {
                build: Some(build_num),
                reason: HostUnavailableReason::DwmCompositionDisabled(err),
            };
        }
    }
    if let Err(err) = seam.ensure_dispatcher_queue() {
        return HostAvailability::Unavailable {
            build: Some(build_num),
            reason: HostUnavailableReason::DispatcherQueue(err),
        };
    }
    let compositor = match seam.create_compositor() {
        Ok(c) => c,
        Err(err) => {
            return HostAvailability::Unavailable {
                build: Some(build_num),
                reason: HostUnavailableReason::Compositor(err),
            };
        }
    };
    let backdrop = match seam.create_host_backdrop_brush(&compositor) {
        Ok(b) => b,
        Err(err) => {
            seam.close_compositor(compositor);
            return HostAvailability::Unavailable {
                build: Some(build_num),
                reason: HostUnavailableReason::HostBackdropBrush(err),
            };
        }
    };
    seam.close_backdrop_brush(backdrop);
    seam.close_compositor(compositor);
    HostAvailability::Available { build: build_num }
}

/// Bounded per-target cache of at most 6 compiled Acrylic brushes keyed by `(SystemMaterial, dark)`.
///
/// If compiling any brush fails (including halfway through the six variants), every previously
/// cached brush is closed immediately and failure is latched so subsequent frames take the WCAG
/// flat-tint fallback with zero retries.
pub(crate) struct AcrylicBrushCache<B> {
    brushes: HashMap<(SystemMaterial, bool), B>,
    failed: bool,
}

impl<B> AcrylicBrushCache<B> {
    pub(crate) fn new() -> Self {
        Self { brushes: HashMap::new(), failed: false }
    }

    pub(crate) fn get_or_create(
        &mut self,
        material: SystemMaterial,
        dark: bool,
        create: impl FnOnce(SystemMaterial, bool) -> Result<B, String>,
        mut close: impl FnMut(B),
    ) -> Result<Option<&B>, String> {
        if self.failed {
            return Ok(None);
        }
        let key = (material, dark);
        if !self.brushes.contains_key(&key) {
            match create(material, dark) {
                Ok(brush) => {
                    self.brushes.insert(key, brush);
                }
                Err(err) => {
                    self.failed = true;
                    for (_, cached) in self.brushes.drain() {
                        close(cached);
                    }
                    return Err(err);
                }
            }
        }
        Ok(self.brushes.get(&key))
    }

    pub(crate) fn clear_with(&mut self, mut close: impl FnMut(B)) {
        for (_, brush) in self.brushes.drain() {
            close(brush);
        }
    }
}

/// All six `(SystemMaterial, dark)` pairs compiled by the Windows 11 Acrylic backend.
#[cfg(test)]
pub(crate) const ALL_ACRYLIC_VARIANTS: [(SystemMaterial, bool); 6] = [
    (SystemMaterial::Glass, false),
    (SystemMaterial::Glass, true),
    (SystemMaterial::Popover, false),
    (SystemMaterial::Popover, true),
    (SystemMaterial::Sidebar, false),
    (SystemMaterial::Sidebar, true),
];

/// Narrow seam for compiling all six Acrylic brushes without a window, ensuring every created
/// brush, backdrop brush, and compositor is closed in reverse order even on partial failure.
#[cfg(test)]
pub(crate) trait AcrylicFactorySeam: HostProbeSeam {
    type EffectBrush;

    fn create_acrylic_brush(
        &mut self,
        compositor: &Self::Compositor,
        backdrop: &Self::BackdropBrush,
        material: SystemMaterial,
        dark: bool,
    ) -> Result<Self::EffectBrush, String>;

    fn close_effect_brush(&mut self, brush: Self::EffectBrush);
}

/// Compiles all six `(SystemMaterial, dark)` Acrylic brushes without opening a window and closes
/// every created COM object in reverse order even if a brush fails halfway through.
#[cfg(test)]
pub(crate) fn compile_all_acrylic_brushes_with<S: AcrylicFactorySeam>(
    seam: &mut S,
) -> Result<usize, String> {
    seam.ensure_dispatcher_queue()?;
    let compositor = seam.create_compositor()?;
    let backdrop = match seam.create_host_backdrop_brush(&compositor) {
        Ok(b) => b,
        Err(err) => {
            seam.close_compositor(compositor);
            return Err(err);
        }
    };
    let mut brushes = Vec::with_capacity(ALL_ACRYLIC_VARIANTS.len());
    let mut err_out = None;
    for (material, dark) in ALL_ACRYLIC_VARIANTS {
        match seam.create_acrylic_brush(&compositor, &backdrop, material, dark) {
            Ok(brush) => brushes.push(brush),
            Err(err) => {
                err_out = Some(err);
                break;
            }
        }
    }
    let compiled = brushes.len();
    for brush in brushes.into_iter().rev() {
        seam.close_effect_brush(brush);
    }
    seam.close_backdrop_brush(backdrop);
    seam.close_compositor(compositor);
    match err_out {
        Some(err) => Err(err),
        None => Ok(compiled),
    }
}
