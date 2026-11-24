use std::time::Duration;

use super::*;
use crate::el::Tx;
use crate::runtime::Runtime;
use crate::window::surface::TIMER_FRAME;

struct SteppedHost {
    rt: Runtime<u32>,
    wall_start: Instant,
    now_s: f64,
    pace: PollPace,
    frames: u32,
    samples: u32,
    active: bool,
}

impl SteppedHost {
    fn new(poll: fn(&mut u32, f64, &mut Tx) -> bool) -> Self {
        fn view(n: &u32, _: &mut crate::Cx<u32>) -> crate::El {
            crate::col().w_full().h_full().child(
                crate::el().key("box").size(10.0 + (*n % 80) as f32, 10.0).fill(crate::Color::WHITE),
            )
        }
        let mut rt = Runtime::new(0u32, view, (100.0, 100.0), crate::Env::default());
        let wall_start = crate::time::now();
        rt.wall_start = Some(wall_start);
        rt.frame.set_test_clock(crate::ManualClock::new(165.0));
        rt.poll = Some(poll);
        // Initial frame at t = 0.
        rt.frame(0.0);
        Self { rt, wall_start, now_s: 0.0, pace: PollPace::Idle, frames: 0, samples: 0, active: true }
    }

    fn now(&self) -> Instant {
        self.wall_start + Duration::from_secs_f64(self.now_s)
    }

    fn seek_to(&mut self, at: Instant) {
        let secs = at.duration_since(self.wall_start).as_secs_f64();
        self.now_s = self.now_s.max(secs);
        self.rt.frame.test_clock_mut().unwrap().seek(self.now_s);
    }

    fn sync(&mut self, grid: Option<VSyncGrid>) {
        let now = self.now();
        let active = self.active;
        let samples = &mut self.samples;
        self.pace.sync_with(self.rt.core(), now, active, |_prev| {
            *samples += 1;
            grid
        });
    }

    fn wake_deadline(&self) -> Option<Instant> {
        self.pace.deadline_for(self.rt.core(), self.active)
    }

    /// Runs the stepped host until `end` (wall clock), firing each due wake on `grid`,
    /// simulating `build_time` between wake and frame finish. Returns the wake instants.
    fn run_for(&mut self, duration: Duration, grid: Option<VSyncGrid>, build_time: Duration) -> Vec<Instant> {
        let end = self.now() + duration;
        let mut ticks = Vec::new();
        self.sync(grid);
        while let Some(due) = self.wake_deadline() {
            if due > end { break; }
            self.seek_to(due);
            ticks.push(self.now());
            // Mirror Host::new_events_in_turn resting_wake -> RedrawRequested -> frame -> turn_ends.
            let stale = self.rt.resting_wake().unwrap_or(true);
            let frame = stale || self.rt.stale();
            self.rt.resting_wake_frame(frame);
            if frame {
                // Simulate Windows separating WM_PAINT into the next turn: turn_ends runs with
                // poll_wake_deadline == None before the frame draws.
                self.sync(grid);
                self.seek_to(due + build_time);
                self.rt.frame(0.0);
                self.frames += 1;
            }
            self.sync(grid);
        }
        ticks
    }
}

fn keep_asking(n: &mut u32, _: f64, tx: &mut Tx) -> bool {
    *n += 1;
    tx.wake_after(Duration::ZERO);
    true
}

fn stop_after_five(n: &mut u32, _: f64, tx: &mut Tx) -> bool {
    *n += 1;
    if *n < 5 {
        tx.wake_after(Duration::ZERO);
        true
    } else {
        false
    }
}

fn idle_poll(_: &mut u32, _: f64, _: &mut Tx) -> bool {
    false
}

