//! A due timer on a window that draws nothing new. `poll` runs on its own first, and the view,
//! layout and drawing wait for a frame until that poll or something else changes what shows:
//! always on a hidden window, and on a shown one while it rests.
use super::*;

/// Frame loop, timer wake deadlines, action queue, pending transaction, live value latch,
/// tracing and hot-reload session state owned by [`RuntimeCore`]. Under `#[repr(C)]`, the
/// cold `pending: Tx` buffer precedes the hot per-frame fields (`wake_deadlines`, `timer_wake`,
/// `queue`, `live`) so they sit directly adjacent to [`CommitState`]'s `dirty`/`relayout` flags.
#[repr(C)]
pub(crate) struct FrameState {
    /// What `update` asked of the next commit.
    pub(in crate::runtime) pending: Tx,
    pub(super) idle_meter: crate::tooling::perf_warn::IdleMeter,
    /// When the frame being built will present; videos pull the picture for it.
    frame_target: Option<crate::time::Instant>,
    #[cfg(test)]
    test_clock: Option<crate::ManualClock>,
    /// Tooling opts in before any callbacks. Absent in ordinary app runs.
    trace_channels: Option<HashMap<&'static str, (&'static str, f32)>>,
    #[cfg(feature = "hot")]
    hot: Option<crate::hot::Session>,
    /// The live-only event this frame's commit checks: its handler, and what the view had built.
    #[cfg(debug_assertions)]
    pub(super) live_check: Option<(String, live_check::ViewPrint)>,
    /// Live-only events checked (and so committed) so far.
    #[cfg(debug_assertions)]
    pub(super) live_checks: u64,
    /// Stale-view warnings printed for `Tx::live_only` handlers, each once.
    #[cfg(debug_assertions)]
    pub(super) live_only_warnings: Vec<String>,
    pub(super) live_clock: Arc<()>,
    pub(super) live_subscriptions: HashMap<live_latch::SourceKey, Arc<crate::live::Subscription>>,
    /// Events whose handler said `Tx::live_only`, and so didn't commit.
    pub(in crate::runtime) live_only_skips: u64,
    /// Incremented once per frame when `latch_live` mutates an in-scene, non-secure `LiveText`
    /// in place without a commit, so accessibility stamps notice in O(1).
    pub(crate) live_text_gen: u64,
    last_dt: f64,
    /// Record `events` without a trace: headless runs. Live hosts turn it off.
    record_events: bool,
    /// Debug builds check `Tx::live_only` events (`skip_view`).
    #[cfg(debug_assertions)]
    pub(super) live_only_check: bool,
    wake_deadlines: Vec<f64>,
    timer_wake: TimerWake,
    pub(in crate::runtime) queue: ActionQueue,
    pub(in crate::runtime) live: live_latch::LiveLane,
}

