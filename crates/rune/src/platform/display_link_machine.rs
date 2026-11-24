//! Pure, clock-injected state machine and pacing rules for macOS `DisplayLink`.
//!
//! Kept free of CoreVideo / Objective-C so every transition and multi-second stream test runs
//! headless on every host with a deterministic clock.

use std::time::Duration;

use crate::frame::surface_stats::Pacing;
use crate::machine::Effects;
use crate::time::Instant;

/// Low Power Mode caps a built-in ProMotion panel at 60 Hz even when CoreVideo still reports
/// 120 Hz.
pub(crate) const LOW_POWER_FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// How long a running display link coasts after a visible window settles idle before stopping.
/// Matches Chromium's `ExternalBeginFrameSourceMac` (`kMaxKeepAliveCount = 20`) and
/// `ImageTransportSurfaceOverlayMacEGL` (`kMaxKeepAliveCounter = 8`).
pub(crate) const KEEP_ALIVE: Duration = Duration::from_millis(150);
pub(crate) const KEEP_ALIVE_MIN_TICKS: u8 = 8;
pub(crate) const KEEP_ALIVE_MAX_TICKS: u8 = 30;

/// Number of idle ticks the link keeps running (~150 ms) before stopping: ~9 at 60 Hz, ~18 at
/// 120 Hz, ~30 at 240 Hz.
pub(crate) fn keep_alive_ticks(period: Duration) -> u8 {
    let period_ns = period.as_nanos();
    if period_ns == 0 { return KEEP_ALIVE_MAX_TICKS; }
    let ticks = (KEEP_ALIVE.as_nanos() + period_ns / 2) / period_ns;
    ticks.clamp(u128::from(KEEP_ALIVE_MIN_TICKS), u128::from(KEEP_ALIVE_MAX_TICKS)) as u8
}

/// The display link's operating mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LinkState {
    /// CoreVideo's display link is stopped. No callbacks fire and no main-thread wakes arrive.
    #[default]
    Stopped,
    /// CoreVideo's display link is running and driving frames. Each tick wakes the main thread
    /// when no wake is already pending.
    Active,
    /// CoreVideo's display link stays running briefly after a visible frame settles idle so the
    /// next stream event (trackpad wheel delta, `NativeSurface` push) resumes without a cold
    /// `CVDisplayLinkStart`. Coasting ticks refresh the period and target without waking the main
    /// thread, drawing, or presenting; when `ticks_left` hits zero, one wake asks the main
    /// thread to stop the link (`CVDisplayLinkStop`, zero redraws).
    Coasting { ticks_left: u8 },
}

impl LinkState {
    /// Whether the display link actively drives redraws (false in `Coasting` and `Stopped`).
    pub(crate) fn active(self) -> bool { matches!(self, Self::Active) }

    /// Whether CoreVideo's underlying link is currently started (`Active` or `Coasting`).
    pub(crate) fn hardware_running(self) -> bool { !matches!(self, Self::Stopped) }
}

/// What happens to a display link's mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkEvent {
    /// A frame needs pacing (`DisplayLink::start` / `DisplayLink::next_frame`).
    Start,
    /// CoreVideo refused `CVDisplayLinkStart` after `LinkEffect::StartLink`.
    StartFailed,
    /// A visible window settled idle after presenting.
    Coast { period: Duration },
    /// One display tick fired while coasting.
    CoastTick,
    /// The main thread received a wake after coasting expired and checks whether to stop.
    StopIfExpired,
    /// Hard stop (occlusion, hide, minimize, missed frame/backoff, `Resized(0, 0)`, prewarm,
    /// device lost, or `Drop`).
    Stop,
}

/// What the caller carries out for a `LinkState` transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkEffect {
    /// Call `CVDisplayLinkStart`, recheck Low Power Mode, and arm active callback wakes.
    StartLink,
    /// Switch an already-running coasting link back to active callback wakes (no
    /// `CVDisplayLinkStart`).
    ResumeActive,
    /// Switch an already-running link to coasting for `ticks_left` ticks (no
    /// `CVDisplayLinkStop`, no redraws).
    ArmCoast { ticks_left: u8 },
    /// Send one main-thread wake so the host calls `StopIfExpired` (`CVDisplayLinkStop`, zero
    /// redraws).
    WakeToStop,
    /// Call `CVDisplayLinkStop` and clear callback state.
    StopLink,
}

