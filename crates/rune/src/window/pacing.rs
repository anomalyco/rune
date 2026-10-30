//! Pacing for frames no display tick drives: timer deadlines, a hidden host's refresh pace,
//! and what follows a frame whose drawable never came.

use super::*;

/// The runtime's next timer, on the wall clock that started with the host.
pub(crate) fn wall_wake_deadline<S>(rt: &Runtime<S>) -> Option<Instant> {
    rt.wall_start?.checked_add(Duration::try_from_secs_f64(rt.wake_deadline()?).ok()?)
}

/// The runtime's next timer as a host with no display link to take it waits for it: hidden,
/// covered, faded out or unpaced. A timer due within a frame interval of the last poll
/// (`Tx::wake_after(Duration::ZERO)` asks for the next frame) waits for that interval, so a
/// poll that keeps asking steps at most once per `TIMER_FRAME` and never spins the loop.
pub(crate) fn timer_deadline<S>(rt: &Runtime<S>) -> Option<Instant> {
    let due = wall_wake_deadline(rt)?;
    let polled = rt.wall_start.and_then(|t0| t0.checked_add(Duration::try_from_secs_f64(rt.polled_at()).ok()?));
    Some(polled.map_or(due, |polled| due.max(polled + TIMER_FRAME)))
}

/// Paces the frame a hidden host keeps ready to show (a launcher's cached pixels, a retained
/// overlay's commit). A change after a quiet spell renders at once; later ones wait until
/// `HIDDEN_REFRESH` has passed since the last render, so state that changes many times a
/// second costs a few builds a second while nobody sees them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HiddenPace { last: Option<Instant>, waiting: bool }

pub(crate) const HIDDEN_REFRESH: Duration = Duration::from_millis(250);

impl HiddenPace {
    /// Whether a change at `now` renders now. Otherwise it waits for `deadline`.
    pub(crate) fn changed(&mut self, now: Instant) -> bool {
        self.waiting = !self.ready(now);
        !self.waiting
    }
    /// Whether a change at `now` would render at once.
    pub(crate) fn ready(&self, now: Instant) -> bool {
        self.last.is_none_or(|last| now >= last + HIDDEN_REFRESH)
    }
    /// When the change that waits renders.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.last.filter(|_| self.waiting).map(|last| last + HIDDEN_REFRESH)
    }
    /// Whether the change that waits is due at `now`; it stops waiting either way it renders.
    pub(crate) fn take_due(&mut self, now: Instant) -> bool {
        let due = self.deadline().is_some_and(|at| at <= now);
        if due { self.waiting = false; }
        due
    }
    /// A hidden frame rendered what had changed.
    pub(crate) fn rendered(&mut self, now: Instant) {
        *self = HiddenPace { last: Some(now), waiting: false };
    }
    /// Shown, the host draws its own frames; hidden again, the first change renders at once.
    pub(crate) fn reset(&mut self) { *self = HiddenPace::default(); }
}

/// Whether a timer asks for the frame after one just presented. Only a surface whose display
/// link paces it (`next_frame` is its next tick) takes a timer due by then as a frame request;
/// anywhere else the timer waits in `timer_deadline`, so it never turns the loop by itself.
pub(crate) fn timer_wants_frame<S>(rt: &Runtime<S>, next_frame: Option<Instant>) -> bool {
    next_frame.is_some_and(|next| wall_wake_deadline(rt).is_some_and(|due| due <= next))
}

/// Why a surface gave no drawable this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Missed {
    /// Nothing of the window is on screen (covered, or its displays asleep).
    Occluded,
    /// No drawable was free, or acquisition failed some other passing way.
    Timeout,
    /// Outdated or lost, and reconfiguring didn't bring it back.
    Outdated,
}

/// How a host waits after a frame it couldn't draw. There is no `ControlFlow::Poll` here:
/// retrying at once is what spins a core while a window is occluded or its displays sleep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NextFrame {
    WaitUntil(Instant),
    /// Sleep until an event: a display tick, a notification, input or a timer.
    Idle,
}

/// The first re-check of an occluded surface. Its occlusion notification usually redraws it
/// first; this catches one that never arrives. Consecutive misses back off to `OCCLUDED_MAX`.
pub(crate) const OCCLUDED_RETRY: Duration = Duration::from_millis(250);
pub(crate) const OCCLUDED_MAX: Duration = Duration::from_secs(2);
/// An unpaced surface's retry after a drawable timeout: about one frame.
pub(crate) const TIMEOUT_RETRY: Duration = Duration::from_millis(16);
/// Motion's frame interval when no display link paces a surface.
pub(crate) const TIMER_FRAME: Duration = Duration::from_micros(16_667);

impl Missed {
    pub(super) fn of(status: wgpu::CurrentSurfaceTexture) -> Result<wgpu::SurfaceTexture, Missed> {
        use wgpu::CurrentSurfaceTexture as S;
        match status {
            S::Success(f) | S::Suboptimal(f) => Ok(f),
            S::Occluded => Err(Missed::Occluded),
            S::Outdated | S::Lost => Err(Missed::Outdated),
            S::Timeout | S::Validation => Err(Missed::Timeout),
        }
    }

    /// What follows a frame this surface couldn't draw. A display link that is still running
    /// retries a timeout on its next tick. `occluded_wait` is the caller's backoff, held to
    /// `OCCLUDED_RETRY..=OCCLUDED_MAX`.
    pub(crate) fn next(self, paced: bool, now: Instant, occluded_wait: Duration) -> NextFrame {
        match self {
            Missed::Occluded => NextFrame::WaitUntil(now + occluded_wait.clamp(OCCLUDED_RETRY, OCCLUDED_MAX)),
            Missed::Timeout if paced => NextFrame::Idle,
            Missed::Timeout => NextFrame::WaitUntil(now + TIMEOUT_RETRY),
            Missed::Outdated => NextFrame::WaitUntil(now + OCCLUDED_RETRY),
        }
    }
}

/// The next drawable. An outdated or lost surface is reconfigured and tried again, up to
/// `reconfigures` times, synchronously; every other miss returns at once. On macOS a surface
/// wgpu would call occluded is never asked: each failed `get_current_texture` leaks a texture
/// id in wgpu-core 30.0.1.
pub(crate) fn acquire(surface: &wgpu::Surface, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration, reconfigures: u32,
    meter: &mut SurfaceMeter) -> Result<wgpu::SurfaceTexture, Missed> {
    acquire_checked(surface, device, config, reconfigures, meter, true)
}

/// `acquire`, skipping the occlusion check when the caller has just read the window hosting
/// the surface as visible (`check_occlusion` false).
pub(crate) fn acquire_checked(surface: &wgpu::Surface, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration, mut reconfigures: u32,
    meter: &mut SurfaceMeter, check_occlusion: bool) -> Result<wgpu::SurfaceTexture, Missed> {
    #[cfg(not(target_os = "macos"))]
    let _ = check_occlusion;
    #[cfg(target_os = "macos")]
    if check_occlusion && crate::macos::surface_occluded(surface) {
        meter.skipped();
        return Err(Missed::Occluded);
    }
    loop {
        let status = Missed::of(surface.get_current_texture());
        if let Err(missed) = status { meter.failed(missed); }
        match status {
            Err(Missed::Outdated) if reconfigures > 0 => {
                surface.configure(device, config);
                reconfigures -= 1;
            }
            Err(Missed::Outdated) => {
                surface.configure(device, config);
                return Err(Missed::Outdated);
            }
            other => return other,
        }
    }
}
