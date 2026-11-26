//! Shared host frame sequencer: one definition of the order every host (window, overlay, Apple
//! layer, headless, virtual desktop, and web) drives a frame through.
//!
//! The sequence is governed by an explicit phase state machine ([`Phase`], [`Event`],
//! [`Effect`], [`step`]) so every ordering invariant — acquiring a drawable before stepping on
//! normal frames, stepping before acquiring on quiet timer frames without double-stepping on a
//! miss, dropping the old drawable before post-step reconfiguration and re-acquisition, and
//! reporting presentation to [`FrameClock`] and interaction timing — is enforced in one place.
//! Each host supplies only its host-specific work through [`FrameHost`].

use crate::frame::clock::FrameClock;
use crate::machine::Effects;
use crate::render::{Gpu, Renderer};
use crate::runtime::RuntimeCore;
use crate::time::Instant;
use crate::window::Missed;

/// Final outcome of one [`run`] call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The gate declined to draw (covered, occluded, suspended, backed off, waiting for a swap
    /// chain slot, or display-link Wait).
    Skipped,
    /// A resting timer frame built the same pixels already on screen and skipped presentation.
    Unchanged,
    /// Acquisition, surface reconfiguration, or a forced miss ended the frame without presenting.
    Missed,
    /// Rendering could not prepare its target (e.g. a CPU-boot frame needs the GPU or lacked a
    /// buffer, or the web host rejected unsupported native surfaces) and deferred to the host's
    /// recovery path.
    #[cfg(any(test, target_os = "windows", target_os = "linux", target_arch = "wasm32"))]
    Aborted,
    /// A frame was drawn, submitted/presented, recorded on the clock and interaction tracker, and
    /// settled.
    Presented,
}

/// Which frame path [`FrameHost::gate`] selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GateKind {
    Skip,
    Draw,
    QuietTimer,
    PreStepped,
}

/// Decision returned by [`FrameHost::gate`] before touching the surface.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Gate {
    /// Do not draw a frame this turn.
    Skip,
    /// Normal acquire-before-step frame.
    Draw,
    /// Resting window or overlay woken only by a due timer: step first, and skip acquisition
    /// and presentation when nothing visible changed.
    QuietTimer,
    /// Multi-window tooling host whose runtime step already ran before compositing visible
    /// windows in z-order.
    PreStepped(StepInfo),
}

impl Gate {
    #[inline(always)]
    fn kind(&self) -> GateKind {
        match self {
            Gate::Skip => GateKind::Skip,
            Gate::Draw => GateKind::Draw,
            Gate::QuietTimer => GateKind::QuietTimer,
            Gate::PreStepped(_) => GateKind::PreStepped,
        }
    }
}

/// Result of [`FrameHost::acquire`].
#[derive(Debug)]
pub(crate) enum Acquire<D> {
    Ready(D),
    Missed(Missed),
}

/// Why an acquired drawable must be dropped and re-acquired after the runtime stepped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReacquireReason {
    /// Native system materials (e.g. macOS visual effect views) turned on during the step; the
    /// host rebuilds with native materials enabled before re-acquiring.
    #[cfg(any(test, target_os = "macos", all(target_os = "windows", feature = "windows-system-material")))]
    NativeMaterials,
    /// A handler changed `Tx::set_srgb_blend`; the surface and renderer reconfigure for the new
    /// target format before re-acquiring.
    Reblend,
    /// A content-sized overlay's committed layout changed height; resize, re-place, and
    /// reconfigure its surface before re-acquiring so the new content presents in the same frame.
    #[cfg(not(target_arch = "wasm32"))]
    ContentResize,
}

/// Result of [`FrameHost::post_step_check`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PostStepCheck {
    /// Keep the current drawable and proceed to sync and render.
    Keep,
    /// Drop the current drawable, reconfigure for `ReacquireReason`, and acquire a replacement.
    Reacquire(ReacquireReason),
    /// Drop the current drawable and treat the frame as missed (used by diagnostic miss hooks).
    #[cfg(any(test, feature = "frame-profile"))]
    Missed(Missed),
    /// Drop the current drawable without presenting or backing off (Wayland layer-shell
    /// content resize waiting for the compositor's matching `configure`).
    #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    Hold,
}

