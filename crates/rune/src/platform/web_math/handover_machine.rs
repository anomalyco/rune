//! Explicit state machine for CPU-to-GPU hand-over on a full-window web page.
//!
//! Kept under `platform/web_math` so every transition compiles and runs in native unit tests off
//! `wasm32`.

use std::time::Duration;

use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;

/// How long the GPU's hand-over frame may take to finish before the page stays on the CPU.
pub(crate) const FINISH_LIMIT: Duration = Duration::from_secs(5);
/// Small padding added to the timeout timer after `FINISH_LIMIT` so `now >= deadline` holds when
/// the timer fires.
pub(crate) const TIMEOUT_PAD: Duration = Duration::from_millis(50);

/// Phases of a full-window web page handing over from its CPU 2D canvas to WebGPU/WebGL2.
///
/// Throughout `Building`, `WaitingChecks`, and `WaitingDone`, the visible CPU canvas continues
/// answering input and animating so motion never freezes. Zero GPU render passes are submitted
/// until the initial queue barrier (`built`) and all async browser error-scope checks
/// (`uber_check` and lazy feature `breakers`) have settled. Once settled, one hand-over frame is
/// submitted (`WaitingDone`) and the loop waits for `onSubmittedWorkDone` or `FINISH_LIMIT` while
/// continuing to paint CPU frames (and submitting zero additional GPU frames). Once
/// `onSubmittedWorkDone` resolves, `ReadyToFlip` requests one `requestAnimationFrame` that draws
/// an up-to-date GPU frame and atomically reveals the GPU canvas while removing the CPU canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandoverState {
    /// Waiting for the initial empty-queue barrier (`built`) after `Renderer::try_new`, or ready
    /// to precheck features and submit the single hand-over frame once `built` is true.
    Building { built: bool },
    /// The initial queue barrier finished (`built == true`), and one or more async browser
    /// error-scope checks (`uber_check` or lazy feature `breakers`) are pending. No GPU frames
    /// are submitted until a browser check wake clears `checking_gpu`.
    WaitingChecks,
    /// The single hand-over GPU frame was submitted; waiting for `onSubmittedWorkDone` or
    /// `deadline` (`FINISH_LIMIT`). CPU frames continue painting into the visible CPU canvas so
    /// animations, scroll, and input never freeze, while zero additional GPU frames are submitted.
    WaitingDone { attempt: Attempt, deadline: Instant },
    /// `onSubmittedWorkDone` resolved for the hand-over frame; waiting for the next rAF to draw
    /// an up-to-date GPU frame and atomically swap canvas visibility.
    ReadyToFlip,
    /// Hand-over completed: GPU canvas is visible and CPU canvas is removed.
    Done,
    /// Hand-over timed out waiting for the GPU frame: GPU destroyed and page stays on CPU.
    Failed,
}

impl Default for HandoverState {
    fn default() -> Self {
        Self::Building { built: false }
    }
}

/// Inputs that advance [`HandoverState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandoverEvent {
    /// The initial empty-queue `onSubmittedWorkDone` after `Renderer::try_new` resolved.
    Built { checking_gpu: bool },
    /// An async browser error-scope check (`uber_check` or feature breaker) resolved.
    ChecksSettled { still_checking: bool },
    /// A frame is preparing to draw while handing over.
    PrepareFrame { checking_gpu: bool },
    /// `PrecheckFeatures` finished scanning the scene's layer features.
    Prechecked { checking_gpu: bool },
    /// One GPU frame was submitted during hand-over.
    ///
    /// Normally `checking_gpu` is `false` (ready to wait for `onSubmittedWorkDone`). If a custom
    /// compositor triggered an unexpected lazy check during encoding, `checking_gpu` is `true`
    /// and the machine returns to `WaitingChecks` without submitting further GPU frames until
    /// that check answers.
    GpuSubmitted { checking_gpu: bool, now: Instant },
    /// `onSubmittedWorkDone` resolved for the submitted hand-over frame `attempt`.
    GpuDone { attempt: Attempt },
    /// Timeout check at `now` (from the `FINISH_LIMIT` timer or frame entry).
    Tick { now: Instant },
}

