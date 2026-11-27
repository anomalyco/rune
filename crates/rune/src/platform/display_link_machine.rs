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

/// How long after the last live user input motion stays at full display rate even if no
/// unbroken run of presents started immediately after that input.
pub(crate) const STEERED_IDLE: Duration = Duration::from_millis(600);
/// Maximum gap between the last input and the first present of a steered run, and between
/// consecutive presents in that run.
pub(crate) const STEERED_RUN_GAP: Duration = Duration::from_millis(100);
/// Maximum total duration after the last input that an unbroken run of presents keeps full
/// display rate before an opt-in ambient cap applies.
pub(crate) const STEERED_RUN_MAX: Duration = Duration::from_secs(2);

/// Whether an unbroken run of presented frames followed the last live input closely enough to
/// stay at full display rate past [`STEERED_IDLE`] (up to [`STEERED_RUN_MAX`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SteeredRun {
    /// No unbroken run started within [`STEERED_RUN_GAP`] of `last_input`, or the run paused.
    None,
    /// An unbroken run of presents started within [`STEERED_RUN_GAP`] of `last_input` and last
    /// presented at `last_present`.
    Active { last_present: Instant },
}

/// Whether recent user input is steering this window's motion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Steering {
    /// No recent user input is steering motion; an opt-in ambient frame-rate cap applies.
    #[default]
    Ambient,
    /// Live user input arrived at `last_input`, optionally followed by an unbroken run of
    /// presents.
    Steered { last_input: Instant, run: SteeredRun },
}

impl Steering {
    /// Whether motion at `now` is steered by recent user input: true within [`STEERED_IDLE`]
    /// (600 ms) of `last_input`, or within [`STEERED_RUN_MAX`] (2 s) while an unbroken run of
    /// presents started within [`STEERED_RUN_GAP`] (100 ms) of `last_input` and has not paused
    /// for more than [`STEERED_RUN_GAP`].
    pub(crate) fn is_steered(&self, now: Instant) -> bool {
        match *self {
            Self::Ambient => false,
            Self::Steered { last_input, run } => {
                let since_input = now.saturating_duration_since(last_input);
                if since_input <= STEERED_IDLE {
                    return true;
                }
                since_input <= STEERED_RUN_MAX
                    && matches!(
                        run,
                        SteeredRun::Active { last_present }
                            if now.saturating_duration_since(last_present) <= STEERED_RUN_GAP
                    )
            }
        }
    }
}

/// What happens to [`Steering`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SteeringEvent {
    /// Live user input (key, pointer, wheel, touch, gesture, or file drag/drop) reached the
    /// window at `at`.
    Input { at: Instant },
    /// A frame finished submitting/presenting at `at`.
    Presented { at: Instant },
}

/// Effects emitted by [`step_steering`] (none today; kept uniform with `rune::machine`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SteeringEffect {}

/// Pure transition for [`Steering`].
pub(crate) fn step_steering(
    state: &mut Steering,
    event: SteeringEvent,
) -> Effects<SteeringEffect> {
    match (*state, event) {
        (Steering::Steered { run: SteeredRun::Active { last_present }, .. }, SteeringEvent::Input { at }) => {
            let run = if at.saturating_duration_since(last_present) <= STEERED_RUN_GAP {
                SteeredRun::Active { last_present }
            } else {
                SteeredRun::None
            };
            *state = Steering::Steered { last_input: at, run };
        }
        (_, SteeringEvent::Input { at }) => {
            *state = Steering::Steered { last_input: at, run: SteeredRun::None };
        }
        (Steering::Ambient, SteeringEvent::Presented { .. }) => {}
        (Steering::Steered { last_input, run }, SteeringEvent::Presented { at }) => {
            let next_run = match run {
                SteeredRun::None => {
                    if at.saturating_duration_since(last_input) <= STEERED_RUN_GAP {
                        SteeredRun::Active { last_present: at }
                    } else {
                        SteeredRun::None
                    }
                }
                SteeredRun::Active { last_present } => {
                    if at.saturating_duration_since(last_present) <= STEERED_RUN_GAP {
                        SteeredRun::Active { last_present: at }
                    } else {
                        SteeredRun::None
                    }
                }
            };
            let candidate = Steering::Steered { last_input, run: next_run };
            *state = if candidate.is_steered(at) { candidate } else { Steering::Ambient };
        }
    }
    Effects::none()
}

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
    #[cfg(test)]
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
    #[cfg(test)]
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
        #[cfg(test)]
        (LinkState::Coasting { ticks_left }, LinkEvent::CoastTick) if ticks_left > 1 => {
            *state = LinkState::Coasting { ticks_left: ticks_left - 1 };
            Effects::none()
        }
        #[cfg(test)]
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