impl FrameState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            queue: ActionQueue::default(),
            pending: Tx::default(),
            wake_deadlines: Vec::new(),
            timer_wake: Default::default(),
            idle_meter: Default::default(),
            last_dt: 0.0,
            frame_target: None,
            #[cfg(test)]
            test_clock: None,
            live: live_latch::LiveLane::new(Arc::new(|| {})),
            live_clock: Arc::new(()),
            live_subscriptions: HashMap::default(),
            live_only_skips: 0,
            live_text_gen: 0,
            #[cfg(debug_assertions)]
            live_only_check: true,
            #[cfg(debug_assertions)]
            live_check: None,
            #[cfg(debug_assertions)]
            live_checks: 0,
            #[cfg(debug_assertions)]
            live_only_warnings: Vec::new(),
            record_events: true,
            trace_channels: None,
            #[cfg(feature = "hot")]
            hot: None,
        }
    }

    #[inline]
    pub(in crate::runtime) fn record_poll(&mut self, changed: bool) -> bool {
        self.idle_meter.polled(changed)
    }

    #[inline]
    pub(in crate::runtime) fn frame_target(&self) -> Option<crate::time::Instant> {
        self.frame_target
    }

    #[inline]
    pub(crate) fn set_frame_target(&mut self, at: crate::time::Instant) {
        self.frame_target = Some(at);
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn set_test_clock(&mut self, clock: crate::ManualClock) {
        self.test_clock = Some(clock);
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn test_clock_mut(&mut self) -> Option<&mut crate::ManualClock> {
        self.test_clock.as_mut()
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn test_clock_now(&self) -> Option<f64> {
        self.test_clock.as_ref().map(crate::ManualClock::now)
    }

    #[inline]
    pub(crate) fn tracing_channels(&self) -> bool {
        self.trace_channels.is_some()
    }

    #[inline]
    pub(crate) fn trace_channels(&self) -> Option<&HashMap<&'static str, (&'static str, f32)>> {
        self.trace_channels.as_ref()
    }

    #[inline]
    pub(crate) fn enable_trace_channels(&mut self, enabled: bool) {
        self.trace_channels = enabled.then(Default::default);
    }

    #[inline]
    pub(in crate::runtime) fn record_trace_channel(&mut self, name: &'static str, kind: &'static str, value: f32) {
        if let Some(channels) = &mut self.trace_channels {
            channels.insert(name, (kind, value));
        }
    }

    #[inline]
    pub(crate) fn has_hot(&self) -> bool {
        #[cfg(feature = "hot")]
        return self.hot.is_some();
        #[cfg(not(feature = "hot"))]
        false
    }

    #[cfg(feature = "hot")]
    #[inline]
    pub(crate) fn hot(&self) -> Option<&crate::hot::Session> {
        self.hot.as_ref()
    }

    #[cfg(all(test, feature = "hot"))]
    #[inline]
    pub(crate) fn hot_mut(&mut self) -> Option<&mut crate::hot::Session> {
        self.hot.as_mut()
    }

    #[cfg(feature = "hot")]
    #[inline]
    pub(crate) fn set_hot(&mut self, session: crate::hot::Session) {
        self.hot = Some(session);
    }

    #[inline]
    pub(in crate::runtime) fn hot_pending(&self) -> bool {
        #[cfg(feature = "hot")]
        return self.hot.as_ref().is_some_and(crate::hot::Session::pending);
        #[cfg(not(feature = "hot"))]
        false
    }

    #[cfg(feature = "hot")]
    #[inline]
    pub(in crate::runtime) fn hot_changed(&mut self) -> bool {
        self.hot.as_mut().is_some_and(crate::hot::Session::changed)
    }

    #[cfg(debug_assertions)]
    #[inline]
    pub(in crate::runtime) fn clear_live_check(&mut self) {
        self.live_check = None;
    }

    #[cfg(debug_assertions)]
    #[inline]
    pub(in crate::runtime) fn take_live_check(&mut self) -> Option<(String, live_check::ViewPrint)> {
        self.live_check.take()
    }

    #[cfg(debug_assertions)]
    #[inline]
    pub(in crate::runtime) fn live_checks(&self) -> u64 {
        self.live_checks
    }

    #[cfg(debug_assertions)]
    #[inline]
    pub(in crate::runtime) fn live_only_warnings(&self) -> &[String] {
        &self.live_only_warnings
    }

    #[inline]
    pub(in crate::runtime) fn has_live_subscriptions(&self) -> bool {
        !self.live_subscriptions.is_empty()
    }

    #[inline]
    pub(in crate::runtime) fn set_last_dt(&mut self, dt: f64) {
        self.last_dt = dt;
    }

    #[inline]
    pub(crate) fn record_events(&self) -> bool {
        self.record_events
    }

    #[inline]
    pub(crate) fn set_record_events(&mut self, record: bool) {
        self.record_events = record;
    }

    #[inline]
    pub(in crate::runtime) fn schedule_wakes_from(&mut self, now: f64, wakes: impl IntoIterator<Item = std::time::Duration>) {
        for delay in wakes {
            let deadline = now + delay.as_secs_f64();
            if let Err(index) = self.wake_deadlines.binary_search_by(|t| t.total_cmp(&deadline)) {
                self.wake_deadlines.insert(index, deadline);
            }
        }
    }

    #[inline]
    pub(in crate::runtime) fn next_wake_deadline(&self) -> Option<f64> {
        self.wake_deadlines.first().copied()
    }

    #[inline]
    pub(in crate::runtime) fn begin_frame_poll(&mut self, clock: f64) -> (bool, usize) {
        let polled = self.timer_wake.frame_polled(self.wake_deadlines.first().is_some_and(|t| *t <= clock));
        let due = self.wake_deadlines.partition_point(|t| *t <= clock);
        (polled, due)
    }

    #[inline]
    pub(in crate::runtime) fn drain_due_wakes(&mut self, due: usize) {
        self.wake_deadlines.drain(..due);
    }

    #[inline]
    pub(in crate::runtime) fn polled_at(&self, fallback: f64) -> f64 {
        self.timer_wake.polled_at().unwrap_or(fallback)
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn wake_deadlines(&self) -> &[f64] {
        &self.wake_deadlines
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn clear_wake_deadlines(&mut self) {
        self.wake_deadlines.clear();
    }
}

/// A resting window's timer poll, and the frame it asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::runtime) enum TimerWake {
    /// No timer poll stands for a frame.
    #[default]
    Idle,
    /// A timer's poll ran `at`, and the frame it asked for is still to come. The frame takes the
    /// poll as its own, and the poll's `wake_after`s wait in `pending` to count from that frame.
    Asked { at: f64 },
    /// A frame took the poll at `polled_at` as its own.
    Taken { polled_at: f64 },
}

pub(in crate::runtime) enum TimerWakeEvent {
    /// A resting wake's poll ran `at` and asked for a frame.
    Asked { at: f64 },
    /// A frame polls; `due`: a deadline came due since the timer's poll, so it polls again.
    Frame { due: bool },
    /// The frame asked for won't stand for its poll: the window hid or showed, or a wake asks
    /// the next frame to poll.
    Cancel,
    /// A poll runs alone: no earlier poll stands for anything now.
    PollAlone,
}

impl TimerWake {
    /// The state after `event`, and, for a frame, whether a timer's poll already stands for it;
    /// for a cancel, the poll time the poll's timers count from.
    pub(in crate::runtime) fn step(self, event: TimerWakeEvent) -> (TimerWake, Option<f64>) {
        match (self, event) {
            (_, TimerWakeEvent::Asked { at }) => (TimerWake::Asked { at }, None),
            (_, TimerWakeEvent::PollAlone) => (TimerWake::Idle, None),
            (TimerWake::Asked { at }, TimerWakeEvent::Frame { due: false }) => (TimerWake::Taken { polled_at: at }, Some(at)),
            (_, TimerWakeEvent::Frame { .. }) => (TimerWake::Idle, None),
            (TimerWake::Asked { at }, TimerWakeEvent::Cancel) => (TimerWake::Idle, Some(at)),
            (state, TimerWakeEvent::Cancel) => (state, None),
        }
    }

    /// At a frame's poll: whether a timer's poll already stands for it. A deadline that came
    /// due since (`due`) still gets this frame's poll.
    pub(in crate::runtime) fn frame_polled(&mut self, due: bool) -> bool {
        let (next, taken) = self.step(TimerWakeEvent::Frame { due });
        *self = next;
        taken.is_some()
    }

    pub(in crate::runtime) fn polled_at(&self) -> Option<f64> {
        match *self {
            TimerWake::Taken { polled_at } => Some(polled_at),
            _ => None,
        }
    }

    fn asked(&self) -> bool { matches!(self, TimerWake::Asked { .. }) }
}

impl RuntimeCore {
    /// `poll` alone, for a hidden window's wake: its timers and inbox advance while nothing
    /// lays out, animates or draws. Like `update`, its changes wait, dirty, for the next frame.
    /// Returns whether the last frame went out of date: `poll` reported a change or asked for a
    /// scroll or text flight, or a live-edit patch arrived. A `wake_after` alone doesn't count.
    ///
    /// A resting wake's poll still stands for a frame it asked for that runs covered or misses
    /// its drawable: its timers count from it, and nothing polls again unless another came due.
    pub(crate) fn poll_hidden(&mut self, app: &mut dyn AppDriver) -> bool {
        let now = self.now_clock();
        if self.frame.timer_wake.asked() && self.frame.wake_deadlines.first().is_none_or(|t| *t > now) {
            self.cancel_timer_frame();
            return self.needs_commit();
        }
        let stale = self.poll_alone(app);
        let wakes = std::mem::take(&mut self.frame.pending.wakes);
        self.schedule_wakes(wakes);
        stale
    }

    /// `poll_hidden`, leaving the poll's `wake_after`s in `pending`.
    fn poll_alone(&mut self, app: &mut dyn AppDriver) -> bool {
        self.frame.timer_wake = self.frame.timer_wake.step(TimerWakeEvent::PollAlone).0;
        if let Some(clock) = self.wall_clock() { self.clock = clock; }
        // An `update`'s `wake_after` counts from now; no frame may come to schedule it.
        let wakes = std::mem::take(&mut self.frame.pending.wakes);
        self.schedule_wakes(wakes);
        // Due timers are spent with or without a poll, as a frame spends them; one left due
        // would wake the host again every frame interval.
        let due = self.frame.wake_deadlines.partition_point(|t| *t <= self.clock);
        self.frame.wake_deadlines.drain(..due);
        // A glyph mask cache write that came due while nothing draws (`mask_cache`).
        let _ = self.text.mask_cache_frame(false);
        let patched = self.patch_pending();
        let has_poll = app.has_poll();
        let has_playground = app.playground().is_some_and(|p| p.polls());
        if !has_poll && !has_playground { return patched; }
        let queued = self.frame.queue.len();
        let mut one = self.transaction(app);
        let mut changed = has_poll && self.frame.idle_meter.polled(crate::tooling::perf_warn::timed("poll", || app.poll(self.clock, &mut one)));
        if has_playground {
            let mut stage = self.transaction(app);
            let clock = self.clock;
            changed |= self.poll_playground(app, clock, &mut stage);
            merge(&mut one, stage);
        }
        self.commit.dirty |= changed;
        self.frame.pending.polled_change |= changed;
        let asks_frame = one.srgb_blend.is_some_and(|b| b != self.paint.srgb_blend()) || one.font_smoothing.is_some_and(|m| m != self.text.smoothing) || self.changes_families(&one) || (one.blur && self.kbd.focus.path().is_some());
        self.latch_requests(app, &mut one);
        let stale = changed || one.frame_work() || patched || asks_frame || self.frame.queue.len() > queued;
        merge(&mut self.frame.pending, one);
        stale
    }

    /// A shown window's due timer. While the window rests, `poll` runs alone, as on a hidden
    /// window, and the result says whether that poll wants a frame: it changed what shows,
    /// asked for a window frame, or asked for the next frame (`wake_after(ZERO)`). The host
    /// answers with `resting_wake_frame`. `None` when anything else wants this frame, which
    /// then draws as any other: motion, scrolls or a window frame in flight, queued input, a
    /// hover or tooltip delay, a loop, a live value, `SurfaceDraw` content, or a poll stepping
    /// every frame.
    pub(crate) fn resting_wake(&mut self, app: &mut dyn AppDriver) -> Option<bool> {
        if !self.rests() { return None; }
        let stale = self.poll_alone(app);
        let steps = self.frame.pending.wakes.iter().any(|d| d.is_zero());
        // A starved fill's wake draws the frame that resumes it; the poll alone never does.
        Some(stale || self.host.window_frame.is_some() || steps || self.commit.virtual_fill.resuming(self.clock))
    }

    /// Whether the host asked for a frame after `resting_wake`. That frame doesn't poll again
    /// and its timers count from it; without one they count from the poll.
    pub(crate) fn resting_wake_frame(&mut self, frame: bool) {
        if frame {
            self.frame.timer_wake = self.frame.timer_wake.step(TimerWakeEvent::Asked { at: self.clock }).0;
        } else {
            let wakes = std::mem::take(&mut self.frame.pending.wakes);
            self.schedule_wakes(wakes);
        }
    }

    /// The frame a resting wake asked for won't stand for its poll: the window hid or showed,
    /// or an `AppHandle::wake` asks the next frame to poll. The poll's timers count from it.
    pub(crate) fn cancel_timer_frame(&mut self) {
        let (next, polled) = self.frame.timer_wake.step(TimerWakeEvent::Cancel);
        self.frame.timer_wake = next;
        let Some(polled) = polled else { return };
        let wakes = std::mem::take(&mut self.frame.pending.wakes);
        self.schedule_wakes_from(polled, wakes);
    }

    /// Whether user input is queued in the action queue (`ActionQueue::has_input`), as
    /// distinguished from layout and state observers. Hosts check this before resting so
    /// queued input never waits for a later timer.
    pub(crate) fn has_input_waiting(&self) -> bool {
        self.frame.queue.has_input()
    }

    /// Nothing but a timer asks this runtime for a frame.
    pub(crate) fn rests(&self) -> bool {
        self.motion.loop_wake().is_none() && self.rests_except_loop()
    }

    /// `rests`, apart from a loop's own wake (`paint_cycle_tick`).
    pub(in crate::runtime) fn rests_except_loop(&self) -> bool {
        !self.animating && self.rests_but_motion()
    }

    /// Nothing waits for a frame but motion: no commit, input, held work, live value or timer
    /// due now (`loop_step` asks the motion apart).
    pub(in crate::runtime) fn rests_but_motion(&self) -> bool {
        self.waiting_work().is_none()
    }

    /// What waits for a frame besides motion, if anything (`rests_but_motion`).
    pub(in crate::runtime) fn waiting_work(&self) -> Option<&'static str> {
        #[cfg(feature = "hot")]
        if self.frame.has_hot() { return Some("live edit"); }
        if self.stale() { Some("a commit or input") }
        else if self.commit.is_settling() || self.host.window_frame.is_some() { Some("a settle or window frame") }
        else if self.pointer.hover_delays.waiting() || self.tooltip_waiting() || self.press_repeat_waiting() || self.drag_ends_pending() || self.pointer.drag_out_request.is_some() { Some("pointer work") }
        else if self.files_held() { Some("files") }
        else if self.live_pending() { Some("a live value") }
        else if !self.surfaces.is_empty() { Some("a native surface") }
        else if self.steps_frames() { Some("a poll that steps every frame") }
        else { None }
    }

    /// The last poll asked for the next frame (`wake_after(ZERO)`, due at once): it steps
    /// something every frame.
    fn steps_frames(&self) -> bool {
        self.frame.wake_deadlines.first().is_some_and(|t| *t <= self.clock)
    }
}