/// Side effects the web host performs for a [`HandoverState`] transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandoverEffect {
    /// Precheck lazy layer features (`Isolation`, `PlusLighter`, `LayerBlur`) for the current
    /// instances without acquiring a surface texture or submitting a GPU frame.
    PrecheckFeatures,
    /// Acquire a canvas texture, encode, and submit the single hand-over verification GPU frame
    /// (while keeping the CPU canvas visible and painting).
    DrawGpuFrame,
    /// Acquire a canvas texture, encode, and submit the up-to-date GPU frame on the flip rAF so
    /// visibility swaps atomically in the same compositor commit.
    DrawFlipFrame,
    /// Arm `onSubmittedWorkDone` and the `FINISH_LIMIT` timeout timer for `attempt`.
    ArmFinishWait { attempt: Attempt, delay: Duration },
    /// Request one redraw so the host advances to the hand-over or flip frame now that waiting ended.
    RequestRedraw,
    /// Reveal the GPU canvas and remove the CPU canvas (`web.handover.done`).
    Complete,
    /// Destroy the GPU device and stay on the CPU canvas (`web.handover.failed`).
    FailTimeout,
}

impl HandoverState {
    /// Whether the hand-over GPU frame is in flight and awaiting `onSubmittedWorkDone`.
    pub(crate) const fn waiting_for_gpu(self) -> bool {
        matches!(self, Self::WaitingDone { .. })
    }

    /// Advances the hand-over state machine on `event`, returning the effects to execute.
    pub(crate) fn step(&mut self, seq: &mut Seq, event: HandoverEvent) -> Effects<HandoverEffect> {
        match (*self, event) {
            (Self::Building { built: false }, HandoverEvent::Built { checking_gpu: true }) => {
                *self = Self::WaitingChecks;
                Effects::none()
            }
            (Self::Building { built: false }, HandoverEvent::Built { checking_gpu: false }) => {
                *self = Self::Building { built: true };
                Effects::one(HandoverEffect::RequestRedraw)
            }
            (Self::WaitingChecks, HandoverEvent::ChecksSettled { still_checking: false }) => {
                *self = Self::Building { built: true };
                Effects::one(HandoverEffect::RequestRedraw)
            }
            (Self::Building { built: true } | Self::ReadyToFlip, HandoverEvent::PrepareFrame { checking_gpu: true }) => {
                *self = Self::WaitingChecks;
                Effects::none()
            }
            (Self::Building { built: true } | Self::ReadyToFlip, HandoverEvent::PrepareFrame { checking_gpu: false }) => {
                Effects::one(HandoverEffect::PrecheckFeatures)
            }
            (Self::WaitingChecks, HandoverEvent::PrepareFrame { checking_gpu: false }) => {
                *self = Self::Building { built: true };
                Effects::one(HandoverEffect::PrecheckFeatures)
            }
            (Self::Building { built: true } | Self::ReadyToFlip, HandoverEvent::Prechecked { checking_gpu: true }) => {
                *self = Self::WaitingChecks;
                Effects::none()
            }
            (Self::Building { built: true }, HandoverEvent::Prechecked { checking_gpu: false }) => {
                Effects::one(HandoverEffect::DrawGpuFrame)
            }
            (Self::ReadyToFlip, HandoverEvent::Prechecked { checking_gpu: false }) => {
                Effects::one(HandoverEffect::DrawFlipFrame)
            }
            (Self::Building { built: true } | Self::ReadyToFlip, HandoverEvent::GpuSubmitted { checking_gpu: true, .. }) => {
                *self = Self::WaitingChecks;
                Effects::none()
            }
            (Self::Building { built: true }, HandoverEvent::GpuSubmitted { checking_gpu: false, now }) => {
                let attempt = seq.next();
                *self = Self::WaitingDone { attempt, deadline: now + FINISH_LIMIT };
                Effects::one(HandoverEffect::ArmFinishWait {
                    attempt,
                    delay: FINISH_LIMIT + TIMEOUT_PAD,
                })
            }
            (Self::ReadyToFlip, HandoverEvent::GpuSubmitted { checking_gpu: false, .. }) => {
                *self = Self::Done;
                Effects::one(HandoverEffect::Complete)
            }
            (Self::WaitingDone { attempt: expected, .. }, HandoverEvent::GpuDone { attempt })
                if attempt == expected =>
            {
                *self = Self::ReadyToFlip;
                Effects::one(HandoverEffect::RequestRedraw)
            }
            (Self::WaitingDone { deadline, .. }, HandoverEvent::Tick { now }) if now >= deadline => {
                *self = Self::Failed;
                Effects::one(HandoverEffect::FailTimeout)
            }
            _ => Effects::none(),
        }
    }
}