/// Whether a one-shot urgent frame admission is armed to bypass `Opportunity::Wait` once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UrgentAdmission {
    /// Normal display-link pacing applies.
    #[default]
    Idle,
    /// One urgent frame has been requested; duplicate requests coalesce here and the next frame
    /// claim or hard stop consumes it.
    Armed,
}

/// What happens to [`UrgentAdmission`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UrgentEvent {
    /// An urgent wake or update asked for the next frame not to wait for a display tick.
    Request,
    /// A frame claim or hard stop consumed any pending one-shot admission.
    Consume,
}

/// What [`step_urgent`] authorizes on a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UrgentEffect {
    /// Bypass `Opportunity::Wait` once for this claim by advancing `last_target` to `now`.
    AdmitOnce,
}

/// Pure transition for [`UrgentAdmission`].
pub(crate) fn step_urgent(
    state: &mut UrgentAdmission,
    event: UrgentEvent,
) -> Effects<UrgentEffect> {
    match (*state, event) {
        (UrgentAdmission::Idle | UrgentAdmission::Armed, UrgentEvent::Request) => {
            *state = UrgentAdmission::Armed;
            Effects::none()
        }
        (UrgentAdmission::Armed, UrgentEvent::Consume) => {
            *state = UrgentAdmission::Idle;
            Effects::one(UrgentEffect::AdmitOnce)
        }
        (UrgentAdmission::Idle, UrgentEvent::Consume) => Effects::none(),
    }
}

/// When a surface's next frame draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opportunity {
    /// On this tick, which presents at its target.
    Tick(Instant),
    /// At once, aimed at no tick (`rested`).
    Now,
    /// At once via one-shot urgent admission, bypassing `Wait` once by advancing `last_target`
    /// to `now`.
    UrgentNow,
    /// On a later tick.
    Wait,
}

impl Opportunity {
    /// The frame's target presentation time if it draws now (`None` when aimed at no tick), or
    /// `None` when it waits.
    pub(crate) fn target(self) -> Option<Option<Instant>> {
        match self {
            Self::Tick(target) => Some(Some(target)),
            Self::Now | Self::UrgentNow => Some(None),
            Self::Wait => None,
        }
    }

    /// Whether this opportunity bypassed display-link `Wait` via one-shot urgent admission.
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    pub(crate) fn urgent_admitted(self) -> bool {
        matches!(self, Self::UrgentNow)
    }
}

/// A fresh tick wins. Without one, a surface that has rested draws at once rather than wait
/// for the link to start or tick, which after idle took one to two refreshes.
#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn permits(
    last: Option<Instant>,
    last_target: Option<Instant>,
    target: Instant,
    now: Instant,
    period: Duration,
) -> bool {
    permits_paced(last, last_target, target, now, period, period)
}

fn permits_paced(
    last: Option<Instant>,
    last_target: Option<Instant>,
    target: Instant,
    now: Instant,
    period: Duration,
    hw_period: Duration,
) -> bool {
    let half = period / 2;
    let min_gap = if period == hw_period {
        period * 3 / 4
    } else {
        period.saturating_sub(hw_period.min(period) / 4)
    };
    now < target
        && last_target.is_none_or(|last| target >= last + min_gap)
        && last.is_none_or(|last| now.saturating_duration_since(last) >= half)
}

fn opportunity_paced(
    tick: Option<Instant>,
    last: Option<Instant>,
    last_target: Option<Instant>,
    now: Instant,
    period: Duration,
    hw_period: Duration,
) -> Opportunity {
    match tick {
        Some(target) if permits_paced(last, last_target, target, now, period, hw_period) => Opportunity::Tick(target),
        _ if rested(last, last_target, now, period) => Opportunity::Now,
        _ => Opportunity::Wait,
    }
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
    ambient_period: Option<Duration>,
    steering: Steering,
    urgent: UrgentAdmission,
}

impl DisplayLinkCore {
    pub(crate) fn new(period: Duration) -> Self {
        Self {
            state: LinkState::Stopped,
            last_present: None,
            last_target: None,
            period,
            capped: false,
            ambient_period: None,
            steering: Steering::Ambient,
            urgent: UrgentAdmission::Idle,
        }
    }

