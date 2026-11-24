//! Windows: paces a shown, uncovered window's or overlay's poll-driven frames
//! (`Tx::wake_after(Duration::ZERO)`) onto the display's vblank grid instead of holding them to
//! the 60 Hz `TIMER_FRAME` floor.
//!
//! Follows Chromium's `VSyncProviderWin` (`ui/gl/vsync_provider_win.cc`) and
//! `DelayBasedTimeSource` (`components/viz/common/frame_sinks/delay_based_time_source.cc`):
//! - While no poll timer is armed (idle), stays `Idle` / `Cached` with no deadline and asks for
//!   zero frames.
//! - When a poll asks sooner than `TIMER_FRAME`, aligns sub-refresh asks onto the display's
//!   vblank grid (`next_tick`, with Chromium's 500 us judder guard so early timer wakes or
//!   vblank-timestamp jitter never double-tick).
//! - Once armed for a poll, keeps its deadline fixed until that poll fires, the runtime's wake
//!   changes, or the window hides/covers/moves - so `wake_deadline` and `new_events` always agree.
//! - On Windows 11 22H2+ (SV3+), `DwmGetCompositionTimingInfo` tracks the fastest active display
//!   rather than necessarily the window's monitor, so `resolve_grid` uses DWM's hardware grid when
//!   it agrees with the window's monitor refresh rate and falls back to the window's monitor
//!   period on mixed-rate displays.

use std::time::Duration;

use crate::machine::Effects;
use crate::runtime::RuntimeCore;
use crate::time::Instant;
use crate::window::surface::{
    TIMER_FRAME, motion_wake_wall_core, poll_wake_wall_core, timer_deadline_core,
};

/// How often an active pacer re-samples DWM timing and the window's monitor refresh rate.
pub(crate) const REFRESH_TTL: Duration = Duration::from_secs(1);

/// Chromium's `DelayBasedTimeSource::kMaxJudderAllowed` (500 us): if a wake lands within this
/// window before the next grid tick, advance one more interval instead of firing a double tick.
pub(crate) const JUDDER_GUARD: Duration = Duration::from_micros(500);

/// Valid refresh-period bounds (10 Hz ..= 1000 Hz), rejecting driver bugs such as a 29 us
/// `qpcRefreshPeriod` (Chromium `TryGetVSyncParamsFromDwmCompInfo`).
pub(crate) const MIN_REFRESH: Duration = Duration::from_millis(1);
pub(crate) const MAX_REFRESH: Duration = Duration::from_millis(100);

/// Display refresh timing for snapping poll ticks onto the vblank grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VSyncGrid {
    /// Known vblank phase anchor (`qpcVBlank` or synthetic anchor on mixed-rate monitors).
    pub(crate) timebase: Instant,
    /// One display refresh period (e.g. 8.333 ms at 120 Hz, 6.061 ms at 165 Hz).
    pub(crate) interval: Duration,
}

/// Explicit state machine for Windows poll pacing on one window or overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PollPace {
    /// No paced poll tick is pending and no display grid is cached.
    #[default]
    Idle,
    /// No paced poll tick is pending, keeping the window's display grid warm across turns
    /// (such as between `new_events` and `WM_PAINT`) until `REFRESH_TTL`, hide or move.
    Cached {
        grid: VSyncGrid,
        sampled_at: Instant,
    },
    /// A poll asked for a frame within the next display refresh; the loop wakes at `at`.
    Armed {
        polled: Instant,
        due: Instant,
        at: Instant,
        grid: VSyncGrid,
        sampled_at: Instant,
    },
    /// Neither DWM nor the monitor yielded a valid refresh rate; falls back to `TIMER_FRAME`
    /// until `REFRESH_TTL` or a display change.
    Fallback {
        sampled_at: Instant,
    },
}

/// Events driving [`PollPace`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PollPaceEvent {
    /// Sync with the runtime's current poll wake before the loop sleeps (`turn_ends`).
    Sync {
        now: Instant,
        active: bool,
        poll: Option<(Instant, Instant)>,
    },
    /// The OS answered [`PollPaceEffect::SampleGrid`] with the resolved grid (or `None`).
    Grid {
        now: Instant,
        polled: Instant,
        due: Instant,
        grid: Option<VSyncGrid>,
    },
    /// The window hid, minimized, or became covered: stop pacing and drop cached display timing.
    Reset,
    /// The window moved or changed scale/monitor: drop cached display timing so the next sync
    /// samples the new monitor.
    DisplayChanged,
}

/// Effects requested by [`PollPace::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PollPaceEffect {
    /// Sample DWM composition timing and the window's monitor refresh rate, then feed
    /// [`PollPaceEvent::Grid`] back into [`PollPace::step`].
    SampleGrid { polled: Instant, due: Instant },
}

