//! Non-Metal per-frame pipeline logic shared by [`super::embed::LayerHost`], compiled on all
//! test targets so the frame-step ordering, clock hand-off and request draining are verified
//! off-Apple too.

use crate::frame::clock::FrameClock;
use crate::runtime::{AppDriver, RuntimeCore};
use crate::time::Instant;

/// Summary produced after presenting (or missing) one embedded layer frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FrameSummary {
    pub(crate) drew: bool,
    pub(crate) animating: bool,
    pub(crate) wake_after_secs: Option<f64>,
    pub(crate) cpu_us: u64,
}

/// Drains and discards host requests an embedded layer cannot serve (window close/minimize/frame,
/// native pop-ups, native file dialogs) and flushes in-memory clipboard writes.
pub(crate) fn flush_requests(rt: &mut RuntimeCore, app: &mut dyn AppDriver) {
    crate::clipboard::flush_memory(rt);
    if rt.popup_menu().is_some() {
        rt.cancel_popups(app);
    }
    rt.refuse_file_dialog("native file dialogs are unsupported in embedded views");
    rt.shell_request = None;
    rt.ignore_minimize("an embedded layer");
    rt.window_frame = None;
}

/// Advances the runtime on `clock` for a frame whose drawable has already been acquired, passing
/// `compositing` so a compositor-driven animation keeps real frame gaps instead of clamping to
/// `RESUME_STEP`.
pub(crate) fn step_and_flush(
    clock: &mut FrameClock,
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    t0: Instant,
    compositing: bool,
) -> (bool, f64, f64) {
    let (animating, dt) = clock.advance_core(rt, app, t0, compositing);
    let update_ms = t0.elapsed().as_secs_f64() * 1000.0;
    flush_requests(rt, app);
    (animating, dt, update_ms)
}

/// Runs due timers, `poll` and queued input when drawable acquisition failed, without advancing
/// `FrameClock` (matching `window/host/draw.rs::missed_frame`).
pub(crate) fn missed_step(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    t0: Instant,
    comp_animating: bool,
    retry_now: bool,
) -> FrameSummary {
    rt.covered_wake(app);
    flush_requests(rt, app);
    FrameSummary {
        drew: false,
        animating: retry_now || comp_animating || rt.animating || rt.stale() || rt.has_pending_input() || rt.live_pending(),
        wake_after_secs: wake_after_secs(rt),
        cpu_us: t0.elapsed().as_micros() as u64,
    }
}

/// Records presentation on `clock` and interaction timing, then computes the frame outcome.
pub(crate) fn post_present(
    clock: &mut FrameClock,
    rt: &mut RuntimeCore,
    submitted: Instant,
    t0: Instant,
    animating: bool,
    comp_animating: bool,
) -> FrameSummary {
    clock.presented_core(rt, submitted);
    rt.interaction_presented(submitted, None);
    let cpu_us = submitted.saturating_duration_since(t0).as_micros() as u64;
    let keep_ticking = animating || comp_animating || rt.stale() || rt.has_pending_input() || rt.live_pending();
    FrameSummary {
        drew: true,
        animating: keep_ticking,
        wake_after_secs: wake_after_secs(rt),
        cpu_us,
    }
}

pub(crate) fn wake_after_secs(rt: &RuntimeCore) -> Option<f64> {
    rt.wake_deadline().and_then(|d| {
        let elapsed = rt.wall_start?.elapsed().as_secs_f64();
        Some((d - elapsed).max(0.0))
    })
}

#[cfg(test)]
#[path = "embed_frame_tests.rs"]
mod tests;