/// Result of [`FrameHost::prepare_render`].
#[derive(Debug)]
pub(crate) enum RenderPrep<P> {
    Ready(P),
    #[cfg(any(test, target_os = "windows", target_os = "linux", target_arch = "wasm32"))]
    Abort,
}

/// Timing and motion summary carried from the runtime step into sync, render, present and settle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StepInfo {
    /// Whether the runtime itself (before any compositor pass) reported motion still running.
    pub(crate) runtime_animating: bool,
    /// Whether either the runtime, native layers, or a compositor reported motion still running.
    pub(crate) animating: bool,
    /// Seconds stepped for this frame.
    pub(crate) dt: f64,
    /// CPU milliseconds spent updating and laying out the frame before rendering.
    pub(crate) update_ms: f64,
    /// Display-link target presentation time, when paced.
    pub(crate) presents_at: Option<Instant>,
}

/// Explicit phase of a single host frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Start,
    Gating,
    FrameMoving,
    PreReblending { stepped: bool },
    QuietStepping,
    Acquiring { stepped: bool },
    Stepping,
    PostStepChecking { reacquired: u8 },
    Reacquiring { reacquired: u8 },
    Syncing,
    Preparing,
    Rendering,
    Presenting,
    RecordingPresented,
    Settling,
    Done(Outcome),
}

impl Phase {
    #[inline(always)]
    fn outcome(self) -> Outcome {
        match self {
            Phase::Done(outcome) => outcome,
            other => unreachable!("host_step ended in non-terminal phase {other:?}"),
        }
    }
}

/// Events fed into [`step`] as each host hook finishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Begin,
    Gate(GateKind),
    FrameMoved,
    ReblendOk,
    ReblendFailed,
    QuietUnchanged,
    QuietChanged,
    Acquired,
    AcquireMissed(Missed),
    Stepped,
    PostStepKeep,
    PostStepReacquire(ReacquireReason),
    #[cfg(any(test, feature = "frame-profile"))]
    PostStepMissed(Missed),
    #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    PostStepHold,
    Reacquired,
    ReacquireMissed(Missed),
    Synced,
    Prepared,
    #[cfg(any(test, target_os = "windows", target_os = "linux", target_arch = "wasm32"))]
    PrepareAborted,
    Rendered,
    Presented,
    Recorded,
    Settled,
}

/// Coarse host action returned by [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    Gate,
    FrameMove,
    PreReblend,
    QuietStep,
    Unchanged,
    Acquire,
    Step,
    PostStepCheck,
    DropAndReacquire(ReacquireReason),
    #[cfg(any(test, feature = "frame-profile"))]
    DropAndMiss(Missed),
    #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    DropAndHold,
    Missed { missed: Missed, stepped: bool },
    Sync,
    Prepare,
    Render,
    Present,
    RecordPresented,
    Settle,
}

