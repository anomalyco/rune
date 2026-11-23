//! One drawable submission per display tick. Metal's drawable pool is not a frame clock:
//! acquisition may return immediately even with FIFO presentation enabled. The exception is a
//! frame after rest, such as the first after a key press: it draws at once (`rested`).
//!
//! Low Power Mode caps a ProMotion panel at 60 Hz while CVDisplayLink, CADisplayLink and
//! NSScreen all still report 120 Hz. Submitting on every tick then fills the drawable queue
//! with a few fast frames before backpressure settles it at 60 Hz, so pace at the cap instead.
#![allow(deprecated)] // CVDisplayLink supports the same macOS versions as winit.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{CVDisplayLink, CVGetCurrentHostTime, CVGetHostClockFrequency, CVOptionFlags, CVReturn, CVTime, CVTimeFlags, CVTimeStamp, CVTimeStampFlags};
use winit::{platform::macos::MonitorHandleExtMacOS, window::Window};

struct Callback {
    target: AtomicU64,
    period_ns: AtomicU64,
    wake: crate::runtime::Waker,
    host_frequency: f64,
}

pub(crate) struct DisplayLink {
    link: CFRetained<CVDisplayLink>,
    // Stable allocation, retained until stop has joined the callback thread.
    callback: Box<Callback>,
    last_present: Option<Instant>,
    last_target: Option<Instant>,
    display: Option<u32>,
    capped: bool,
    running: bool,
}

const LOW_POWER_FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);
static CREATE_FAILED: AtomicBool = AtomicBool::new(false);
/// The last creation's CoreVideo status (0 once one succeeds), and when it failed.
static LAST_FAILURE: Mutex<Option<(i32, Instant)>> = Mutex::new(None);
/// How long after a failed creation `retry` tries again.
const RETRY: Duration = Duration::from_secs(1);

/// Why the last display link couldn't be made, while no creation has succeeded since.
pub(crate) fn failure() -> Option<i32> { LAST_FAILURE.lock().unwrap().map(|(status, _)| status) }

