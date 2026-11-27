//! `App::on_frame_encoded`: what each of a window's frames came to, for apps that log their
//! own frame timing.

use std::rc::Rc;

use crate::time::Instant;

/// `App::on_frame_encoded`'s hook.
pub(crate) type OnFrameEncoded = Box<dyn FnMut(&FrameEncoded)>;

/// `App::on_frame_drawn`'s hook.
pub(crate) type OnFrameDrawn<S> = Box<dyn FnMut(&crate::Runtime<S>)>;

/// One frame a window ran, as `App::on_frame_encoded` reports it.
#[derive(Clone, Debug)]
pub struct FrameEncoded {
    at: Instant,
    present_returned: Option<Instant>,
    outcome: FrameOutcome,
    window: Option<Rc<str>>,
    seq: u64,
    presents_at: Option<Instant>,
    started: Option<Instant>,
    refresh_hz: Option<f64>,
    update_ms: Option<f64>,
    encode_ms: Option<f64>,
    urgent_admitted: bool,
}

/// What a frame came to.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Encoded and submitted to the GPU, then presented: a visible window's frame, its first
    /// frame during creation included, or a hidden launcher's.
    Drawn,
    /// A frame only a timer asked for, whose build draws what is already on screen: it took
    /// no drawable and encoded nothing.
    Unchanged,
    /// Due but not drawn: the window is covered, minimised or occluded, or no drawable came in
    /// time. The runtime may still have stepped.
    Skipped,
}

impl FrameEncoded {
    /// For `Drawn`, when encoding finished: right after the frame's commands were submitted,
    /// before it was presented. Otherwise, when the window decided to draw nothing.
    pub fn at(&self) -> Instant { self.at }

    pub fn outcome(&self) -> FrameOutcome { self.outcome }

    /// The window's key (`WindowSpec::new`; `"main"` for `App::run`).
    pub fn window(&self) -> Option<&str> { self.window.as_deref() }

    /// This window's reports so far, from 0: one per call, whatever the outcome.
    pub fn seq(&self) -> u64 { self.seq }

    /// The display refresh the window's display link aimed this frame at, when one paced it:
    /// macOS windows that are visible and not being resized. `None` elsewhere, for `Skipped`
    /// frames, and for a frame drawn at once after the window rested (the first after a key
    /// press, say), which aims at the next refresh without waiting for a tick.
    pub fn presents_at(&self) -> Option<Instant> { self.presents_at }

    /// For `Drawn`, when the frame started, after its drawable came: `at` minus the update and
    /// the encode. A display-paced window starts each frame on a refresh, so the gaps between
    /// starts are the gaps between frames on screen. `None` otherwise.
    pub fn started(&self) -> Option<Instant> { self.started }

    /// The refresh rate of the display the window was on, in Hz: the display link's on macOS
    /// (120 on a ProMotion panel), else what the monitor reports. `None` when neither knows.
    pub fn refresh_hz(&self) -> Option<f64> { self.refresh_hz }

    /// For `Drawn`, the milliseconds from the frame's start (after its drawable came) until the
    /// runtime had built it: events, `poll`, the view, layout and paint. The same value a
    /// compositor gets as `Frame::update_ms`. `None` otherwise.
    pub fn update_ms(&self) -> Option<f64> { self.update_ms }

    /// For `Drawn`, the milliseconds from the end of the update until the frame's commands were
    /// submitted (`at`): uploads, the renderer's prepare and draw, or a compositor's `render`.
    /// CPU wall time; not GPU time, and not the wait to present. `None` otherwise.
    pub fn encode_ms(&self) -> Option<f64> { self.encode_ms }

    /// For `Drawn` on a live window, when the CPU present call returned (right after
    /// `wgpu::Queue::present` or CPU surface present completed, before post-present bookkeeping).
    /// `>= at()`, and distinct from actual display scanout time (see
    /// [`FramePresented::presented_at`](crate::FramePresented::presented_at)). `None` for
    /// `Unchanged`, `Skipped`, and offscreen/headless frames without a window present.
    pub fn present_returned(&self) -> Option<Instant> { self.present_returned }

    /// Whether this `Drawn` frame passed macOS display-link pacing via a one-shot urgent
    /// admission ([`AppHandle::wake_urgent`](crate::AppHandle::wake_urgent),
    /// [`AppHandle::update_urgent`](crate::AppHandle::update_urgent), or
    /// [`Tx::request_urgent_frame`](crate::Tx::request_urgent_frame)), bypassing `Wait` once
    /// by advancing the link's target to now. `false` for normal paced or rested frames,
    /// `Unchanged`/`Skipped` frames, and platforms without display-link pacing.
    pub fn urgent_admitted(&self) -> bool { self.urgent_admitted }
}