/// Pure transition function for a single host frame.
///
/// The `stepped` flag carried by `PreReblending` and `Acquiring` guarantees that a frame missed
/// after a quiet pre-step or after a post-step reconfiguration never runs the runtime a second
/// time. `PostStepChecking` caps re-acquisition to two passes (native materials and blend switch).
#[inline(always)]
pub(crate) fn step(phase: &mut Phase, event: Event) -> Effects<Effect> {
    match (*phase, event) {
        (Phase::Start, Event::Begin) => {
            *phase = Phase::Gating;
            Effects::one(Effect::Gate)
        }
        (Phase::Gating, Event::Gate(GateKind::Skip)) => {
            *phase = Phase::Done(Outcome::Skipped);
            Effects::none()
        }
        (Phase::Gating, Event::Gate(GateKind::Draw)) => {
            *phase = Phase::FrameMoving;
            Effects::one(Effect::FrameMove)
        }
        (Phase::Gating, Event::Gate(GateKind::QuietTimer)) => {
            *phase = Phase::QuietStepping;
            Effects::one(Effect::QuietStep)
        }
        (Phase::Gating, Event::Gate(GateKind::PreStepped)) => {
            *phase = Phase::PreReblending { stepped: true };
            Effects::one(Effect::PreReblend)
        }
        (Phase::FrameMoving, Event::FrameMoved) => {
            *phase = Phase::PreReblending { stepped: false };
            Effects::one(Effect::PreReblend)
        }
        (Phase::PreReblending { stepped }, Event::ReblendOk) => {
            *phase = Phase::Acquiring { stepped };
            Effects::one(Effect::Acquire)
        }
        (Phase::PreReblending { stepped }, Event::ReblendFailed) => {
            *phase = Phase::Done(Outcome::Missed);
            Effects::one(Effect::Missed { missed: Missed::Unconfigured, stepped })
        }
        (Phase::QuietStepping, Event::QuietUnchanged) => {
            *phase = Phase::Done(Outcome::Unchanged);
            Effects::one(Effect::Unchanged)
        }
        (Phase::QuietStepping, Event::QuietChanged) => {
            *phase = Phase::Acquiring { stepped: true };
            Effects::one(Effect::Acquire)
        }
        (Phase::Acquiring { stepped: false }, Event::Acquired) => {
            *phase = Phase::Stepping;
            Effects::one(Effect::Step)
        }
        (Phase::Acquiring { stepped: true }, Event::Acquired) => {
            *phase = Phase::PostStepChecking { reacquired: 0 };
            Effects::one(Effect::PostStepCheck)
        }
        (Phase::Acquiring { stepped }, Event::AcquireMissed(missed)) => {
            *phase = Phase::Done(Outcome::Missed);
            Effects::one(Effect::Missed { missed, stepped })
        }
        (Phase::Stepping, Event::Stepped) => {
            *phase = Phase::PostStepChecking { reacquired: 0 };
            Effects::one(Effect::PostStepCheck)
        }
        (Phase::PostStepChecking { .. }, Event::PostStepKeep) => {
            *phase = Phase::Syncing;
            Effects::one(Effect::Sync)
        }
        (Phase::PostStepChecking { reacquired }, Event::PostStepReacquire(reason)) if reacquired < 2 => {
            *phase = Phase::Reacquiring { reacquired: reacquired + 1 };
            Effects::one(Effect::DropAndReacquire(reason))
        }
        (Phase::PostStepChecking { .. }, Event::PostStepReacquire(_)) => {
            *phase = Phase::Syncing;
            Effects::one(Effect::Sync)
        }
        #[cfg(any(test, feature = "frame-profile"))]
        (Phase::PostStepChecking { .. }, Event::PostStepMissed(missed)) => {
            *phase = Phase::Done(Outcome::Missed);
            Effects::one(Effect::DropAndMiss(missed))
        }
        #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        (Phase::PostStepChecking { .. }, Event::PostStepHold) => {
            *phase = Phase::Done(Outcome::Skipped);
            Effects::one(Effect::DropAndHold)
        }
        (Phase::Reacquiring { reacquired }, Event::Reacquired) => {
            *phase = Phase::PostStepChecking { reacquired };
            Effects::one(Effect::PostStepCheck)
        }
        (Phase::Reacquiring { .. }, Event::ReacquireMissed(missed)) => {
            *phase = Phase::Done(Outcome::Missed);
            Effects::one(Effect::Missed { missed, stepped: true })
        }
        (Phase::Syncing, Event::Synced) => {
            *phase = Phase::Preparing;
            Effects::one(Effect::Prepare)
        }
        (Phase::Preparing, Event::Prepared) => {
            *phase = Phase::Rendering;
            Effects::one(Effect::Render)
        }
        #[cfg(any(test, target_os = "windows", target_os = "linux", target_arch = "wasm32"))]
        (Phase::Preparing, Event::PrepareAborted) => {
            *phase = Phase::Done(Outcome::Aborted);
            Effects::none()
        }
        (Phase::Rendering, Event::Rendered) => {
            *phase = Phase::Presenting;
            Effects::one(Effect::Present)
        }
        (Phase::Presenting, Event::Presented) => {
            *phase = Phase::RecordingPresented;
            Effects::one(Effect::RecordPresented)
        }
        (Phase::RecordingPresented, Event::Recorded) => {
            *phase = Phase::Settling;
            Effects::one(Effect::Settle)
        }
        (Phase::Settling, Event::Settled) => {
            *phase = Phase::Done(Outcome::Presented);
            Effects::none()
        }
        (state, ev) => unreachable!("invalid host_step transition: {state:?} + {ev:?}"),
    }
}