/// How a surface's frames are paced, from its display link (or the lack of one).
pub(crate) fn pacing(link: Option<&DisplayLink>, moving: bool) -> crate::frame::surface_stats::Pacing {
    use crate::frame::surface_stats::Pacing;
    match (link, failure()) {
        (Some(link), _) if link.running() => Pacing::DisplayLink,
        (None, Some(status)) => Pacing::Failed(status),
        _ if moving => Pacing::Unpaced,
        _ => Pacing::Idle,
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" { fn CGDisplayIsBuiltin(display: u32) -> u32; }

fn low_power_caps(display: Option<u32>) -> bool {
    use objc2_foundation::{NSObjectProtocol, NSProcessInfo};
    let info = NSProcessInfo::processInfo();
    // SAFETY: `CGDisplayIsBuiltin` accepts any `CGDirectDisplayID`.
    display.is_some_and(|display| unsafe { CGDisplayIsBuiltin(display) } != 0)
        && info.respondsToSelector(objc2::sel!(isLowPowerModeEnabled))
        && info.isLowPowerModeEnabled()
}

unsafe extern "C-unwind" fn tick(
    _: NonNull<CVDisplayLink>, _: NonNull<CVTimeStamp>, output: NonNull<CVTimeStamp>,
    _: CVOptionFlags, _: NonNull<CVOptionFlags>, context: *mut c_void,
) -> CVReturn {
    // SAFETY: CoreVideo borrows the boxed context until DisplayLink::drop stops the link.
    let callback = unsafe { &*context.cast::<Callback>() };
    // SAFETY: CoreVideo provides a valid timestamp for this callback invocation.
    let output = unsafe { output.as_ref() };
    if let Some(ns) = cv_period_ns(output.videoRefreshPeriod, output.videoTimeScale) {
        callback.period_ns.store(ns, Ordering::Relaxed);
    }
    let target = if output.flags & CVTimeStampFlags::HostTimeValid.0 != 0 {
        output.hostTime
    } else {
        CVGetCurrentHostTime() + (callback.period_ns.load(Ordering::Relaxed) as f64 * 1e-9 * callback.host_frequency) as u64
    };
    let pending = callback.target.swap(target, Ordering::Release) != 0;
    // Coalesce a backed-up event loop; missed refreshes never become a catch-up burst.
    // Never call Window::request_redraw here: winit synchronously waits on the main
    // thread on macOS, which can itself be waiting in CVDisplayLinkStop.
    if !pending { (callback.wake)(); }
    0
}

impl DisplayLink {
    pub(crate) fn running(&self) -> bool { self.running }
    pub(crate) fn refresh_hz(&self) -> f64 { 1.0 / self.frame_period().as_secs_f64() }

    fn frame_period(&self) -> Duration {
        let period = Duration::from_nanos(self.callback.period_ns.load(Ordering::Relaxed));
        if self.capped { period.max(LOW_POWER_FRAME) } else { period }
    }

    pub(crate) fn new(window: &Window, wake: crate::runtime::Waker) -> Option<Self> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: valid out pointer; the create rule transfers one retained reference.
        let status = unsafe { CVDisplayLink::create_with_active_cg_displays(NonNull::from(&mut raw)) };
        // Every display asleep fails this until one wakes, so say so once per sleep.
        if status != 0 {
            if !CREATE_FAILED.swap(true, Ordering::Relaxed) { eprintln!("display link creation failed: {status}"); }
            *LAST_FAILURE.lock().unwrap() = Some((status, crate::time::now()));
            return None;
        }
        if CREATE_FAILED.swap(false, Ordering::Relaxed) { eprintln!("display link created once a display woke"); }
        *LAST_FAILURE.lock().unwrap() = None;
        // SAFETY: `create_with_active_cg_displays` succeeded (`status == 0`), so non-null `raw` is a +1 retained `CVDisplayLink` per the Create Rule.
        let link = unsafe { CFRetained::from_raw(NonNull::new(raw)?) };
        let mut callback = Box::new(Callback {
            target: AtomicU64::new(0),
            period_ns: AtomicU64::new(1_000_000_000 / 60),
            wake,
            host_frequency: CVGetHostClockFrequency(),
        });
        // SAFETY: `tick` matches `CVDisplayLinkOutputCallback`, and `callback` stays at a stable heap address until `DisplayLink::drop` stops the link.
        let status = unsafe { link.set_output_callback(Some(tick), (&mut *callback as *mut Callback).cast()) };
        if status != 0 { eprintln!("display link callback failed: {status}"); return None; }
        let mut pacing = Self { link, callback, last_present: None, last_target: None, display: None, capped: false, running: false };
        pacing.update_display(window);
        Some(pacing)
    }

    /// A surface presenting without a display link (its creation failed while the displays
    /// slept) tries again after each present, at most once a `RETRY` across surfaces.
    pub(crate) fn retry(window: &Window, wake: crate::runtime::Waker) -> Option<Self> {
        if LAST_FAILURE.lock().unwrap().is_some_and(|(_, at)| at.elapsed() < RETRY) { return None; }
        Self::new(window, wake)
    }

    /// AppKit monitor lookup is only needed on creation and display/visibility changes.
    pub(crate) fn update_display(&mut self, window: &Window) {
        let monitor = window.current_monitor();
        let display = monitor.as_ref().map(|m| m.native_id());
        let changed = display != self.display;
        if changed {
            if let Some(display) = display { self.link.set_current_cg_display(display); }
            self.display = display;
        }
        if (changed || !self.running)
            && let Some(ns) = nominal_period_ns(self.link.nominal_output_video_refresh_period())
                .or_else(|| monitor.as_ref().and_then(|m| m.refresh_rate_millihertz()).and_then(millihertz_period_ns))
        {
            self.callback.period_ns.store(ns, Ordering::Relaxed);
        }
        self.capped = low_power_caps(self.display);
    }

    /// Starts the link if it was stopped; false if Core Video refused to start it.
    pub(crate) fn start(&mut self) -> bool {
        if !self.running {
            let status = self.link.start();
            if status != 0 { eprintln!("display link start failed: {status}"); return false; }
            self.running = true;
            // Power state can change while idle; each burst rechecks it.
            self.capped = low_power_caps(self.display);
        }
        true
    }

    /// Claims one display opportunity: a tick and its target presentation time, or at once
    /// when the surface has rested (`rested`). Consume before rendering so a tick arriving
    /// during rendering can queue the next opportunity.
    pub(crate) fn next_frame(&mut self) -> Opportunity {
        if !self.start() { return Opportunity::Now; }
        let host_target = self.callback.target.swap(0, Ordering::Acquire);
        let now = crate::time::now();
        let tick = (host_target != 0).then(|| target_time(now, CVGetCurrentHostTime(), host_target, self.callback.host_frequency));
        let next = opportunity(tick, self.last_present, self.last_target, now, self.frame_period());
        match next {
            Opportunity::Tick(target) => self.last_target = Some(target),
            Opportunity::Now => self.last_target = Some(now),
            Opportunity::Wait => {}
        }
        next
    }

    /// Whether a frame that may draw nothing (a timer's, on a resting surface) can decide that
    /// before starting the link: it's stopped, and `next_frame` would answer `Now` whenever
    /// asked from here on, so starting it once the frame turns out to draw paces it the same.
    pub(crate) fn defers_start(&self, now: Instant) -> bool {
        defers_start(self.running, self.last_present, self.last_target, now, self.frame_period())
    }

    /// The tick after `now`, as if the link were running.
    pub(crate) fn tick_after(&self, now: Instant) -> Instant { now + self.frame_period() }

    pub(crate) fn presented(&mut self, at: Instant) {
        self.last_present = Some(at);
    }

    pub(crate) fn stop(&mut self) {
        if self.running { self.link.stop(); self.running = false; }
        self.callback.target.store(0, Ordering::Relaxed);
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) { self.stop(); }
}

fn cv_period_ns(value: i64, scale: i32) -> Option<u64> {
    if value <= 0 || scale <= 0 { return None; }
    let period = Duration::try_from_secs_f64(value as f64 / scale as f64).ok()?;
    u64::try_from(period.as_nanos()).ok().filter(|&ns| ns > 0)
}

fn nominal_period_ns(time: CVTime) -> Option<u64> {
    if time.flags & CVTimeFlags::IsIndefinite.0 != 0 { return None; }
    cv_period_ns(time.timeValue, time.timeScale)
}

fn millihertz_period_ns(mhz: u32) -> Option<u64> {
    if mhz == 0 { return None; }
    let period = Duration::try_from_secs_f64(1000.0 / f64::from(mhz)).ok()?;
    u64::try_from(period.as_nanos()).ok().filter(|&ns| ns > 0)
}

fn target_time(now: Instant, host_now: u64, host_target: u64, frequency: f64) -> Instant {
    let delta = Duration::from_secs_f64(host_target.abs_diff(host_now) as f64 / frequency);
    if host_target >= host_now { now + delta } else { now - delta }
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
fn opportunity(tick: Option<Instant>, last: Option<Instant>, last_target: Option<Instant>, now: Instant, period: Duration) -> Opportunity {
    // A late render followed by an on-time tick must skip, not submit two frames
    // together. Also discard stale callbacks after main-thread stalls.
    match tick {
        Some(target) if permits(last, last_target, target, now, period) => Opportunity::Tick(target),
        _ if rested(last, last_target, now, period) => Opportunity::Now,
        _ => Opportunity::Wait,
    }
}

fn permits(last: Option<Instant>, last_target: Option<Instant>, target: Instant, now: Instant, period: Duration) -> bool {
    let half = period / 2;
    now < target && last_target.is_none_or(|last| target >= last + period * 3 / 4)
        && last.is_none_or(|last| now.saturating_duration_since(last) >= half)
}

/// Whether the last frame has surely reached the display: its target has passed, and a whole
/// refresh has gone by since it was submitted, plus half of one for the GPU. Another frame then
/// can't queue behind it, so one at once costs no pacing: it shows at the next refresh.
fn rested(last: Option<Instant>, last_target: Option<Instant>, now: Instant, period: Duration) -> bool {
    last_target.is_none_or(|target| target <= now)
        && last.is_none_or(|last| now.saturating_duration_since(last) >= period * 3 / 2)
}

/// A stopped link has no tick waiting (`stop` clears it), so a rested surface's next frame is
/// `Now` until the link starts: deciding first costs it no pacing.
fn defers_start(running: bool, last: Option<Instant>, last_target: Option<Instant>, now: Instant, period: Duration) -> bool {
    !running && rested(last, last_target, now, period)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_defers_starting_the_link_only_when_it_would_draw_at_once_anyway() {
        let t = crate::time::now();
        let ms = |n| t + Duration::from_millis(n);
        let period = Duration::from_millis(8);
        // A timer 33 ms after the last frame: whenever it starts the link, it draws at once.
        assert!(defers_start(false, Some(ms(100)), Some(ms(104)), ms(133), period));
        for later in [133, 134, 150, 500] {
            assert_eq!(opportunity(None, Some(ms(100)), Some(ms(104)), ms(later), period), Opportunity::Now);
        }
        assert!(defers_start(false, None, None, ms(0), period));
        // The last frame may still be queued: start the link now and wait for its tick.
        assert!(!defers_start(false, Some(ms(100)), Some(ms(100)), ms(105), period));
        assert!(!defers_start(false, Some(ms(100)), Some(ms(130)), ms(120), period));
        // A running link already paces the surface.
        assert!(!defers_start(true, Some(ms(100)), Some(ms(104)), ms(133), period));
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
        assert_eq!(target_time(t, 100, 116, 1000.0), ms(16));
        assert_eq!(target_time(ms(32), 132, 116, 1000.0), ms(16));
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
        // The link just started after idle: no tick yet, the last frame long gone.
        assert_eq!(opportunity(None, Some(ms(0)), Some(ms(8)), ms(500), period), Opportunity::Now);
        assert_eq!(opportunity(None, None, None, ms(0), period), Opportunity::Now);
        // A stale tick after a stall doesn't hold a rested surface either.
        assert_eq!(opportunity(Some(ms(400)), Some(ms(0)), Some(ms(8)), ms(500), period), Opportunity::Now);
        // A fresh tick keeps its target.
        assert_eq!(opportunity(Some(ms(510)), Some(ms(0)), Some(ms(8)), ms(500), period), Opportunity::Tick(ms(510)));
    }

    #[test]
    fn a_frame_still_on_its_way_to_the_display_waits_for_a_tick() {
        let t = crate::time::now();
        let ms = |n| t + Duration::from_millis(n);
        let period = Duration::from_millis(8);
        // Submitted under a refresh and a half ago: it may still be queued.
        assert_eq!(opportunity(None, Some(ms(100)), Some(ms(100)), ms(111), period), Opportunity::Wait);
        assert_eq!(opportunity(None, Some(ms(100)), Some(ms(100)), ms(112), period), Opportunity::Now);
        // Its target hasn't passed.
        assert_eq!(opportunity(None, Some(ms(100)), Some(ms(130)), ms(120), period), Opportunity::Wait);
        // A Low Power cap stretches the period, and with it the rest.
        assert_eq!(opportunity(None, Some(ms(100)), Some(ms(100)), ms(118), Duration::from_millis(16)), Opportunity::Wait);
        // While moving, a rejected tick right after a frame waits for the next one.
        assert_eq!(opportunity(Some(ms(116)), Some(ms(100)), Some(ms(108)), ms(102), period), Opportunity::Wait);
    }

    #[test]
    fn nominal_and_monitor_refresh_periods_seed_the_display_rate() {
        let ns_120 = nominal_period_ns(CVTime { timeValue: 1, timeScale: 120, flags: 0 }).unwrap();
        assert!((1.0 / Duration::from_nanos(ns_120).as_secs_f64() - 120.0).abs() < 0.01);
        let ns_ntsc = nominal_period_ns(CVTime { timeValue: 1001, timeScale: 60000, flags: 0 }).unwrap();
        assert!((1.0 / Duration::from_nanos(ns_ntsc).as_secs_f64() - 59.94).abs() < 0.01);
        assert_eq!(nominal_period_ns(CVTime { timeValue: 1, timeScale: 120, flags: CVTimeFlags::IsIndefinite.0 }), None);
        assert_eq!(nominal_period_ns(CVTime { timeValue: 0, timeScale: 120, flags: 0 }), None);
        assert_eq!(nominal_period_ns(CVTime { timeValue: 1, timeScale: 0, flags: 0 }), None);
        assert_eq!(millihertz_period_ns(120_000), Some(ns_120));
        assert_eq!(millihertz_period_ns(0), None);
    }
}
