//! `App::on_frame_presented`: actual screen presentation timestamps for drawn frames.
//!
//! On macOS Metal, `CAMetalLayer` drawables report their actual `MTLDrawable.presentedTime`
//! asynchronously after submission. This module correlates each acquired drawable sequence number
//! with its `FrameEncoded::seq()` in a bounded FIFO queue (`MAX_PENDING = 64`), delivers
//! `FramePresented` callbacks on the UI thread in submission order, and enables the layer log's
//! main-loop wake watch only while drawables are in flight so idle windows never wake.

use std::rc::Rc;

use crate::time::Instant;

#[cfg(any(test, target_os = "macos"))]
use std::collections::VecDeque;
#[cfg(any(test, target_os = "macos"))]
use std::sync::Arc;
#[cfg(any(test, target_os = "macos"))]
use crate::machine::Effects;

/// `App::on_frame_presented`'s hook.
pub(crate) type OnFramePresented = Box<dyn FnMut(&FramePresented)>;

/// Maximum in-flight drawable-to-frame correlations kept per window.
///
/// Metal presentation runs a few frames behind submission; if a drawable never reports (for
/// example after a stalled or lost surface), the oldest entry is evicted so memory stays bounded
/// and newer landed frames are never blocked indefinitely.
#[cfg(any(test, target_os = "macos"))]
pub(crate) const MAX_PENDING: usize = 64;

/// When a drawn frame actually reached the screen, as
/// [`App::on_frame_presented`](crate::App::on_frame_presented) reports it.
///
/// Correlates with [`FrameEncoded`](crate::FrameEncoded) via [`seq`](Self::seq): every `Drawn`
/// frame reported to `on_frame_encoded` shares its sequence number with the matching
/// `FramePresented` callback once the system presents or skips that frame's drawable.
///
/// ```no_run
/// use std::cell::RefCell;
/// use std::collections::HashMap;
/// use std::rc::Rc;
/// use rune::{App, AppSpec, FrameOutcome, el};
///
/// let targets = Rc::new(RefCell::new(HashMap::new()));
/// let targets_out = targets.clone();
/// let spec = AppSpec::new("Demo", (320.0, 240.0), || (), |_, _| el());
/// App::new(spec)
///     .on_frame_encoded(move |f| {
///         if f.outcome() == FrameOutcome::Drawn {
///             if let Some(target) = f.presents_at() {
///                 targets.borrow_mut().insert(f.seq(), target);
///             }
///         }
///     })
///     .on_frame_presented(move |p| {
///         if let (Some(target), Some(actual)) =
///             (targets_out.borrow_mut().remove(&p.seq()), p.presented_at())
///         {
///             let _lag = actual.saturating_duration_since(target);
///         }
///     })
///     .run();
/// ```
#[derive(Clone, Debug)]
pub struct FramePresented {
    seq: u64,
    window: Option<Rc<str>>,
    presented_at: Option<Instant>,
}

impl FramePresented {
    /// The per-window frame sequence number, matching
    /// [`FrameEncoded::seq`](crate::FrameEncoded::seq) for the `Drawn` frame that submitted this
    /// drawable.
    pub fn seq(&self) -> u64 { self.seq }

    /// The window's key (`WindowSpec::new`; `"main"` for `App::run`).
    pub fn window(&self) -> Option<&str> { self.window.as_deref() }

    /// When the drawable reached the display, or `None` when the system skipped it (for example,
    /// Metal reported `presentedTime == 0.0`).
    pub fn presented_at(&self) -> Option<Instant> { self.presented_at }
}

/// Source of drawable presentation completions (Metal's `MTLDrawable::addPresentedHandler` on
/// macOS; fakes in headless tests).
#[cfg(any(test, target_os = "macos"))]
pub(crate) trait PresentSource: Send + Sync {
    /// Drains completed `(drawable_seq, presented_at)` pairs into `out` and updates whether
    /// future completions should wake the UI thread (`watch`).
    fn poll_and_watch(&self, out: &mut Vec<(u64, Option<Instant>)>, watch: bool);

    /// Updates whether future completions should wake the UI thread (`watch`).
    fn set_watch(&self, watch: bool);
}