impl PollPace {
    /// Steps the pacer state machine for `event`, returning any OS query effect to run.
    pub(crate) fn step(&mut self, event: PollPaceEvent) -> Effects<PollPaceEffect> {
        match event {
            PollPaceEvent::Reset | PollPaceEvent::DisplayChanged => {
                *self = PollPace::Idle;
                Effects::none()
            }
            PollPaceEvent::Sync { active: false, .. } => {
                *self = PollPace::Idle;
                Effects::none()
            }
            PollPaceEvent::Sync { now, active: true, poll: None } => {
                *self = match *self {
                    PollPace::Armed { grid, sampled_at, .. }
                    | PollPace::Cached { grid, sampled_at }
                        if now < sampled_at + REFRESH_TTL =>
                    {
                        PollPace::Cached { grid, sampled_at }
                    }
                    PollPace::Fallback { sampled_at } if now < sampled_at + REFRESH_TTL => {
                        PollPace::Fallback { sampled_at }
                    }
                    _ => PollPace::Idle,
                };
                Effects::none()
            }
            PollPaceEvent::Sync { now, active: true, poll: Some((polled, due)) } => {
                // Timers at or beyond TIMER_FRAME (including wakes after idle) keep their own
                // time without querying DWM or snapping to vblank.
                if due >= polled + TIMER_FRAME {
                    *self = match *self {
                        PollPace::Armed { grid, sampled_at, .. }
                        | PollPace::Cached { grid, sampled_at }
                            if now < sampled_at + REFRESH_TTL =>
                        {
                            PollPace::Cached { grid, sampled_at }
                        }
                        PollPace::Fallback { sampled_at } if now < sampled_at + REFRESH_TTL => {
                            PollPace::Fallback { sampled_at }
                        }
                        _ => PollPace::Idle,
                    };
                    return Effects::none();
                }
                match *self {
                    PollPace::Armed {
                        polled: armed_polled,
                        due: armed_due,
                        grid,
                        sampled_at,
                        ..
                    } => {
                        // Keep an already-armed tick fixed until it fires or changes so
                        // `wake_deadline` and `new_events` always agree.
                        if armed_polled == polled && armed_due == due {
                            return Effects::none();
                        }
                        if now < sampled_at + REFRESH_TTL {
                            let at = pace_deadline(polled, due, grid);
                            *self = PollPace::Armed { polled, due, at, grid, sampled_at };
                            return Effects::none();
                        }
                        Effects::one(PollPaceEffect::SampleGrid { polled, due })
                    }
                    PollPace::Cached { grid, sampled_at } if now < sampled_at + REFRESH_TTL => {
                        let at = pace_deadline(polled, due, grid);
                        *self = PollPace::Armed { polled, due, at, grid, sampled_at };
                        Effects::none()
                    }
                    PollPace::Fallback { sampled_at } if now < sampled_at + REFRESH_TTL => {
                        Effects::none()
                    }
                    PollPace::Idle | PollPace::Cached { .. } | PollPace::Fallback { .. } => {
                        Effects::one(PollPaceEffect::SampleGrid { polled, due })
                    }
                }
            }
            PollPaceEvent::Grid { now, polled, due, grid } => {
                match grid.filter(|g| valid_period(g.interval)) {
                    Some(grid) => {
                        let at = pace_deadline(polled, due, grid);
                        *self = PollPace::Armed { polled, due, at, grid, sampled_at: now };
                    }
                    None => {
                        *self = PollPace::Fallback { sampled_at: now };
                    }
                }
                Effects::none()
            }
        }
    }

    /// The last sampled display grid, so re-sampling on a mixed-rate monitor keeps phase.
    pub(crate) fn previous_grid(&self) -> Option<VSyncGrid> {
        match *self {
            PollPace::Cached { grid, .. } | PollPace::Armed { grid, .. } => Some(grid),
            PollPace::Idle | PollPace::Fallback { .. } => None,
        }
    }

    /// Syncs with `rt`'s current poll wake at `now`, invoking `sample` only when the machine
    /// emits [`PollPaceEffect::SampleGrid`].
    pub(crate) fn sync_with(
        &mut self,
        rt: &RuntimeCore,
        now: Instant,
        active: bool,
        mut sample: impl FnMut(Option<VSyncGrid>) -> Option<VSyncGrid>,
    ) {
        let poll = poll_wake_wall_core(rt).and_then(|(polled, due)| Some((polled?, due)));
        let previous = self.previous_grid();
        for effect in self.step(PollPaceEvent::Sync { now, active, poll }) {
            match effect {
                PollPaceEffect::SampleGrid { polled, due } => {
                    let grid = sample(previous);
                    let _ = self.step(PollPaceEvent::Grid { now, polled, due, grid });
                }
            }
        }
    }

    /// The runtime's next timer deadline as both `wake_deadline` and `new_events` see it.
    /// Pure read: never mutates state or calls the OS, so both callers always agree.
    pub(crate) fn deadline_for(&self, rt: &RuntimeCore, active: bool) -> Option<Instant> {
        if !active {
            return timer_deadline_core(rt);
        }
        let poll = poll_wake_wall_core(rt).map(|(polled, due)| match (polled, *self) {
            (
                Some(polled),
                PollPace::Armed { polled: armed_polled, due: armed_due, at, .. },
            ) if armed_polled == polled && armed_due == due => at,
            (Some(polled), _) => due.max(polled + TIMER_FRAME),
            (None, _) => due,
        });
        poll.into_iter().chain(motion_wake_wall_core(rt)).min()
    }
}