/// Host-specific callbacks driven in one shared order by [`run`].
pub(crate) trait FrameHost {
    /// Acquired drawable handle held across the step and dropped before any post-step
    /// reconfiguration/re-acquisition. Zero-sized `()` for offscreen and CPU hosts.
    type Drawable;
    /// Prepared render state passed from [`Self::prepare_render`] through [`Self::render`] to
    /// [`Self::submit_and_present`].
    type Prepared;

    /// Decides whether a frame proceeds and which path it takes (normal draw, quiet resting timer,
    /// pre-stepped multi-window render, or skip).
    fn gate(&mut self) -> Gate;

    /// Moves an animated window frame before surface reconfiguration and drawable acquisition.
    #[inline]
    fn before_acquire(&mut self) {}

    /// Whether the surface/renderer blend mode or format needs updating before acquisition.
    #[inline]
    fn needs_reblend(&self) -> bool { false }

    /// Reconfigures the surface and rebuilds the renderer to match `rt.srgb_blend()`. Returns
    /// `false` when the surface refused the new configuration.
    #[inline]
    fn reblend(&mut self) -> bool { true }

    /// Steps a resting timer frame before acquiring a drawable (only called after
    /// [`Gate::QuietTimer`]).
    #[inline]
    fn step_quiet(&mut self) -> StepInfo {
        unreachable!("host returned Gate::QuietTimer without implementing step_quiet")
    }

    /// Whether the quiet timer frame produced identical pixels and needs no presentation.
    #[inline]
    fn is_unchanged(&mut self, _info: &StepInfo) -> bool { false }

    /// Syncs platform state and stops pacing when a quiet timer frame skips presentation.
    #[inline]
    fn on_unchanged(&mut self, _info: &StepInfo) {}

    /// Prepares pacing or present-worker state once a quiet timer frame discovers it must draw.
    #[inline]
    fn on_quiet_will_draw(&mut self) {}

    /// Acquires the next drawable.
    fn acquire(&mut self) -> Acquire<Self::Drawable>;

    /// Advances the frame clock and runs the runtime pass for this frame.
    fn step_runtime(&mut self) -> StepInfo;

    /// Checks after stepping whether the acquired drawable must be replaced (macOS native
    /// materials or `Tx::set_srgb_blend`) or treated as missed.
    #[inline]
    fn post_step_check(&mut self, _info: &mut StepInfo) -> PostStepCheck { PostStepCheck::Keep }

    /// Rebuilds or reconfigures after [`run`] has dropped the old drawable, right before the
    /// replacement [`Self::acquire`]. Returning `false` ends the frame with
    /// `Missed::Unconfigured`.
    #[inline]
    fn prepare_reacquire(&mut self, _reason: ReacquireReason, _info: &mut StepInfo) -> bool { true }

    /// Called immediately after [`run`] drops an acquired drawable early (before post-step
    /// reacquisition, diagnostic miss, or Wayland content-resize hold) so hosts can release
    /// frame-texture tracking before any surface teardown or reconfiguration.
    #[inline]
    fn on_drawable_dropped(&mut self) {}

    /// Handles a missed acquire or failed reconfiguration. `stepped` is `true` iff the runtime
    /// already stepped for this frame, so the host must not step it a second time.
    #[inline]
    fn on_missed(&mut self, _missed: Missed, _stepped: bool) {}

