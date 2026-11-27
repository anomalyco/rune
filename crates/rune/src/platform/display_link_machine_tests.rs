use std::time::Duration;

use super::*;
use crate::frame::surface_stats::Pacing;

/// Deterministic headless simulator of a macOS `CVDisplayLink` + host event loop on an injected
/// clock. Models CoreVideo's cold-start delay (1.5 refreshes after `CVDisplayLinkStart` before the
/// first tick arrives) and tracks hardware `Start`/`Stop` toggles, draws, presents, coasting
/// wakes, and missed latest-wins slots.
struct SimLink {
    core: DisplayLinkCore,
    period: Duration,
    /// When the hardware link is running, the instant of its next CoreVideo tick.
    next_hw_tick: Option<Instant>,
    /// Fresh target stored by the most recent CoreVideo callback (cleared on `StopLink` or claim).
    cached_target: Option<Instant>,
    /// Whether the callback thread sent a `WakeEvent::DisplayTick` that the main thread hasn't
    /// handled yet.
    pending_wake: bool,
    /// Whether coasting expired on the callback thread and is waiting for `stop_expired`.
    callback_expired: bool,
    hw_starts: u32,
    hw_stops: u32,
    coasting_ticks: u32,
    coasting_wakes: u32,
    draws_during_coast: u32,
    presents_during_coast: u32,
    draws_on_expiry: u32,
    presents_on_expiry: u32,
    total_draws: u32,
    total_presents: u32,
    tick_opportunities: u32,
    wait_opportunities: u32,
    now_opportunities: u32,
}

impl SimLink {
    fn new(period: Duration) -> Self {
        Self {
            core: DisplayLinkCore::new(period),
            period,
            next_hw_tick: None,
            cached_target: None,
            pending_wake: false,
            callback_expired: false,
            hw_starts: 0,
            hw_stops: 0,
            coasting_ticks: 0,
            coasting_wakes: 0,
            draws_during_coast: 0,
            presents_during_coast: 0,
            draws_on_expiry: 0,
            presents_on_expiry: 0,
            total_draws: 0,
            total_presents: 0,
            tick_opportunities: 0,
            wait_opportunities: 0,
            now_opportunities: 0,
        }
    }

    fn apply(&mut self, effects: Effects<LinkEffect>, now: Instant) {
        for effect in effects {
            match effect {
                LinkEffect::StartLink => {
                    self.hw_starts += 1;
                    // Cold CVDisplayLinkStart takes 1-2 refreshes before delivering its first tick.
                    self.next_hw_tick = Some(now + self.period * 3 / 2);
                    self.pending_wake = false;
                    self.callback_expired = false;
                }
                LinkEffect::ResumeActive => {
                    // Already running: keep next_hw_tick and cached_target intact.
                    self.pending_wake = false;
                    self.callback_expired = false;
                }
                LinkEffect::ArmCoast { .. } => {
                    self.pending_wake = false;
                    self.callback_expired = false;
                }
                LinkEffect::WakeToStop => {
                    self.callback_expired = true;
                    self.pending_wake = true;
                    self.coasting_wakes += 1;
                }
                LinkEffect::StopLink => {
                    self.hw_stops += 1;
                    self.next_hw_tick = None;
                    self.cached_target = None;
                    self.pending_wake = false;
                    self.callback_expired = false;
                }
            }
        }
    }

    /// Fires one CoreVideo callback at `tick_at` (target is one refresh ahead).
    fn fire_hw_tick(&mut self, tick_at: Instant) {
        let target = tick_at + self.period;
        self.cached_target = Some(target);
        self.next_hw_tick = Some(tick_at + self.period);
        match self.core.state() {
            LinkState::Active => {
                self.pending_wake = true;
            }
            LinkState::Coasting { .. } => {
                self.coasting_ticks += 1;
                let effects = self.core.coast_tick();
                self.apply(effects, tick_at);
            }
            LinkState::Stopped => {}
        }
    }

    /// Processes a pending `WakeEvent::DisplayTick` on the main thread, returning whether a frame
    /// drew and presented.
    fn handle_display_tick(&mut self, now: Instant, frame_work: Duration) -> bool {
        if !self.pending_wake { return false; }
        self.pending_wake = false;
        if self.core.paced() {
            self.attempt_draw(now, frame_work)
        } else {
            let was_expired = self.callback_expired;
            let effects = self.core.stop_if_expired(self.callback_expired);
            self.apply(effects, now);
            if was_expired {
                assert_eq!(self.core.state(), LinkState::Stopped);
            }
            false
        }
    }