/// Whether the present source is currently asked to wake the UI thread on drawable completion.
#[cfg(any(test, target_os = "macos"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WatchState {
    #[default]
    Idle,
    Watching,
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchEvent {
    Pending,
    Drained,
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchEffect {
    SetWatch(bool),
}

#[cfg(any(test, target_os = "macos"))]
fn step_watch(state: &mut WatchState, event: WatchEvent) -> Effects<WatchEffect> {
    match (*state, event) {
        (WatchState::Idle, WatchEvent::Pending) => {
            *state = WatchState::Watching;
            Effects::one(WatchEffect::SetWatch(true))
        }
        (WatchState::Watching, WatchEvent::Drained) => {
            *state = WatchState::Idle;
            Effects::one(WatchEffect::SetWatch(false))
        }
        _ => Effects::none(),
    }
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Waiting,
    Ready(Option<Instant>),
}

#[cfg(any(test, target_os = "macos"))]
struct Pending {
    drawable_seq: u64,
    frame_seq: u64,
    status: Status,
}

#[cfg(any(test, target_os = "macos"))]
struct Tracker {
    hook: OnFramePresented,
    window: Option<Rc<str>>,
    source: Option<Arc<dyn PresentSource>>,
    pending: VecDeque<Pending>,
    scratch: Vec<(u64, Option<Instant>)>,
    ready: Vec<FramePresented>,
    watch: WatchState,
}

/// A window's `App::on_frame_presented` hook and bounded drawable-to-frame correlation queue.
///
/// Without a hook every method is one `Option` check and does no locking, allocation, or wake
/// management.
#[cfg(any(test, target_os = "macos"))]
#[derive(Default)]
pub(crate) struct FramePresentReports(Option<Tracker>);

#[cfg(any(test, target_os = "macos"))]
impl FramePresentReports {
    pub(crate) fn new(hook: Option<OnFramePresented>, window: Option<&str>) -> Self {
        Self(hook.map(|hook| Tracker {
            hook,
            window: window.map(Rc::from),
            source: None,
            pending: VecDeque::new(),
            scratch: Vec::new(),
            ready: Vec::new(),
            watch: WatchState::Idle,
        }))
    }

    /// Whether a callback is registered for this window.
    pub(crate) fn wanted(&self) -> bool { self.0.is_some() }

    /// Connects the platform completion source (called once when the Metal layer log is created).
    pub(crate) fn set_source(&mut self, source: Arc<dyn PresentSource>) {
        if let Some(t) = &mut self.0 {
            t.source = Some(source);
        }
    }

    /// Records a `Drawn` frame's `(drawable_seq, frame_seq)` correlation and immediately drains
    /// any completions that have already landed.
    pub(crate) fn push_and_drain(&mut self, drawable_seq: u64, frame_seq: u64) {
        let Some(t) = self.0.as_mut() else { return };
        if t.source.is_none() { return; }
        if t.pending.len() >= MAX_PENDING {
            t.pending.pop_front();
        }
        t.pending.push_back(Pending { drawable_seq, frame_seq, status: Status::Waiting });
        self.drain();
    }

    /// Drains ready presentation completions on the UI thread in submission order and clears the
    /// source watch automatically once the queue empties so idle windows never wake.
    pub(crate) fn drain(&mut self) {
        let Some(t) = self.0.as_mut() else { return };
        if t.pending.is_empty() { return; }
        let Some(source) = t.source.as_ref() else { return };
        let _ = step_watch(&mut t.watch, WatchEvent::Pending);
        t.scratch.clear();
        source.poll_and_watch(&mut t.scratch, true);
        for (drawable_seq, presented_at) in t.scratch.drain(..) {
            if let Some(p) = t.pending.iter_mut().find(|p| p.drawable_seq == drawable_seq && p.status == Status::Waiting) {
                p.status = Status::Ready(presented_at);
            }
        }
        t.ready.clear();
        while t.pending.front().is_some_and(|front| matches!(front.status, Status::Ready(_))) {
            let Some(entry) = t.pending.pop_front() else { break };
            let Status::Ready(presented_at) = entry.status else { continue };
            t.ready.push(FramePresented {
                seq: entry.frame_seq,
                window: t.window.clone(),
                presented_at,
            });
        }
        if t.pending.is_empty() {
            for effect in step_watch(&mut t.watch, WatchEvent::Drained) {
                match effect {
                    WatchEffect::SetWatch(on) => source.set_watch(on),
                }
            }
        }
        for frame in t.ready.drain(..) {
            (t.hook)(&frame);
        }
    }

    /// Delivers any already-landed completions, drops orphaned in-flight entries (for example
    /// after a surface replacement or release), and turns off the source watch.
    pub(crate) fn reset(&mut self) {
        self.drain();
        let Some(t) = self.0.as_mut() else { return };
        if !t.pending.is_empty() {
            t.pending.clear();
            for effect in step_watch(&mut t.watch, WatchEvent::Drained) {
                match effect {
                    WatchEffect::SetWatch(on) => {
                        if let Some(source) = &t.source {
                            source.set_watch(on);
                        }
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.0.as_ref().map_or(0, |t| t.pending.len())
    }

    #[cfg(test)]
    pub(crate) fn is_watching(&self) -> bool {
        self.0.as_ref().is_some_and(|t| t.watch == WatchState::Watching)
    }
}

#[cfg(test)]
#[path = "presented_tests.rs"]
mod tests;
