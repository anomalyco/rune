//! Pure eligibility policy, protocol state machine, and seam for Wayland behind-window blur.
//!
//! Compiles on every platform under `cfg(test)` so the eligibility matrix, protocol request
//! ordering, resize/scale deduplication, per-element `El::system_material` shaped blur,
//! runtime capability revocation, and teardown safety are proven off-screen without a live
//! Wayland compositor.

use super::region::{BlurRegion, RegionRect, RegionSpec, build_blur_region_rects};
use crate::WindowBackdrop;
use crate::system_material::Mode as MaterialMode;

/// `ext_background_effect_manager_v1::capability::blur` bit (value 1 in
/// `staging/ext-background-effect/ext-background-effect-v1.xml`).
pub(crate) const EXT_CAPABILITY_BLUR: u32 = 1;

/// Which Wayland blur protocol is bound on the active display connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CompositorProtocol {
    /// Neither `ext_background_effect_manager_v1` nor `org_kde_kwin_blur_manager` is available.
    #[default]
    None,
    /// `ext_background_effect_manager_v1` (version 1) is bound. `blur_capable` reflects the
    /// latest `capabilities` bitfield event (`(flags & 1) != 0`).
    ExtBackgroundEffect { blur_capable: bool },
    /// Legacy `org_kde_kwin_blur_manager` (version 1) fallback, used only when
    /// `ext_background_effect_manager_v1` is absent from the registry.
    KdeKwinBlur,
}

impl CompositorProtocol {
    /// Whether the compositor can currently apply behind-window blur through this protocol.
    #[inline]
    pub(crate) const fn supports_blur(self) -> bool {
        match self {
            Self::ExtBackgroundEffect { blur_capable } => blur_capable,
            Self::KdeKwinBlur => true,
            Self::None => false,
        }
    }
}

/// Decodes the raw `ext_background_effect_manager_v1::capabilities` bitfield, checking bit 0
/// (`blur = 1`) and ignoring any unknown future capability bits.
#[inline]
pub(crate) const fn ext_capabilities_has_blur(raw_bits: u32) -> bool {
    (raw_bits & EXT_CAPABILITY_BLUR) != 0
}

/// Chooses the display's Wayland blur protocol from advertised registry globals.
///
/// Prefers `ext_background_effect_manager_v1` whenever it is present in the registry — even if
/// its `capabilities` bitfield currently omits `blur` (for example when the user disabled KWin's
/// Blur desktop effect) — and falls back to `org_kde_kwin_blur_manager` only when
/// `ext_background_effect_manager_v1` is absent.
#[inline]
pub(crate) const fn choose_protocol(
    has_ext: bool,
    ext_blur_capable: bool,
    has_kde: bool,
) -> CompositorProtocol {
    if has_ext {
        CompositorProtocol::ExtBackgroundEffect { blur_capable: ext_blur_capable }
    } else if has_kde {
        CompositorProtocol::KdeKwinBlur
    } else {
        CompositorProtocol::None
    }
}

/// All runtime facts deciding whether Wayland compositor blur is supported and eligible on a
/// window or overlay surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WaylandBackdropInputs {
    /// Requested backdrop from app/window/overlay builders or runtime setters.
    pub requested: WindowBackdrop,
    /// `true` on a Wayland session; `false` on X11 or XWayland.
    pub is_wayland: bool,
    /// Whether the window was created transparent (always `true` for overlays).
    pub transparent: bool,
    /// Active wgpu surface composite alpha mode, or `None` when no GPU surface is configured.
    pub alpha_mode: Option<wgpu::CompositeAlphaMode>,
    /// `true` while drawing through the CPU painter (`SurfaceMode::Cpu` / `CpuFirst::Only`).
    pub cpu_painter: bool,
    /// Bound Wayland blur protocol and its latest capability state.
    pub protocol: CompositorProtocol,
}

/// Whether the surface and Wayland compositor can currently activate `WindowBackdrop::Blur`,
/// independent of `inputs.requested`.
///
/// Powers `Cx::window_backdrop_supported(WindowBackdrop::Blur)` even while
/// `WindowBackdrop::None` is selected so settings UIs and `El::system_material` views can
/// query blur capability accurately.
#[inline]
pub(crate) const fn surface_supports_blur(inputs: WaylandBackdropInputs) -> bool {
    if !inputs.is_wayland || !inputs.transparent || inputs.cpu_painter {
        return false;
    }
    let has_translucent_alpha = matches!(
        inputs.alpha_mode,
        Some(
            wgpu::CompositeAlphaMode::PreMultiplied
                | wgpu::CompositeAlphaMode::PostMultiplied
                | wgpu::CompositeAlphaMode::Inherit
        )
    );
    has_translucent_alpha && inputs.protocol.supports_blur()
}

