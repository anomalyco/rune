use super::*;
use std::num::NonZeroU64;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Compare a Tx call on the UI thread with a handle request published under the queue lock.
/// The counter carries no request data; only its modification order is shared.
pub(crate) fn window_request_order() -> NonZeroU64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NonZeroU64::new(NEXT.fetch_add(1, Ordering::Relaxed)).expect("window request order exhausted")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowRequest {
    /// Show and focus, with an activation token when one was forwarded.
    Open(Option<String>),
    Show,
    Hide,
    Close,
}

/// What `AppHandle` asked of the window's activation: bring it forward, or minimize it. The
/// later of the two wins. A show leaves it alone so a minimize can follow one; opening,
/// hiding, closing or focusing the window takes a minimize back out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Activation {
    /// Focus and activate an already-visible window, restoring it if minimized.
    Focus,
    /// Minimize the window, after a show in the same turn.
    Minimize,
}

/// The non-generic part of [`Requests<S>`], drained by the window host.
#[derive(Default)]
pub(crate) struct WindowRequests {
    last_cancel: Option<NonZeroU64>,
    pub window: Option<WindowRequest>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub dock: Option<bool>,
    /// `AppHandle::set_color_scheme`: app-wide light/dark appearance override; last-wins.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub color_scheme: Option<crate::ColorScheme>,
    /// `AppHandle::focus` or `AppHandle::minimize`; last-wins (`cancel_minimize`).
    pub activation: Option<Activation>,
    pub position: Option<(i32, i32)>,
    pub logical_position: Option<(f64, f64)>,
    pub place: Option<crate::windows::PlaceRequest>,
    /// `AppHandle::quit` or `AppHandle::relaunch`, and what follows the exit.
    pub quit: Option<crate::quit::After>,
    /// `AppHandle::wake` asked for `poll`.
    pub woken: bool,
    /// `AppHandle::wake_urgent` asked for one-shot urgent frame admission on macOS.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub urgent: bool,
}

impl WindowRequests {
    /// Asks something of the window itself: show, hide or close it, focus or minimize it, or
    /// move it. A request added here reaches both `only_updates` and `Shared::window_pending`.
    fn asks_window(&self) -> bool {
        self.window.is_some()
            || self.activation.is_some()
            || self.position.is_some()
            || self.logical_position.is_some()
            || self.place.is_some()
    }

    /// A later open, hide, close or focus takes a minimize back out; a show leaves it, so a
    /// minimize can follow one.
    fn cancel_minimize(&mut self) {
        self.last_cancel = Some(window_request_order());
        if self.activation == Some(Activation::Minimize) { self.activation = None; }
    }

    /// Nothing but updates: no wake for `poll`, no window, focus or geometry request.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn only_updates(&self) -> bool {
        !self.woken && !self.urgent && !self.asks_window()
    }
}

#[cfg(test)]
pub(crate) struct Requests<S> {
    pub(crate) window_requests: WindowRequests,
    pub updates: Vec<Update<S>>,
}

#[cfg(test)]
impl<S> Default for Requests<S> {
    fn default() -> Self {
        Requests {
            window_requests: WindowRequests::default(),
            updates: Vec::new(),
        }
    }
}

#[cfg(test)]
impl<S> std::ops::Deref for Requests<S> {
    type Target = WindowRequests;
    fn deref(&self) -> &WindowRequests { &self.window_requests }
}

