//! Terminal GPU failure policy ([`GpuLossPolicy`]) and explicit first-reason latch state machine
//! ([`LatchState`], [`LatchEvent`], [`LatchEffect`], [`step`], [`GpuLossLatch`]).
//!
//! Recovery ([`GpuLossPolicy::Recover`]) remains Rune's default—mirroring Chromium's bounded GPU
//! restarts with hardware-to-software fallback. Apps whose embedded renderer owns GPU resources
//! that cannot silently migrate to a replacement device opt into [`GpuLossPolicy::Terminal`],
//! matching WebGL's permanent context loss without `preventDefault()`, WebGPU's permanent
//! `GPUDevice.lost`, Bevy's `RenderErrorPolicy::StopRendering`, and GPUI's permanent stop on
//! device loss.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::machine::Effects;

/// Maximum UTF-8 byte length stored for a latched GPU failure reason so an unbounded driver or
/// validation message cannot grow memory.
pub(crate) const MAX_REASON_BYTES: usize = 512;

/// How a window responds when its GPU device is lost or encounters an uncaptured GPU error
/// ([`App::gpu_loss_policy`](crate::App::gpu_loss_policy),
/// [`WindowSpec::gpu_loss_policy`](crate::WindowSpec::gpu_loss_policy)).
///
/// Defaults to [`GpuLossPolicy::Recover`]: Rune uses Chromium-style bounded recovery (retrying
/// device creation, falling back to a software adapter, then giving up while still allowing
/// reopen). Opt into [`GpuLossPolicy::Terminal`] when an embedded renderer owns device-bound
/// resources that cannot silently migrate to a replacement device: Rune records the first
/// failure permanently, stops input and rendering for that window, and never replaces its GPU
/// device (including on reopen).
///
/// # Limits
///
/// - Commands already handed to `wgpu::Queue::submit` or `wgpu::Queue::present` cannot be
///   recalled from the driver.
/// - On macOS, Rune's default native Metal backend implements `Device::set_device_lost_callback`
///   (failed command buffers and `Device::destroy`), while `Device::on_uncaptured_error` is a
///   no-op; apps relying on uncaptured validation/OOM errors on macOS should opt into
///   [`App::gpu_backend`](crate::App::gpu_backend)`(`[`GpuBackend::Wgpu`](crate::GpuBackend::Wgpu)`)`.
/// - On `wasm32`, [`GpuLossPolicy::Terminal`] skips CPU canvas fallback and WebGL2 context
///   restoration (`event.preventDefault()` is omitted on `webglcontextlost`).
/// - Windows only in this step: overlays share the process GPU pool; when a `Terminal` window
///   latches, Rune marks that device lost and clears the shared pool so overlays do not keep
///   drawing with a device the app declared dead.
///
/// ```no_run
/// use rune::{App, AppSpec, GpuLossPolicy, el};
///
/// let spec = AppSpec::new("Viewer", (800.0, 600.0), || (), |_, _| el());
/// let app = App::new(spec).gpu_loss_policy(GpuLossPolicy::Terminal);
/// let handle = app.handle(); // inspect handle.window_error() once latched
/// let _ = handle;
/// app.run();
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GpuLossPolicy {
    /// Default: Chromium-style bounded recovery (retry, hardware-to-software fallback, then give
    /// up while allowing reopen).
    #[default]
    Recover,
    /// Record the first GPU failure permanently, stop input and rendering for this window, and
    /// never replace its device (including on reopen).
    Terminal,
}

/// Classification of the first failure recorded by [`GpuLossLatch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum GpuFailureKind {
    /// `Device::set_device_lost_callback` fired (`Unknown` or `Destroyed` while armed).
    DeviceLost,
    /// D3D12 `ID3D12Device::GetDeviceRemovedReason` reported removal before wgpu's callback ran.
    DeviceRemoved,
    /// Uncaptured `wgpu::Error` (`Validation`, `OutOfMemory`, or `Internal`).
    UncapturedError,
    /// Initial GPU/surface creation failed in `Host::try_create`.
    InitFailed,
    /// Same-device surface recreation or `configure_checked` failed unrecoverably.
    SurfaceFailed,
}

/// Explicit state machine for a [`GpuLossPolicy::Terminal`] failure latch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum LatchState {
    /// Watching the current GPU device; any failure transitions permanently to [`Self::Failed`].
    #[default]
    Armed,
    /// Temporarily detached immediately before an intentional clean GPU drop
    /// (`release_gpu_when_closed` or normal quit) so wgpu's drop-triggered
    /// `DeviceLostReason::Destroyed` never falsely latches a healthy close/reopen.
    Detached,
    /// Permanently latched on the first failure; never overwritten and never re-armed.
    Failed {
        kind: GpuFailureKind,
        message: String,
    },
}

