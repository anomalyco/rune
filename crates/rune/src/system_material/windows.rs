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
use crate::machine::Effects;
use rune_motion::NodeId;

/// Minimum Windows 11 build that documents `DWMWA_USE_HOSTBACKDROPBRUSH` (`dwmapi.h`).
pub(crate) const MIN_HOST_BACKDROP_BUILD: u32 = 22000;

/// `GUID_POWER_SAVING_STATUS` (`{E00958C0-C213-4ACE-AC77-FECCED2EEEA5}`, WinNT.h):
/// legacy Battery Saver state (`DWORD`: 0 = off, 1 = on).
pub(crate) const GUID_POWER_SAVING_STATUS_PARTS: (u32, u16, u16, [u8; 8]) =
    (0xE009_58C0, 0xC213, 0x4ACE, [0xAC, 0x77, 0xFE, 0xCC, 0xED, 0x2E, 0xEE, 0xA5]);

/// `GUID_ENERGY_SAVER_STATUS` (`{550E8400-E29B-41D4-A716-446655440000}`, WinNT.h, Windows 11 24H2+):
/// three-state Energy Saver mode (`DWORD`: 0 = `ENERGY_SAVER_OFF`, 1 = `ENERGY_SAVER_STANDARD`,
/// 2 = `ENERGY_SAVER_HIGH_SAVINGS`).
pub(crate) const GUID_ENERGY_SAVER_STATUS_PARTS: (u32, u16, u16, [u8; 8]) =
    (0x550E_8400, 0xE29B, 0x41D4, [0xA7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00]);

/// Decoded power-setting notification payload from `PowerSettingRegisterNotification`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PowerSettingUpdate {
    /// `GUID_POWER_SAVING_STATUS`: `true` when Battery Saver is active.
    PowerSaving(bool),
    /// `GUID_ENERGY_SAVER_STATUS`: `true` when Standard or High Savings Energy Saver is active.
    EnergySaver(bool),
}

/// Pure parser for `POWERBROADCAST_SETTING` payloads delivered by `PowerSettingRegisterNotification`.
pub(crate) fn parse_power_setting(
    guid: (u32, u16, u16, [u8; 8]),
    data: &[u8],
) -> Option<PowerSettingUpdate> {
    if data.len() < 4 {
        return None;
    }
    let value = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if guid == GUID_POWER_SAVING_STATUS_PARTS {
        Some(PowerSettingUpdate::PowerSaving(value != 0))
    } else if guid == GUID_ENERGY_SAVER_STATUS_PARTS {
        // ENERGY_SAVER_OFF == 0; ENERGY_SAVER_STANDARD == 1, ENERGY_SAVER_HIGH_SAVINGS == 2.
        Some(PowerSettingUpdate::EnergySaver(value != 0))
    } else {
        None
    }
}

/// System-level Windows policy facts that decide whether DWM will actually draw
/// `HostBackdropBrush` rather than a solid black rectangle.
///
/// Mirrors Microsoft's documented Acrylic / `HostBackdropBrush` fallback conditions:
/// - `advanced_effects`: `Windows.UI.ViewManagement.UISettings::AdvancedEffectsEnabled`
///   (Settings -> Personalization -> Colors / Accessibility -> Visual effects -> Transparency effects;
///   also re-read on `WM_SETTINGCHANGE`).
/// - `energy_saver`: `true` while Battery Saver or Windows 11 24H2+ Energy Saver (Standard or
///   High Savings) is active (`GUID_POWER_SAVING_STATUS` / `GUID_ENERGY_SAVER_STATUS` via
///   `PowerSettingRegisterNotification`, seeded from `GetSystemPowerStatus().SystemStatusFlag`).
/// - `composition_effects`: `Windows.UI.Composition.CompositionCapabilities`
///   (`AreEffectsSupported() && AreEffectsFast()`, or `true` when unavailable on a non-CoreWindow
///   desktop thread so GPU adapter checks remain authoritative).
/// - `high_contrast`: `SystemParametersInfoW(SPI_GETHIGHCONTRAST)` (`HCF_HIGHCONTRASTON`).
/// - `remote_session`: `GetSystemMetrics(SM_REMOTESESSION) != 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectsPolicy {
    pub advanced_effects: bool,
    pub energy_saver: bool,
    pub composition_effects: bool,
    pub high_contrast: bool,
    pub remote_session: bool,
}