    /// Simulates `Host::draw` -> `pacing.next_frame()` -> render -> `settle` (`coast`).
    fn attempt_draw(&mut self, now: Instant, frame_work: Duration) -> bool {
        let effects = self.core.start();
        self.apply(effects, now);
        self.pending_wake = false;
        let tick = self.cached_target.take();
        let opp = self.core.claim(tick, now);
        match opp {
            Opportunity::Tick(_) => self.tick_opportunities += 1,
            Opportunity::Now | Opportunity::UrgentNow => self.now_opportunities += 1,
            Opportunity::Wait => {
                self.wait_opportunities += 1;
                return false;
            }
        }
        let submitted = now + frame_work;
        self.total_draws += 1;
        self.total_presents += 1;
        self.core.presented(submitted);
        let coast = self.core.coast();
        self.apply(coast, submitted);
        true
    }

    /// Advances the hardware clock up to `until`, firing ticks and handling main-thread wakes.
    /// Any tick while `Coasting` must draw nothing and present nothing.
    fn advance_until(
        &mut self,
        until: Instant,
        frame_work: Duration,
        mut on_draw: impl FnMut(Instant),
    ) {
        while let Some(tick_at) = self.next_hw_tick {
            if tick_at > until { break; }
            let coasting_before = matches!(self.core.state(), LinkState::Coasting { .. });
            let draws_before = self.total_draws;
            let presents_before = self.total_presents;
            self.fire_hw_tick(tick_at);
            let expired_now = self.callback_expired;
            if self.pending_wake && self.handle_display_tick(tick_at, frame_work) {
                on_draw(tick_at);
            }
            if coasting_before {
                self.draws_during_coast += self.total_draws - draws_before;
                self.presents_during_coast += self.total_presents - presents_before;
            }
            if expired_now {
                self.draws_on_expiry += self.total_draws - draws_before;
                self.presents_on_expiry += self.total_presents - presents_before;
            }
        }
    }
}

#[test]
fn keep_alive_ticks_scales_with_refresh_rate_and_clamps() {
    assert_eq!(keep_alive_ticks(Duration::from_nanos(1_000_000_000 / 60)), 9);
    assert_eq!(keep_alive_ticks(Duration::from_nanos(1_000_000_000 / 120)), 18);
    assert_eq!(keep_alive_ticks(Duration::from_nanos(1_000_000_000 / 240)), 30);
    assert_eq!(keep_alive_ticks(Duration::from_nanos(1_000_000_000 / 30)), 8);
    assert_eq!(keep_alive_ticks(Duration::ZERO), 30);
}

