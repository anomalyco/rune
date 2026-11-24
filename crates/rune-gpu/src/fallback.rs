//! GPU adapter fallback chain: an explicit state machine and pure policy for moving to the
//! next adapter when the picked one cannot draw a surface.
//!
//! Matches Google Chrome's GPU fallback model:
//! - Hardware adapters are tried in priority order before falling back to software rendering
//!   (D3D11 WARP on Windows, `DeviceType::Cpu`), matching `GpuDataManagerImplPrivate`
//!   (`content/browser/gpu/gpu_data_manager_impl_private.cc`) and Windows WARP selection
//!   (`ui/gl/gl_features.cc`, `ui/gl/gl_implementation.cc`).
//! - Persistent adapter or surface incompatibilities (failed device creation, unsupported
//!   surface, or a refused `Surface::configure` on a live device) move on to the next adapter.
//! - Transient failures (`OutOfMemory`, `DeviceLost`) and native surface-handle failures
//!   (`SurfaceCreate`) stop instead of abandoning the active hardware GPU.
//! - An adapter explicitly named by the user (`WGPU_ADAPTER_NAME`) or the app (`Policy::Pinned`)
//!   is never replaced.
//! - On the common path where the first adapter succeeds, no fallback adapters are enumerated.
//! - On Windows DX12, presentation stays on DirectComposition by default and follows Chromium
//!   (CL 6215560, CL 6324950): there is no automatic downgrade to an HWND swap chain; developers
//!   who need a plain HWND swap chain (RenderDoc, Wine/Proton) set
//!   `WGPU_DX12_PRESENTATION_SYSTEM=hwnd` explicitly.
//!
//! Repeated device loss over time (matching Chromium's `GpuProcessHost::IncrementCrashCount`
//! and `GpuDataManagerImplPrivate::FallBackToNextGpuModeDueToCrash` in
//! `content/browser/gpu/gpu_process_host.cc` and `gpu_data_manager_impl_private.cc`):
//! - A single `Failure::DeviceLost` keeps the active hardware GPU.
//! - After [`LOSS_LIMIT`] (3) losses in one mode within the [`FORGIVE_LOSS`] (5-minute) decay
//!   window (one loss forgiven per 5 minutes elapsed since the previous loss; a single healthy
//!   frame does not reset the count), [`Failure::RepeatedDeviceLost`] moves an automatic pick
//!   from [`GpuMode::Hardware`] to [`GpuMode::Software`] with zero losses, or stops if the
//!   adapter is [`Policy::Pinned`], no software adapter is available, or software mode also hits
//!   [`LOSS_LIMIT`] losses.
//!
//! Extension points:
//! - A later failure reason (such as a rejected last-resort shader) adds a variant to
//!   [`Failure`] and one arm to [`decide`].

use std::time::Duration;

use rune_core::machine::Effects;
use rune_core::time::Instant;

/// Whether the chain may replace the first adapter with another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Rune chose the adapter automatically (display GPU, compositor GPU, power preference, or
    /// lowest power): persistent adapter/surface failures move on to the next candidate.
    Automatic,
    /// The user (`WGPU_ADAPTER_NAME`) or the app explicitly named this adapter: never replace it.
    Pinned,
}

/// Why an adapter attempt failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// `Adapter::request_device` failed.
    DeviceCreate,
    /// The adapter cannot present to this surface (unsupported surface, no compatible format, or
    /// missing required transparency support).
    SurfaceUnsupported,
    /// `Surface::configure` was refused by the driver on a live device (internal or validation
    /// error).
    SurfaceRefused,
    /// The adapter's driver rejected Rune's last-resort (Classic) uber shader or pipeline.
    ShaderRejected,
    /// Transient out-of-memory error (`wgpu::Error::OutOfMemory`).
    OutOfMemory,
    /// A single device loss or removal (driver reset, TDR, Modern Standby).
    DeviceLost,
    /// The active GPU mode reached [`LOSS_LIMIT`] device losses within [`FORGIVE_LOSS`].
    RepeatedDeviceLost,
    /// Creating the native `wgpu::Surface` from the window handle failed.
    SurfaceCreate,
}

/// Pure choice returned by [`decide`]: try the next adapter, or stop and report the error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    MoveOn,
    Stop,
}

/// Pure choice function: whether `failure` under `policy` should move on to the next adapter.
pub const fn decide(policy: Policy, failure: Failure) -> Decision {
    match policy {
        Policy::Pinned => Decision::Stop,
        Policy::Automatic => match failure {
            Failure::DeviceCreate
            | Failure::SurfaceUnsupported
            | Failure::SurfaceRefused
            | Failure::ShaderRejected
            | Failure::RepeatedDeviceLost => Decision::MoveOn,
            Failure::OutOfMemory | Failure::DeviceLost | Failure::SurfaceCreate => Decision::Stop,
        },
    }
}

