//! One drawable submission per display tick. Metal's drawable pool is not a frame clock:
//! acquisition may return immediately even with FIFO presentation enabled. The exception is a
//! frame after rest, such as the first after a key press: it draws at once (`rested`).
//!
//! When a visible window settles idle after presenting, the link coasts briefly (`LinkState::Coasting`,
//! ~150 ms) without waking the main thread, drawing, or presenting, so trackpad wheel deltas and
//! steady `NativeSurface` pushes resume without cold `CVDisplayLinkStop`/`Start` stalls.
//!
//! Low Power Mode caps frame pacing at 60 Hz across both built-in ProMotion panels (where
//! CVDisplayLink, CADisplayLink and NSScreen still report 120 Hz) and high-refresh external
//! displays. Thermal pressure (`NSProcessInfo.thermalState`) steps pacing down automatically
//! under `Fair`, `Serious`, and `Critical` load, and active links re-poll both once a second.
#![allow(deprecated)] // CVDisplayLink supports the same macOS versions as winit.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{CVDisplayLink, CVGetCurrentHostTime, CVGetHostClockFrequency, CVOptionFlags, CVReturn, CVTime, CVTimeFlags, CVTimeStamp, CVTimeStampFlags};
use winit::{platform::macos::MonitorHandleExtMacOS, window::Window};

use super::display_link_machine::{DisplayLinkCore, LinkEffect, ThermalState, pacing_of, power_poll_due};
pub(crate) use super::display_link_machine::Opportunity;

const INITIAL_PERIOD_NS: u64 = 1_000_000_000 / 60;
const MODE_ACTIVE: u8 = 0;
const MODE_EXPIRED: u8 = 1;

fn coast_code(ticks_left: u8) -> u8 { ticks_left.saturating_add(MODE_EXPIRED) }

struct Callback {
    target: AtomicU64,
    period_ns: AtomicU64,
    /// `MODE_ACTIVE` (0) = active wakes; `MODE_EXPIRED` (1) = coasting expired, stop pending;
    /// `n + 1` = coasting with `n` ticks left.
    coast: AtomicU8,
    /// Coalesces active-mode wakes until the main thread consumes `target`.
    wake_pending: AtomicBool,
    wake: crate::runtime::Waker,
    host_frequency: f64,
}

impl Callback {
    fn arm_active(&self) {
        self.coast.store(MODE_ACTIVE, Ordering::SeqCst);
        self.wake_pending.store(false, Ordering::SeqCst);
    }

    fn arm_coast(&self, ticks_left: u8) {
        self.coast.store(coast_code(ticks_left), Ordering::SeqCst);
        self.wake_pending.store(false, Ordering::SeqCst);
    }

    fn coast_expired(&self) -> bool {
        self.coast.load(Ordering::Acquire) == MODE_EXPIRED
    }

    /// Clears `wake_pending` before reading `target` so a tick arriving after this read is
    /// guaranteed to wake the main thread.
    fn take_target(&self) -> u64 {
        self.wake_pending.store(false, Ordering::SeqCst);
        self.target.swap(0, Ordering::SeqCst)
    }

    fn reset(&self) {
        self.coast.store(MODE_ACTIVE, Ordering::Relaxed);
        self.wake_pending.store(false, Ordering::Relaxed);
        self.target.store(0, Ordering::Relaxed);
    }

    /// Handles one CoreVideo tick on the callback thread; returns whether to wake the main thread.
    fn on_tick(&self, target: u64) -> bool {
        self.target.store(target, Ordering::SeqCst);
        let prev = self.coast.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |m| {
            (m > MODE_EXPIRED).then(|| m - 1)
        });
        match prev {
            Ok(m) => m - 1 == MODE_EXPIRED,
            Err(MODE_ACTIVE) => !self.wake_pending.swap(true, Ordering::SeqCst),
            Err(_) => false,
        }
    }
}

pub(crate) struct DisplayLink {
    link: CFRetained<CVDisplayLink>,
    // Stable allocation, retained until stop has joined the callback thread.
    callback: Box<Callback>,
    core: DisplayLinkCore,
    display: Option<u32>,
    last_power_check: Option<Instant>,
}

static CREATE_FAILED: AtomicBool = AtomicBool::new(false);
/// The last creation's CoreVideo status (0 once one succeeds), and when it failed.
static LAST_FAILURE: Mutex<Option<(i32, Instant)>> = Mutex::new(None);
/// How long after a failed creation `retry` tries again.
const RETRY: Duration = Duration::from_secs(1);

/// Why the last display link couldn't be made, while no creation has succeeded since.
pub(crate) fn failure() -> Option<i32> { LAST_FAILURE.lock().unwrap().map(|(status, _)| status) }

/// How a surface's frames are paced, from its display link (or the lack of one).
pub(crate) fn pacing(link: Option<&DisplayLink>, moving: bool) -> crate::frame::surface_stats::Pacing {
    pacing_of(link.map(|link| link.core.state()), failure(), moving)
}