/// Explicit state machine driving non-blocking WebGL2 `Device::poll(PollType::Poll)` on
/// `requestAnimationFrame` turns until an async wgpu callback (`on_submitted_work_done` or
/// `map_async`) completes, fails, is cancelled, or reaches `deadline`.
///
/// Always waits for at least one `requestAnimationFrame` before the first poll because WebGL2
/// sync objects only transition to `SIGNALED` across browser turns, and polling on rAF keeps
/// handover completion phase-locked to display vsync instead of drifting on a fixed timer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GlSyncPoll {
    #[default]
    Idle,
    WaitingFrame { attempt: Attempt, deadline: Instant },
    Polling { attempt: Attempt, deadline: Instant },
    Done,
}

/// Inputs that advance [`GlSyncPoll`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlSyncPollEvent {
    /// Arm rAF-driven polling for a newly submitted WebGL2 callback until `deadline`.
    Start { deadline: Instant },
    /// The scheduled `requestAnimationFrame` callback for `attempt` ran at `now`.
    AnimationFrame { attempt: Attempt, now: Instant },
    /// `device.poll(wgpu::PollType::Poll)` finished for `attempt` at `now`.
    Polled { attempt: Attempt, completed: bool, device_error: bool, now: Instant },
    /// The awaited callback already resolved or its barrier was dropped.
    Cancel,
}

/// Side effects the web host performs for a [`GlSyncPoll`] transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlSyncPollEffect {
    /// Register a `requestAnimationFrame` callback tagged with `attempt`.
    RequestAnimationFrame { attempt: Attempt },
    /// Run one non-blocking `device.poll(wgpu::PollType::Poll)` and feed `Polled` back in.
    PollDevice { attempt: Attempt },
}

impl GlSyncPoll {
    /// Advances the WebGL2 sync polling state machine on `event`.
    pub(crate) fn step(&mut self, seq: &mut Seq, event: GlSyncPollEvent) -> Effects<GlSyncPollEffect> {
        match (*self, event) {
            (Self::Idle, GlSyncPollEvent::Start { deadline }) => {
                let attempt = seq.next();
                *self = Self::WaitingFrame { attempt, deadline };
                Effects::one(GlSyncPollEffect::RequestAnimationFrame { attempt })
            }
            (Self::WaitingFrame { attempt: expected, deadline }, GlSyncPollEvent::AnimationFrame { attempt, now })
                if attempt == expected =>
            {
                if now >= deadline {
                    *self = Self::Done;
                    Effects::none()
                } else {
                    *self = Self::Polling { attempt, deadline };
                    Effects::one(GlSyncPollEffect::PollDevice { attempt })
                }
            }
            (Self::Polling { attempt: expected, deadline }, GlSyncPollEvent::Polled { attempt, completed, device_error, now })
                if attempt == expected =>
            {
                if completed || device_error || now >= deadline {
                    *self = Self::Done;
                    Effects::none()
                } else {
                    let next = seq.next();
                    *self = Self::WaitingFrame { attempt: next, deadline };
                    Effects::one(GlSyncPollEffect::RequestAnimationFrame { attempt: next })
                }
            }
            (Self::WaitingFrame { .. } | Self::Polling { .. }, GlSyncPollEvent::Cancel) => {
                *self = Self::Done;
                Effects::none()
            }
            _ => Effects::none(),
        }
    }
}

/// Whether the web frame loop should immediately request another rAF after settling a frame.
///
/// Async GPU error-scope checks (`Renderer::checking_gpu`) wake the event loop directly via
/// `Wake::GpuCheck` when `popErrorScope()` resolves and must never spin rAF while idle.
pub(crate) const fn web_wants_frame(
    animating: bool,
    pending_input: bool,
    live_pending: bool,
    comp_animating: bool,
) -> bool {
    animating || pending_input || live_pending || comp_animating
}

/// Whether an already-handed-over `Draw::Gpu` page should request one redraw when a browser
/// error-scope check wake (`Wake::GpuCheck`) arrives.
///
/// Redraws once all pending checks have cleared and either a lazy feature breaker just settled
/// (switching from its fallback draw to the verified pipeline) or `last_presented` was left empty
/// while a check was in flight.
pub(crate) const fn should_redraw_after_gpu_check(
    checking_gpu: bool,
    breaker_settled: bool,
    has_last_presented: bool,
) -> bool {
    !checking_gpu && (breaker_settled || !has_last_presented)
}

#[cfg(test)]
#[path = "handover_machine_tests.rs"]
mod tests;