#[test]
fn poll_pace_ticks_at_165hz_and_120hz_with_stepped_clock() {
    for (hz, expected_min, expected_max) in [(165u32, 164, 166), (120, 119, 121), (144, 143, 145), (240, 239, 241), (60, 59, 61)] {
        let mut host = SteppedHost::new(keep_asking);
        let period = Duration::from_nanos(1_000_000_000 / u64::from(hz));
        let grid = VSyncGrid { timebase: host.wall_start, interval: period };
        let ticks = host.run_for(Duration::from_secs(1), Some(grid), Duration::from_micros(1500));
        assert!(
            (expected_min..=expected_max).contains(&ticks.len()),
            "{hz} Hz display: expected {expected_min}..={expected_max} ticks in 1 s, got {}",
            ticks.len()
        );
        assert_eq!(host.frames as usize, ticks.len());
        // Every tick aligns to the vblank grid and spaces by one refresh period.
        for pair in ticks.windows(2) {
            let delta = pair[1].duration_since(pair[0]);
            assert_eq!(delta, period, "{hz} Hz tick spacing drifted: {delta:?} != {period:?}");
        }
        // Grid is cached across turns (even with WM_PAINT split turns): at most 2 samples in 1 s.
        assert!(host.samples <= 2, "{hz} Hz sampled DWM {} times in 1 s", host.samples);
    }
}

#[test]
fn idle_window_asks_for_zero_frames_and_stops_when_poll_finishes() {
    // Truly idle window: poll never arms a wake.
    let mut idle = SteppedHost::new(idle_poll);
    let period = Duration::from_nanos(1_000_000_000 / 165);
    let grid = VSyncGrid { timebase: idle.wall_start, interval: period };
    let ticks = idle.run_for(Duration::from_secs(1), Some(grid), Duration::from_micros(500));
    assert!(ticks.is_empty(), "idle window must wake zero times");
    assert_eq!(idle.frames, 0, "idle window must draw zero frames");
    assert_eq!(idle.wake_deadline(), None, "idle window must arm no timer");
    assert_eq!(idle.samples, 0, "idle window must not query DWM");
    assert_eq!(idle.pace, PollPace::Idle);

    // Poll that animates for 5 ticks then stops asking: draws 4 more frames then rests with 0 wakes.
    let mut finite = SteppedHost::new(stop_after_five);
    let grid = VSyncGrid { timebase: finite.wall_start, interval: period };
    let ticks = finite.run_for(Duration::from_secs(1), Some(grid), Duration::from_micros(500));
    assert_eq!(ticks.len(), 4, "4 ticks after initial poll bring count to 5 then stop");
    assert_eq!(finite.rt.state, 5);
    assert_eq!(finite.wake_deadline(), None, "stopped poll must leave no timer armed");
}

#[test]
fn judder_guard_prevents_double_ticks_and_late_wakes_stay_on_grid() {
    let t0 = crate::time::now();
    let period = Duration::from_nanos(1_000_000_000 / 165); // 6.060606 ms
    let grid = VSyncGrid { timebase: t0, interval: period };

    // Exact on vblank -> advances one full period.
    assert_eq!(next_tick(t0, grid), t0 + period);
    // 50 us early before tick 1 -> judder guard (500 us) advances to tick 2, no double tick.
    let early = t0 + period - Duration::from_micros(50);
    assert_eq!(next_tick(early, grid), t0 + period * 2);
    // 200 us late after tick 1 -> stays locked to tick 2, no drift.
    let late = t0 + period + Duration::from_micros(200);
    assert_eq!(next_tick(late, grid), t0 + period * 2);
    // 600 us before tick 1 (outside 500 us guard) -> snaps to tick 1.
    let before = t0 + period - Duration::from_micros(600);
    assert_eq!(next_tick(before, grid), t0 + period);
    // Timebase in the future still snaps cleanly backwards/forwards on the grid.
    let future_grid = VSyncGrid { timebase: t0 + period * 3, interval: period };
    assert_eq!(snap_to_next_tick(t0, future_grid), t0);
    assert_eq!(next_tick(t0, future_grid), t0 + period);
}