    pub(crate) fn state(&self) -> LinkState { self.state }
    pub(crate) fn paced(&self) -> bool { self.state.active() }
    pub(crate) fn hardware_running(&self) -> bool { self.state.hardware_running() }

    pub(crate) fn frame_period(&self) -> Duration {
        if self.capped { self.period.max(LOW_POWER_FRAME) } else { self.period }
    }

    /// Effective frame period at `now`, applying `ambient_period` only when no recent user input
    /// is steering the window (composing monotonically with Low Power Mode).
    pub(crate) fn frame_period_at(&self, now: Instant) -> Duration {
        let base = self.frame_period();
        match self.ambient_period {
            Some(ambient) if !self.steering.is_steered(now) => base.max(ambient),
            _ => base,
        }
    }

    pub(crate) fn refresh_hz(&self) -> f64 { 1.0 / self.frame_period().as_secs_f64() }

    pub(crate) fn set_period(&mut self, period: Duration) {
        if !period.is_zero() { self.period = period; }
    }

    pub(crate) fn set_capped(&mut self, capped: bool) { self.capped = capped; }

    pub(crate) fn set_ambient_period(&mut self, period: Option<Duration>) {
        self.ambient_period = period.filter(|p| !p.is_zero());
    }

    pub(crate) fn note_input(&mut self, at: Instant) {
        let _ = step_steering(&mut self.steering, SteeringEvent::Input { at });
    }

    #[cfg(test)]
    pub(crate) fn steering(&self) -> Steering { self.steering }

    pub(crate) fn start(&mut self) -> Effects<LinkEffect> {
        step(&mut self.state, LinkEvent::Start)
    }

    pub(crate) fn start_failed(&mut self) {
        let _ = step(&mut self.state, LinkEvent::StartFailed);
    }

    pub(crate) fn coast(&mut self) -> Effects<LinkEffect> {
        // CoreVideo fires coasting ticks at the hardware refresh period, so compute the
        // ~150 ms budget from `self.period` even when an ambient or Low Power cap is active.
        step(&mut self.state, LinkEvent::Coast { period: self.period })
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

    /// Arms one-shot urgent admission so the next `claim` may bypass `Opportunity::Wait`
    /// once by advancing `last_target` to `now`. Duplicate calls coalesce into one admission.
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    pub(crate) fn admit_once(&mut self) {
        let _ = step_urgent(&mut self.urgent, UrgentEvent::Request);
    }

    /// Consumes any pending one-shot urgent admission, returning whether one was armed.
    pub(crate) fn take_urgent(&mut self) -> bool {
        !step_urgent(&mut self.urgent, UrgentEvent::Consume).is_empty()
    }

    /// Whether a one-shot urgent admission is currently armed.
    pub(crate) fn is_urgent(&self) -> bool {
        self.urgent == UrgentAdmission::Armed
    }

    pub(crate) fn stop(&mut self) -> Effects<LinkEffect> {
        let _ = self.take_urgent();
        step(&mut self.state, LinkEvent::Stop)
    }

    pub(crate) fn claim(&mut self, tick: Option<Instant>, now: Instant) -> Opportunity {
        let period = self.frame_period_at(now);
        let urgent = self.take_urgent();
        let next = match opportunity_paced(tick, self.last_present, self.last_target, now, period, self.period) {
            Opportunity::Wait if urgent => Opportunity::UrgentNow,
            other => other,
        };
        match next {
            Opportunity::Tick(target) => self.last_target = Some(target),
            Opportunity::Now | Opportunity::UrgentNow => self.last_target = Some(now),
            Opportunity::Wait => {}
        }
        next
    }

    pub(crate) fn defers_start(&self, now: Instant) -> bool {
        !self.is_urgent() && defers_start(self.state, self.last_present, self.last_target, now, self.frame_period_at(now))
    }

    pub(crate) fn tick_after(&self, now: Instant) -> Instant { now + self.frame_period_at(now) }

    pub(crate) fn presented(&mut self, at: Instant) {
        self.last_present = Some(at);
        let _ = step_steering(&mut self.steering, SteeringEvent::Presented { at });
    }
}

#[cfg(test)]
#[path = "display_link_machine_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "display_link_urgent_tests.rs"]
mod urgent_tests;