impl EffectsPolicy {
    /// All system conditions allowing DWM host backdrop blur.
    #[cfg(test)]
    pub(crate) const ENABLED: Self = Self {
        advanced_effects: true,
        energy_saver: false,
        composition_effects: true,
        high_contrast: false,
        remote_session: false,
    };

    /// Whether the Windows system policy permits `HostBackdropBrush` to render blur.
    pub(crate) const fn allows_blur(self) -> bool {
        self.advanced_effects
            && !self.energy_saver
            && self.composition_effects
            && !self.high_contrast
            && !self.remote_session
    }

    /// Compact bitmask encoding for change deduplication across event sources.
    #[cfg_attr(not(any(test, all(target_os = "windows", feature = "windows-system-material"))), allow(dead_code))]
    pub(crate) const fn bits(self) -> u8 {
        (self.advanced_effects as u8)
            | ((self.energy_saver as u8) << 1)
            | ((self.composition_effects as u8) << 2)
            | ((self.high_contrast as u8) << 3)
            | ((self.remote_session as u8) << 4)
    }
}

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
    /// Live Windows effects policy (Transparency effects, Energy/Battery Saver,
    /// `CompositionCapabilities`, High Contrast, Remote Desktop).
    pub effects: EffectsPolicy,
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
/// 9. the window or overlay holds a live DirectComposition `Handover`;
/// 10. Windows system effects policy permits host backdrop blur (`effects.allows_blur()`).
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
    if !inputs.effects.allows_blur() {
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

/// Windows 11 backdrop brush / tint mode (`RUNE_WIN_MATERIAL_TINT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TintMode {
    /// Microsoft's Windows 11 Acrylic Luminosity + Color `CompositionEffectBrush` (default).
    /// Degrades gracefully to [`Self::Flat`] if `CreateEffectFactory` fails at runtime.
    #[default]
    Acrylic,
    /// WCAG >=4.5:1 flat-tint `CompositionColorBrush` child visual over `HostBackdropBrush`
    /// (diagnostic comparison and graceful fallback when `CreateEffectFactory` fails).
    Flat,
    /// Raw `CreateHostBackdropBrush()` with no Acrylic effect or tint layer (diagnostics).
    Raw,
}

impl TintMode {
    pub(crate) fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("flat" | "subtle") => Self::Flat,
            Some("raw" | "none") => Self::Raw,
            _ => Self::Acrylic,
        }
    }

    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    pub(crate) fn from_env() -> Self {
        Self::parse(rune_core::env::var("RUNE_WIN_MATERIAL_TINT").as_deref())
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
    /// Element's requested `SystemMaterial` (selects the Windows 11 Acrylic recipe).
    pub material: SystemMaterial,
    /// Active dark theme flag (selects dark vs light Acrylic / flat-tint recipe).
    pub dark: bool,
    /// Active brush/tint mode (`Acrylic`, `Flat`, or `Raw`).
    pub tint_mode: TintMode,
}

impl VisualSpec {
    /// Converts one `MaterialDraw` (already in physical pixels) into a `VisualSpec`.
    pub(crate) fn from_draw(
        draw: MaterialDraw,
        clip: ClipKind,
        tint_mode: TintMode,
        dark: bool,
    ) -> Self {
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
            material: draw.material,
            dark,
            tint_mode,
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

/// Lifecycle events for the Windows system-material host state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// A frame finished layout/paint and evaluated material eligibility.
    Frame {
        hwnd: Option<isize>,
        inputs: MaterialInputs,
        found: bool,
    },
    /// The OS system effects policy changed (Transparency effects, Energy/Battery Saver,
    /// `CompositionCapabilities`, High Contrast, or Remote Desktop session).
    PolicyChanged {
        policy: EffectsPolicy,
        had_materials: bool,
    },
    /// Activation failed on `hwnd`: latch failed for this HWND and rebuild fallback if mounted.
    ActivationFailed {
        hwnd: isize,
        found: bool,
    },
    /// GPU device loss: tear down active visuals and clear any failure latch.
    DeviceLoss,
    /// Window or overlay teardown on close.
    Teardown,
}

/// Side effects emitted by [`step`] for the controller / seam to execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Activate `Handover(topmost = true)` -> `DesktopWindowTarget(false)` -> `DWMWA_USE_HOSTBACKDROPBRUSH`.
    Activate { hwnd: isize },
    /// Tear down `DesktopWindowTarget`, clear `DWMWA_USE_HOSTBACKDROPBRUSH`, and restore `Handover(topmost = false)`.
    Teardown { hwnd: isize },
    /// Rebuild this frame with the updated `native()` flag before rendering so neither activation
    /// nor fallback flashes a black or wrong-fill frame.
    RebuildFrame,
    /// Schedule a redraw so the window/overlay repaints with fallback fills or re-activates blur.
    RequestRedraw,
}