#[test]
fn coasting_keeps_120hz_and_118hz_wheel_scrolls_paced_without_stop_start_or_idle_draws() {
    let t0 = crate::time::now();
    let period = Duration::from_nanos(1_000_000_000 / 120); // 8.333 ms
    let frame_work = Duration::from_micros(900);

    // 1. Steady 120 Hz wheel deltas over 5 seconds (600 deltas).
    let mut steady = SimLink::new(period);
    let mut drawn = 0u32;
    let mut now = t0;
    for i in 0..600u32 {
        let delta_at = t0 + Duration::from_nanos(u64::from(i) * (1_000_000_000 / 120) + 500_000);
        steady.advance_until(delta_at, frame_work, |_| drawn += 1);
        now = delta_at;
        if steady.attempt_draw(now, frame_work) {
            drawn += 1;
        }
    }
    assert_eq!(steady.hw_starts, 1, "steady 120 Hz wheel gesture must start the link once and never restart");
    assert_eq!(steady.hw_stops, 0, "steady 120 Hz wheel gesture must not stop the link between deltas");
    assert!(steady.tick_opportunities >= 590, "steady wheel deltas should hit Opportunity::Tick directly, got {}", steady.tick_opportunities);
    assert_eq!(steady.draws_during_coast, 0, "coasting ticks must draw nothing");
    assert_eq!(steady.presents_during_coast, 0, "coasting ticks must present nothing");

    // Finish any final queued delta waiting for the next tick so the link is Coasting, then rest
    // for 250 ms: coasting and expiry must draw and present nothing and stop cleanly after ~150 ms.
    if steady.core.paced() {
        steady.advance_until(now + period * 2, frame_work, |_| drawn += 1);
    }
    assert!(matches!(steady.core.state(), LinkState::Coasting { .. }), "window must settle into Coasting after the final delta");
    assert_eq!(drawn, 600, "all 600 wheel deltas must draw");
    let draws_at_end = steady.total_draws;
    let presents_at_end = steady.total_presents;
    steady.advance_until(now + Duration::from_millis(250), frame_work, |_| drawn += 1);
    assert_eq!(steady.core.state(), LinkState::Stopped, "link must stop after ~150 ms idle");
    assert_eq!(steady.hw_stops, 1, "link must stop once after coasting expires");
    assert_eq!(steady.coasting_wakes, 1, "coasting expiry must send exactly one wake");
    assert_eq!(steady.total_draws, draws_at_end, "idle coasting and expiry must draw nothing");
    assert_eq!(steady.total_presents, presents_at_end, "idle coasting and expiry must present nothing");
    assert_eq!(steady.draws_on_expiry, 0);
    assert_eq!(steady.presents_on_expiry, 0);

    // 2. 118 Hz + jittery wheel deltas over 5 seconds (~590 deltas, including sub-refresh and
    // empty-refresh gaps).
    let mut jittery = SimLink::new(period);
    let mut jitter_draws = 0u32;
    let mut last_draw_at = None;
    let mut max_gap = Duration::ZERO;
    let mut last_t = t0;
    for i in 0..590u32 {
        // 118 Hz base (~8.475 ms) with deterministic +-1.5 ms jitter.
        let jitter_us = match i % 5 {
            0 => 0i64,
            1 => 1_200,
            2 => -1_400,
            3 => 800,
            _ => -600,
        };
        let base_us = i64::from(i) * 8_475 + 1_000;
        let delta_at = t0 + Duration::from_micros((base_us + jitter_us).max(0) as u64);
        jittery.advance_until(delta_at, frame_work, |at| {
            jitter_draws += 1;
            if let Some(prev) = last_draw_at { max_gap = max_gap.max(at.saturating_duration_since(prev)); }
            last_draw_at = Some(at);
        });
        last_t = delta_at;
        if jittery.attempt_draw(delta_at, frame_work) {
            jitter_draws += 1;
            if let Some(prev) = last_draw_at { max_gap = max_gap.max(delta_at.saturating_duration_since(prev)); }
            last_draw_at = Some(delta_at);
        }
    }
    assert_eq!(jittery.hw_starts, 1, "118 Hz jittery scroll must have 0 Start/Stop toggles during the gesture");
    assert_eq!(jittery.hw_stops, 0, "118 Hz jittery scroll must not stop between wheel deltas");
    assert!(jitter_draws >= 570, "jittery 118 Hz scroll should draw nearly every delta without cold-start stalls, got {jitter_draws}");
    assert!(max_gap < period * 3, "no 2+ refresh cold-start stall during scroll (max_gap = {max_gap:?})");
    assert_eq!(jittery.draws_during_coast, 0);
    assert_eq!(jittery.presents_during_coast, 0);

    if jittery.core.paced() {
        jittery.advance_until(last_t + period * 2, frame_work, |_| {});
    }
    assert!(matches!(jittery.core.state(), LinkState::Coasting { .. }));
    let draws_before_rest = jittery.total_draws;
    let presents_before_rest = jittery.total_presents;
    jittery.advance_until(last_t + Duration::from_millis(250), frame_work, |_| {});
    assert_eq!(jittery.core.state(), LinkState::Stopped);
    assert_eq!(jittery.hw_stops, 1);
    assert_eq!(jittery.total_draws, draws_before_rest);
    assert_eq!(jittery.total_presents, presents_before_rest);
}

#[test]
fn coasting_draws_all_50fps_and_40fps_native_surface_pushes_at_60hz_without_overwrite() {
    let t0 = crate::time::now();
    let period_60 = Duration::from_nanos(1_000_000_000 / 60); // 16.667 ms
    let frame_work = Duration::from_micros(1_100); // 1.1 ms from #1141

    for (rate_label, push_interval, total_pushes) in [
        ("50 fps", Duration::from_millis(20), 250u32),
        ("40 fps", Duration::from_millis(25), 200u32),
    ] {
        let mut sim = SimLink::new(period_60);
        // Latest-wins slot matching NativeSurface::replace: holds the latest un-drawn frame id.
        let mut pending_frame: Option<u32> = None;
        let mut overwritten = 0u32;
        let mut drawn_ids = Vec::new();
        let mut last_push_at = t0;

        for id in 1..=total_pushes {
            let push_at = t0 + push_interval * (id - 1);
            last_push_at = push_at;
            sim.advance_until(push_at, frame_work, |_| {
                if let Some(fid) = pending_frame.take() {
                    drawn_ids.push(fid);
                }
            });
            // Producer pushes `id` into the latest-wins slot and wakes the app -> request_redraw.
            if pending_frame.replace(id).is_some() {
                overwritten += 1;
            }
            if sim.attempt_draw(push_at, frame_work)
                && let Some(fid) = pending_frame.take()
            {
                drawn_ids.push(fid);
            }
        }
        // Let any final queued frame draw on the next tick, then rest past 150 ms.
        sim.advance_until(last_push_at + Duration::from_millis(250), frame_work, |_| {
            if let Some(fid) = pending_frame.take() {
                drawn_ids.push(fid);
            }
        });

        assert_eq!(overwritten, 0, "{rate_label}: no pushed frame may be overwritten before drawing");
        assert_eq!(
            drawn_ids.len() as u32,
            total_pushes,
            "{rate_label}: all {total_pushes} pushes must draw (got {})",
            drawn_ids.len()
        );
        assert_eq!(sim.hw_starts, 1, "{rate_label}: 0 Start/Stop toggles during playback");
        assert_eq!(sim.hw_stops, 1, "{rate_label}: 1 clean stop after ~150 ms idle");
        assert_eq!(sim.total_draws, total_pushes, "{rate_label}: 0 extra draws on idle ticks");
        assert_eq!(sim.total_presents, total_pushes, "{rate_label}: 0 extra presents on idle ticks");
        assert_eq!(sim.draws_during_coast, 0);
        assert_eq!(sim.presents_during_coast, 0);
        assert_eq!(sim.core.state(), LinkState::Stopped, "{rate_label}: stopped cleanly after ~150 ms idle");
    }
}

