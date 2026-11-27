//! `App::on_frame_presented`: actual screen presentation timestamps for drawn frames.
//!
//! Correlates each platform presentation token (macOS Metal drawable sequence, Windows DXGI
//! swap-chain `PresentCount`, or Linux Wayland `wp_presentation_feedback` sequence) with its
//! `FrameEncoded::seq()` in a bounded FIFO queue (`MAX_PENDING = 64`), delivers `FramePresented`
//! callbacks on the UI thread in submission order, and clears platform watches/poll timers as soon
//! as all in-flight frames land so idle windows never wake.

use std::rc::Rc;

use crate::time::Instant;

#[cfg(any(test, not(target_arch = "wasm32")))]
use std::collections::VecDeque;
#[cfg(any(test, not(target_arch = "wasm32")))]
use std::sync::Arc;
#[cfg(any(test, not(target_arch = "wasm32")))]
use crate::machine::Effects;

/// `App::on_frame_presented`'s hook.
pub(crate) type OnFramePresented = Box<dyn FnMut(&FramePresented)>;

/// Maximum in-flight drawable-to-frame correlations kept per window.
///
/// Presentation runs a few frames behind submission; if a frame never reports (for example after a
/// stalled or lost surface), the oldest entry is evicted so memory stays bounded and newer landed
/// frames are never blocked indefinitely.
#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) const MAX_PENDING: usize = 64;

/// When a drawn frame actually reached the screen, as
/// [`App::on_frame_presented`](crate::App::on_frame_presented) reports it.
///
/// Correlates with [`FrameEncoded`](crate::FrameEncoded) via [`seq`](Self::seq): every `Drawn`
/// frame reported to `on_frame_encoded` shares its sequence number with the matching
/// `FramePresented` callback once the platform reports that frame presented or skipped.
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
    /// frame.
    pub fn seq(&self) -> u64 { self.seq }

    /// The window's key (`WindowSpec::new`; `"main"` for `App::run`).
    pub fn window(&self) -> Option<&str> { self.window.as_deref() }

    /// When the frame actually reached the display, or `None` when the system skipped/discarded
    /// it or could not retain an individual timestamp for it (for example: Metal reported
    /// `presentedTime == 0.0`, Wayland sent `wp_presentation_feedback.discarded`, or Windows
    /// DXGI's single-slot `GetFrameStatistics` advanced past an older frame before it was polled).
    pub fn presented_at(&self) -> Option<Instant> { self.presented_at }
}

/// Source of drawable presentation completions (Metal's `MTLDrawable::addPresentedHandler` on
/// macOS; fakes in headless tests).
#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) trait PresentSource: Send + Sync {
    /// Drains completed `(drawable_seq, presented_at)` pairs into `out` and updates whether
    /// future completions should wake the UI thread (`watch`).
    fn poll_and_watch(&self, out: &mut Vec<(u64, Option<Instant>)>, watch: bool);

    /// Updates whether future completions should wake the UI thread (`watch`).
    fn set_watch(&self, watch: bool);
}

/// Whether the present source is currently asked to wake the UI thread on drawable completion.
#[cfg(any(test, not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WatchState {
    #[default]
    Idle,
    Watching,
}

#[cfg(any(test, not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchEvent {
    Pending,
    Drained,
}

#[cfg(any(test, not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchEffect {
    SetWatch(bool),
}

#[cfg(any(test, not(target_arch = "wasm32")))]
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

#[cfg(any(test, not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Waiting,
    Ready(Option<Instant>),
}

#[cfg(any(test, not(target_arch = "wasm32")))]
struct Pending {
    drawable_seq: u64,
    frame_seq: u64,
    status: Status,
}

#[cfg(any(test, not(target_arch = "wasm32")))]
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
#[cfg(any(test, not(target_arch = "wasm32")))]
#[derive(Default)]
pub(crate) struct FramePresentReports(Option<Tracker>);

