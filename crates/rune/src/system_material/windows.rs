//! Windows 11 per-element behind-window blur for `El::system_material`, behind the off-by-default
//! Cargo feature `windows-system-material`.
//!
//! Uses only Microsoft-documented Win32 and WinRT APIs (never `SetWindowCompositionAttribute`,
//! accent policies, Win2D, or screen capture):
//! - `DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &TRUE, 4)` (Windows 11 Build 22000+)
//! - `IDCompositionDevice::CreateTargetForHwnd` (topmost slot for Rune's premultiplied UI swap chain)
//! - `CoreMessaging.dll` `CreateDispatcherQueueController(DQTYPE_THREAD_CURRENT, DQTAT_COM_NONE)`
//!   loaded lazily from System32 on the first eligible material frame
//! - `Windows::UI::Composition::Compositor::CreateHostBackdropBrush` hosted via
//!   `ICompositorDesktopInterop::CreateDesktopWindowTarget(hwnd, false)` underneath the UI target
//! - Per-element rounded clipping via `Compositor::CreateRectangleClipWithSidesAndRadius` (default)
//!   or `CompositionGeometricClip` + `CompositionRoundedRectangleGeometry` (`RUNE_WIN_MATERIAL_CLIP=geo`)

use super::{MAX_NATIVE_MATERIALS_CAP, MaterialDraw, Mode, SystemMaterial, assign};
use rune_motion::NodeId;

/// Minimum Windows 11 build that documents `DWMWA_USE_HOSTBACKDROPBRUSH` (`dwmapi.h`).
pub(crate) const MIN_HOST_BACKDROP_BUILD: u32 = 22000;

/// All runtime facts that decide whether Windows 11 per-element behind-window blur may activate
/// on a window or overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MaterialInputs {
    /// Whether the window was created transparent (always `true` for overlays).
    pub transparent: bool,
    /// Windows build number from `RtlGetVersion`, or `None` if unavailable.
    pub build: Option<u32>,
    /// Active DX12 swap-chain presentation path (`Gpu::dx12_swapchain_kind()`).
    pub swapchain: wgpu::Dx12SwapchainKind,
    /// Active surface composite alpha mode, or `None` when no GPU surface is configured.
    pub alpha_mode: Option<wgpu::CompositeAlphaMode>,
    /// `true` while drawing through the CPU painter (`SurfaceMode::Cpu` / `HandingOver`).
    pub cpu_painter: bool,
    /// Active renderer kind (`RendererKind::Cpu` overlays have no DXGI swap chain and never
    /// host Windows 11 backdrop visuals; views query `Cx::renderer_kind()`).
    pub renderer_kind: crate::RendererKind,
    /// Active GPU adapter type, or `None` when no GPU device is attached.
    pub device_type: Option<wgpu::DeviceType>,
    /// Active whole-window `WindowBackdrop` (`DWMWA_SYSTEMBACKDROP_TYPE` is mutually exclusive
    /// with `DWMWA_USE_HOSTBACKDROPBRUSH`).
    pub window_backdrop: crate::WindowBackdrop,
    /// `RUNE_SYSTEM_MATERIAL` mode (`Mode::Off` disables native materials).
    pub mode: Mode,
    /// Whether the window or overlay holds a live Rune-owned DirectComposition `Handover`.
    pub has_handover: bool,
}