fn pace_deadline(polled: Instant, due: Instant, grid: VSyncGrid) -> Instant {
    if due < polled + grid.interval {
        next_tick(polled, grid)
    } else {
        due
    }
}

fn valid_period(period: Duration) -> bool {
    period >= MIN_REFRESH && period <= MAX_REFRESH
}

fn periods_match(a: Duration, b: Duration) -> bool {
    let diff = a.abs_diff(b).as_nanos();
    let max = a.max(b).as_nanos();
    max > 0 && diff * 20 <= max // within 5%
}

/// Validates raw `DWM_TIMING_INFO` + QPC values and converts them to a [`VSyncGrid`] on `now`'s
/// clock, matching Chromium's `TryGetVSyncParamsFromDwmCompInfo` (`ui/gl/vsync_provider_win.cc`).
pub(crate) fn dwm_grid_from_raw(
    now: Instant,
    qpc_now: i64,
    qpc_freq: i64,
    qpc_vblank: u64,
    qpc_refresh_period: u64,
    rate_num: u32,
    rate_den: u32,
) -> Option<VSyncGrid> {
    if qpc_freq <= 0 || qpc_vblank > i64::MAX as u64 || qpc_refresh_period > i64::MAX as u64 {
        return None;
    }
    let freq = qpc_freq as u128;
    let rate_interval = (rate_num > 0 && rate_den > 0).then(|| {
        Duration::from_nanos((u128::from(rate_den) * 1_000_000_000 / u128::from(rate_num)) as u64)
    });
    let qpc_interval = Duration::from_nanos((u128::from(qpc_refresh_period) * 1_000_000_000 / freq) as u64);
    let interval = if qpc_interval < MIN_REFRESH
        || rate_interval.is_some_and(|r| !r.is_zero() && qpc_interval < r / 2)
    {
        rate_interval?
    } else {
        qpc_interval
    };
    if !valid_period(interval) {
        return None;
    }
    let delta_ticks = i128::from(qpc_vblank as i64) - i128::from(qpc_now);
    let delta_ns = (delta_ticks.unsigned_abs() * 1_000_000_000 / freq) as u64;
    let delta = Duration::from_nanos(delta_ns);
    let timebase = if delta_ticks >= 0 {
        now.checked_add(delta)?
    } else {
        now.checked_sub(delta)?
    };
    Some(VSyncGrid { timebase, interval })
}

/// Resolves the window's display vblank grid from DWM timing and the window's monitor refresh
/// rate in millihertz, following Chromium's `VSyncProviderWin` mixed-rate monitor rules.
pub(crate) fn resolve_grid(
    dwm: Option<VSyncGrid>,
    monitor_mhz: Option<u32>,
    previous: Option<VSyncGrid>,
    now: Instant,
) -> Option<VSyncGrid> {
    let dwm = dwm.filter(|g| valid_period(g.interval));
    let monitor = monitor_mhz
        .filter(|&mhz| mhz >= 1_000)
        .map(|mhz| Duration::from_nanos(1_000_000_000_000u64 / u64::from(mhz)))
        .filter(|&d| valid_period(d));
    match (dwm, monitor) {
        (Some(dwm), Some(mon)) if periods_match(dwm.interval, mon) => Some(dwm),
        (_, Some(mon)) => {
            let timebase = previous
                .filter(|prev| periods_match(prev.interval, mon))
                .map_or(now, |prev| prev.timebase);
            Some(VSyncGrid { timebase, interval: mon })
        }
        (Some(dwm), None) => Some(dwm),
        (None, None) => None,
    }
}

/// The earliest tick `timebase + k * interval` at or after `target` (Chromium's
/// `base::TimeTicks::SnappedToNextTick`).
pub(crate) fn snap_to_next_tick(target: Instant, grid: VSyncGrid) -> Instant {
    let period_ns = grid.interval.as_nanos();
    debug_assert!(period_ns > 0);
    if target >= grid.timebase {
        let diff = target.duration_since(grid.timebase).as_nanos();
        let rem = diff % period_ns;
        if rem == 0 {
            target
        } else {
            target + Duration::from_nanos((period_ns - rem) as u64)
        }
    } else {
        let diff = grid.timebase.duration_since(target).as_nanos();
        let rem = diff % period_ns;
        target + Duration::from_nanos(rem as u64)
    }
}

/// The next display tick strictly after `anchor` on `grid`, advancing an extra interval when
/// `anchor` falls within `JUDDER_GUARD` (500 us, clamped to `interval / 4`) of the snapped
/// tick so early timer wakes or vblank jitter never double-tick (Chromium's
/// `DelayBasedTimeSource::PostNextTickTask`).
pub(crate) fn next_tick(anchor: Instant, grid: VSyncGrid) -> Instant {
    let mut tick = snap_to_next_tick(anchor, grid);
    let guard = JUDDER_GUARD.min(grid.interval / 4);
    if anchor + guard >= tick {
        tick += grid.interval;
    }
    tick
}

#[cfg(test)]
#[path = "poll_pace_tests.rs"]
mod tests;