#[test]
fn coasting_reports_idle_pacing_keeps_timers_awake_and_hard_stops_immediately() {
    let t0 = crate::time::now();
    let period = Duration::from_nanos(1_000_000_000 / 120);
    let mut core = DisplayLinkCore::new(period);

    // PR #1718 invariant: created in Stopped; coasting a Stopped link must stay Stopped.
    assert_eq!(core.state(), LinkState::Stopped);
    assert!(!core.paced());
    assert!(!core.hardware_running());
    assert_eq!(pacing_of(Some(core.state()), None, false), Pacing::Idle);
    assert_eq!(core.coast(), Effects::none());
    assert_eq!(core.state(), LinkState::Stopped);

    // Start -> Active -> Coasting.
    assert_eq!(core.start(), Effects::one(LinkEffect::StartLink));
    assert!(core.paced());
    assert_eq!(pacing_of(Some(core.state()), None, true), Pacing::DisplayLink);

    assert_eq!(core.coast(), Effects::one(LinkEffect::ArmCoast { ticks_left: 18 }));
    assert_eq!(core.state(), LinkState::Coasting { ticks_left: 18 });
    assert!(!core.paced(), "paced() must be false in Coasting so wake_deadline fires timers and DisplayTick does not redraw");
    assert!(core.hardware_running(), "underlying CVDisplayLink stays running during Coasting");
    assert_eq!(pacing_of(Some(core.state()), None, false), Pacing::Idle, "SurfaceMeter must report Pacing::Idle while Coasting");

    // Quiet timer firing after rest during Coasting defers bumping back to Active.
    core.presented(t0);
    assert!(core.defers_start(t0 + Duration::from_millis(50)));
    assert!(!core.defers_start(t0 + Duration::from_millis(4)));

    // Hard stop (occlusion, hide, minimize, missed frame/backoff, Resized(0,0), prewarm, drop)
    // stops Coasting immediately.
    assert_eq!(core.stop(), Effects::one(LinkEffect::StopLink));
    assert_eq!(core.state(), LinkState::Stopped);
    assert_eq!(core.stop(), Effects::none(), "stopping an already-stopped link is a no-op");

    // Failed start returns cleanly to Stopped; Low Power Mode cap stretches period and tick_after.
    assert_eq!(core.start(), Effects::one(LinkEffect::StartLink));
    core.start_failed();
    assert_eq!(core.state(), LinkState::Stopped);
    core.set_period(Duration::from_nanos(1_000_000_000 / 120));
    assert!((core.refresh_hz() - 120.0).abs() < 0.1);
    assert_eq!(core.tick_after(t0), t0 + Duration::from_nanos(1_000_000_000 / 120));
    core.set_capped(true);
    assert!((core.refresh_hz() - 60.0).abs() < 0.1);
    assert_eq!(Opportunity::Tick(t0).target(), Some(Some(t0)));
    assert_eq!(Opportunity::Now.target(), Some(None));
    assert_eq!(Opportunity::Wait.target(), None);
}

#[test]
fn a_frame_defers_starting_the_link_only_when_it_would_draw_at_once_anyway() {
    let t = crate::time::now();
    let ms = |n| t + Duration::from_millis(n);
    let period = Duration::from_millis(8);
    // A timer 33 ms after the last frame: whenever it starts the link, it draws at once.
    assert!(defers_start(LinkState::Stopped, Some(ms(100)), Some(ms(104)), ms(133), period));
    assert!(defers_start(LinkState::Coasting { ticks_left: 10 }, Some(ms(100)), Some(ms(104)), ms(133), period));
    for later in [133, 134, 150, 500] {
        assert_eq!(opportunity(None, Some(ms(100)), Some(ms(104)), ms(later), period), Opportunity::Now);
    }
    assert!(defers_start(LinkState::Stopped, None, None, ms(0), period));
    // The last frame may still be queued: start/resume the link now and wait for its tick.
    assert!(!defers_start(LinkState::Stopped, Some(ms(100)), Some(ms(100)), ms(105), period));
    assert!(!defers_start(LinkState::Stopped, Some(ms(100)), Some(ms(130)), ms(120), period));
    // An active link already paces the surface.
    assert!(!defers_start(LinkState::Active, Some(ms(100)), Some(ms(104)), ms(133), period));
}