/// A window's hook and the count it numbers reports with. Without a hook every call is one
/// `Option` check and nothing else, so reporting never changes how the window draws.
#[derive(Default)]
pub(crate) struct FrameReports(Option<Reporter>);

struct Reporter {
    hook: Option<OnFrameEncoded>,
    window: Option<Rc<str>>,
    seq: u64,
    refresh_hz: Option<f64>,
}

impl FrameReports {
    pub(crate) fn new(hook: Option<OnFrameEncoded>, window: Option<&str>) -> Self {
        Self::with_presented_seq(hook, false, window)
    }

    /// Creates frame reporting, also numbering frames when `track_seq` is true (for
    /// `App::on_frame_presented`) without stamping encode times or reading refresh rates when no
    /// `on_frame_encoded` hook is registered.
    pub(crate) fn with_presented_seq(hook: Option<OnFrameEncoded>, track_seq: bool, window: Option<&str>) -> Self {
        let active = hook.is_some() || track_seq;
        Self(active.then(|| Reporter { hook, window: window.map(Rc::from), seq: 0, refresh_hz: None }))
    }

    /// The time to report a `Drawn` frame at, taken only when there's an `on_frame_encoded` hook.
    pub(crate) fn stamp(&self) -> Option<Instant> {
        self.0.as_ref().filter(|r| r.hook.is_some()).map(|_| crate::time::now())
    }

    /// Whether `on_frame_encoded` reads the reports, so a host asks for what only it needs.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn wanted(&self) -> bool { self.0.as_ref().is_some_and(|r| r.hook.is_some()) }

    /// The sequence number the next reported frame will carry, when sequence tracking is active
    /// (`on_frame_encoded`, `on_frame_presented`, or `on_frame_stage`).
    pub(crate) fn next_seq(&self) -> Option<u64> { self.0.as_ref().map(|r| r.seq) }

    /// The display refresh the next reports carry, until it changes.
    pub(crate) fn set_refresh(&mut self, hz: Option<f64>) {
        if let Some(r) = &mut self.0 { r.refresh_hz = hz.filter(|hz| hz.is_finite() && *hz > 0.0); }
    }

    /// A frame encoded at `at` (from `stamp`), which started at `started` and spent `update_ms`
    /// of that building. Returns this frame's `seq` when either `on_frame_encoded` or
    /// `on_frame_presented` tracks it.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn drawn(&mut self, at: Option<Instant>, presents_at: Option<Instant>, started: Instant, update_ms: f64) -> Option<u64> {
        self.drawn_with_present(at, None, presents_at, started, update_ms, false)
    }

    /// Like [`Self::drawn`], also recording when the live window's CPU present call returned
    /// and whether one-shot urgent admission bypassed macOS display-link `Wait`.
    pub(crate) fn drawn_with_present(
        &mut self,
        at: Option<Instant>,
        present_returned: Option<Instant>,
        presents_at: Option<Instant>,
        started: Instant,
        update_ms: f64,
        urgent_admitted: bool,
    ) -> Option<u64> {
        let r = self.0.as_mut()?;
        let seq = r.seq;
        if r.hook.is_some() {
            let at = at?;
            let encode_ms = (at.saturating_duration_since(started).as_secs_f64() * 1000.0 - update_ms).max(0.0);
            self.report(FrameOutcome::Drawn, at, present_returned, presents_at, Some((started, update_ms, encode_ms)), urgent_admitted);
        } else {
            r.seq += 1;
        }
        Some(seq)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn unchanged(&mut self, presents_at: Option<Instant>) {
        let Some(r) = self.0.as_mut() else { return };
        if r.hook.is_some() {
            self.report(FrameOutcome::Unchanged, crate::time::now(), None, presents_at, None, false);
        } else {
            r.seq += 1;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn skipped(&mut self) {
        let Some(r) = self.0.as_mut() else { return };
        if r.hook.is_some() {
            self.report(FrameOutcome::Skipped, crate::time::now(), None, None, None, false);
        } else {
            r.seq += 1;
        }
    }

    fn report(
        &mut self,
        outcome: FrameOutcome,
        at: Instant,
        present_returned: Option<Instant>,
        presents_at: Option<Instant>,
        work: Option<(Instant, f64, f64)>,
        urgent_admitted: bool,
    ) {
        let Some(r) = &mut self.0 else { return };
        let Some(hook) = r.hook.as_mut() else { return };
        let (started, update_ms, encode_ms) = (work.map(|w| w.0), work.map(|w| w.1), work.map(|w| w.2));
        let frame = FrameEncoded {
            at,
            present_returned,
            outcome,
            window: r.window.clone(),
            seq: r.seq,
            presents_at,
            started,
            refresh_hz: r.refresh_hz,
            update_ms,
            encode_ms,
            urgent_admitted,
        };
        r.seq += 1;
        hook(&frame);
    }
}

#[cfg(test)]
#[path = "encoded_tests.rs"]
mod tests;