#[cfg(any(test, not(target_arch = "wasm32")))]
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

    /// Whether any submitted frames are still waiting for presentation feedback.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn has_pending(&self) -> bool {
        self.0.as_ref().is_some_and(|t| !t.pending.is_empty())
    }

    /// Connects the platform completion source (called once when the Metal layer log is created).
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    pub(crate) fn set_source(&mut self, source: Arc<dyn PresentSource>) {
        if let Some(t) = &mut self.0 {
            t.source = Some(source);
        }
    }

    /// Records a `Drawn` frame's `(drawable_seq, frame_seq)` correlation for an attached
    /// `PresentSource` (macOS Metal) and immediately drains any completions that have landed.
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    pub(crate) fn push_and_drain(&mut self, drawable_seq: u64, frame_seq: u64) {
        let Some(t) = self.0.as_mut() else { return };
        if t.source.is_none() { return; }
        if t.pending.len() >= MAX_PENDING {
            t.pending.pop_front();
        }
        t.pending.push_back(Pending { drawable_seq, frame_seq, status: Status::Waiting });
        self.drain();
    }

    /// Records a `Drawn` frame's `(drawable_seq, frame_seq)` correlation for a UI-thread
    /// presentation tracker (Windows DXGI or Linux Wayland `wp_presentation`).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn push_pending(&mut self, drawable_seq: u64, frame_seq: u64) {
        let Some(t) = self.0.as_mut() else { return };
        if t.pending.len() >= MAX_PENDING {
            t.pending.pop_front();
        }
        t.pending.push_back(Pending { drawable_seq, frame_seq, status: Status::Waiting });
    }

    /// Writes every currently `Waiting` `drawable_seq` in submission order into `out`.
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    pub(crate) fn waiting_drawables(&self, out: &mut Vec<u64>) {
        out.clear();
        let Some(t) = self.0.as_ref() else { return };
        out.extend(
            t.pending
                .iter()
                .filter(|p| p.status == Status::Waiting)
                .map(|p| p.drawable_seq),
        );
    }

    /// Applies completions produced by a UI-thread presentation tracker (Windows DXGI or Linux
    /// Wayland `wp_presentation`) and delivers contiguous ready frames in submission order.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn apply_completions(&mut self, completions: impl IntoIterator<Item = (u64, Option<Instant>)>) {
        let Some(t) = self.0.as_mut() else { return };
        if t.pending.is_empty() { return; }
        for (drawable_seq, presented_at) in completions {
            if let Some(p) = t.pending.iter_mut().find(|p| p.drawable_seq == drawable_seq && p.status == Status::Waiting) {
                p.status = Status::Ready(presented_at);
            }
        }
        deliver_ready(t);
    }

    /// Drops unlanded entries without faking callbacks (for example after DXGI reports
    /// `DXGI_ERROR_UNSUPPORTED` or the settle window expires while a window is occluded).
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    pub(crate) fn drop_pending(&mut self) {
        let Some(t) = self.0.as_mut() else { return };
        t.pending.clear();
    }

    /// Drains ready presentation completions from an attached `PresentSource` on the UI thread in
    /// submission order and clears the source watch automatically once the queue empties so idle
    /// windows never wake.
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
            if let Some(at) = frame.presented_at {
                record_first_on_display(at);
            }
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

#[cfg(any(test, not(target_arch = "wasm32")))]
fn record_first_on_display(at: Instant) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FIRST: AtomicBool = AtomicBool::new(false);
    if !FIRST.swap(true, Ordering::Relaxed) {
        crate::marks::mark_at("frame.first.on_display", at);
    }
}

#[cfg(any(test, not(target_arch = "wasm32")))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn deliver_ready(t: &mut Tracker) {
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
    for frame in t.ready.drain(..) {
        if let Some(at) = frame.presented_at {
            record_first_on_display(at);
        }
        (t.hook)(&frame);
    }
}

#[cfg(test)]
#[path = "presented_tests.rs"]
mod tests;