#[test]
fn late_frames_and_idle_restarts_cannot_catch_up_in_a_burst() {
    let t = crate::time::now();
    let ms = |n| t + Duration::from_millis(n);
    let period = Duration::from_millis(16);
    assert!(permits(None, None, ms(16), ms(1), period));
    assert!(!permits(Some(ms(15)), Some(ms(16)), ms(32), ms(17), period));
    assert!(!permits(Some(ms(15)), Some(ms(16)), ms(32), ms(33), period));
    assert!(!permits(Some(ms(15)), Some(ms(32)), ms(32), ms(30), period));
    assert!(permits(Some(ms(15)), Some(ms(16)), ms(32), ms(30), period));
    assert!(!permits(Some(ms(100)), Some(ms(112)), ms(128), ms(102), period));
}

#[test]
fn a_capped_period_skips_ticks_between_frames() {
    let t = crate::time::now();
    let ms = |n| t + Duration::from_millis(n);
    let capped = Duration::from_millis(16);
    assert!(!permits(Some(ms(0)), Some(ms(8)), ms(16), ms(9), capped));
    assert!(permits(Some(ms(2)), Some(ms(8)), ms(24), ms(17), capped));
    assert!(permits(Some(ms(2)), Some(ms(8)), ms(16), ms(9), Duration::from_millis(8)));
}

#[test]
fn input_at_rest_draws_without_waiting_for_a_tick() {
    let t = crate::time::now();
    let ms = |n| t + Duration::from_millis(n);
    let period = Duration::from_millis(8);
    assert_eq!(opportunity(None, Some(ms(0)), Some(ms(8)), ms(500), period), Opportunity::Now);
    assert_eq!(opportunity(None, None, None, ms(0), period), Opportunity::Now);
    assert_eq!(opportunity(Some(ms(400)), Some(ms(0)), Some(ms(8)), ms(500), period), Opportunity::Now);
    assert_eq!(opportunity(Some(ms(510)), Some(ms(0)), Some(ms(8)), ms(500), period), Opportunity::Tick(ms(510)));
}

#[test]
fn a_frame_still_on_its_way_to_the_display_waits_for_a_tick() {
    let t = crate::time::now();
    let ms = |n| t + Duration::from_millis(n);
    let period = Duration::from_millis(8);
    assert_eq!(opportunity(None, Some(ms(100)), Some(ms(100)), ms(111), period), Opportunity::Wait);
    assert_eq!(opportunity(None, Some(ms(100)), Some(ms(100)), ms(112), period), Opportunity::Now);
    assert_eq!(opportunity(None, Some(ms(100)), Some(ms(130)), ms(120), period), Opportunity::Wait);
    assert_eq!(opportunity(None, Some(ms(100)), Some(ms(100)), ms(118), Duration::from_millis(16)), Opportunity::Wait);
    assert_eq!(opportunity(Some(ms(116)), Some(ms(100)), Some(ms(108)), ms(102), period), Opportunity::Wait);
}

/// Runs a continuous animating stream on `core` for `ticks` hardware ticks at `hw_period`,
/// calling `before_tick(index, tick_at)` before each tick's `claim`. Returns
/// `(draws, waits)` for the ticks in `count_range`.
fn drive_stream(
    core: &mut DisplayLinkCore,
    t0: Instant,
    hw_period: Duration,
    frame_work: Duration,
    ticks: u32,
    count_range: std::ops::Range<u32>,
    mut before_tick: impl FnMut(&mut DisplayLinkCore, u32, Instant),
) -> (u32, u32) {
    let _ = core.start();
    let mut draws = 0u32;
    let mut waits = 0u32;
    for i in 0..ticks {
        let now = t0 + hw_period * i;
        let target = now + hw_period;
        before_tick(core, i, now);
        match core.claim(Some(target), now) {
            Opportunity::Tick(_) | Opportunity::Now | Opportunity::UrgentNow => {
                core.presented(now + frame_work);
                if count_range.contains(&i) {
                    draws += 1;
                }
            }
            Opportunity::Wait => {
                if count_range.contains(&i) {
                    waits += 1;
                }
            }
        }
    }
    (draws, waits)
}

#[test]
fn ambient_fps_default_paces_every_120hz_tick_bit_identically() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);
    let mut core = DisplayLinkCore::new(period_120);

    assert_eq!(core.frame_period_at(t0), core.frame_period());
    assert!((core.refresh_hz() - 120.0).abs() < 0.1);

    let (draws, waits) = drive_stream(&mut core, t0, period_120, frame_work, 120, 0..120, |_, _, _| {});
    assert_eq!(draws, 120, "default uncapped pacing must draw every 120 Hz tick with no input");
    assert_eq!(waits, 0, "default uncapped pacing must never skip a 120 Hz tick");
    assert_eq!(core.coast(), Effects::one(LinkEffect::ArmCoast { ticks_left: 18 }));
}