/// Pure eligibility rule for Windows 11 `El::system_material`.
///
/// Returns `true` only when every precondition holds:
/// 1. window is transparent;
/// 2. Windows build >= 22000 (`DWMWA_USE_HOSTBACKDROPBRUSH`);
/// 3. DX12 DirectComposition swap chain (`DxgiFromVisual`, never `DxgiFromHwnd`);
/// 4. surface composite alpha is `PreMultiplied`;
/// 5. not drawing on the CPU painter;
/// 6. a hardware/non-CPU GPU adapter is attached (`device_type != DeviceType::Cpu`, excluding WARP);
/// 7. no whole-window `WindowBackdrop` is active (mutually exclusive with `DWMWA_SYSTEMBACKDROP_TYPE`);
/// 8. `RUNE_SYSTEM_MATERIAL` is not `off`;
/// 9. the window or overlay holds a live DirectComposition `Handover`.
pub(crate) const fn eligible(inputs: MaterialInputs) -> bool {
    if !inputs.transparent {
        return false;
    }
    let Some(build) = inputs.build else {
        return false;
    };
    if build < MIN_HOST_BACKDROP_BUILD {
        return false;
    }
    if !matches!(inputs.swapchain, wgpu::Dx12SwapchainKind::DxgiFromVisual) {
        return false;
    }
    if !matches!(inputs.alpha_mode, Some(wgpu::CompositeAlphaMode::PreMultiplied)) {
        return false;
    }
    if inputs.cpu_painter || inputs.renderer_kind.is_cpu() {
        return false;
    }
    if matches!(inputs.device_type, Some(wgpu::DeviceType::Cpu) | None) {
        return false;
    }
    if inputs.window_backdrop.is_active() {
        return false;
    }
    if matches!(inputs.mode, Mode::Off) {
        return false;
    }
    inputs.has_handover
}

/// Which rounded clip API backs each material visual (`RUNE_WIN_MATERIAL_CLIP`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ClipKind {
    /// `Compositor::CreateRectangleClipWithSidesAndRadius` (Windows 10 Build 20348+).
    #[default]
    Rectangle,
    /// `CompositionGeometricClip` + `CompositionRoundedRectangleGeometry` (Windows 10 Build 17763+).
    Geometric,
}

impl ClipKind {
    pub(crate) fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("geo" | "geometric") => Self::Geometric,
            _ => Self::Rectangle,
        }
    }

    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    pub(crate) fn from_env() -> Self {
        Self::parse(rune_core::env::var("RUNE_WIN_MATERIAL_CLIP").as_deref())
    }
}

/// Optional subtle tint visual placed over `CreateHostBackdropBrush` (`RUNE_WIN_MATERIAL_TINT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TintMode {
    /// Raw `CreateHostBackdropBrush()` with no extra tint layer.
    #[default]
    None,
    /// Subtle light/dark neutral tint child visual for readability comparison.
    Subtle,
}

impl TintMode {
    pub(crate) fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("subtle") => Self::Subtle,
            _ => Self::None,
        }
    }

    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    pub(crate) fn from_env() -> Self {
        Self::parse(rune_core::env::var("RUNE_WIN_MATERIAL_TINT").as_deref())
    }
}

/// ARGB tint color for `TintMode::Subtle`, or `None` for `TintMode::None`.
pub(crate) const fn tint_color(mode: TintMode, dark: bool) -> Option<[u8; 4]> {
    match (mode, dark) {
        (TintMode::None, _) => None,
        (TintMode::Subtle, true) => Some([0x38, 0x20, 0x20, 0x22]),
        (TintMode::Subtle, false) => Some([0x48, 0xFC, 0xFC, 0xFC]),
    }
}

/// Parses a boolean diagnostic env var (`"1"`, `"true"`, `"yes"`).
pub(crate) fn parse_bool_env(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "TRUE" | "yes" | "YES"))
}

/// Diagnostic switch `RUNE_WIN_OVERLAY_NO_LAYERED=1`: omits `WS_EX_LAYERED` /
/// `SetLayeredWindowAttributes` on Windows overlays to isolate whether layered attributes
/// interfere with `DWMWA_USE_HOSTBACKDROPBRUSH`.
#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
pub(crate) fn overlay_no_layered() -> bool {
    parse_bool_env(rune_core::env::var("RUNE_WIN_OVERLAY_NO_LAYERED").as_deref())
}

/// Per-visual geometry and appearance applied to one pooled `SpriteVisual` in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VisualSpec {
    /// Physical-pixel `[x, y]` offset from the window's top-left corner.
    pub offset: [f32; 2],
    /// Physical-pixel `[width, height]`, clamped non-negative.
    pub size: [f32; 2],
    /// Physical-pixel corner radius, clamped to `0.0..=(min(w, h) * 0.5)`.
    pub radius: f32,
    /// Clamped opacity in `0.0..=1.0`.
    pub opacity: f32,
    /// Active clip implementation (`Rectangle` or `Geometric`).
    pub clip: ClipKind,
    /// Optional `[a, r, g, b]` tint visual color.
    pub tint: Option<[u8; 4]>,
}