    /// Syncs host state and requests after stepping (clipboard, IME, cursor, secure input, help,
    /// hit regions, unsupported request dropping, headless popups/placement).
    #[inline]
    fn sync_after_step(&mut self, _info: &mut StepInfo) {}

    /// Uploads images, prepares native surfaces, and creates the view and command encoder.
    fn prepare_render(&mut self, drawable: Self::Drawable, info: &mut StepInfo) -> RenderPrep<Self::Prepared>;

    /// Encodes either the app's custom compositor or the default UI pass. Returns `true` when a
    /// compositor reported that it is still animating.
    fn render(&mut self, prepared: &mut Self::Prepared, info: &StepInfo) -> bool;

    /// Submits GPU work, presents the drawable, performs any post-present GPU sync, and returns
    /// the submission timestamp recorded on the frame clock and interaction tracker.
    fn submit_and_present(&mut self, prepared: Self::Prepared, info: &StepInfo) -> Instant;

    /// Supplies the wall-clock [`FrameClock`] (when the host has one) and [`RuntimeCore`] so
    /// [`run`] itself records presentation on both the clock and interaction latency tracker.
    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore);

    /// Updates idle/presented state, frame reports, traces, and next-frame scheduling.
    #[inline]
    fn settle(&mut self, _submitted: Instant, _info: &StepInfo) {}
}

#[inline(always)]
fn run_pre_reblend<H: FrameHost>(phase: &mut Phase, host: &mut H) -> bool {
    let ok = !host.needs_reblend() || host.reblend();
    let event = if ok { Event::ReblendOk } else { Event::ReblendFailed };
    match step(phase, event) {
        Effects::One(Effect::Acquire) => true,
        Effects::One(Effect::Missed { missed, stepped }) => {
            host.on_missed(missed, stepped);
            false
        }
        _ => unreachable!(),
    }
}

#[inline(always)]
fn run_acquire<H: FrameHost>(phase: &mut Phase, host: &mut H) -> Option<H::Drawable> {
    match host.acquire() {
        Acquire::Ready(drawable) => {
            let _ = step(phase, Event::Acquired);
            Some(drawable)
        }
        Acquire::Missed(missed) => {
            let Effects::One(Effect::Missed { missed, stepped }) = step(phase, Event::AcquireMissed(missed)) else { unreachable!() };
            host.on_missed(missed, stepped);
            None
        }
    }
}