/// Reads macOS Low Power Mode and thermal pressure (`NSProcessInfo`), applying to both
/// built-in and external displays.
fn power_state() -> (bool, ThermalState) {
    use objc2_foundation::{NSObjectProtocol, NSProcessInfo, NSProcessInfoThermalState};
    let info = NSProcessInfo::processInfo();
    let low_power = info.respondsToSelector(objc2::sel!(isLowPowerModeEnabled))
        && info.isLowPowerModeEnabled();
    let thermal = if info.respondsToSelector(objc2::sel!(thermalState)) {
        match info.thermalState() {
            NSProcessInfoThermalState::Fair => ThermalState::Fair,
            NSProcessInfoThermalState::Serious => ThermalState::Serious,
            NSProcessInfoThermalState::Critical => ThermalState::Critical,
            _ => ThermalState::Nominal,
        }
    } else {
        ThermalState::Nominal
    };
    (low_power, thermal)
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
    // Coalesce a backed-up event loop; missed refreshes never become a catch-up burst.
    // Never call Window::request_redraw here: winit synchronously waits on the main
    // thread on macOS, which can itself be waiting in CVDisplayLinkStop.
    if callback.on_tick(target) { (callback.wake)(); }
    0
}

impl DisplayLink {
    /// True only while actively driving redraws (`LinkState::Active`); false while `Coasting` or `Stopped`.
    pub(crate) fn paced(&self) -> bool { self.core.paced() }

    pub(crate) fn refresh_hz(&self) -> f64 {
        let period = Duration::from_nanos(self.callback.period_ns.load(Ordering::Relaxed));
        let mut core = self.core.clone();
        core.set_period(period);
        core.refresh_hz()
    }

