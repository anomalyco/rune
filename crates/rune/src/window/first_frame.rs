//! A window's first frame and its reveals, as one machine: whether the last completed frame holds
//! the current state, the reveal that waits on one, whether the first frame has been reported,
//! and when `on_ready` runs. OpenCode Desktop keeps a new window hidden until its content is
//! ready, or a timeout shows it anyway (`packages/desktop/src/main/windows/reveal-machine.ts`:
//! `loading` → `content-ready` → `shown`, `REVEAL_TIMEOUT_MS`).
//!
//! A reveal requested before the hidden frame completes is held behind the fence, as is a
//! reveal of cached pixels older than the state. Hiding cancels that intent, including when the
//! GPU frame completes later. `on_ready` runs at the first frame, or at its fallback when there is
//! none to wait for: the window is covered, occluded or hidden, or no frame came within
//! `READY_FALLBACK` of opening.

use super::*;
use crate::machine::Effects;

/// How long a hidden frame may prepare before the reveal shows the cached one (a launcher's
/// first frame reports instead).
const PREPARE_FOR: Duration = Duration::from_secs(2);

/// Whether a completed frame holds the window's current state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Frame {
    /// No completed frame holds the current state.
    Stale,
    /// A hidden frame is being prepared: tried again at `retry`, given up at `until`.
    Preparing { retry: Instant, until: Instant },
    /// The GPU finished a frame of the current state. `preparing` is the preparation it ended,
    /// until the host lands the frame (`Complete`).
    Drawn { preparing: Option<(Instant, Instant)> },
}

/// Whether a reveal waits for the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reveal {
    Idle,
    Wanted,
}

/// Whether the window's first frame has been reported (`WindowHandle::is_ready`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Notice {
    Owed,
    Sent,
}

/// When `on_ready` runs, once per native window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OnReady {
    /// It waits for the first frame, or for `fallback` when no frame may come.
    Waiting { fallback: Option<Instant> },
    Ran,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FirstFrame {
    frame: Frame,
    reveal: Reveal,
    notice: Notice,
    on_ready: OnReady,
}