/// Runs one frame through the shared [`step`] state machine for `host`.
#[inline]
pub(crate) fn run<H: FrameHost>(host: &mut H) -> Outcome {
    let mut phase = Phase::Start;
    let Effects::One(Effect::Gate) = step(&mut phase, Event::Begin) else { unreachable!() };
    let gate = host.gate();
    let (mut drawable, mut info) = match step(&mut phase, Event::Gate(gate.kind())) {
        Effects::None => return phase.outcome(),
        Effects::One(Effect::FrameMove) => {
            host.before_acquire();
            let Effects::One(Effect::PreReblend) = step(&mut phase, Event::FrameMoved) else { unreachable!() };
            if !run_pre_reblend(&mut phase, host) {
                return phase.outcome();
            }
            let Some(drawable) = run_acquire(&mut phase, host) else {
                return phase.outcome();
            };
            debug_assert_eq!(phase, Phase::Stepping);
            let info = host.step_runtime();
            let Effects::One(Effect::PostStepCheck) = step(&mut phase, Event::Stepped) else { unreachable!() };
            (drawable, info)
        }
        Effects::One(Effect::QuietStep) => {
            let info = host.step_quiet();
            let event = if host.is_unchanged(&info) { Event::QuietUnchanged } else { Event::QuietChanged };
            match step(&mut phase, event) {
                Effects::One(Effect::Unchanged) => {
                    host.on_unchanged(&info);
                    return phase.outcome();
                }
                Effects::One(Effect::Acquire) => {
                    host.on_quiet_will_draw();
                    let Some(drawable) = run_acquire(&mut phase, host) else {
                        return phase.outcome();
                    };
                    (drawable, info)
                }
                _ => unreachable!(),
            }
        }
        Effects::One(Effect::PreReblend) => {
            let Gate::PreStepped(info) = gate else { unreachable!() };
            if !run_pre_reblend(&mut phase, host) {
                return phase.outcome();
            }
            let Some(drawable) = run_acquire(&mut phase, host) else {
                return phase.outcome();
            };
            (drawable, info)
        }
        _ => unreachable!(),
    };

    loop {
        let event = match host.post_step_check(&mut info) {
            PostStepCheck::Keep => Event::PostStepKeep,
            PostStepCheck::Reacquire(reason) => Event::PostStepReacquire(reason),
            #[cfg(any(test, feature = "frame-profile"))]
            PostStepCheck::Missed(missed) => Event::PostStepMissed(missed),
            #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
            PostStepCheck::Hold => Event::PostStepHold,
        };
        match step(&mut phase, event) {
            Effects::One(Effect::Sync) => break,
            Effects::One(Effect::DropAndReacquire(reason)) => {
                drop(drawable);
                host.on_drawable_dropped();
                let reacquired = if host.prepare_reacquire(reason, &mut info) {
                    host.acquire()
                } else {
                    Acquire::Missed(Missed::Unconfigured)
                };
                match reacquired {
                    Acquire::Ready(next) => {
                        drawable = next;
                        let Effects::One(Effect::PostStepCheck) = step(&mut phase, Event::Reacquired) else { unreachable!() };
                    }
                    Acquire::Missed(missed) => {
                        let Effects::One(Effect::Missed { missed, stepped }) = step(&mut phase, Event::ReacquireMissed(missed)) else { unreachable!() };
                        host.on_missed(missed, stepped);
                        return phase.outcome();
                    }
                }
            }
            #[cfg(any(test, feature = "frame-profile"))]
            Effects::One(Effect::DropAndMiss(missed)) => {
                drop(drawable);
                host.on_drawable_dropped();
                host.on_missed(missed, true);
                return phase.outcome();
            }
            #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
            Effects::One(Effect::DropAndHold) => {
                drop(drawable);
                host.on_drawable_dropped();
                return phase.outcome();
            }
            _ => unreachable!(),
        }
    }

    host.sync_after_step(&mut info);
    let Effects::One(Effect::Prepare) = step(&mut phase, Event::Synced) else { unreachable!() };
    let mut prepared = match host.prepare_render(drawable, &mut info) {
        RenderPrep::Ready(prepared) => {
            let Effects::One(Effect::Render) = step(&mut phase, Event::Prepared) else { unreachable!() };
            prepared
        }
        #[cfg(any(test, target_os = "windows", target_os = "linux", target_arch = "wasm32"))]
        RenderPrep::Abort => {
            let Effects::None = step(&mut phase, Event::PrepareAborted) else { unreachable!() };
            return phase.outcome();
        }
    };
    let comp_animating = host.render(&mut prepared, &info);
    info.animating |= comp_animating;
    let Effects::One(Effect::Present) = step(&mut phase, Event::Rendered) else { unreachable!() };
    let submitted = host.submit_and_present(prepared, &info);
    let Effects::One(Effect::RecordPresented) = step(&mut phase, Event::Presented) else { unreachable!() };
    {
        let (clock, rt) = host.present_targets();
        if let Some(clock) = clock {
            clock.presented_core(rt, submitted);
        }
        rt.interaction_presented(submitted, info.presents_at);
    }
    let Effects::One(Effect::Settle) = step(&mut phase, Event::Recorded) else { unreachable!() };
    host.settle(submitted, &info);
    let Effects::None = step(&mut phase, Event::Settled) else { unreachable!() };
    phase.outcome()
}