    fn sync_period(&mut self) {
        let ns = self.callback.period_ns.load(Ordering::Relaxed);
        self.core.set_period(Duration::from_nanos(ns));
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
            period_ns: AtomicU64::new(INITIAL_PERIOD_NS),
            coast: AtomicU8::new(MODE_ACTIVE),
            wake_pending: AtomicBool::new(false),
            wake,
            host_frequency: CVGetHostClockFrequency(),
        });
        // SAFETY: `tick` matches `CVDisplayLinkOutputCallback`, and `callback` stays at a stable heap address until `DisplayLink::drop` stops the link.
        let status = unsafe { link.set_output_callback(Some(tick), (&mut *callback as *mut Callback).cast()) };
        if status != 0 { eprintln!("display link callback failed: {status}"); return None; }
        let mut pacing = Self {
            link,
            callback,
            core: DisplayLinkCore::new(Duration::from_nanos(INITIAL_PERIOD_NS)),
            display: None,
            last_power_check: None,
        };
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
        if (changed || !self.core.hardware_running())
            && let Some(ns) = nominal_period_ns(self.link.nominal_output_video_refresh_period())
                .or_else(|| monitor.as_ref().and_then(|m| m.refresh_rate_millihertz()).and_then(millihertz_period_ns))
        {
            self.callback.period_ns.store(ns, Ordering::Relaxed);
            self.core.set_period(Duration::from_nanos(ns));
        }
        self.refresh_power_state(crate::time::now());
    }

    fn refresh_power_state(&mut self, now: Instant) {
        let (low_power, thermal) = power_state();
        self.core.set_capped(low_power);
        self.core.set_thermal(thermal);
        self.last_power_check = Some(now);
    }

    /// Starts or resumes the link into `Active`; false if Core Video refused `CVDisplayLinkStart`.
    pub(crate) fn start(&mut self) -> bool {
        self.sync_period();
        for effect in self.core.start() {
            match effect {
                LinkEffect::StartLink => {
                    self.callback.arm_active();
                    let status = self.link.start();
                    if status != 0 {
                        eprintln!("display link start failed: {status}");
                        self.core.start_failed();
                        return false;
                    }
                    // Power and thermal state can change while idle; each burst rechecks them.
                    self.refresh_power_state(crate::time::now());
                }
                LinkEffect::ResumeActive => {
                    self.callback.arm_active();
                }
                LinkEffect::ArmCoast { .. } | LinkEffect::StopLink => {}
                #[cfg(test)]
                LinkEffect::WakeToStop => {}
            }
        }
        true
    }

    /// Arms one-shot urgent admission so the next frame may bypass `Opportunity::Wait` once.
    /// Duplicate calls before the next frame coalesce into a single admission.
    pub(crate) fn admit_once(&mut self) { self.core.admit_once(); }

    /// Consumes any pending one-shot urgent admission when a frame draws without `next_frame`.
    pub(crate) fn consume_urgent(&mut self) { let _ = self.core.take_urgent(); }

    /// Whether a one-shot urgent admission is currently armed.
    pub(crate) fn is_urgent(&self) -> bool { self.core.is_urgent() }

    /// Claims one display opportunity: a tick and its target presentation time, or at once
    /// when the surface has rested (`rested`) or one-shot urgent admission bypassed `Wait`.
    /// Consume before rendering so a tick arriving during rendering can queue the next opportunity.
    pub(crate) fn next_frame(&mut self) -> Opportunity {
        if !self.start() {
            let _ = self.core.take_urgent();
            return Opportunity::Now;
        }
        let host_target = self.callback.take_target();
        let now = crate::time::now();
        if power_poll_due(self.last_power_check, now) {
            self.refresh_power_state(now);
        }
        let tick = (host_target != 0).then(|| target_time(now, CVGetCurrentHostTime(), host_target, self.callback.host_frequency));
        self.sync_period();
        self.core.claim(tick, now)
    }

    /// Whether a frame that may draw nothing (a timer's, on a resting surface) can decide that
    /// before starting or resuming the link: it's not actively pacing, and `next_frame` would
    /// answer `Now` whenever asked from here on.
    pub(crate) fn defers_start(&self, now: Instant) -> bool {
        let mut core = self.core.clone();
        core.set_period(Duration::from_nanos(self.callback.period_ns.load(Ordering::Relaxed)));
        core.defers_start(now)
    }

    /// The tick after `now`, as if the link were running.
    pub(crate) fn tick_after(&self, now: Instant) -> Instant {
        let mut core = self.core.clone();
        core.set_period(Duration::from_nanos(self.callback.period_ns.load(Ordering::Relaxed)));
        core.tick_after(now)
    }

    pub(crate) fn with_ambient_period(mut self, period: Option<Duration>) -> Self {
        self.core.set_ambient_period(period);
        self
    }

    pub(crate) fn with_unfocused_period(mut self, period: Option<Duration>) -> Self {
        self.core.set_unfocused_period(period);
        self
    }

    pub(crate) fn with_focused(mut self, focused: bool) -> Self {
        self.core.set_focused(focused);
        self
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.core.set_focused(focused);
    }

    pub(crate) fn note_input(&mut self, at: Instant) { self.core.note_input(at); }

    pub(crate) fn presented(&mut self, at: Instant) { self.core.presented(at); }

    /// Keeps an active link coasting briefly (~150 ms) with zero draws and zero presents after
    /// a visible window settles idle. A stopped link stays stopped.
    pub(crate) fn coast(&mut self) {
        self.sync_period();
        for effect in self.core.coast() {
            if let LinkEffect::ArmCoast { ticks_left } = effect {
                self.callback.arm_coast(ticks_left);
            }
        }
    }

    /// Stops a coasting link once its keep-alive tick budget hits zero (zero redraws).
    pub(crate) fn stop_expired(&mut self) -> bool {
        let expired = self.callback.coast_expired();
        let mut stopped = false;
        for effect in self.core.stop_if_expired(expired) {
            if effect == LinkEffect::StopLink {
                self.link.stop();
                self.callback.reset();
                stopped = true;
            }
        }
        stopped
    }

    /// Hard-stops the link immediately (occlusion, hide, minimize, backoff, zero resize,
    /// prewarm, device loss, or drop).
    pub(crate) fn stop(&mut self) {
        for effect in self.core.stop() {
            if effect == LinkEffect::StopLink {
                self.link.stop();
            }
        }
        self.callback.reset();
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;

    #[test]
    fn target_time_converts_future_and_past_host_timestamps() {
        let t = crate::time::now();
        let ms = |n| t + Duration::from_millis(n);
        assert_eq!(target_time(t, 100, 116, 1000.0), ms(16));
        assert_eq!(target_time(ms(32), 132, 116, 1000.0), ms(16));
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

    #[test]
    fn callback_coasting_refreshes_target_silently_and_wakes_once_on_expiry() {
        let cb = Callback {
            target: AtomicU64::new(0),
            period_ns: AtomicU64::new(INITIAL_PERIOD_NS),
            coast: AtomicU8::new(MODE_ACTIVE),
            wake_pending: AtomicBool::new(false),
            wake: Arc::new(|| {}),
            host_frequency: 1_000_000_000.0,
        };
        // Active mode coalesces wakes until take_target consumes them.
        assert!(cb.on_tick(100));
        assert!(!cb.on_tick(200));
        assert_eq!(cb.take_target(), 200);
        assert!(cb.on_tick(300));

        // Coasting for 3 ticks keeps refreshing target without waking until tick 3.
        cb.arm_coast(3);
        assert!(!cb.coast_expired());
        assert!(!cb.on_tick(400));
        assert!(!cb.coast_expired());
        assert!(!cb.on_tick(500));
        assert!(!cb.coast_expired());
        assert!(cb.on_tick(600), "final coasting tick sends one wake to stop");
        assert!(cb.coast_expired());
        assert!(!cb.on_tick(700), "further ticks after expiry send no extra wakes");

        // Resuming Active before stop_expired preserves the fresh target and re-arms wakes.
        cb.arm_active();
        assert!(!cb.coast_expired());
        assert_eq!(cb.take_target(), 700);
        assert!(cb.on_tick(800), "next active tick wakes cleanly with no lost wake");
    }
}