#[test]
fn mixed_rate_multi_monitor_follows_windows_monitor_and_preserves_phase() {
    let t0 = crate::time::now();
    let dwm_165 = VSyncGrid { timebase: t0, interval: Duration::from_nanos(1_000_000_000 / 165) };
    // Matching monitor (within 5%, e.g. fractional 164.99 vs 165_000 mHz) keeps DWM's exact grid.
    let exact = resolve_grid(Some(dwm_165), Some(165_000), None, t0 + Duration::from_millis(2));
    assert_eq!(exact, Some(dwm_165));

    // Win11 SV3+: DWM reports 165 Hz (fastest monitor), window is on 60 Hz secondary monitor.
    let mon_60 = resolve_grid(Some(dwm_165), Some(60_000), None, t0 + Duration::from_millis(2)).unwrap();
    assert_eq!(mon_60.interval, Duration::from_nanos(1_000_000_000_000 / 60_000));
    assert_eq!(mon_60.timebase, t0 + Duration::from_millis(2));

    // Re-sampling 1 s later on the same 60 Hz monitor preserves its phase anchor.
    let resampled = resolve_grid(Some(dwm_165), Some(60_000), Some(mon_60), t0 + Duration::from_secs(1)).unwrap();
    assert_eq!(resampled.timebase, mon_60.timebase);
    assert_eq!(resampled.interval, mon_60.interval);

    // DWM unavailable: falls back to monitor refresh rate.
    let mon_144 = resolve_grid(None, Some(144_000), None, t0).unwrap();
    assert_eq!(mon_144.interval, Duration::from_nanos(1_000_000_000_000 / 144_000));

    // Both unavailable: returns None (host falls back to TIMER_FRAME).
    assert_eq!(resolve_grid(None, None, None, t0), None);
}

#[test]
fn dwm_validation_rejects_bogus_driver_values() {
    let now = crate::time::now();
    let freq = 10_000_000i64; // 10 MHz QPC (100 ns ticks)
    // Valid 120 Hz DWM timing (8.3333 ms = 83_333 ticks).
    let g = dwm_grid_from_raw(now, 1_000_000, freq, 950_000, 83_333, 120, 1).unwrap();
    assert_eq!(g.interval, Duration::from_nanos(8_333_300));
    assert_eq!(g.timebase, now - Duration::from_millis(5));

    // crbug.com/1499654: qpcVBlank > i64::MAX rejected.
    assert_eq!(dwm_grid_from_raw(now, 1_000_000, freq, i64::MAX as u64 + 1, 83_333, 120, 1), None);
    // Driver bug: qpcRefreshPeriod < 1 ms (e.g. 29 us) falls back to rateRefresh.
    let fb = dwm_grid_from_raw(now, 1_000_000, freq, 1_000_000, 290, 165, 1).unwrap();
    assert_eq!(fb.interval, Duration::from_nanos(1_000_000_000 / 165));
    // Driver bug: qpcRefreshPeriod < rateRefresh / 2 falls back to rateRefresh.
    let fb2 = dwm_grid_from_raw(now, 1_000_000, freq, 1_000_000, 20_000, 60, 1).unwrap();
    assert_eq!(fb2.interval, Duration::from_nanos(1_000_000_000 / 60));
    // Zero frequency or both invalid -> None.
    assert_eq!(dwm_grid_from_raw(now, 1_000_000, 0, 1_000_000, 83_333, 120, 1), None);
    assert_eq!(dwm_grid_from_raw(now, 1_000_000, freq, 1_000_000, 0, 0, 0), None);
}