/// Deferred overlay builders queued by `AppHandle::overlay`, closed once the owning host ends.
#[cfg(not(target_arch = "wasm32"))]
enum OverlayQueue {
    Open(Vec<crate::overlay::OverlayBuilder>),
    Closed,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for OverlayQueue {
    fn default() -> Self {
        Self::Open(Vec::new())
    }
}

/// Non-generic shared state backing [`AppHandle<S>`] and the window host.
#[derive(Default)]
pub(crate) struct SharedCore {
    requests: Mutex<WindowRequests>,
    pub(super) wake: Mutex<Option<crate::runtime::Waker>>,
    /// Published by the window host for `AppHandle::is_visible` / `window_error`.
    pub(crate) visible: AtomicBool,
    /// `AppHandle::quit` was called, or the web app is ending.
    pub(crate) quitting: AtomicBool,
    pub(crate) window_error: Mutex<Option<String>>,
    /// Shared terminal GPU-loss latch synced from `App::gpu_loss_policy` so any `AppHandle`
    /// cloned before or after `.gpu_loss_policy(...)` sees the app's latch.
    #[cfg(not(target_arch = "wasm32"))]
    gpu_loss_latch: Mutex<Option<Arc<crate::render::loss_latch::GpuLossLatch>>>,
    /// Overlays queued dynamically through `AppHandle::overlay`.
    #[cfg(not(target_arch = "wasm32"))]
    overlays: Mutex<OverlayQueue>,
}

impl SharedCore {
    /// Installs the event loop's wake-up and wakes it once for requests made before it existed.
    pub fn connect(&self, wake: crate::runtime::Waker) {
        *self.wake.lock().unwrap() = Some(wake);
        self.wake();
    }