/// Returns the `WindowBackdrop` support bitmask for `inputs` (`Blur` bit when supported, else 0).
#[inline]
pub(crate) const fn supported_mask(inputs: WaylandBackdropInputs) -> u8 {
    if surface_supports_blur(inputs) {
        WindowBackdrop::Blur.support_bit()
    } else {
        0
    }
}

/// Pure eligibility rule for Linux Wayland whole-surface blur (`WindowBackdrop::Blur`).
///
/// Only `WindowBackdrop::Blur` activates on Wayland; Windows-specific materials (`Mica`,
/// `MicaAlt`, `Acrylic`) never activate on Linux.
#[inline]
pub(crate) const fn eligible_backdrop(inputs: WaylandBackdropInputs) -> Option<WindowBackdrop> {
    if matches!(inputs.requested, WindowBackdrop::Blur) && surface_supports_blur(inputs) {
        Some(WindowBackdrop::Blur)
    } else {
        None
    }
}

/// Whether per-element `El::system_material` shaped Wayland blur mode should be enabled on
/// `RuntimeCore`.
///
/// Active only when Wayland blur is supported on this surface, `WindowBackdrop::None` is active
/// (`WindowBackdrop::Blur` takes precedence as whole-surface blur), and `RUNE_SYSTEM_MATERIAL`
/// is not `off`.
#[inline]
pub(crate) const fn should_enable_wayland_blur_materials(
    inputs: WaylandBackdropInputs,
    active_window_backdrop: WindowBackdrop,
    env_mode: MaterialMode,
) -> bool {
    !matches!(env_mode, MaterialMode::Off)
        && matches!(inputs.requested, WindowBackdrop::None)
        && matches!(active_window_backdrop, WindowBackdrop::None)
        && surface_supports_blur(inputs)
}

/// Whether a Linux transparent window whose requested backdrop could not activate must clear its
/// surface to an opaque theme-matched fallback so it never opens a transparent hole.
#[inline]
pub(crate) const fn needs_opaque_fallback(
    is_overlay: bool,
    transparent: bool,
    requested: WindowBackdrop,
    active: WindowBackdrop,
) -> bool {
    !is_overlay && transparent && requested.is_active() && !active.is_active()
}

/// Narrow atomic Wayland blur protocol seam for off-screen unit testing of exact wire request
/// order and lifecycle teardown.
pub(crate) trait WaylandBlurSeam {
    /// Handle to a temporary `wl_region`.
    type Region;

    /// `wl_compositor.create_region`.
    fn create_region(&mut self) -> Option<Self::Region>;
    /// `wl_region.add(x, y, width, height)`.
    fn region_add(&mut self, region: &Self::Region, rect: RegionRect);
    /// `wl_region.destroy`.
    fn region_destroy(&mut self, region: Self::Region);

    /// `ext_background_effect_manager_v1.get_background_effect(surface)`.
    fn ext_get_background_effect(&mut self) -> bool;
    /// `ext_background_effect_surface_v1.set_blur_region(region)`.
    fn ext_set_blur_region(&mut self, region: Option<&Self::Region>);
    /// `ext_background_effect_surface_v1.destroy`.
    fn ext_destroy_effect(&mut self);

    /// `org_kde_kwin_blur_manager.create(surface)`.
    fn kde_create(&mut self) -> bool;
    /// `org_kde_kwin_blur.set_region(region)`.
    fn kde_set_region(&mut self, region: Option<&Self::Region>);
    /// `org_kde_kwin_blur.commit`.
    fn kde_commit(&mut self);
    /// `org_kde_kwin_blur_manager.unset(surface)`.
    fn kde_unset(&mut self);
    /// `org_kde_kwin_blur.release`.
    fn kde_release(&mut self);
}

/// Explicit per-surface Wayland blur state machine.
///
/// Guarantees:
/// - In `Inactive`, no per-surface blur object exists on `wl_surface` (so `WindowBackdrop::None`
///   with no active material blurs never overrides Hyprland global blur rules).
/// - In `ActiveExt`, `get_background_effect` is never called a second time (preventing the
///   fatal `background_effect_exists` protocol error across resize, springing animations, and
///   seamless `WindowBackdrop::Blur` <-> `El::system_material` transitions).
/// - In `Closed`, every subsequent sync is ignored (preventing the fatal `surface_destroyed`
///   protocol error).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum BlurState {
    #[default]
    Inactive,
    ActiveExt { applied: BlurRegion },
    ActiveKde { applied: BlurRegion },
    Closed,
}