#[test]
fn hidden_covered_and_fallback_use_timer_frame_and_long_timers_keep_their_time() {
    let mut host = SteppedHost::new(keep_asking);
    let period_165 = Duration::from_nanos(1_000_000_000 / 165);
    let grid = VSyncGrid { timebase: host.wall_start, interval: period_165 };

    // Inactive (hidden/covered/minimized): resets to Idle and falls back to 60 Hz TIMER_FRAME.
    host.active = false;
    host.sync(Some(grid));
    assert_eq!(host.pace, PollPace::Idle);
    assert_eq!(host.samples, 0);
    assert_eq!(host.wake_deadline(), Some(host.wall_start + TIMER_FRAME));

    // Active, but OS returns no grid: enters Fallback and does not spam DWM before REFRESH_TTL.
    host.active = true;
    host.sync(None);
    assert!(matches!(host.pace, PollPace::Fallback { .. }));
    assert_eq!(host.samples, 1);
    assert_eq!(host.wake_deadline(), Some(host.wall_start + TIMER_FRAME));
    host.seek_to(host.wall_start + Duration::from_millis(50));
    host.sync(None);
    assert_eq!(host.samples, 1, "Fallback caches failure for REFRESH_TTL");

    // DisplayChanged drops Fallback and re-samples on next sync.
    let _ = host.pace.step(PollPaceEvent::DisplayChanged);
    host.sync(Some(grid));
    assert_eq!(host.samples, 2);
    assert!(matches!(host.pace, PollPace::Armed { .. }));

    // Sub-60Hz explicit timer (10 ms on 165 Hz display) fires at 10 ms instead of 16.67 ms.
    fn ten_ms(_: &mut u32, _: f64, tx: &mut Tx) -> bool {
        tx.wake_after(Duration::from_millis(10));
        false
    }
    let mut fast_timer = SteppedHost::new(ten_ms);
    let g = VSyncGrid { timebase: fast_timer.wall_start, interval: period_165 };
    fast_timer.sync(Some(g));
    assert_eq!(fast_timer.wake_deadline(), Some(fast_timer.wall_start + Duration::from_millis(10)));

    // Long timer (100 ms): keeps exact 100 ms deadline without sampling DWM.
    fn hundred_ms(_: &mut u32, _: f64, tx: &mut Tx) -> bool {
        tx.wake_after(Duration::from_millis(100));
        false
    }
    let mut slow_timer = SteppedHost::new(hundred_ms);
    slow_timer.sync(Some(g));
    assert_eq!(slow_timer.samples, 0);
    assert_eq!(slow_timer.wake_deadline(), Some(slow_timer.wall_start + Duration::from_millis(100)));
}

#[test]
fn armed_deadline_stays_fixed_across_turns_and_wake_after_idle_fires_immediately() {
    let mut host = SteppedHost::new(keep_asking);
    let period = Duration::from_nanos(1_000_000_000 / 165);
    let grid = VSyncGrid { timebase: host.wall_start, interval: period };
    host.sync(Some(grid));
    let armed_at = host.wake_deadline().expect("armed");
    assert_eq!(armed_at, host.wall_start + period);

    // Repeated syncs before the tick fires (e.g. pointer moves) keep the exact same deadline
    // even if a hypothetical DWM re-query would report a jittered timebase.
    let jittered = VSyncGrid {
        timebase: host.wall_start + Duration::from_micros(90),
        interval: period,
    };
    host.seek_to(host.wall_start + Duration::from_millis(2));
    host.sync(Some(jittered));
    assert_eq!(host.wake_deadline(), Some(armed_at));
    assert_eq!(host.samples, 1, "no re-query while the same wake is armed");

    // Reset clears Armed back to Idle.
    let _ = host.pace.step(PollPaceEvent::Reset);
    assert_eq!(host.pace, PollPace::Idle);

    // An external wake_after(ZERO) after 1 s of idle fires immediately (<= now), not held to vblank.
    let mut rested = SteppedHost::new(idle_poll);
    rested.seek_to(rested.wall_start + Duration::from_secs(1));
    rested.rt.update_if(|_, tx| { tx.wake_after(Duration::ZERO); false });
    rested.rt.schedule_pending_wakes();
    rested.sync(Some(VSyncGrid { timebase: rested.wall_start, interval: period }));
    assert_eq!(rested.samples, 0, "overdue wake after idle needs no DWM query");
    assert!(rested.wake_deadline().expect("due") <= rested.now());
}
