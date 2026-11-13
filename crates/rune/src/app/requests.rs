use super::*;
use std::num::NonZeroU64;
use std::sync::atomic::AtomicU64;

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

pub(crate) struct Requests<S> {
    last_cancel: Option<NonZeroU64>,
    pub window: Option<WindowRequest>,
    pub dock: Option<bool>,
    /// `AppHandle::focus` or `AppHandle::minimize`; last-wins (`cancel_minimize`).
    pub activation: Option<Activation>,
    pub position: Option<(i32, i32)>,
    pub place: Option<crate::windows::PlaceRequest>,
    /// `AppHandle::quit` or `AppHandle::relaunch`, and what follows the exit.
    pub quit: Option<crate::quit::After>,
    /// `AppHandle::wake` asked for `poll`.
    pub woken: bool,
    pub updates: Vec<Update<S>>,
}

impl<S> Requests<S> {
    /// Asks something of the window itself: show, hide or close it, focus or minimize it, or
    /// move it. A request added here reaches both `only_updates` and `Shared::window_pending`.
    fn asks_window(&self) -> bool {
        self.window.is_some() || self.activation.is_some() || self.position.is_some() || self.place.is_some()
    }

    /// A later open, hide, close or focus takes a minimize back out; a show leaves it, so a
    /// minimize can follow one.
    pub(super) fn cancel_minimize(&mut self) {
        self.last_cancel = Some(window_request_order());
        if self.activation == Some(Activation::Minimize) { self.activation = None; }
    }

    /// Nothing but updates: no wake for `poll`, no window, focus or geometry request.
    pub(crate) fn only_updates(&self) -> bool {
        !self.woken && !self.asks_window()
    }
}

impl<S> Default for Requests<S> {
    fn default() -> Self {
        Requests {
            last_cancel: None,
            window: None,
            dock: None,
            activation: None,
            position: None,
            place: None,
            quit: None,
            woken: false,
            updates: Vec::new(),
        }
    }
}

pub(crate) struct Shared<S> {
    pub(crate) menus: crate::native_menu::Hub<S>,
    pub(crate) shortcuts: crate::shortcuts::Hub<S>,
    pub(super) requests: Mutex<Requests<S>>,
    wake: Mutex<Option<crate::runtime::Waker>>,
    /// Published by the window host for `AppHandle::is_visible` / `window_error`.
    pub(crate) visible: AtomicBool,
    /// `AppHandle::quit` was called, or the web app is ending.
    pub(crate) quitting: AtomicBool,
    pub(crate) window_error: Mutex<Option<String>>,
}

impl<S> Default for Shared<S> {
    fn default() -> Self {
        Shared {
            menus: crate::native_menu::Hub::default(),
            shortcuts: crate::shortcuts::Hub::default(),
            requests: Mutex::default(),
            wake: Mutex::new(None),
            visible: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            window_error: Mutex::new(None),
        }
    }
}

impl<S> Shared<S> {
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

    pub fn take(&self) -> Requests<S> {
        std::mem::take(&mut self.requests.lock().unwrap())
    }

    /// The queued updates alone; the other requests stay.
    pub(crate) fn take_updates(&self) -> Vec<Update<S>> {
        std::mem::take(&mut self.requests.lock().unwrap().updates)
    }

    /// Runs the queued updates with `run`, then takes the other requests, so a window an update
    /// opens (or shows, focuses, places, quits) applies in this turn. Updates the updates queue
    /// wait for the next turn, which their wake brings.
    pub(crate) fn drain(&self, mut run: impl FnMut(Update<S>)) -> Requests<S> {
        for f in self.take_updates() { run(f); }
        let mut requests = self.requests.lock().unwrap();
        let later = std::mem::take(&mut requests.updates);
        std::mem::replace(&mut *requests, Requests { updates: later, ..Requests::default() })
    }

    /// Drain a native window's updates, running `after` (menu callbacks) after each update.
    pub(crate) fn drain_window(&self, rt: &mut crate::runtime::Runtime<S>, mut after: impl FnMut(bool)) -> Requests<S>
    where S: 'static {
        // Only these updates’ minimizes, and only those that survive their pick callbacks, are
        // ordered here; a minimize a handler or nested callback latched is already in the machine.
        let mut minimize = None;
        let batch = self.drain(|f| {
            let (changed, ordered) = rt.update_if_ordered(f);
            minimize = minimize.max(ordered);
            after(changed);
        });
        if minimize > batch.last_cancel { rt.minimize_step(crate::window::MinimizeEvent::Asked); }
        batch
    }

    /// Queues `f` for the next `take`, as `AppHandle::update_if` does, without waking the loop.
    pub(crate) fn queue_update(&self, f: Update<S>) {
        self.requests.lock().unwrap().updates.push(f);
    }

    /// A request a `--desktop` run applies to the window (dock and quit aside) waits for `take`.
    pub(crate) fn window_pending(&self) -> bool {
        let r = self.requests.lock().unwrap();
        r.asks_window() || !r.updates.is_empty()
    }

    pub fn take_place(&self) -> Option<crate::windows::PlaceRequest> {
        self.requests.lock().unwrap().place.take()
    }
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod drain_tests;