#[test]
fn ambient_fps_60_skips_alternate_120hz_ticks_and_coasts_to_stopped() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);
    let mut sim = SimLink::new(period_120);
    sim.core.set_ambient_period(crate::app::ambient_period_from_fps(60.0));

    // Unsteered streaming for 1 second (120 hardware ticks): alternates Tick and Wait -> 60 draws.
    let mut pattern = Vec::new();
    let _ = sim.core.start();
    sim.apply(Effects::one(LinkEffect::StartLink), t0);
    for i in 0..120u32 {
        let now = t0 + period_120 * i;
        let target = now + period_120;
        let opp = sim.core.claim(Some(target), now);
        pattern.push(matches!(opp, Opportunity::Tick(_)));
        if matches!(opp, Opportunity::Tick(_) | Opportunity::Now) {
            sim.total_draws += 1;
            sim.total_presents += 1;
            sim.core.presented(now + frame_work);
        } else {
            sim.wait_opportunities += 1;
        }
    }
    assert_eq!(sim.total_draws, 60, "ambient_fps(60.0) on 120 Hz must draw 60 frames/s");
    assert_eq!(sim.wait_opportunities, 60, "ambient_fps(60.0) on 120 Hz must skip alternate ticks");
    for (i, drew) in pattern.iter().enumerate() {
        assert_eq!(*drew, i % 2 == 0, "tick {i} must alternate Tick / Wait");
    }

    // Streaming stops: coast() budgets ~150 ms from hardware self.period (18 ticks, not 9),
    // draws/presents zero times while coasting, and stops cleanly to Stopped.
    let end_t = t0 + period_120 * 120;
    let coast = sim.core.coast();
    assert_eq!(coast, Effects::one(LinkEffect::ArmCoast { ticks_left: 18 }));
    sim.apply(coast, end_t);
    sim.next_hw_tick = Some(end_t + period_120);
    let draws_before_coast = sim.total_draws;
    let presents_before_coast = sim.total_presents;
    sim.advance_until(end_t + Duration::from_millis(250), frame_work, |_| {});
    assert_eq!(sim.core.state(), LinkState::Stopped, "ambient link must stop cleanly after ~150 ms coasting");
    assert_eq!(sim.hw_stops, 1);
    assert_eq!(sim.coasting_ticks, 18);
    assert_eq!(sim.total_draws, draws_before_coast, "coasting must draw nothing");
    assert_eq!(sim.total_presents, presents_before_coast, "coasting must present nothing");
    assert_eq!(sim.draws_during_coast, 0);
    assert_eq!(sim.presents_during_coast, 0);
    assert_eq!(sim.draws_on_expiry, 0);
    assert_eq!(sim.presents_on_expiry, 0);
}

#[test]
fn ambient_fps_input_lifts_cap_on_next_120hz_tick() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);
    let mut core = DisplayLinkCore::new(period_120);
    core.set_ambient_period(crate::app::ambient_period_from_fps(60.0));
    let _ = core.start();

    // Tick 0 draws, tick 1 is skipped under ambient 60 Hz, tick 2 draws.
    assert!(matches!(core.claim(Some(t0 + period_120), t0), Opportunity::Tick(_)));
    core.presented(t0 + frame_work);
    assert_eq!(core.claim(Some(t0 + period_120 * 2), t0 + period_120), Opportunity::Wait);
    assert!(matches!(core.claim(Some(t0 + period_120 * 3), t0 + period_120 * 2), Opportunity::Tick(_)));
    core.presented(t0 + period_120 * 2 + frame_work);

    // Input arrives before tick 3 (which ambient 60 Hz would have skipped): cap lifts immediately.
    core.note_input(t0 + period_120 * 2 + Duration::from_millis(4));
    assert!(
        matches!(core.claim(Some(t0 + period_120 * 4), t0 + period_120 * 3), Opportunity::Tick(_)),
        "input must lift the ambient cap on the very next 120 Hz tick"
    );
    core.presented(t0 + period_120 * 3 + frame_work);
    assert!(
        matches!(core.claim(Some(t0 + period_120 * 5), t0 + period_120 * 4), Opportunity::Tick(_)),
        "subsequent steered ticks must stay at 120 Hz"
    );
}