impl VisualSpec {
    /// Converts one `MaterialDraw` (already in physical pixels) into a `VisualSpec`.
    pub(crate) fn from_draw(draw: MaterialDraw, clip: ClipKind, tint: Option<[u8; 4]>) -> Self {
        let w = draw.rect[2].max(0.0);
        let h = draw.rect[3].max(0.0);
        let max_r = w.min(h) * 0.5;
        let radius = if draw.radius.is_finite() { draw.radius.clamp(0.0, max_r) } else { 0.0 };
        let opacity = if draw.opacity.is_finite() { draw.opacity.clamp(0.0, 1.0) } else { 0.0 };
        Self {
            offset: [draw.rect[0], draw.rect[1]],
            size: [w, h],
            radius,
            opacity,
            clip,
            tint,
        }
    }
}

/// Lifecycle state of one window or overlay's Windows system-material host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    /// Inactive: zero dispatcher queue, compositor, target, visuals, or DWM calls;
    /// `Handover` stays at `topmost = false`.
    #[default]
    Idle,
    /// Active on `hwnd`: `Handover` is at `topmost = true`, `DesktopWindowTarget(false)` sits
    /// underneath, and `DWMWA_USE_HOSTBACKDROPBRUSH` is enabled.
    Active { hwnd: isize },
    /// A DWM or COM call failed on `hwnd`: rolled back to idle resources and latched failed for
    /// this HWND so later frames draw fallback fills with zero seam calls.
    Failed { hwnd: isize },
}

/// Narrow seam isolating all Win32 DWM, DirectComposition target-slot switching, and
/// `Windows.UI.Composition` operations for off-screen unit testing on every platform.
pub(crate) trait WindowsMaterialSeam {
    /// Moves Rune's DirectComposition swap-chain target (`Handover`) to `topmost` (`true` = above
    /// child/non-topmost targets, `false` = default slot).
    fn set_handover_topmost(&mut self, hwnd: isize, topmost: bool) -> Result<(), String>;

    /// Calls `DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &enabled, 4)`.
    fn set_host_backdrop_brush(&mut self, hwnd: isize, enabled: bool) -> Result<(), String>;

    /// Lazily initializes the UI thread's `DispatcherQueueController` (`CoreMessaging.dll`) and
    /// `Compositor`, creates the shared `HostBackdropBrush` and root `ContainerVisual`, and binds
    /// `ICompositorDesktopInterop::CreateDesktopWindowTarget(hwnd, false)`.
    fn create_target(&mut self, hwnd: isize) -> Result<(), String>;

    /// Closes and releases `DesktopWindowTarget`, all pooled visuals, and `Compositor`, freeing
    /// the `isTopmost = false` target slot before `Handover` returns to `topmost = false`.
    fn destroy_target(&mut self);

    /// Creates a new pooled `SpriteVisual` with the shared `HostBackdropBrush` in the root visual.
    fn create_visual(&mut self, slot_id: usize) -> Result<(), String>;

    /// Updates a pooled visual's physical-pixel offset, size, opacity, rounded clip, optional tint,
    /// and sets it visible.
    fn update_visual(&mut self, slot_id: usize, spec: VisualSpec) -> Result<(), String>;

    /// Hides a pooled visual without destroying it yet.
    fn hide_visual(&mut self, slot_id: usize) -> Result<(), String>;

    /// Removes and closes a spare pooled visual from the root visual.
    fn remove_visual(&mut self, slot_id: usize);

    /// Initiates a composition commit (`Compositor::RequestCommitAsync`) right before
    /// `gpu.queue.present(frame)`.
    fn commit(&mut self) -> Result<(), String>;
}

struct PooledSlot {
    id: usize,
    material: SystemMaterial,
    owner: Option<NodeId>,
    last_spec: Option<VisualSpec>,
}

/// Pure controller managing activation, per-frame visual syncing, slot pooling, teardown,
/// device-loss reset, and failure rollback behind [`WindowsMaterialSeam`].
pub(crate) struct Controller {
    mode: Mode,
    clip: ClipKind,
    tint: TintMode,
    pub(super) state: State,
    next_slot_id: usize,
    slots: Vec<PooledSlot>,
}