/// `PowerPreference::LowPower`'s order; a software adapter (`Cpu`, e.g. WARP or llvmpipe) is last.
pub const fn power_rank(device: wgpu::DeviceType) -> u8 {
    match device {
        wgpu::DeviceType::IntegratedGpu => 0,
        wgpu::DeviceType::DiscreteGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

/// Plain-data description of an enumerated adapter for pure deduplication and fallback ordering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub index: usize,
    pub name: String,
    pub backend: wgpu::Backend,
    pub device_type: wgpu::DeviceType,
    pub vendor: u32,
    pub device: u32,
    pub pci: String,
    pub surface_supported: bool,
    pub translucent: bool,
}

impl Candidate {
    pub fn from_info(
        index: usize,
        info: &wgpu::AdapterInfo,
        surface_supported: bool,
        translucent: bool,
    ) -> Self {
        Self {
            index,
            name: info.name.clone(),
            backend: info.backend,
            device_type: info.device_type,
            vendor: info.vendor,
            device: info.device,
            pci: info.device_pci_bus_id.clone(),
            surface_supported,
            translucent,
        }
    }

    /// Whether two candidates refer to the same adapter.
    pub fn same_adapter(&self, other: &Self) -> bool {
        self.backend == other.backend
            && self.vendor == other.vendor
            && self.device == other.device
            && self.device_type == other.device_type
            && self.name == other.name
            && self.pci.eq_ignore_ascii_case(&other.pci)
    }
}

fn same_pci(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Orders fallback `candidates` to try after `tried` failed, returning their `Candidate::index`
/// values in attempt order (first element tried first).
///
/// Filters out already-tried adapters and adapters that do not support the surface, then orders:
/// 1. Hardware adapters before software adapters (`DeviceType::Cpu`, e.g. WARP).
/// 2. Adapters on the Wayland compositor's PCI device (if known) before other devices.
/// 3. Hardware adapters driving a desktop display (in `displays` order, primary first).
/// 4. Translucent-capable adapters first when `want_translucent` is true (without dropping
///    opaque-only adapters so a window can still degrade to opaque if needed).
/// 5. [`power_rank`] (integrated -> discrete -> virtual -> other -> cpu), then enumeration order.
pub fn order_fallbacks(
    tried: &[Candidate],
    candidates: &[Candidate],
    displays: &[(u32, u32)],
    compositor_pci: Option<&str>,
    want_translucent: bool,
) -> Vec<usize> {
    let mut usable: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.surface_supported && !tried.iter().any(|seen| seen.same_adapter(candidate))
        })
        .collect();
    usable.sort_by_key(|candidate| {
        let is_cpu = candidate.device_type == wgpu::DeviceType::Cpu;
        let not_compositor = compositor_pci.is_some_and(|pci| !same_pci(pci, &candidate.pci));
        let display_rank = if is_cpu {
            usize::MAX
        } else {
            displays
                .iter()
                .position(|ids| *ids == (candidate.vendor, candidate.device))
                .unwrap_or(usize::MAX)
        };
        let lacks_translucent = want_translucent && !candidate.translucent;
        (
            is_cpu,
            not_compositor,
            display_rank,
            lacks_translucent,
            power_rank(candidate.device_type),
            candidate.index,
        )
    });
    usable.into_iter().map(|candidate| candidate.index).collect()
}

/// Explicit state of one adapter-selection and surface-configuration attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Trying the initially picked adapter before enumerating any others.
    First { policy: Policy },
    /// The first adapter failed with `Decision::MoveOn`; waiting for enumerated candidates.
    NeedFallbacks { first_error: String },
    /// Trying fallback candidate `current`; `remaining` is a stack whose back is tried next.
    Trying { current: usize, remaining: Vec<usize>, first_error: String },
    /// An adapter succeeded.
    Done,
    /// No adapter worked, or a non-fallback error stopped the chain.
    Exhausted { error: String },
}

impl State {
    /// Starts the chain on the first picked adapter under `policy`.
    pub const fn first(policy: Policy) -> Self {
        Self::First { policy }
    }

    /// Starts directly at fallback enumeration when a prewarmed or pooled GPU already failed with
    /// `Decision::MoveOn`.
    pub const fn need_fallbacks(first_error: String) -> Self {
        Self::NeedFallbacks { first_error }
    }

    /// The first adapter's failure message once the chain has moved into fallbacks.
    pub fn first_error(&self) -> Option<&str> {
        match self {
            Self::NeedFallbacks { first_error } | Self::Trying { first_error, .. } => {
                Some(first_error)
            }
            Self::First { .. } | Self::Done | Self::Exhausted { .. } => None,
        }
    }
}