/// Pure transition for `LinkState`.
pub(crate) fn step(state: &mut LinkState, event: LinkEvent) -> Effects<LinkEffect> {
    match (*state, event) {
        (LinkState::Stopped, LinkEvent::Start) => {
            *state = LinkState::Active;
            Effects::one(LinkEffect::StartLink)
        }
        (LinkState::Coasting { .. }, LinkEvent::Start) => {
            *state = LinkState::Active;
            Effects::one(LinkEffect::ResumeActive)
        }
        (LinkState::Active, LinkEvent::Start) => Effects::none(),
        (LinkState::Active, LinkEvent::StartFailed) => {
            *state = LinkState::Stopped;
            Effects::none()
        }
        (LinkState::Active | LinkState::Coasting { .. }, LinkEvent::Coast { period }) => {
            let ticks_left = keep_alive_ticks(period);
            *state = LinkState::Coasting { ticks_left };
            Effects::one(LinkEffect::ArmCoast { ticks_left })
        }
        (LinkState::Stopped, LinkEvent::Coast { .. }) => Effects::none(),
        (LinkState::Coasting { ticks_left }, LinkEvent::CoastTick) if ticks_left > 1 => {
            *state = LinkState::Coasting { ticks_left: ticks_left - 1 };
            Effects::none()
        }
        (LinkState::Coasting { ticks_left: 1 }, LinkEvent::CoastTick) => {
            *state = LinkState::Coasting { ticks_left: 0 };
            Effects::one(LinkEffect::WakeToStop)
        }
        (LinkState::Coasting { ticks_left: 0 }, LinkEvent::StopIfExpired) => {
            *state = LinkState::Stopped;
            Effects::one(LinkEffect::StopLink)
        }
        (LinkState::Active | LinkState::Coasting { .. }, LinkEvent::Stop) => {
            *state = LinkState::Stopped;
            Effects::one(LinkEffect::StopLink)
        }
        _ => Effects::none(),
    }
}

/// When a surface's next frame draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opportunity {
    /// On this tick, which presents at its target.
    Tick(Instant),
    /// At once, aimed at no tick (`rested`).
    Now,
    /// On a later tick.
    Wait,
}

impl Opportunity {
    /// The frame's target presentation time if it draws now (`None` when aimed at no tick), or
    /// `None` when it waits.
    pub(crate) fn target(self) -> Option<Option<Instant>> {
        match self {
            Self::Tick(target) => Some(Some(target)),
            Self::Now => Some(None),
            Self::Wait => None,
        }
    }
}

/// A fresh tick wins. Without one, a surface that has rested draws at once rather than wait
/// for the link to start or tick, which after idle took one to two refreshes.
pub(crate) fn opportunity(
    tick: Option<Instant>,
    last: Option<Instant>,
    last_target: Option<Instant>,
    now: Instant,
    period: Duration,
) -> Opportunity {
    // A late render followed by an on-time tick must skip, not submit two frames
    // together. Also discard stale callbacks after main-thread stalls.
    match tick {
        Some(target) if permits(last, last_target, target, now, period) => Opportunity::Tick(target),
        _ if rested(last, last_target, now, period) => Opportunity::Now,
        _ => Opportunity::Wait,
    }
}

pub(crate) fn permits(
    last: Option<Instant>,
    last_target: Option<Instant>,
    target: Instant,
    now: Instant,
    period: Duration,
) -> bool {
    let half = period / 2;
    now < target
        && last_target.is_none_or(|last| target >= last + period * 3 / 4)
        && last.is_none_or(|last| now.saturating_duration_since(last) >= half)
}

/// Whether the last frame has surely reached the display: its target has passed, and a whole
/// refresh has gone by since it was submitted, plus half of one for the GPU. Another frame then
/// can't queue behind it, so one at once costs no pacing: it shows at the next refresh.
pub(crate) fn rested(
    last: Option<Instant>,
    last_target: Option<Instant>,
    now: Instant,
    period: Duration,
) -> bool {
    last_target.is_none_or(|target| target <= now)
        && last.is_none_or(|last| now.saturating_duration_since(last) >= period * 3 / 2)
}