/// Pure state transition function for the Windows system-material lifecycle.
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Idle, Event::Frame { hwnd: Some(hwnd), inputs, found: true }) if eligible(inputs) => {
            *state = State::Active { hwnd };
            Effects::two(Effect::Activate { hwnd }, Effect::RebuildFrame)
        }
        (State::Idle, Event::Frame { .. }) => Effects::none(),
        (State::Active { hwnd: active }, Event::Frame { hwnd, inputs, found }) => {
            let can_run = found && eligible(inputs);
            if can_run && hwnd == Some(active) {
                return Effects::none();
            }
            if can_run && let Some(next) = hwnd {
                *state = State::Active { hwnd: next };
                return Effects::two(Effect::Teardown { hwnd: active }, Effect::Activate { hwnd: next });
            }
            *state = State::Idle;
            if found {
                Effects::two(Effect::Teardown { hwnd: active }, Effect::RebuildFrame)
            } else {
                Effects::one(Effect::Teardown { hwnd: active })
            }
        }
        (State::Failed { hwnd: failed }, Event::Frame { hwnd, inputs, found }) => {
            if hwnd.is_some_and(|h| h != failed) {
                *state = State::Idle;
                return step(state, Event::Frame { hwnd, inputs, found });
            }
            Effects::none()
        }
        (State::Active { hwnd }, Event::PolicyChanged { policy, .. }) if !policy.allows_blur() => {
            *state = State::Idle;
            Effects::two(Effect::Teardown { hwnd }, Effect::RequestRedraw)
        }
        (State::Idle, Event::PolicyChanged { policy, had_materials: true }) if policy.allows_blur() => {
            Effects::one(Effect::RequestRedraw)
        }
        (_, Event::PolicyChanged { .. }) => Effects::none(),
        (_, Event::ActivationFailed { hwnd, found }) => {
            *state = State::Failed { hwnd };
            if found { Effects::one(Effect::RebuildFrame) } else { Effects::none() }
        }
        (State::Active { hwnd }, Event::Teardown | Event::DeviceLoss) => {
            *state = State::Idle;
            Effects::one(Effect::Teardown { hwnd })
        }
        (State::Failed { .. }, Event::Teardown | Event::DeviceLoss) => {
            *state = State::Idle;
            Effects::none()
        }
        (State::Idle, Event::Teardown | Event::DeviceLoss) => Effects::none(),
    }
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

    /// Initiates a composition commit (`Compositor::RequestCommitAsync`) right after
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
    had_materials: bool,
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
            had_materials: false,
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
        let was_active = self.native();
        self.had_materials = found;
        let effects = step(&mut self.state, Event::Frame { hwnd, inputs, found });
        let mut rebuild = false;
        for effect in effects {
            match effect {
                Effect::Activate { hwnd } => {
                    if let Err(err) = self.activate(hwnd, seam) {
                        eprintln!("rune: Windows system material activation failed: {err}");
                        // Only request a fallback rebuild when we were previously active (switching
                        // HWNDs); a first-time failure from Idle already painted its fallback fill.
                        let fail_effects = step(
                            &mut self.state,
                            Event::ActivationFailed { hwnd, found: was_active && found },
                        );
                        return fail_effects.into_iter().any(|e| e == Effect::RebuildFrame);
                    }
                }
                Effect::Teardown { hwnd } => self.teardown_active(hwnd, seam),
                Effect::RebuildFrame => rebuild = true,
                Effect::RequestRedraw => {}
            }
        }
        rebuild
    }

    /// Handles an OS system-effects policy change event (Transparency effects, Energy/Battery
    /// Saver, `CompositionCapabilities`, High Contrast, or Remote Desktop) without polling.
    ///
    /// Returns `true` when the host must redraw (either an active material host just tore down to
    /// draw fallback fills, or effects came back while material elements are mounted).
    pub(crate) fn on_policy_changed(
        &mut self,
        policy: EffectsPolicy,
        seam: &mut impl WindowsMaterialSeam,
    ) -> bool {
        let effects = step(
            &mut self.state,
            Event::PolicyChanged { policy, had_materials: self.had_materials },
        );
        let mut redraw = false;
        for effect in effects {
            match effect {
                Effect::Teardown { hwnd } => self.teardown_active(hwnd, seam),
                Effect::RequestRedraw => redraw = true,
                Effect::Activate { .. } | Effect::RebuildFrame => {}
            }
        }
        redraw
    }

    /// Presents the swap chain first, then syncs pooled visuals to `draws` (physical pixels) and
    /// requests a composition commit (`Compositor::RequestCommitAsync`).
    ///
    /// Queuing `IDXGISwapChain::Present` before updating `SpriteVisual`s and calling
    /// `RequestCommitAsync` prevents DWM from committing frame N's backdrop ahead of frame N's
    /// swap-chain flip (which caused the blur to lean ~1 frame ahead during fast springs).
    ///
    /// Returns `true` if native materials remain active (or were already idle), or `false` if a
    /// COM/DWM error forced a rollback to fallback fills (caller should clear `native_materials`
    /// and schedule a redraw).
    pub(crate) fn present(
        &mut self,
        draws: &[MaterialDraw],
        dark: bool,
        seam: &mut impl WindowsMaterialSeam,
        present: impl FnOnce(),
    ) -> bool {
        present();
        let State::Active { hwnd } = self.state else {
            return true;
        };
        let draws = &draws[..draws.len().min(MAX_NATIVE_MATERIALS_CAP)];
        if draws.is_empty() {
            self.teardown(seam);
            return true;
        }
        let wanted: Vec<_> = draws.iter().map(|d| (d.material, d.id)).collect();
        let views: Vec<_> = self.slots.iter().map(|s| (s.material, s.owner)).collect();
        let assigned = assign(&views, &wanted);
        let mut taken = vec![false; self.slots.len()];
        let mut changed = false;

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
            let spec = VisualSpec::from_draw(*draw, self.clip, self.tint, dark);
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
        self.had_materials = false;
        for effect in step(&mut self.state, Event::Teardown) {
            if let Effect::Teardown { hwnd } = effect {
                self.teardown_active(hwnd, seam);
            }
        }
    }

    /// Tears down active visuals on GPU device loss and clears any failure latch so a recovered
    /// hardware GPU can re-evaluate eligibility cleanly (while a WARP fallback stays inactive).
    pub(crate) fn on_device_loss(&mut self, seam: &mut impl WindowsMaterialSeam) {
        self.had_materials = false;
        for effect in step(&mut self.state, Event::DeviceLoss) {
            if let Effect::Teardown { hwnd } = effect {
                self.teardown_active(hwnd, seam);
            }
        }
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
    }

    fn rollback_and_latch(&mut self, hwnd: isize, err: &str, seam: &mut impl WindowsMaterialSeam) {
        eprintln!("rune: Windows system material failed; falling back to fill: {err}");
        self.teardown_active(hwnd, seam);
        self.state = State::Failed { hwnd };
    }
}

pub(crate) mod acrylic;
#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
mod policy;
#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
mod real;

#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
pub(crate) use policy::{current_policy, notify_wm_setting_change, set_policy_waker};
#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
pub(crate) use real::Materials;

#[cfg(test)]
mod acrylic_tests;
#[cfg(test)]
mod tests;