/// Inputs to [`step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The current adapter created its device and configured the surface.
    Succeeded,
    /// The current adapter failed with `kind` and diagnostic `error`.
    Failed { kind: Failure, error: String },
    /// Ordered fallback candidate indices (front = highest priority), supplied after
    /// `Effect::EnumerateFallbacks`.
    Candidates(Vec<usize>),
}

/// Actions the caller carries out for a transition of [`step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Enumerate and order remaining adapters on the instance, then feed `Event::Candidates`.
    /// Never emitted when the first adapter succeeds.
    EnumerateFallbacks,
    /// Try fallback candidate `index`, then feed `Event::Succeeded` or `Event::Failed`.
    TryCandidate(usize),
    /// The chain succeeded; `fell_back` is true when a fallback adapter succeeded.
    FinishOk { fell_back: bool },
    /// The chain stopped or exhausted all candidates; return `error` to the caller.
    FinishErr(String),
}

/// Advances the adapter fallback state machine by one `event`.
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let current = std::mem::replace(state, State::Done);
    match (current, event) {
        (State::First { .. }, Event::Succeeded) => {
            *state = State::Done;
            Effects::one(Effect::FinishOk { fell_back: false })
        }
        (State::First { policy }, Event::Failed { kind, error }) => match decide(policy, kind) {
            Decision::MoveOn => {
                *state = State::NeedFallbacks { first_error: error };
                Effects::one(Effect::EnumerateFallbacks)
            }
            Decision::Stop => {
                *state = State::Exhausted { error: error.clone() };
                Effects::one(Effect::FinishErr(error))
            }
        },
        (State::NeedFallbacks { first_error }, Event::Candidates(mut candidates)) => {
            candidates.reverse();
            match candidates.pop() {
                Some(next) => {
                    *state = State::Trying { current: next, remaining: candidates, first_error };
                    Effects::one(Effect::TryCandidate(next))
                }
                None => {
                    *state = State::Exhausted { error: first_error.clone() };
                    Effects::one(Effect::FinishErr(first_error))
                }
            }
        }
        (State::Trying { .. }, Event::Succeeded) => {
            *state = State::Done;
            Effects::one(Effect::FinishOk { fell_back: true })
        }
        (State::Trying { mut remaining, first_error, .. }, Event::Failed { kind, error }) => {
            match decide(Policy::Automatic, kind) {
                Decision::MoveOn => match remaining.pop() {
                    Some(next) => {
                        *state = State::Trying { current: next, remaining, first_error };
                        Effects::one(Effect::TryCandidate(next))
                    }
                    None => {
                        *state = State::Exhausted { error: first_error.clone() };
                        Effects::one(Effect::FinishErr(first_error))
                    }
                },
                Decision::Stop => {
                    *state = State::Exhausted { error: error.clone() };
                    Effects::one(Effect::FinishErr(error))
                }
            }
        }
        (other, _) => {
            *state = other;
            Effects::none()
        }
    }
}

/// Losses in one GPU mode before falling back to the next mode (or giving up), matching
/// Chromium's `GpuProcessHost::GetFallbackCrashLimit()` on desktop.
pub const LOSS_LIMIT: u8 = 3;

/// Forgive one recorded loss per 5 minutes elapsed since the previous loss in the same mode,
/// matching Chromium's `kForgiveGpuCrashMinutes = 5` (`content/browser/gpu/gpu_process_host.cc`).
pub const FORGIVE_LOSS: Duration = Duration::from_secs(5 * 60);

/// Whether this target platform can have a software adapter fallback (WARP on Windows,
/// llvmpipe/lavapipe on Linux; Metal on macOS exposes no software adapter).
pub const fn software_fallback_available() -> bool {
    !cfg!(target_os = "macos")
}

/// Which tier of the adapter fallback stack the window is currently drawing on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuMode {
    #[default]
    Hardware,
    Software,
}

/// Explicit state machine for repeated device-loss accounting across [`GpuMode`]s.
///
/// Only a mode with at least one recorded loss carries a count and timestamp:
/// - `Clean { mode }`: zero losses recorded in `mode` (initial state and right after switching
///   from `Hardware` to `Software`).
/// - `Counting { mode, losses, last_loss }`: `1 <= losses < LOSS_LIMIT` recent losses in `mode`,
///   with `last_loss` used for [`FORGIVE_LOSS`] time decay.
/// - `GivenUp`: the fallback stack is exhausted (or pinned/unavailable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossState {
    Clean {
        mode: GpuMode,
    },
    Counting {
        mode: GpuMode,
        losses: u8,
        last_loss: Instant,
    },
    GivenUp,
}

impl Default for LossState {
    fn default() -> Self {
        Self::Clean { mode: GpuMode::Hardware }
    }
}

impl LossState {
    /// Starts with zero recorded losses in `mode`.
    pub const fn for_mode(mode: GpuMode) -> Self {
        Self::Clean { mode }
    }