/// Uploads dirty images, optionally kicks off asynchronous native-surface pipeline compilation,
/// runs `before_surfaces` (for example Windows DirectComposition video layers), prepares native
/// surfaces, and registers the frame wake for off-thread pipeline builds.
#[inline]
pub(crate) fn prepare_gpu_resources_with(
    renderer: &mut Renderer,
    gpu: &Gpu,
    rt: &mut RuntimeCore,
    size: (u32, u32),
    async_surfaces: bool,
    before_surfaces: impl FnOnce(&mut Renderer, &RuntimeCore),
) {
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    if async_surfaces {
        renderer.build_surfaces(&gpu.device, &rt.surfaces, rt.live_notifier());
    }
    before_surfaces(renderer, rt);
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, size);
    if async_surfaces {
        renderer.set_frame_wake(rt.live_wake());
    }
}

/// Uploads dirty images and prepares native surfaces (with async build and frame wake when
/// `async_surfaces` is true).
#[inline]
pub(crate) fn prepare_gpu_resources(
    renderer: &mut Renderer,
    gpu: &Gpu,
    rt: &mut RuntimeCore,
    size: (u32, u32),
    async_surfaces: bool,
) {
    prepare_gpu_resources_with(renderer, gpu, rt, size, async_surfaces, |_, _| {});
}

/// Whether a frame on `backend` should run one non-blocking `Device::poll(Poll)` right after
/// `create_command_encoder` and before recording passes, retiring completed submissions from
/// earlier frames or sibling surfaces sharing the device.
///
/// Only live Vulkan devices poll here: wgpu 30 retires finished command buffers only inside
/// `Device::maintain`, and the draw path otherwise runs `maintain(Poll)` only at the end of
/// `Queue::submit` (after the next frame is already recorded), so back-to-back draws on a shared
/// Vulkan device keep extra command buffers live (growing Mesa ANV's non-shrinking binding-table
/// pool). D3D12 skips this to avoid Intel Iris Xe's post-poll cost, Metal uses Rune's native
/// backend, and lost devices skip it so `poll` is never entered on a known-dead device.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) const fn should_poll_after_encoder(backend: wgpu::Backend, lost: bool) -> bool {
    matches!(backend, wgpu::Backend::Vulkan) && !lost
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn create_frame_encoder_with<E>(
    backend: wgpu::Backend,
    lost: bool,
    create: impl FnOnce() -> E,
    poll: impl FnOnce() -> Result<wgpu::PollStatus, rune_gpu::health::PollFailed>,
    mark_lost: impl FnOnce(),
) -> E {
    let encoder = create();
    if should_poll_after_encoder(backend, lost)
        && matches!(poll(), Err(rune_gpu::health::PollFailed::Lost))
    {
        mark_lost();
    }
    encoder
}

/// Creates a frame command encoder and, on live Vulkan devices, runs one non-blocking
/// `Device::poll(Poll)` immediately afterward (before any passes are recorded) so completed
/// submissions release their command buffers back to wgpu's pool.
///
/// Taking the encoder before polling matters on NVIDIA Vulkan, where the preceding frame is often
/// still in flight when `prepare_render` starts and polling before `create_command_encoder`
/// causes per-submission command-buffer allocation churn.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn create_frame_encoder(gpu: &Gpu) -> wgpu::CommandEncoder {
    create_frame_encoder_with(
        gpu.backend(),
        gpu.is_lost(),
        || gpu.device.create_command_encoder(&Default::default()),
        || rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll),
        || gpu.mark_lost(),
    )
}

/// Default UI render pass when a host has no custom [`crate::Compositor`]: uploads glyph atlas
/// and instance buffers, then draws into `target` with `clear`.
#[inline]
pub(crate) fn draw_ui(
    renderer: &mut Renderer,
    gpu: &Gpu,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    rt: &mut RuntimeCore,
    size: (u32, u32),
    clear: [f32; 4],
) {
    renderer.set_fade_stops(rt.fade_stops());
    renderer.prepare_ui(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    renderer.draw(encoder, target, clear);
}

#[cfg(test)]
#[path = "host_step_tests.rs"]
mod tests;
