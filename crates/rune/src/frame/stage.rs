//! Per-window input and frame-stage observability hooks: `App::on_frame_stage`,
//! `App::on_window_key`, and `App::on_window_focus`.
//!
//! All three hooks are zero-cost when unset: each boundary checks one `Option` or one
//! thread-local `bool` without reading clocks, locking, or allocating.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::time::Instant;

/// `App::on_frame_stage`'s hook.
pub(crate) type OnFrameStage = Box<dyn FnMut(&FrameStageEvent)>;

/// `App::on_window_key`'s hook.
pub(crate) type OnWindowKey = Box<dyn FnMut(&WindowKeyEvent)>;

/// `App::on_window_focus`'s hook.
pub(crate) type OnWindowFocus = Box<dyn FnMut(&WindowFocusEvent)>;

/// Whether a [`FrameStageEvent`] marks entering or leaving its stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StageBoundary {
    /// Entering the stage.
    Begin,
    /// Leaving the stage.
    End,
}

/// One CPU wall-time stage of a window frame, nested runtime pass, or text subsystem boundary,
/// as [`App::on_frame_stage`](crate::App::on_frame_stage) reports it.
///
/// Stages nest naturally in real time (for example `Text` inside `Layout` inside `Commit`
/// inside `Runtime`, `SystemFontFallback` inside `Text`, and `GlyphAtlas` inside `Paint` or
/// `Encode`), so callers building flat totals must not double-count nested stages.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameStage {
    /// One redraw attempt on the UI thread, from entry until the frame finishes (drawn,
    /// unchanged, missed, or skipped). Always closes before
    /// [`App::on_frame_encoded`](crate::App::on_frame_encoded) runs for the same frame.
    Redraw,
    /// Display-link pacing query (`DisplayLink::next_frame` on macOS).
    DisplayLink,
    /// Swap-chain or drawable acquisition.
    Acquire,
    /// Stepping the runtime (queued input, `poll`, commits, layout, and paint) for the frame.
    Runtime,
    /// Nested runtime phase: running the app's `view` function.
    View,
    /// Nested runtime phase: committing a rebuilt tree (inclusive of nested `View`/`Layout`/`Text`).
    Commit,
    /// Nested runtime phase: layout computation (inclusive of nested `Text` measurement).
    Layout,
    /// Nested runtime phase: text shaping and line breaking.
    Text,
    /// Glyph rasterization on atlas cache misses, dirty atlas row uploads, and atlas mask fills.
    GlyphAtlas,
    /// Nested runtime phase: painting the committed tree into draw instances.
    Paint,
    /// Nested runtime phase: applying committed layout, scroll, focus, and motion state.
    Apply,
    /// Uploading GPU frame resources (images, native surfaces, instances) before encoding.
    Upload,
    /// Encoding GPU draw commands (or painting on CPU-first windows).
    Encode,
    /// Submitting encoded commands to the GPU queue.
    Submit,
    /// Presenting the rendered frame (`queue.present`, CPU blit, or resize-sync commit).
    Present,
    /// Text setup and warming: `TextSystem` initialization, family switches, and idle
    /// `Tx::prepare_text`.
    TextWarm,
    /// On-demand system font fallback lookup (`load_fallbacks`, missing-slot fallback, or
    /// `system_mono`).
    SystemFontFallback,
}

/// One stage boundary event delivered to [`App::on_frame_stage`](crate::App::on_frame_stage)
/// and [`WindowSpec::on_frame_stage`](crate::WindowSpec::on_frame_stage).
#[derive(Clone, Debug)]
pub struct FrameStageEvent {
    at: Instant,
    stage: FrameStage,
    boundary: StageBoundary,
    window: Option<Rc<str>>,
    seq: Option<u64>,
}

impl FrameStageEvent {
    /// CPU wall-clock timestamp when this boundary was reached.
    pub fn at(&self) -> Instant { self.at }

    /// Which stage entered or exited.
    pub fn stage(&self) -> FrameStage { self.stage }

    /// Whether this event marks entering ([`StageBoundary::Begin`]) or leaving
    /// ([`StageBoundary::End`]) the stage.
    pub fn boundary(&self) -> StageBoundary { self.boundary }

    /// The window's key (`WindowSpec::new`; `"main"` for `App::run`).
    pub fn window(&self) -> Option<&str> { self.window.as_deref() }

    /// Per-window frame sequence number matching [`FrameEncoded::seq`](crate::FrameEncoded::seq)
    /// and [`FramePresented::seq`](crate::FramePresented::seq) for work inside a frame, or
    /// `None` for out-of-frame work (startup text setup, idle `Tx::prepare_text` warming, or
    /// event-turn fallback outside a redraw).
    pub fn seq(&self) -> Option<u64> { self.seq }
}