/// Outcome of syncing [`SurfaceBlurTracker`] for one surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlurSync {
    /// What `Cx::window_backdrop()` should report (`Blur` only for whole-surface blur).
    pub active: WindowBackdrop,
    /// Support bitmask for `Cx::window_backdrop_supported(...)`.
    pub supported_mask: u8,
    /// Whether `RuntimeCore::set_wayland_blur_materials` should be enabled.
    pub material_blur_enabled: bool,
    /// Whether a non-overlay transparent window should clear to an opaque fallback colour.
    pub opaque_fallback: bool,
    /// `true` whenever wire requests were sent that require a `wl_surface.commit` (for example
    /// via wgpu present) to land atomically.
    pub needs_commit: bool,
}

/// Per-`wl_surface` state machine driving `ext_background_effect_v1` and
/// `org_kde_kwin_blur_manager` through [`WaylandBlurSeam`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SurfaceBlurTracker {
    state: BlurState,
}

impl SurfaceBlurTracker {
    #[cfg(test)]
    pub(crate) fn state(&self) -> &BlurState {
        &self.state
    }

    /// True when a per-surface blur object is currently live on the compositor.
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        matches!(self.state, BlurState::ActiveExt { .. } | BlurState::ActiveKde { .. })
    }

    /// Synchronizes whole-surface blur protocol state without per-element materials.
    #[cfg(test)]
    pub(crate) fn sync<S: WaylandBlurSeam>(
        &mut self,
        inputs: WaylandBackdropInputs,
        region_spec: RegionSpec,
        is_overlay: bool,
        force_reapply: bool,
        seam: &mut S,
    ) -> BlurSync {
        self.sync_with_materials(
            inputs,
            region_spec,
            BlurRegion::Empty,
            MaterialMode::Synced,
            is_overlay,
            force_reapply,
            seam,
        )
    }

    /// Synchronizes blur protocol state on this surface against whole-surface `inputs` and
    /// per-element `material_region` (`WindowBackdrop::Blur` takes precedence).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sync_with_materials<S: WaylandBlurSeam>(
        &mut self,
        inputs: WaylandBackdropInputs,
        whole_surface_spec: RegionSpec,
        material_region: BlurRegion,
        env_mode: MaterialMode,
        is_overlay: bool,
        force_reapply: bool,
        seam: &mut S,
    ) -> BlurSync {
        let supported = supported_mask(inputs);
        if self.state == BlurState::Closed {
            return BlurSync {
                active: WindowBackdrop::None,
                supported_mask: 0,
                material_blur_enabled: false,
                opaque_fallback: needs_opaque_fallback(
                    is_overlay,
                    inputs.transparent,
                    inputs.requested,
                    WindowBackdrop::None,
                ),
                needs_commit: false,
            };
        }

        let whole_target = eligible_backdrop(inputs);
        let mut needs_commit = false;

        let active = match whole_target {
            Some(WindowBackdrop::Blur) => {
                if whole_surface_spec.is_empty() {
                    // Never send an empty wl_region for a temporary 0x0 size; keep any active
                    // whole-surface blur untouched until a valid size arrives.
                    match &self.state {
                        BlurState::ActiveExt { applied: BlurRegion::WholeSurface(_) }
                        | BlurState::ActiveKde { applied: BlurRegion::WholeSurface(_) } => {
                            WindowBackdrop::Blur
                        }
                        _ => WindowBackdrop::None,
                    }
                } else {
                    let applied = self.apply_region(
                        inputs.protocol,
                        BlurRegion::WholeSurface(whole_surface_spec),
                        force_reapply,
                        seam,
                        &mut needs_commit,
                    );
                    if applied { WindowBackdrop::Blur } else { WindowBackdrop::None }
                }
            }
            _ => {
                let mat_enabled = should_enable_wayland_blur_materials(
                    inputs,
                    WindowBackdrop::None,
                    env_mode,
                );
                if mat_enabled && !material_region.is_empty() {
                    let _ = self.apply_region(
                        inputs.protocol,
                        material_region,
                        force_reapply,
                        seam,
                        &mut needs_commit,
                    );
                } else {
                    needs_commit = self.deactivate(seam);
                }
                WindowBackdrop::None
            }
        };

        let material_blur_enabled =
            should_enable_wayland_blur_materials(inputs, active, env_mode);
        let opaque_fallback =
            needs_opaque_fallback(is_overlay, inputs.transparent, inputs.requested, active);
        BlurSync {
            active,
            supported_mask: supported,
            material_blur_enabled,
            opaque_fallback,
            needs_commit,
        }
    }

    /// Destroys any active per-surface blur object and marks the surface closed so later calls
    /// never touch a destroyed `wl_surface`. Returns `true` if a wire destroy/unset request was
    /// emitted.
    pub(crate) fn close<S: WaylandBlurSeam>(&mut self, seam: &mut S) -> bool {
        if self.state == BlurState::Closed {
            return false;
        }
        let sent = self.deactivate(seam);
        self.state = BlurState::Closed;
        sent
    }

    fn apply_region<S: WaylandBlurSeam>(
        &mut self,
        protocol: CompositorProtocol,
        region: BlurRegion,
        force_reapply: bool,
        seam: &mut S,
        needs_commit: &mut bool,
    ) -> bool {
        match protocol {
            CompositorProtocol::ExtBackgroundEffect { blur_capable: true } => {
                self.sync_ext(region, force_reapply, seam, needs_commit)
            }
            CompositorProtocol::KdeKwinBlur => {
                self.sync_kde(region, force_reapply, seam, needs_commit)
            }
            CompositorProtocol::ExtBackgroundEffect { blur_capable: false }
            | CompositorProtocol::None => {
                *needs_commit |= self.deactivate(seam);
                false
            }
        }
    }

    fn deactivate<S: WaylandBlurSeam>(&mut self, seam: &mut S) -> bool {
        match self.state {
            BlurState::ActiveExt { .. } => {
                seam.ext_destroy_effect();
                self.state = BlurState::Inactive;
                true
            }
            BlurState::ActiveKde { .. } => {
                seam.kde_unset();
                seam.kde_release();
                self.state = BlurState::Inactive;
                true
            }
            BlurState::Inactive | BlurState::Closed => false,
        }
    }

    fn sync_ext<S: WaylandBlurSeam>(
        &mut self,
        desired: BlurRegion,
        force_reapply: bool,
        seam: &mut S,
        needs_commit: &mut bool,
    ) -> bool {
        match &self.state {
            BlurState::ActiveExt { applied } if !force_reapply && *applied == desired => {
                return true;
            }
            BlurState::ActiveKde { .. } => {
                *needs_commit |= self.deactivate(seam);
            }
            BlurState::Inactive | BlurState::ActiveExt { .. } | BlurState::Closed => {}
        }

        let rects = build_blur_region_rects(&desired);
        if rects.is_empty() {
            *needs_commit |= self.deactivate(seam);
            return false;
        }
        let Some(region) = seam.create_region() else {
            return matches!(self.state, BlurState::ActiveExt { .. });
        };
        for rect in rects {
            seam.region_add(&region, rect);
        }
        if matches!(self.state, BlurState::Inactive) && !seam.ext_get_background_effect() {
            seam.region_destroy(region);
            return false;
        }
        seam.ext_set_blur_region(Some(&region));
        seam.region_destroy(region);
        self.state = BlurState::ActiveExt { applied: desired };
        *needs_commit = true;
        true
    }

    fn sync_kde<S: WaylandBlurSeam>(
        &mut self,
        desired: BlurRegion,
        force_reapply: bool,
        seam: &mut S,
        needs_commit: &mut bool,
    ) -> bool {
        match &self.state {
            BlurState::ActiveKde { applied } => {
                if !force_reapply && *applied == desired {
                    return true;
                }
                // Under `org_kde_kwin_blur`, `set_region(None)` blurs the entire square surface
                // dynamically, so a square-to-square whole-surface resize needs no wire traffic.
                if !force_reapply && applied.is_kde_full_surface() && desired.is_kde_full_surface() {
                    self.state = BlurState::ActiveKde { applied: desired };
                    return true;
                }
            }
            BlurState::ActiveExt { .. } => {
                *needs_commit |= self.deactivate(seam);
            }
            BlurState::Inactive | BlurState::Closed => {}
        }

        if desired.is_kde_full_surface() {
            let creating = matches!(self.state, BlurState::Inactive);
            if creating && !seam.kde_create() {
                return false;
            }
            seam.kde_set_region(None);
            seam.kde_commit();
            self.state = BlurState::ActiveKde { applied: desired };
            *needs_commit = true;
            return true;
        }

        let rects = build_blur_region_rects(&desired);
        if rects.is_empty() {
            *needs_commit |= self.deactivate(seam);
            return false;
        }
        let creating = matches!(self.state, BlurState::Inactive);
        if creating && !seam.kde_create() {
            return false;
        }
        let Some(region) = seam.create_region() else {
            if creating {
                seam.kde_unset();
                seam.kde_release();
                self.state = BlurState::Inactive;
                *needs_commit = true;
                return false;
            }
            return true;
        };
        for rect in rects {
            seam.region_add(&region, rect);
        }
        seam.kde_set_region(Some(&region));
        seam.kde_commit();
        seam.region_destroy(region);
        self.state = BlurState::ActiveKde { applied: desired };
        *needs_commit = true;
        true
    }
}