impl Default for FirstFrame {
    fn default() -> Self {
        FirstFrame { frame: Frame::Stale, reveal: Reveal::Idle, notice: Notice::Owed, on_ready: OnReady::Waiting { fallback: None } }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum FirstFrameEvent {
    /// A hidden frame starts preparing at `now` (a launcher's first, or a reveal's refresh).
    Prepare { now: Instant },
    /// The host brings the next try forward to `retry` while the frame prepares.
    Retry { retry: Instant },
    /// A reveal: the state changed since the cached frame (`stale`), and whether it shows
    /// already.
    RequestReveal { stale: bool, visible: bool, now: Instant },
    Hide,
    /// The preparation's give-up time passed.
    Expire,
    /// The GPU finished a frame.
    GpuDone,
    /// The host lands the finished frame.
    Complete,
    /// The first frame was reported.
    Notified,
    /// A frame went by undrawn at `now` because the window isn't on screen.
    Skipped { now: Instant },
    /// `on_ready` is taken.
    TakeReady,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FirstFrameEffect {
    /// Show the frame now (the cached one, or the one just landed).
    Show,
    /// Run `on_ready`.
    RunOnReady,
}

impl FirstFrame {
    /// A window opened at `now`.
    pub(super) fn opened(now: Instant) -> Self {
        FirstFrame { on_ready: OnReady::Waiting { fallback: Some(now + super::win::READY_FALLBACK) }, ..FirstFrame::default() }
    }

    pub(super) fn step(&mut self, event: FirstFrameEvent) -> Effects<FirstFrameEffect> {
        use FirstFrameEffect::*;
        match event {
            FirstFrameEvent::Prepare { now } => {
                let preparing = (now, now + PREPARE_FOR);
                self.frame = match self.frame {
                    Frame::Drawn { .. } => Frame::Drawn { preparing: Some(preparing) },
                    Frame::Stale | Frame::Preparing { .. } => Frame::Preparing { retry: preparing.0, until: preparing.1 },
                };
                Effects::none()
            }
            FirstFrameEvent::Retry { retry } => {
                self.frame = match self.frame {
                    Frame::Preparing { until, .. } => Frame::Preparing { retry, until },
                    Frame::Drawn { preparing: Some((_, until)) } => Frame::Drawn { preparing: Some((retry, until)) },
                    frame => frame,
                };
                Effects::none()
            }
            FirstFrameEvent::RequestReveal { visible: true, .. } => Effects::one(Show),
            FirstFrameEvent::RequestReveal { stale, visible: false, now } => {
                if stale { self.unready(); }
                if self.ready() { return Effects::one(Show); }
                self.reveal = Reveal::Wanted;
                if self.deadline().is_none() { self.step(FirstFrameEvent::Prepare { now }); }
                Effects::none()
            }
            FirstFrameEvent::Hide => {
                // After readiness, a cancelled refresh keeps the cached frame and stops retrying.
                self.reveal = Reveal::Idle;
                if self.notice == Notice::Sent { self.frame = Frame::Drawn { preparing: None }; }
                Effects::none()
            }
            FirstFrameEvent::Expire => {
                // The first frame stays hidden (the host reports it); a refresh reveals the
                // cached frame rather than nothing.
                self.frame = match self.frame {
                    Frame::Preparing { .. } => Frame::Stale,
                    Frame::Drawn { .. } => Frame::Drawn { preparing: None },
                    Frame::Stale => Frame::Stale,
                };
                if self.notice == Notice::Owed { return Effects::none(); }
                self.frame = Frame::Drawn { preparing: None };
                self.take_reveal()
            }
            FirstFrameEvent::GpuDone => {
                self.frame = match self.frame {
                    Frame::Stale => Frame::Drawn { preparing: None },
                    Frame::Preparing { retry, until } => Frame::Drawn { preparing: Some((retry, until)) },
                    drawn => drawn,
                };
                Effects::none()
            }
            FirstFrameEvent::Complete => {
                self.frame = Frame::Drawn { preparing: None };
                self.take_reveal()
            }
            FirstFrameEvent::Notified => {
                self.notice = Notice::Sent;
                Effects::none()
            }
            FirstFrameEvent::Skipped { now } => {
                if let OnReady::Waiting { fallback } = &mut self.on_ready {
                    *fallback = Some(fallback.map_or(now, |at| at.min(now)));
                }
                Effects::none()
            }
            FirstFrameEvent::TakeReady => match std::mem::replace(&mut self.on_ready, OnReady::Ran) {
                OnReady::Waiting { .. } => Effects::one(RunOnReady),
                OnReady::Ran => Effects::none(),
            },
        }
    }

    fn unready(&mut self) {
        self.frame = match self.frame {
            Frame::Drawn { preparing: Some((retry, until)) } => Frame::Preparing { retry, until },
            Frame::Drawn { preparing: None } => Frame::Stale,
            frame => frame,
        };
    }

    fn take_reveal(&mut self) -> Effects<FirstFrameEffect> {
        match std::mem::replace(&mut self.reveal, Reveal::Idle) {
            Reveal::Wanted => Effects::one(FirstFrameEffect::Show),
            Reveal::Idle => Effects::none(),
        }
    }

    fn shows(effects: Effects<FirstFrameEffect>) -> bool { effects.into_iter().any(|e| e == FirstFrameEffect::Show) }

    /// A completed frame holds the current state.
    pub(super) fn ready(&self) -> bool { matches!(self.frame, Frame::Drawn { .. }) }
    /// (next retry, give up) while a hidden frame prepares.
    pub(super) fn deadline(&self) -> Option<(Instant, Instant)> {
        match self.frame {
            Frame::Stale => None,
            Frame::Preparing { retry, until } => Some((retry, until)),
            Frame::Drawn { preparing } => preparing,
        }
    }
    /// A reveal waits for the frame.
    pub(super) fn revealing(&self) -> bool { self.reveal == Reveal::Wanted }
    /// The first frame has been reported.
    pub(super) fn notified(&self) -> bool { self.notice == Notice::Sent }

    /// A frame went by undrawn because the window isn't on screen, which may last until it is
    /// uncovered: `on_ready` stops waiting for a drawn one.
    pub(super) fn skipped(&mut self, now: Instant) { self.step(FirstFrameEvent::Skipped { now }); }
    /// When `on_ready` runs without a first frame, until it has run.
    pub(super) fn ready_deadline(&self) -> Option<Instant> {
        match self.on_ready {
            OnReady::Waiting { fallback } => fallback,
            OnReady::Ran => None,
        }
    }
    /// `on_ready` is owed without a first frame.
    pub(super) fn ready_due(&self, now: Instant) -> bool { self.ready_deadline().is_some_and(|at| at <= now) }
    /// Takes the first `on_ready`: true once per native window.
    pub(super) fn take_ready(&mut self) -> bool { !self.step(FirstFrameEvent::TakeReady).is_empty() }
    /// Whether to show the cached frame now. One already on screen shows at once; its next
    /// frame (or retry) draws a stale state, with no first-frame deadline.
    pub(super) fn request_reveal(&mut self, stale: bool, visible: bool, now: Instant) -> bool {
        Self::shows(self.step(FirstFrameEvent::RequestReveal { stale, visible, now }))
    }
    /// After readiness, a canceled refresh keeps the cached frame and stops retrying.
    pub(super) fn hide(&mut self) { self.step(FirstFrameEvent::Hide); }
    /// The drawable deadline passed. The first frame stays hidden (the host reports it); a
    /// refresh returns whether to reveal the cached frame rather than nothing.
    pub(super) fn expire(&mut self) -> bool { Self::shows(self.step(FirstFrameEvent::Expire)) }
    /// A hidden frame that got no drawable leaves the cached one stale. A preparation's deadline
    /// retries it; otherwise the host waits, and backs off (`Backoff`).
    pub(super) fn missed(&mut self) { self.unready(); }
    /// A hidden frame is prepared from `now`.
    pub(super) fn prepare(&mut self, now: Instant) { self.step(FirstFrameEvent::Prepare { now }); }
    /// The host brings the next try forward while the frame prepares.
    pub(super) fn retry_at(&mut self, retry: Instant) { self.step(FirstFrameEvent::Retry { retry }); }
    /// The GPU finished the frame it presents.
    pub(super) fn gpu_done(&mut self) { self.step(FirstFrameEvent::GpuDone); }
    /// The first frame was reported.
    pub(super) fn notify(&mut self) { self.step(FirstFrameEvent::Notified); }
    /// Hidden frames settle, except the refresh a reveal waits on: its new motion runs on.
    pub(super) fn settles(&self) -> bool { !(self.revealing() && self.notified()) }
    /// A finished frame still owes the first readiness, or a refresh owes its reveal.
    pub(super) fn pending(&self) -> bool { self.ready() && (!self.notified() || self.revealing()) }
    pub(super) fn complete(&mut self) -> bool { Self::shows(self.step(FirstFrameEvent::Complete)) }

    /// A launcher whose first frame has completed and been reported.
    #[cfg(test)]
    pub(super) fn drawn_notified() -> Self {
        FirstFrame { frame: Frame::Drawn { preparing: None }, notice: Notice::Sent, ..FirstFrame::default() }
    }
}