/// A link that is not actively driving frames (`Stopped` or `Coasting`) can decide whether a
/// quiet timer frame changes anything before bumping to `Active` whenever `rested` holds.
pub(crate) fn defers_start(
    state: LinkState,
    last: Option<Instant>,
    last_target: Option<Instant>,
    now: Instant,
    period: Duration,
) -> bool {
    !state.active() && rested(last, last_target, now, period)
}

/// How a surface's frames are paced, from its link's state (or the lack of one).
pub(crate) fn pacing_of(state: Option<LinkState>, failure: Option<i32>, moving: bool) -> Pacing {
    match (state, failure) {
        (Some(state), _) if state.active() => Pacing::DisplayLink,
        (None, Some(status)) => Pacing::Failed(status),
        _ if moving => Pacing::Unpaced,
        _ => Pacing::Idle,
    }
}

/// Pure, clock-injected core of macOS display-link pacing.
#[derive(Clone, Debug)]
pub(crate) struct DisplayLinkCore {
    state: LinkState,
    last_present: Option<Instant>,
    last_target: Option<Instant>,
    period: Duration,
    capped: bool,
}

impl DisplayLinkCore {
    pub(crate) fn new(period: Duration) -> Self {
        Self {
            state: LinkState::Stopped,
            last_present: None,
            last_target: None,
            period,
            capped: false,
        }
    }

    pub(crate) fn state(&self) -> LinkState { self.state }
    pub(crate) fn paced(&self) -> bool { self.state.active() }
    pub(crate) fn hardware_running(&self) -> bool { self.state.hardware_running() }

    pub(crate) fn frame_period(&self) -> Duration {
        if self.capped { self.period.max(LOW_POWER_FRAME) } else { self.period }
    }

    pub(crate) fn refresh_hz(&self) -> f64 { 1.0 / self.frame_period().as_secs_f64() }

    pub(crate) fn set_period(&mut self, period: Duration) {
        if !period.is_zero() { self.period = period; }
    }

    pub(crate) fn set_capped(&mut self, capped: bool) { self.capped = capped; }

    pub(crate) fn start(&mut self) -> Effects<LinkEffect> {
        step(&mut self.state, LinkEvent::Start)
    }

    pub(crate) fn start_failed(&mut self) {
        let _ = step(&mut self.state, LinkEvent::StartFailed);
    }

    pub(crate) fn coast(&mut self) -> Effects<LinkEffect> {
        let period = self.frame_period();
        step(&mut self.state, LinkEvent::Coast { period })
    }

    #[cfg(test)]
    pub(crate) fn coast_tick(&mut self) -> Effects<LinkEffect> {
        step(&mut self.state, LinkEvent::CoastTick)
    }

    pub(crate) fn stop_if_expired(&mut self, callback_expired: bool) -> Effects<LinkEffect> {
        if callback_expired && matches!(self.state, LinkState::Coasting { .. }) {
            self.state = LinkState::Coasting { ticks_left: 0 };
        }
        step(&mut self.state, LinkEvent::StopIfExpired)
    }

    pub(crate) fn stop(&mut self) -> Effects<LinkEffect> {
        step(&mut self.state, LinkEvent::Stop)
    }

    pub(crate) fn claim(&mut self, tick: Option<Instant>, now: Instant) -> Opportunity {
        let next = opportunity(tick, self.last_present, self.last_target, now, self.frame_period());
        match next {
            Opportunity::Tick(target) => self.last_target = Some(target),
            Opportunity::Now => self.last_target = Some(now),
            Opportunity::Wait => {}
        }
        next
    }

    pub(crate) fn defers_start(&self, now: Instant) -> bool {
        defers_start(self.state, self.last_present, self.last_target, now, self.frame_period())
    }

    pub(crate) fn tick_after(&self, now: Instant) -> Instant { now + self.frame_period() }

    pub(crate) fn presented(&mut self, at: Instant) { self.last_present = Some(at); }
}

#[cfg(test)]
#[path = "display_link_machine_tests.rs"]
mod tests;