/// Inputs to [`step`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum LatchEvent {
    /// `Device::set_device_lost_callback` fired. `destroyed` is `true` for
    /// `DeviceLostReason::Destroyed`.
    DeviceLost {
        destroyed: bool,
        message: String,
    },
    /// Any non-destroy-callback failure: D3D12 removal, uncaptured error, init failure, or
    /// unrecoverable surface failure.
    Failure {
        kind: GpuFailureKind,
        message: String,
    },
    /// Host is intentionally dropping a healthy GPU device (`release_gpu_when_closed` or quit).
    Detach,
    /// Host attached a fresh healthy GPU device after a clean detach.
    Rearm,
}

/// Side effect emitted once when [`LatchState`] first enters [`LatchState::Failed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum LatchEffect {
    /// Wake the event loop once so the host and app observe the latched failure immediately.
    Wake,
}

/// Advances `state` on `event`, returning at most a one-shot [`LatchEffect::Wake`].
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn step(state: &mut LatchState, event: LatchEvent) -> Effects<LatchEffect> {
    match (&*state, event) {
        // Terminal state: first failure wins permanently; ignore every subsequent event.
        (LatchState::Failed { .. }, _) => Effects::none(),
        // Intentional clean release ignores drop-triggered Destroyed callbacks.
        (LatchState::Detached, LatchEvent::DeviceLost { destroyed: true, .. }) => Effects::none(),
        (LatchState::Detached, LatchEvent::Rearm) => {
            *state = LatchState::Armed;
            Effects::none()
        }
        (LatchState::Detached, LatchEvent::Detach) => Effects::none(),
        (LatchState::Detached, LatchEvent::DeviceLost { destroyed: false, message }) => {
            *state = LatchState::Failed {
                kind: GpuFailureKind::DeviceLost,
                message: truncate_reason(&message, GpuFailureKind::DeviceLost),
            };
            Effects::one(LatchEffect::Wake)
        }
        (LatchState::Detached, LatchEvent::Failure { kind, message }) => {
            *state = LatchState::Failed {
                kind,
                message: truncate_reason(&message, kind),
            };
            Effects::one(LatchEffect::Wake)
        }
        (LatchState::Armed, LatchEvent::Detach) => {
            *state = LatchState::Detached;
            Effects::none()
        }
        (LatchState::Armed, LatchEvent::Rearm) => Effects::none(),
        (LatchState::Armed, LatchEvent::DeviceLost { message, .. }) => {
            *state = LatchState::Failed {
                kind: GpuFailureKind::DeviceLost,
                message: truncate_reason(&message, GpuFailureKind::DeviceLost),
            };
            Effects::one(LatchEffect::Wake)
        }
        (LatchState::Armed, LatchEvent::Failure { kind, message }) => {
            *state = LatchState::Failed {
                kind,
                message: truncate_reason(&message, kind),
            };
            Effects::one(LatchEffect::Wake)
        }
    }
}