#[test]
fn ambient_fps_never_caps_pointer_drag_wheel_scroll_key_or_touch_following_finger() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);
    let cap_60 = crate::app::ambient_period_from_fps(60.0);

    // Each input modality drives motion for 2.5 s (300 ticks, past both 600 ms and 2 s),
    // followed by a 1.2 s unbroken glide/spring tail after release (144 ticks).
    for (label, input_every_ticks) in [
        ("pointer drag", 2u32),         // ~60 Hz mouse moves while dragging
        ("wheel/trackpad scroll", 1u32),// 120 Hz wheel deltas + glide tail
        ("keyboard input", 4u32),       // ~30 Hz key repeats
        ("touch / finger spring", 1u32),// 120 Hz touch moves + spring settle tail
    ] {
        let mut core = DisplayLinkCore::new(period_120);
        core.set_ambient_period(cap_60);
        let active_ticks = 300u32; // 2.5 s of active user steering
        let tail_ticks = 144u32;   // 1.2 s unbroken settle/glide after last input (< 2 s max)
        let total = active_ticks + tail_ticks;

        let (draws, waits) = drive_stream(
            &mut core,
            t0,
            period_120,
            frame_work,
            total,
            0..total,
            |core, i, now| {
                if i < active_ticks && i % input_every_ticks == 0 {
                    core.note_input(now);
                }
            },
        );
        assert_eq!(draws, total, "{label}: must stay at full 120 Hz throughout input and release tail");
        assert_eq!(waits, 0, "{label}: ambient cap must never drop a steered 120 Hz tick");
    }
}

#[test]
fn ambient_fps_unbroken_steered_run_holds_120hz_until_2s_ceiling_then_caps() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);
    let mut core = DisplayLinkCore::new(period_120);
    core.set_ambient_period(crate::app::ambient_period_from_fps(60.0));

    // Single input at t = 0; unbroken animation presents from tick 0 through 3 s (360 ticks).
    // Ticks 0..240 cover [0, 2.0 s): stays at 120 Hz past the 600 ms idle window.
    let (draws_first_2s, waits_first_2s) = drive_stream(
        &mut core,
        t0,
        period_120,
        frame_work,
        240,
        0..240,
        |core, i, now| {
            if i == 0 {
                core.note_input(now);
            }
        },
    );
    assert_eq!(draws_first_2s, 240, "unbroken steered run must stay at 120 Hz through 2 s");
    assert_eq!(waits_first_2s, 0);

    // Ticks 241..361 cover > 2.0 s through 3.0 s: exceeds STEERED_RUN_MAX (2 s) and caps to 60 Hz.
    let mut draws_after_2s = 0u32;
    let mut waits_after_2s = 0u32;
    for i in 241..361u32 {
        let now = t0 + period_120 * i;
        let target = now + period_120;
        match core.claim(Some(target), now) {
            Opportunity::Tick(_) | Opportunity::Now | Opportunity::UrgentNow => {
                core.presented(now + frame_work);
                draws_after_2s += 1;
            }
            Opportunity::Wait => waits_after_2s += 1,
        }
    }
    assert_eq!(core.steering(), Steering::Ambient, "steering must fall back to Ambient past 2 s");
    assert_eq!(draws_after_2s, 60, "unsteered motion past 2 s ceiling must cap to 60 Hz");
    assert_eq!(waits_after_2s, 60);
}

#[test]
fn ambient_fps_reply_after_gap_caps_at_60hz_once_600ms_elapses() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);
    let mut core = DisplayLinkCore::new(period_120);
    core.set_ambient_period(crate::app::ambient_period_from_fps(60.0));
    let _ = core.start();

    // User presses Enter at t = 0 and the composer clears on one frame at t = 5 ms.
    core.note_input(t0);
    assert!(matches!(core.claim(Some(t0 + period_120), t0), Opportunity::Tick(_)));
    core.presented(t0 + Duration::from_millis(5));

    // > 100 ms idle pause waiting for the model's first token (reply starts at t = 250 ms).
    // From 250 ms to 600 ms (42 ticks), motion is within STEERED_IDLE (600 ms) so stays 120 Hz,
    // but the > 100 ms gap broke the steered run so it will NOT extend past 600 ms.
    let start_tick = 30u32; // 250 ms
    let mut draws_before_600 = 0u32;
    for i in start_tick..72u32 {
        let now = t0 + period_120 * i;
        let target = now + period_120;
        assert!(matches!(core.claim(Some(target), now), Opportunity::Tick(_) | Opportunity::Now));
        core.presented(now + frame_work);
        draws_before_600 += 1;
    }
    assert_eq!(draws_before_600, 42);
    assert_eq!(core.steering(), Steering::Steered { last_input: t0, run: SteeredRun::None });

    // From 608 ms through 1608 ms (120 ticks = 1 s): caps cleanly at 60 Hz.
    let mut draws_after_600 = 0u32;
    let mut waits_after_600 = 0u32;
    for i in 73..193u32 {
        let now = t0 + period_120 * i;
        let target = now + period_120;
        match core.claim(Some(target), now) {
            Opportunity::Tick(_) | Opportunity::Now | Opportunity::UrgentNow => {
                core.presented(now + frame_work);
                draws_after_600 += 1;
            }
            Opportunity::Wait => waits_after_600 += 1,
        }
    }
    assert_eq!(core.steering(), Steering::Ambient);
    assert_eq!(draws_after_600, 60, "reply after > 100 ms gap must cap at 60 Hz once 600 ms elapses");
    assert_eq!(waits_after_600, 60);
}