    pub fn wake(&self) {
        let wake = self.wake.lock().unwrap().clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    fn request(&self, f: impl FnOnce(&mut WindowRequests)) {
        f(&mut self.requests.lock().unwrap());
        self.wake();
    }

    pub(crate) fn wake_app(&self) {
        self.request(|q| q.woken = true);
    }

    pub(crate) fn wake_urgent(&self) {
        self.request(|q| { q.woken = true; q.urgent = true; });
    }

    pub(crate) fn open(&self) {
        self.request(|q| { q.window = Some(WindowRequest::Open(None)); q.cancel_minimize(); });
    }

    pub(crate) fn show(&self) {
        self.request(|q| q.window = Some(WindowRequest::Show));
    }

    pub(crate) fn hide(&self) {
        self.request(|q| { q.window = Some(WindowRequest::Hide); q.cancel_minimize(); });
    }

    pub(crate) fn focus(&self) {
        self.request(|q| { q.cancel_minimize(); q.activation = Some(Activation::Focus); });
    }

    pub(crate) fn set_position(&self, position: (i32, i32)) {
        self.request(|q| { q.position = Some(position); q.logical_position = None; q.place = None; });
    }

    pub(crate) fn set_logical_position(&self, position: (f64, f64)) {
        self.request(|q| { q.logical_position = Some(position); q.position = None; q.place = None; });
    }

    pub(crate) fn place(&self, display: crate::DisplayTarget, placement: crate::OverlayPlacement) {
        let place = crate::windows::PlaceRequest::new(display, placement);
        self.request(|q| { q.place = Some(place); q.position = None; q.logical_position = None; });
    }

    pub(crate) fn activate(&self, token: String) {
        self.request(|q| { q.window = Some(WindowRequest::Open(Some(token).filter(|t| !t.is_empty()))); q.cancel_minimize(); });
    }

    pub(crate) fn close(&self) {
        self.request(|q| { q.window = Some(WindowRequest::Close); q.cancel_minimize(); });
    }

    pub(crate) fn minimize(&self) {
        self.request(|q| { q.activation = Some(Activation::Minimize); });
    }

    pub(crate) fn quit(&self) {
        self.quitting.store(true, Ordering::Release);
        self.request(|q| q.quit = q.quit.max(Some(crate::quit::After::Exit)));
    }

    pub(crate) fn relaunch(&self) {
        self.quitting.store(true, Ordering::Release);
        self.request(|q| q.quit = Some(crate::quit::After::Relaunch));
    }

    pub(crate) fn set_dock_icon(&self, visible: bool) {
        self.request(|q| q.dock = Some(visible));
    }

    pub(crate) fn set_color_scheme(&self, scheme: crate::ColorScheme) {
        self.request(|q| q.color_scheme = Some(scheme));
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_color_scheme(&self) -> Option<crate::ColorScheme> {
        self.requests.lock().unwrap().color_scheme.take()
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Acquire)
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        self.visible.store(visible, Ordering::Release);
    }

    pub(crate) fn window_error(&self) -> Option<String> {
        self.window_error.lock().unwrap().clone()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn set_gpu_loss_latch(
        &self,
        latch: Option<Arc<crate::render::loss_latch::GpuLossLatch>>,
    ) {
        *self.gpu_loss_latch.lock().unwrap_or_else(|e| e.into_inner()) = latch;
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn gpu_loss_latch(&self) -> Option<Arc<crate::render::loss_latch::GpuLossLatch>> {
        self.gpu_loss_latch.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn set_window_error(&self, error: Option<String>) {
        *self.window_error.lock().unwrap() = error;
    }

    pub(crate) fn take_window(&self) -> WindowRequests {
        std::mem::take(&mut self.requests.lock().unwrap())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn finish_drain_window(&self, rt: &mut crate::runtime::RuntimeCore, minimize: Option<NonZeroU64>) -> WindowRequests {
        let batch = self.take_window();
        if minimize > batch.last_cancel {
            rt.minimize_step(crate::window::MinimizeEvent::Asked);
        }
        batch
    }

    pub(crate) fn asks_window(&self) -> bool {
        self.requests.lock().unwrap().asks_window()
    }

    pub(crate) fn take_place(&self) -> Option<crate::windows::PlaceRequest> {
        self.requests.lock().unwrap().place.take()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn push_overlay(&self, builder: crate::overlay::OverlayBuilder) -> bool {
        let rejected = match &mut *self.overlays.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            OverlayQueue::Open(pending) => {
                pending.push(builder);
                None
            }
            OverlayQueue::Closed => Some(builder),
        };
        if let Some(builder) = rejected {
            drop(builder);
            false
        } else {
            self.wake();
            true
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn has_overlays(&self) -> bool {
        match &*self.overlays.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            OverlayQueue::Open(pending) => !pending.is_empty(),
            OverlayQueue::Closed => false,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn take_overlays(&self) -> Vec<crate::overlay::OverlayBuilder> {
        match &mut *self.overlays.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            OverlayQueue::Open(pending) => std::mem::take(pending),
            OverlayQueue::Closed => Vec::new(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn close_overlays(&self) {
        let dropped = std::mem::replace(
            &mut *self.overlays.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            OverlayQueue::Closed,
        );
        drop(dropped);
    }
}

pub(crate) struct Shared<S> {
    pub(crate) core: Arc<SharedCore>,
    pub(crate) menus: crate::native_menu::Hub<S>,
    pub(crate) shortcuts: crate::shortcuts::Hub<S>,
    updates: Mutex<Vec<Update<S>>>,
}

impl<S> Default for Shared<S> {
    fn default() -> Self {
        Shared {
            core: Arc::default(),
            menus: crate::native_menu::Hub::default(),
            shortcuts: crate::shortcuts::Hub::default(),
            updates: Mutex::default(),
        }
    }
}

impl<S> std::ops::Deref for Shared<S> {
    type Target = SharedCore;
    fn deref(&self) -> &SharedCore { &self.core }
}

impl<S> Shared<S> {
    #[cfg(test)]
    pub fn take(&self) -> Requests<S> {
        let updates = self.take_updates();
        let window_requests = self.core.take_window();
        Requests { window_requests, updates }
    }

    /// The queued updates alone; the other requests stay.
    pub(crate) fn take_updates(&self) -> Vec<Update<S>> {
        std::mem::take(&mut self.updates.lock().unwrap())
    }

    /// Runs the queued updates with `run`, then takes the other requests, so a window an update
    /// opens (or shows, focuses, places, quits) applies in this turn. Updates the updates queue
    /// wait for the next turn, which their wake brings.
    pub(crate) fn drain(&self, mut run: impl FnMut(Update<S>)) -> WindowRequests {
        for f in self.take_updates() { run(f); }
        self.core.take_window()
    }

    /// Drain a native window's updates, running `after` (menu callbacks) after each update.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn drain_window(&self, rt: &mut crate::runtime::Runtime<S>, mut after: impl FnMut(bool)) -> WindowRequests
    where S: 'static {
        // Only these updates’ minimizes, and only those that survive their pick callbacks, are
        // ordered here; a minimize a handler or nested callback latched is already in the machine.
        let mut minimize = None;
        for f in self.take_updates() {
            let (changed, ordered) = rt.update_if_ordered(f);
            minimize = minimize.max(ordered);
            after(changed);
        }
        self.core.finish_drain_window(rt.core_mut(), minimize)
    }

    /// Queues `f` for the next `take`, as `AppHandle::update_if` does, without waking the loop.
    pub(crate) fn queue_update(&self, f: Update<S>) {
        self.updates.lock().unwrap().push(f);
    }

    /// Queues `f` and wakes the event loop.
    pub(crate) fn push_update(&self, f: Update<S>) {
        self.queue_update(f);
        self.core.wake();
    }

    /// Whether any typed updates wait for `take_updates`.
    pub(crate) fn has_updates(&self) -> bool {
        !self.updates.lock().unwrap().is_empty()
    }
}

impl<S: 'static> AppHandle<S> {
    /// Wake the event loop so `AppSpec::poll` runs, also while the window is hidden, and arm
    /// one-shot urgent frame admission on macOS so the next drawn frame may bypass display-link
    /// `Wait` once by advancing its target to now.
    ///
    /// Duplicate urgent wakes before that frame claims pacing coalesce into one admission. Normal
    /// display-link pacing resumes immediately after (even if drawable acquisition or presentation
    /// fails), leaving drawable backpressure and recovery behavior unchanged. On platforms without
    /// a display link this behaves like [`wake`](Self::wake).
    #[inline]
    pub fn wake_urgent(&self) { self.shared.core.wake_urgent(); }

    /// Like [`update`](Self::update), and arms one-shot urgent frame admission on macOS for the
    /// resulting frame (see [`wake_urgent`](Self::wake_urgent)).
    ///
    /// For conditional updates that may leave state unchanged, call
    /// [`Tx::request_urgent_frame`](crate::Tx::request_urgent_frame) inside
    /// [`update_if`](Self::update_if) when returning `true`.
    pub fn update_urgent(&self, f: impl FnOnce(&mut S, &mut Tx) + Send + 'static) {
        self.update_if(move |s, tx| {
            f(s, tx);
            tx.request_urgent_frame();
            true
        });
    }

    /// Adds a transparent overlay to the running app from any thread and returns its
    /// [`OverlayHandle`](crate::OverlayHandle) immediately.
    ///
    /// `build` runs on the UI thread in the next event-loop turn (or at startup if called before
    /// [`App::run`]) and returns the overlay's configuration and initial state. Requests queued on
    /// the returned handle before `build` runs (such as `show`, `update`, or `close`) apply as
    /// soon as the overlay connects. Inherits this app's fonts, font families, font smoothing
    /// (unless overridden on [`ConfiguredOverlay`](crate::ConfiguredOverlay)), emoji family,
    /// prewarmed effects, and GPU-loss policy. Headless runs and web (`wasm32`) return a
    /// disconnected handle.
    pub fn overlay<T: 'static, C: Into<crate::ConfiguredOverlay<T>>>(
        &self,
        build: impl FnOnce() -> (C, T) + Send + 'static,
    ) -> crate::overlay::OverlayHandle<T> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (builder, handle) = crate::overlay::make_pending_overlay(build);
            if self.shared.core.push_overlay(builder) {
                handle
            } else {
                crate::overlay::OverlayHandle::disconnected()
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = build;
            crate::overlay::OverlayHandle::disconnected()
        }
    }
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod drain_tests;