/// Metadata for a native key event received by a window before Rune routes it, as
/// [`App::on_window_key`](crate::App::on_window_key) reports it.
///
/// Intentionally omits key codes, characters, and typed text so latency telemetry never
/// captures user input content.
#[derive(Clone, Debug)]
pub struct WindowKeyEvent {
    at: Instant,
    input_time: Option<Instant>,
    window: Option<Rc<str>>,
    pressed: bool,
    repeat: bool,
    has_focus: bool,
}

impl WindowKeyEvent {
    /// CPU wall-clock timestamp when the window host received the native key event on the UI thread.
    pub fn at(&self) -> Instant { self.at }

    /// Platform event timestamp when available (on macOS, the `NSEvent` timestamp; otherwise
    /// the host receipt time), matching `Tx::input_time`. Subtracting `input_time` from
    /// [`at`](Self::at) isolates OS/winit delivery delay before Rune received the key.
    pub fn input_time(&self) -> Option<Instant> { self.input_time }

    /// The window's key (`WindowSpec::new`; `"main"` for `App::run`).
    pub fn window(&self) -> Option<&str> { self.window.as_deref() }

    /// `true` for a key press, `false` for a key release.
    pub fn pressed(&self) -> bool { self.pressed }

    /// `true` when the platform marked this key event as an auto-repeat.
    pub fn repeat(&self) -> bool { self.repeat }

    /// `true` when a focusable control in this window had keyboard focus before routing.
    pub fn has_focus(&self) -> bool { self.has_focus }
}

/// A native window focus change, as [`App::on_window_focus`](crate::App::on_window_focus)
/// reports it.
#[derive(Clone, Debug)]
pub struct WindowFocusEvent {
    at: Instant,
    window: Option<Rc<str>>,
    focused: bool,
}

impl WindowFocusEvent {
    /// CPU wall-clock timestamp when the window host received the native focus change.
    pub fn at(&self) -> Instant { self.at }

    /// The window's key (`WindowSpec::new`; `"main"` for `App::run`).
    pub fn window(&self) -> Option<&str> { self.window.as_deref() }

    /// `true` when the window gained focus, `false` when it lost focus.
    pub fn focused(&self) -> bool { self.focused }
}

/// Per-window observability hooks configured on `App` / `WindowSpec`.
#[derive(Default)]
pub(crate) struct WindowObservability {
    pub(crate) on_frame_stage: Option<OnFrameStage>,
    pub(crate) on_window_key: Option<OnWindowKey>,
    pub(crate) on_window_focus: Option<OnWindowFocus>,
}

impl WindowObservability {
    pub(crate) fn take(&mut self) -> Self {
        Self {
            on_frame_stage: self.on_frame_stage.take(),
            on_window_key: self.on_window_key.take(),
            on_window_focus: self.on_window_focus.take(),
        }
    }
}

struct StageSlot {
    hook: Rc<RefCell<Option<OnFrameStage>>>,
    window: Option<Rc<str>>,
}

struct ActiveSink {
    hook: OnFrameStage,
    window: Option<Rc<str>>,
    seq: Option<u64>,
}

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static SINK: RefCell<Option<ActiveSink>> = const { RefCell::new(None) };
}

/// Live per-window observability hooks owned by a window host.
///
/// Without hooks, every method is a single `Option` check with no clocks, allocations, or
/// thread-local writes.
#[derive(Default)]
pub(crate) struct HostObservability {
    stages: Option<StageSlot>,
    on_key: Option<(OnWindowKey, Option<Rc<str>>)>,
    on_focus: Option<(OnWindowFocus, Option<Rc<str>>)>,
}

impl HostObservability {
    pub(crate) fn new(obs: WindowObservability, window: Option<&str>) -> Self {
        let win: Option<Rc<str>> = window.map(Rc::from);
        let stages = obs.on_frame_stage.map(|hook| {
            crate::frame::profile::install_text_hook();
            StageSlot { hook: Rc::new(RefCell::new(Some(hook))), window: win.clone() }
        });
        let on_key = obs.on_window_key.map(|hook| (hook, win.clone()));
        let on_focus = obs.on_window_focus.map(|hook| (hook, win));
        Self { stages, on_key, on_focus }
    }

    /// Whether an `on_frame_stage` hook is registered (so `FrameReports` must track `seq`).
    pub(crate) fn wants_stages(&self) -> bool { self.stages.is_some() }

    /// Activates this window's `on_frame_stage` hook on the current thread for the lifetime of
    /// the returned guard, tagging every emitted stage boundary with `seq`.
    ///
    /// When no hook is registered, this is a single `Option` check and returns a no-op guard.
    #[inline]
    pub(crate) fn scope_stages(&self, seq: Option<u64>) -> StageScopeGuard {
        let Some(slot) = &self.stages else { return StageScopeGuard(None) };
        let Some(hook) = slot.hook.borrow_mut().take() else { return StageScopeGuard(None) };
        let prev = SINK.with_borrow_mut(|cur| {
            cur.replace(ActiveSink { hook, window: slot.window.clone(), seq })
        });
        ACTIVE.set(true);
        StageScopeGuard(Some(StageScopeInner { slot: slot.hook.clone(), prev }))
    }