#[test]
fn ambient_fps_composes_monotonically_with_low_power_mode_and_rates() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let period_60 = Duration::from_nanos(1_000_000_000 / 60);
    let frame_work = Duration::from_micros(900);

    // Low Power Mode (60 Hz) + ambient_fps(30.0) on 120 Hz -> 30 Hz wins.
    let mut lpm_30 = DisplayLinkCore::new(period_120);
    lpm_30.set_capped(true);
    lpm_30.set_ambient_period(crate::app::ambient_period_from_fps(30.0));
    let (draws, waits) = drive_stream(&mut lpm_30, t0, period_120, frame_work, 120, 0..120, |_, _, _| {});
    assert_eq!(draws, 30, "Low Power Mode + ambient_fps(30.0) must cap at 30 Hz");
    assert_eq!(waits, 90);

    // Low Power Mode (60 Hz) + ambient_fps(90.0) on 120 Hz -> Low Power Mode 60 Hz wins.
    let mut lpm_90 = DisplayLinkCore::new(period_120);
    lpm_90.set_capped(true);
    lpm_90.set_ambient_period(crate::app::ambient_period_from_fps(90.0));
    let (draws, waits) = drive_stream(&mut lpm_90, t0, period_120, frame_work, 120, 0..120, |_, _, _| {});
    assert_eq!(draws, 60, "Low Power Mode + ambient_fps(90.0) must stay capped at 60 Hz");
    assert_eq!(waits, 60);

    // Low Power Mode (60 Hz) + steered input -> Low Power Mode 60 Hz still bounds steered frames.
    let mut lpm_steered = DisplayLinkCore::new(period_120);
    lpm_steered.set_capped(true);
    lpm_steered.set_ambient_period(crate::app::ambient_period_from_fps(30.0));
    let (draws, waits) = drive_stream(&mut lpm_steered, t0, period_120, frame_work, 120, 0..120, |core, i, now| {
        if i % 2 == 0 { core.note_input(now); }
    });
    assert_eq!(draws, 60, "steered input in Low Power Mode lifts ambient 30 Hz back to Low Power 60 Hz");
    assert_eq!(waits, 60);

    // ambient_fps(30.0) on a 60 Hz panel -> 30 Hz (alternates Tick / Wait).
    let mut panel_60_cap_30 = DisplayLinkCore::new(period_60);
    panel_60_cap_30.set_ambient_period(crate::app::ambient_period_from_fps(30.0));
    let (draws, waits) = drive_stream(&mut panel_60_cap_30, t0, period_60, frame_work, 60, 0..60, |_, _, _| {});
    assert_eq!(draws, 30, "ambient_fps(30.0) on 60 Hz panel must draw 30 frames/s");
    assert_eq!(waits, 30);
}

#[test]
fn steering_state_machine_handles_boundaries_and_re_input() {
    let t0 = crate::time::now();
    let ms = |n| t0 + Duration::from_millis(n);
    let mut s = Steering::default();
    assert_eq!(s, Steering::Ambient);
    assert!(!s.is_steered(t0));

    // Ambient ignores Presented.
    let _ = step_steering(&mut s, SteeringEvent::Presented { at: ms(10) });
    assert_eq!(s, Steering::Ambient);

    // Input enters Steered { run: None }; exact STEERED_IDLE (600 ms) boundary is inclusive.
    let _ = step_steering(&mut s, SteeringEvent::Input { at: ms(0) });
    assert!(s.is_steered(ms(600)));
    assert!(!s.is_steered(ms(601)));

    // First present within STEERED_RUN_GAP (100 ms) starts Active.
    let _ = step_steering(&mut s, SteeringEvent::Presented { at: ms(100) });
    assert_eq!(s, Steering::Steered { last_input: ms(0), run: SteeredRun::Active { last_present: ms(100) } });

    // Input while Active preserves Active if within 100 ms of last_present, else drops to None.
    let _ = step_steering(&mut s, SteeringEvent::Input { at: ms(200) });
    assert_eq!(s, Steering::Steered { last_input: ms(200), run: SteeredRun::Active { last_present: ms(100) } });
    let _ = step_steering(&mut s, SteeringEvent::Input { at: ms(201) });
    assert_eq!(s, Steering::Steered { last_input: ms(201), run: SteeredRun::None });

    // Present past STEERED_RUN_GAP from last_input (101 ms) does not start Active.
    let _ = step_steering(&mut s, SteeringEvent::Presented { at: ms(302) });
    assert_eq!(s, Steering::Steered { last_input: ms(201), run: SteeredRun::None });

    // Present past STEERED_IDLE with no active run transitions cleanly to Ambient.
    let _ = step_steering(&mut s, SteeringEvent::Presented { at: ms(802) });
    assert_eq!(s, Steering::Ambient);
}