/// Truncates `message` to at most [`MAX_REASON_BYTES`] on a valid UTF-8 character boundary.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn truncate_reason(message: &str, kind: GpuFailureKind) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return match kind {
            GpuFailureKind::DeviceLost => "the GPU device was lost".into(),
            GpuFailureKind::DeviceRemoved => "the GPU device was removed".into(),
            GpuFailureKind::UncapturedError => "an uncaptured GPU error occurred".into(),
            GpuFailureKind::InitFailed => "GPU initialization failed".into(),
            GpuFailureKind::SurfaceFailed => "GPU surface configuration failed".into(),
        };
    }
    if trimmed.len() <= MAX_REASON_BYTES {
        return trimmed.to_owned();
    }
    let suffix = "...";
    let mut end = MAX_REASON_BYTES - suffix.len();
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + suffix.len());
    out.push_str(&trimmed[..end]);
    out.push_str(suffix);
    out
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
type WakeHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Thread-safe, shareable terminal GPU failure latch stored in `GpuCallbacks`, the window host,
/// and owned overlays.
///
/// Healthy-frame guard checks read only `failed` (one `Acquire` load, zero locks, zero heap
/// allocations). First transition into `LatchState::Failed` stores `failed = true` with `Release`
/// ordering and invokes every registered wake hook (publishing `SharedCore::window_error` before
/// sending `WakeEvent::App`) once.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) struct GpuLossLatch {
    failed: AtomicBool,
    state: Mutex<LatchState>,
    wakes: Mutex<Vec<WakeHook>>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl GpuLossLatch {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            failed: AtomicBool::new(false),
            state: Mutex::new(LatchState::Armed),
            wakes: Mutex::new(Vec::new()),
        })
    }

    /// Registers a one-shot hook called with the first failure reason when the latch trips
    /// (publishes `SharedCore::window_error` and sends `WakeEvent::App`, or wakes owned overlays).
    /// If the latch is already failed when registered, invokes `wake` immediately once.
    pub(crate) fn add_wake(&self, wake: WakeHook) {
        if let Some(reason) = self.reason() {
            wake(&reason);
            return;
        }
        self.wakes.lock().unwrap_or_else(|e| e.into_inner()).push(wake);
    }

    /// Lock-free fast check: true once any terminal failure has latched.
    #[inline]
    pub(crate) fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Returns the latched first failure reason, or `None` while healthy.
    pub(crate) fn reason(&self) -> Option<String> {
        if !self.is_failed() {
            return None;
        }
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*guard {
            LatchState::Failed { message, .. } => Some(message.clone()),
            LatchState::Armed | LatchState::Detached => None,
        }
    }

    /// Returns the latched first failure kind, or `None` while healthy.
    #[cfg(test)]
    pub(crate) fn kind(&self) -> Option<GpuFailureKind> {
        if !self.is_failed() {
            return None;
        }
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*guard {
            LatchState::Failed { kind, .. } => Some(*kind),
            LatchState::Armed | LatchState::Detached => None,
        }
    }

    /// Records a `Device::set_device_lost_callback` event. Returns `true` if the latch is now
    /// failed (either newly tripped or already failed), `false` if ignored while `Detached`.
    pub(crate) fn record_device_lost(&self, destroyed: bool, message: impl Into<String>) -> bool {
        self.apply(LatchEvent::DeviceLost {
            destroyed,
            message: message.into(),
        })
    }

    /// Records a non-destroy-callback failure (D3D12 removal, uncaptured error, init/surface
    /// failure). Returns `true` (always failed after this call).
    pub(crate) fn record_failure(&self, kind: GpuFailureKind, message: impl Into<String>) -> bool {
        self.apply(LatchEvent::Failure {
            kind,
            message: message.into(),
        })
    }

    /// Marks the active device intentionally detached before dropping it on clean close or quit.
    pub(crate) fn detach(&self) {
        let _ = self.apply(LatchEvent::Detach);
    }

    /// Re-arms the latch after a fresh healthy device is attached following a clean detach.
    pub(crate) fn rearm(&self) {
        let _ = self.apply(LatchEvent::Rearm);
    }

    fn apply(&self, event: LatchEvent) -> bool {
        let (effects, failed_reason) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let effects = step(&mut state, event);
            let failed_reason = match &*state {
                LatchState::Failed { message, .. } => {
                    self.failed.store(true, Ordering::Release);
                    Some(message.clone())
                }
                LatchState::Armed | LatchState::Detached => None,
            };
            (effects, failed_reason)
        };
        for effect in effects {
            match effect {
                LatchEffect::Wake => {
                    if let Some(reason) = failed_reason.as_deref() {
                        let wakes = self.wakes.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        for wake in wakes {
                            wake(reason);
                        }
                    }
                }
            }
        }
        failed_reason.is_some()
    }
}

impl<S: 'static> crate::App<S> {
    /// Sets how this app's window and owned overlays respond to GPU device loss or uncaptured GPU
    /// errors ([`GpuLossPolicy`]).
    ///
    /// Defaults to [`GpuLossPolicy::Recover`]: Rune uses Chromium-style bounded recovery (retry,
    /// hardware-to-software fallback, then give up while allowing reopen). Pass
    /// [`GpuLossPolicy::Terminal`] when an embedded renderer owns device-bound resources that
    /// cannot silently migrate to a replacement GPU: Rune latches the first failure permanently,
    /// keeps an open window on its last presented frame with input frozen and close/quit paths
    /// working, publishes the reason to [`AppHandle::window_error`](crate::AppHandle::window_error),
    /// wakes [`AppSpec::poll`](crate::AppSpec::poll) once so the app can update the window title or
    /// exit, stops owned overlays from creating or using a device, and refuses all future device
    /// replacements (including on reopen).
    ///
    /// Applies to owned overlays registered via [`App::overlay`](Self::overlay) regardless of call
    /// order.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, GpuLossPolicy, el};
    ///
    /// let spec = AppSpec::new("Terminal", (960.0, 640.0), || (), |_, _| el());
    /// let app = App::new(spec).gpu_loss_policy(GpuLossPolicy::Terminal);
    /// let handle = app.handle(); // inspect handle.window_error() after the one-shot poll wake
    /// let _ = handle;
    /// app.run();
    /// ```
    pub fn gpu_loss_policy(mut self, policy: GpuLossPolicy) -> Self {
        self.gpu_loss_policy = policy;
        #[cfg(not(target_arch = "wasm32"))]
        {
            match policy {
                GpuLossPolicy::Terminal => {
                    let latch = self.gpu_loss_latch.get_or_insert_with(GpuLossLatch::new).clone();
                    for overlay in &mut self.overlays {
                        overlay.set_gpu_loss_latch(Some(Arc::clone(&latch)));
                    }
                }
                GpuLossPolicy::Recover => {
                    self.gpu_loss_latch = None;
                    for overlay in &mut self.overlays {
                        overlay.set_gpu_loss_latch(None);
                    }
                }
            }
        }
        self
    }
}

#[cfg(test)]
#[path = "loss_latch_tests.rs"]
mod tests;