    /// Reports a native key event received before routing. No-op when unset.
    #[inline]
    pub(crate) fn report_key(
        &mut self,
        pressed: bool,
        repeat: bool,
        has_focus: bool,
        input_time: Option<Instant>,
    ) {
        let Some((hook, window)) = &mut self.on_key else { return };
        let event = WindowKeyEvent {
            at: crate::time::now(),
            input_time,
            window: window.clone(),
            pressed,
            repeat,
            has_focus,
        };
        hook(&event);
    }

    /// Reports a native window focus change. No-op when unset.
    #[inline]
    pub(crate) fn report_focus(&mut self, focused: bool) {
        let Some((hook, window)) = &mut self.on_focus else { return };
        let event = WindowFocusEvent {
            at: crate::time::now(),
            window: window.clone(),
            focused,
        };
        hook(&event);
    }
}

struct StageScopeInner {
    slot: Rc<RefCell<Option<OnFrameStage>>>,
    prev: Option<ActiveSink>,
}

/// RAII guard restoring the thread-local stage hook and returning the window's hook even on panic.
#[must_use]
pub(crate) struct StageScopeGuard(Option<StageScopeInner>);

impl Drop for StageScopeGuard {
    fn drop(&mut self) {
        let Some(inner) = self.0.take() else { return };
        let current = SINK.with_borrow_mut(|cur| std::mem::replace(cur, inner.prev));
        ACTIVE.set(SINK.with_borrow(Option::is_some));
        if let Some(sink) = current {
            *inner.slot.borrow_mut() = Some(sink.hook);
        }
    }
}

/// Whether an `on_frame_stage` hook is active on the current thread. One TLS `bool` load.
#[inline(always)]
pub(crate) fn active() -> bool { ACTIVE.get() }

/// Emits one stage boundary to the active thread-local hook, if any.
#[inline(always)]
pub(crate) fn emit(stage: FrameStage, boundary: StageBoundary) {
    if !ACTIVE.get() {
        return;
    }
    emit_slow(stage, boundary);
}

/// Maps an internal `frame::profile::Phase` to its public [`FrameStage`] and emits `boundary`.
#[inline(always)]
pub(crate) fn emit_phase(phase: crate::frame::profile::Phase, boundary: StageBoundary) {
    if !ACTIVE.get() {
        return;
    }
    let stage = match phase {
        crate::frame::profile::Phase::View => FrameStage::View,
        crate::frame::profile::Phase::Commit => FrameStage::Commit,
        crate::frame::profile::Phase::Layout => FrameStage::Layout,
        crate::frame::profile::Phase::Text => FrameStage::Text,
        crate::frame::profile::Phase::Paint => FrameStage::Paint,
        crate::frame::profile::Phase::Apply => FrameStage::Apply,
    };
    emit_slow(stage, boundary);
}

#[cold]
#[inline(never)]
fn emit_slow(stage: FrameStage, boundary: StageBoundary) {
    let Some(sink) = SINK.with_borrow_mut(Option::take) else {
        return;
    };
    ACTIVE.set(false);
    struct Restore(Option<ActiveSink>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(sink) = self.0.take() {
                SINK.with_borrow_mut(|slot| *slot = Some(sink));
                ACTIVE.set(true);
            }
        }
    }
    let event = FrameStageEvent {
        at: crate::time::now(),
        stage,
        boundary,
        window: sink.window.clone(),
        seq: sink.seq,
    };
    let mut restore = Restore(Some(sink));
    if let Some(sink) = restore.0.as_mut() {
        (sink.hook)(&event);
    }
}

/// RAII stage boundary guard. Emits [`StageBoundary::Begin`] on creation and
/// [`StageBoundary::End`] on [`end`](Self::end) or drop (whichever comes first).
///
/// Costs one thread-local `bool` load when no `on_frame_stage` hook is active.
#[derive(Default)]
pub(crate) struct StageGuard(Option<FrameStage>);

impl StageGuard {
    #[inline(always)]
    pub(crate) fn enter(stage: FrameStage) -> Self {
        if !ACTIVE.get() {
            return Self(None);
        }
        emit_slow(stage, StageBoundary::Begin);
        Self(Some(stage))
    }

    /// Explicitly closes this stage early (for example closing `FrameStage::Redraw` before
    /// `App::on_frame_encoded` fires for the same frame). Idempotent.
    #[inline(always)]
    pub(crate) fn end(&mut self) {
        if let Some(stage) = self.0.take() {
            emit_slow(stage, StageBoundary::End);
        }
    }
}

impl Drop for StageGuard {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some(stage) = self.0.take() {
            emit_slow(stage, StageBoundary::End);
        }
    }
}

#[cfg(test)]
#[path = "stage_tests.rs"]
mod tests;