impl Controller {
    pub(crate) fn new(mode: Mode, clip: ClipKind, tint: TintMode) -> Self {
        Self {
            mode,
            clip,
            tint,
            state: State::Idle,
            next_slot_id: 0,
            slots: Vec::new(),
        }
    }

    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether elements should omit their fallback fill and cut shadow holes for native visuals.
    pub(crate) fn native(&self) -> bool {
        matches!(self.state, State::Active { .. })
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> State {
        self.state
    }

    /// Called after layout/paint (before render) to activate or tear down native backdrop visuals.
    ///
    /// Returns `true` when `native()` changed while material elements are mounted (`found == true`),
    /// signalling the host to rebuild the frame with the updated `native_materials` flag before
    /// rendering so neither activation nor deactivation flashes a wrong fill.
    pub(crate) fn update(
        &mut self,
        hwnd: Option<isize>,
        inputs: MaterialInputs,
        found: bool,
        seam: &mut impl WindowsMaterialSeam,
    ) -> bool {
        let can_run = found && eligible(inputs);
        match self.state {
            State::Idle => {
                if !can_run {
                    return false;
                }
                let Some(hwnd) = hwnd else {
                    return false;
                };
                if let Err(err) = self.activate(hwnd, seam) {
                    eprintln!("rune: Windows system material activation failed: {err}");
                    return false;
                }
                true
            }
            State::Active { hwnd: active_hwnd } => {
                if can_run && hwnd == Some(active_hwnd) {
                    return false;
                }
                self.teardown_active(active_hwnd, seam);
                if can_run && let Some(next_hwnd) = hwnd {
                    if let Err(err) = self.activate(next_hwnd, seam) {
                        eprintln!("rune: Windows system material activation failed: {err}");
                        return found;
                    }
                    return false;
                }
                // If materials are still mounted but became ineligible (e.g. WindowBackdrop turned
                // on), rebuild this frame with `native_materials = false` so fallback fills draw.
                found
            }
            State::Failed { hwnd: failed_hwnd } => {
                // Stay latched failed for the same HWND; if the window was recreated with a new
                // HWND, reset to Idle so the new window can evaluate eligibility cleanly.
                if hwnd.is_some_and(|h| h != failed_hwnd) {
                    self.state = State::Idle;
                    return self.update(hwnd, inputs, found, seam);
                }
                false
            }
        }
    }

    /// Syncs pooled visuals to `draws` (physical pixels) right before `gpu.queue.present(frame)`.
    ///
    /// Returns `true` if native materials remain active (or were already idle), or `false` if a
    /// COM/DWM error forced a rollback to fallback fills (caller should clear `native_materials`
    /// and schedule a redraw).
    pub(crate) fn sync_before_present(
        &mut self,
        draws: &[MaterialDraw],
        dark: bool,
        seam: &mut impl WindowsMaterialSeam,
    ) -> bool {
        let State::Active { hwnd } = self.state else {
            return true;
        };
        let draws = &draws[..draws.len().min(MAX_NATIVE_MATERIALS_CAP)];
        if draws.is_empty() {
            self.teardown_active(hwnd, seam);
            return true;
        }
        let wanted: Vec<_> = draws.iter().map(|d| (d.material, d.id)).collect();
        let views: Vec<_> = self.slots.iter().map(|s| (s.material, s.owner)).collect();
        let assigned = assign(&views, &wanted);
        let mut taken = vec![false; self.slots.len()];
        let mut changed = false;
        let tint = tint_color(self.tint, dark);

        for (draw, (at, &(material, id))) in draws.iter().zip(assigned.into_iter().zip(&wanted)) {
            let k = match at {
                Some(k) => k,
                None => {
                    let slot_id = self.next_slot_id;
                    self.next_slot_id += 1;
                    if let Err(err) = seam.create_visual(slot_id) {
                        self.rollback_and_latch(hwnd, &err, seam);
                        return false;
                    }
                    self.slots.push(PooledSlot {
                        id: slot_id,
                        material,
                        owner: None,
                        last_spec: None,
                    });
                    taken.push(false);
                    self.slots.len() - 1
                }
            };
            taken[k] = true;
            let slot = &mut self.slots[k];
            slot.owner = Some(id);
            let spec = VisualSpec::from_draw(*draw, self.clip, tint);
            if slot.last_spec != Some(spec) {
                if let Err(err) = seam.update_visual(slot.id, spec) {
                    self.rollback_and_latch(hwnd, &err, seam);
                    return false;
                }
                slot.last_spec = Some(spec);
                changed = true;
            }
        }

        for (slot, was_taken) in self.slots.iter_mut().zip(taken) {
            if !was_taken && slot.last_spec.is_some() {
                if let Err(err) = seam.hide_visual(slot.id) {
                    self.rollback_and_latch(hwnd, &err, seam);
                    return false;
                }
                slot.last_spec = None;
                slot.owner = None;
                changed = true;
            }
        }

        // Keep at most one spare visual per material kind, matching macOS pooling bounds.
        let mut k = self.slots.len();
        while k > 0 {
            k -= 1;
            let is_spare = |s: &PooledSlot| s.last_spec.is_none();
            if is_spare(&self.slots[k])
                && self.slots[..k].iter().any(|s| is_spare(s) && s.material == self.slots[k].material)
            {
                let removed = self.slots.remove(k);
                seam.remove_visual(removed.id);
                changed = true;
            }
        }

        if changed && let Err(err) = seam.commit() {
            self.rollback_and_latch(hwnd, &err, seam);
            return false;
        }
        true
    }

    /// Cleanly tears down active visuals and restores `Handover` to `topmost = false` when a
    /// window or overlay closes.
    pub(crate) fn teardown(&mut self, seam: &mut impl WindowsMaterialSeam) {
        if let State::Active { hwnd } = self.state {
            self.teardown_active(hwnd, seam);
        }
        self.state = State::Idle;
    }

    /// Tears down active visuals on GPU device loss and clears any failure latch so a recovered
    /// hardware GPU can re-evaluate eligibility cleanly (while a WARP fallback stays inactive).
    pub(crate) fn on_device_loss(&mut self, seam: &mut impl WindowsMaterialSeam) {
        self.teardown(seam);
    }

    fn activate(&mut self, hwnd: isize, seam: &mut impl WindowsMaterialSeam) -> Result<(), String> {
        // 1. Move Rune's DirectComposition swap-chain target to topmost = true first, freeing the
        //    non-topmost slot on this HWND.
        if let Err(err) = seam.set_handover_topmost(hwnd, true) {
            self.state = State::Failed { hwnd };
            return Err(err);
        }
        // 2. Bind Windows.UI.Composition DesktopWindowTarget(isTopmost = false) underneath it.
        if let Err(err) = seam.create_target(hwnd) {
            seam.destroy_target();
            // Best-effort rollback on activation failure; primary error is returned below.
            seam.set_handover_topmost(hwnd, false).ok();
            self.state = State::Failed { hwnd };
            return Err(err);
        }
        // 3. Enable DWMWA_USE_HOSTBACKDROPBRUSH on this HWND.
        if let Err(err) = seam.set_host_backdrop_brush(hwnd, true) {
            seam.destroy_target();
            // Best-effort rollback on activation failure; primary error is returned below.
            seam.set_host_backdrop_brush(hwnd, false).ok();
            seam.set_handover_topmost(hwnd, false).ok();
            self.state = State::Failed { hwnd };
            return Err(err);
        }
        self.state = State::Active { hwnd };
        Ok(())
    }

    fn teardown_active(&mut self, hwnd: isize, seam: &mut impl WindowsMaterialSeam) {
        self.slots.clear();
        // Free the non-topmost DesktopWindowTarget slot first, then clear DWM attribute and move
        // Handover back into the default non-topmost slot (best-effort during close/device-loss).
        seam.destroy_target();
        seam.set_host_backdrop_brush(hwnd, false).ok();
        seam.set_handover_topmost(hwnd, false).ok();
        self.state = State::Idle;
    }

    fn rollback_and_latch(&mut self, hwnd: isize, err: &str, seam: &mut impl WindowsMaterialSeam) {
        eprintln!("rune: Windows system material failed; falling back to fill: {err}");
        self.teardown_active(hwnd, seam);
        self.state = State::Failed { hwnd };
    }
}

#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
mod real;

#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
pub(crate) use real::Materials;

#[cfg(test)]
mod tests;
