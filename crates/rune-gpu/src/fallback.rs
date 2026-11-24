//! GPU adapter fallback chain: an explicit state machine and pure policy for moving to the
//! next adapter when the picked one cannot draw a surface.
//!
//! Matches Google Chrome's GPU fallback model:
//! - Hardware adapters are tried in priority order before falling back to software rendering
//!   (D3D11 WARP on Windows, `DeviceType::Cpu`), matching `GpuDataManagerImplPrivate`
//!   (`content/browser/gpu/gpu_data_manager_impl_private.cc`) and Windows WARP selection
//!   (`ui/gl/gl_features.cc`, `ui/gl/gl_implementation.cc`).
//! - Persistent adapter or surface incompatibilities (failed device creation, unsupported
//!   surface, or a refused `Surface::configure` on a live device such as `E_NOTIMPL` on
//!   `CreateSwapChainForComposition`) move on to the next adapter.
//! - Transient failures (`OutOfMemory`, `DeviceLost`) and native surface-handle failures
//!   (`SurfaceCreate`) stop instead of abandoning the active hardware GPU.
//! - An adapter explicitly named by the user (`WGPU_ADAPTER_NAME`) or the app (`Policy::Pinned`)
//!   is never replaced.
//! - On the common path where the first adapter succeeds, no fallback adapters are enumerated.
//!
//! Extension points:
//! - A later failure reason (such as a rejected last-resort shader) adds a variant to
//!   [`Failure`] and one arm to [`decide`].
//! - A presentation-mode fallback (such as a plain HWND swap chain when DirectComposition is
//!   absent on every adapter) plugs in where [`step`] exhausts candidates after
//!   `Failure::SurfaceRefused`.

use rune_core::machine::Effects;

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
    /// error, e.g. `E_NOTIMPL` on `CreateSwapChainForComposition`).
    SurfaceRefused,
    /// Transient out-of-memory error (`wgpu::Error::OutOfMemory`).
    OutOfMemory,
    /// The device was lost or removed (driver reset, TDR, Modern Standby).
    DeviceLost,
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
            Failure::DeviceCreate | Failure::SurfaceUnsupported | Failure::SurfaceRefused => {
                Decision::MoveOn
            }
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
    Trying {
        current: usize,
        remaining: Vec<usize>,
        first_error: String,
    },
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

#[cfg(test)]
#[path = "fallback_tests.rs"]
mod tests;