    /// Current GPU mode, or `None` once given up.
    pub const fn mode(self) -> Option<GpuMode> {
        match self {
            Self::Clean { mode } | Self::Counting { mode, .. } => Some(mode),
            Self::GivenUp => None,
        }
    }

    /// Effective recorded losses in the active mode at `now` after forgiving one loss per
    /// [`FORGIVE_LOSS`] elapsed since `last_loss`.
    pub fn effective_losses(self, now: Instant) -> u8 {
        match self {
            Self::Clean { .. } => 0,
            Self::Counting { losses, last_loss, .. } => {
                let elapsed = now.saturating_duration_since(last_loss);
                let forgiven = (elapsed.as_secs() / FORGIVE_LOSS.as_secs()).min(u64::from(losses)) as u8;
                losses.saturating_sub(forgiven)
            }
            Self::GivenUp => LOSS_LIMIT,
        }
    }

    /// Next retry deadline from `now` (increasing with consecutive losses in the current mode),
    /// or `None` if the next loss at `now` would exhaust the fallback stack and give up.
    pub fn next_backoff(
        self,
        now: Instant,
        policy: Policy,
        software_available: bool,
        base: Duration,
    ) -> Option<Instant> {
        match self {
            Self::GivenUp => None,
            Self::Clean { .. } => Some(now + base),
            Self::Counting { mode, .. } => {
                let remaining = self.effective_losses(now);
                let can_fallback = mode == GpuMode::Hardware
                    && software_available
                    && decide(policy, Failure::RepeatedDeviceLost) == Decision::MoveOn;
                if remaining.saturating_add(1) >= LOSS_LIMIT && !can_fallback {
                    return None;
                }
                Some(now + base * u32::from(remaining.max(1)))
            }
        }
    }
}

/// Inputs to [`step_loss`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossEvent {
    /// The window needs a replacement device/surface at `now` under `policy`.
    Replace {
        now: Instant,
        policy: Policy,
        software_available: bool,
    },
    /// The replacement attempt itself failed at `now`. When the chain just entered
    /// `Clean { mode: Software }`, counts that failed switch as the first software loss so it
    /// does not grant a free extra try.
    ReplaceFailed {
        now: Instant,
    },
    /// The window is now drawing on a software adapter (`DeviceType::Cpu`), e.g. after an
    /// initial or `SurfaceRefused` fallback landed on software.
    OnSoftware,
    /// Software fallback found no usable software adapter on this host.
    NoSoftwareAdapter,
}

/// Actions the caller carries out for a transition of [`step_loss`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossEffect {
    /// Recreate the device and surface in `mode`; `switched_to_software` is true on the single
    /// transition from `Hardware` to `Software` so the caller logs it once.
    Replace {
        mode: GpuMode,
        switched_to_software: bool,
    },
    /// The fallback stack is exhausted (or pinned/unavailable): give the window up.
    GiveUp,
}

/// Advances the repeated-device-loss state machine by one `event`.
pub fn step_loss(state: &mut LossState, event: LossEvent) -> Effects<LossEffect> {
    match (*state, event) {
        (LossState::GivenUp, LossEvent::Replace { .. }) => Effects::one(LossEffect::GiveUp),
        (
            LossState::Clean { mode } | LossState::Counting { mode, .. },
            LossEvent::Replace { now, policy, software_available },
        ) => {
            let count = state.effective_losses(now).saturating_add(1);
            if count < LOSS_LIMIT {
                *state = LossState::Counting { mode, losses: count, last_loss: now };
                Effects::one(LossEffect::Replace { mode, switched_to_software: false })
            } else if mode == GpuMode::Hardware
                && software_available
                && decide(policy, Failure::RepeatedDeviceLost) == Decision::MoveOn
            {
                *state = LossState::Clean { mode: GpuMode::Software };
                Effects::one(LossEffect::Replace {
                    mode: GpuMode::Software,
                    switched_to_software: true,
                })
            } else {
                *state = LossState::GivenUp;
                Effects::one(LossEffect::GiveUp)
            }
        }
        (LossState::Clean { mode: GpuMode::Software }, LossEvent::ReplaceFailed { now }) => {
            *state = LossState::Counting { mode: GpuMode::Software, losses: 1, last_loss: now };
            Effects::none()
        }
        (
            LossState::Clean { mode: GpuMode::Hardware }
            | LossState::Counting { mode: GpuMode::Hardware, .. },
            LossEvent::OnSoftware,
        ) => {
            *state = LossState::Clean { mode: GpuMode::Software };
            Effects::none()
        }
        (_, LossEvent::NoSoftwareAdapter) => {
            *state = LossState::GivenUp;
            Effects::none()
        }
        _ => Effects::none(),
    }
}

#[cfg(test)]
#[path = "fallback_tests.rs"]
mod tests;
