//! UI-thread dispatch of secondary Wayland `wl_display` event queues on winit's shared display.
//!
//! Apps that bind extra Wayland protocols on winit's `wl_display` (via
//! [`WindowHandle::with_raw_display_handle`](crate::WindowHandle::with_raw_display_handle) and
//! `wayland_backend::client::Backend::from_foreign_display`) create their own `EventQueue` on
//! that display. Without a watch, winit sleeps in `ControlFlow::Wait` when a Wayland read queues
//! events solely on a foreign queue. Registering a watch with [`watch_display_queue`] turns on
//! winit's wake-on-Wayland-read flag while any watch is alive and dispatches pending queues on the
//! UI thread at the end of every turn, without spawning a reader thread or polling while idle.

use std::fmt;
use std::marker::PhantomData;

#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;

/// Registers a secondary Wayland event queue on winit's display for UI-thread dispatch, returning
/// an opaque [`DisplayQueueWatch`] guard that unregisters it on drop.
///
/// Use this when binding Wayland protocols that Rune does not expose (such as custom drag-and-drop,
/// data-control clipboards, or portal parent export) on the `wl_display` borrowed from
/// [`WindowHandle::with_raw_display_handle`](crate::WindowHandle::with_raw_display_handle).
/// Several watches may live at once (across multiple windows or a
/// `.windowless().keep_running()` daemon). While at least one watch is active on Wayland, Rune
/// enables winit's wake-on-read flag so any Wayland read that fills your queue wakes the event loop
/// in the same turn; when no watch is registered (and no Rune layer surface or Wayland blur is
/// live), wake-on-read stays off and costs nothing.
///
/// # Callback contracts
///
/// - **`pending`**: runs on the UI thread to check whether the queue already has unread events in
///   memory. It must be cheap, non-blocking, and syscall-free — for `wayland-client`,
///   `queue.prepare_read().is_none()` (immediately dropping the returned `ReadEventsGuard`) — and
///   must return `false` once `dispatch` has drained the queue.
/// - **`dispatch`**: runs on the UI thread in the same turn whenever `pending()` returned `true`.
///   It must never block, never read the Wayland socket (no `blocking_dispatch` or `read_events`),
///   and never dispatch winit's default queue (call `queue.dispatch_pending(&mut state)` only). To
///   mutate UI state or request a frame, notify a window through
///   [`AppHandle::update`](crate::AppHandle::update),
///   [`AppHandle::update_if`](crate::AppHandle::update_if),
///   [`AppHandle::wake`](crate::AppHandle::wake), or
///   [`AppHandle::open`](crate::AppHandle::open).
///
/// # Lifetime and shutdown order
///
/// Dropping the returned [`DisplayQueueWatch`] unregisters that watch immediately (safe even from
/// inside its own `pending` or `dispatch` callback). Watches registered during a callback take
/// effect on the next turn. On shutdown, Rune stops every watch and drops all registered closures
/// strictly **before** [`App::on_display_close`](crate::App::on_display_close) runs, so no callback
/// can touch foreign proxies after teardown begins and any `Rc` captured by the closures is
/// released first.
///
/// # Platforms
///
/// Active on Linux Wayland only. On X11, macOS, Windows, and web (`wasm32`), this function exists
/// and returns a valid [`DisplayQueueWatch`] whose callbacks never fire.
///
/// ```no_run
/// use std::cell::RefCell;
/// use std::rc::Rc;
/// use rune::{App, AppSpec, DisplayQueueWatch, el};
///
/// struct Substrate {
///     pending: bool,
///     watch: Option<DisplayQueueWatch>,
/// }
///
/// let substrate = Rc::new(RefCell::new(Substrate { pending: false, watch: None }));
/// let app = App::new(AppSpec::new("Display", (320.0, 200.0), || (), |_, _| el()));
/// let handle = app.handle();
/// let on_win = substrate.clone();
/// let on_close = substrate.clone();
/// app.on_window(move |window| {
///     let _ = window.with_raw_display_handle(|_display| {
///         if on_win.borrow().watch.is_some() { return; }
///         let p = on_win.clone();
///         let d = on_win.clone();
///         let h = handle.clone();
///         let watch = rune::watch_display_queue(
///             move || p.borrow().pending,
///             move || {
///                 d.borrow_mut().pending = false;
///                 h.wake();
///             },
///         );
///         on_win.borrow_mut().watch = Some(watch);
///     });
/// })
/// .on_display_close(move || {
///     on_close.borrow_mut().watch = None;
/// })
/// .run();
/// ```
pub fn watch_display_queue(
    pending: impl FnMut() -> bool + 'static,
    dispatch: impl FnMut() + 'static,
) -> DisplayQueueWatch {
    #[cfg(not(target_arch = "wasm32"))]
    let id = register(Box::new(pending), Box::new(dispatch));
    #[cfg(target_arch = "wasm32")]
    let id = {
        let _ = (pending, dispatch);
        0
    };
    DisplayQueueWatch { id, _not_send_sync: PhantomData }
}

/// Guard returned by [`watch_display_queue`]; dropping it immediately unregisters the secondary
/// display queue watch on the UI thread.
///
/// `DisplayQueueWatch` is `!Send` and `!Sync` so registration and unregistration stay on the UI
/// thread. It is safe to drop inside its own `pending` or `dispatch` callback, or after
/// [`App::on_display_close`](crate::App::on_display_close) has already stopped all watches.
///
/// ```no_run
/// use rune::DisplayQueueWatch;
///
/// let watch: DisplayQueueWatch = rune::watch_display_queue(|| false, || {});
/// drop(watch);
/// ```
#[must_use = "dropping DisplayQueueWatch immediately unregisters the display queue"]
pub struct DisplayQueueWatch {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    id: u64,
    _not_send_sync: PhantomData<*const ()>,
}

impl fmt::Debug for DisplayQueueWatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DisplayQueueWatch").finish_non_exhaustive()
    }
}

impl Drop for DisplayQueueWatch {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        unregister(self.id);
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct Callbacks {
    pending: Box<dyn FnMut() -> bool>,
    dispatch: Box<dyn FnMut()>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Entry {
    id: u64,
    /// `None` while this watch's `pending` or `dispatch` closure is currently running.
    callbacks: Option<Callbacks>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct Registry {
    next_id: u64,
    closed: bool,
    entries: Vec<Entry>,
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

#[cfg(not(target_arch = "wasm32"))]
fn register(pending: Box<dyn FnMut() -> bool>, dispatch: Box<dyn FnMut()>) -> u64 {
    let mut dropped = None;
    let id = REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        if reg.closed {
            dropped = Some(Callbacks { pending, dispatch });
            return 0;
        }
        reg.next_id += 1;
        let id = reg.next_id;
        reg.entries.push(Entry { id, callbacks: Some(Callbacks { pending, dispatch }) });
        id
    });
    drop(dropped);
    id
}

#[cfg(not(target_arch = "wasm32"))]
fn unregister(id: u64) {
    if id == 0 {
        return;
    }
    let removed = REGISTRY
        .try_with(|cell| {
            let mut reg = cell.borrow_mut();
            reg.entries
                .iter()
                .position(|e| e.id == id)
                .and_then(|idx| reg.entries.remove(idx).callbacks)
        })
        .ok()
        .flatten();
    drop(removed);
}

#[cfg(not(target_arch = "wasm32"))]
fn take_callbacks(id: u64) -> Option<Callbacks> {
    REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        if reg.closed {
            return None;
        }
        reg.entries.iter_mut().find(|e| e.id == id).and_then(|e| e.callbacks.take())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn is_live(id: u64) -> bool {
    REGISTRY.with(|cell| {
        let reg = cell.borrow();
        !reg.closed && reg.entries.iter().any(|e| e.id == id)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn restore_or_drop(id: u64, callbacks: Callbacks) {
    let leftover = REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        if reg.closed {
            return Some(callbacks);
        }
        if let Some(entry) = reg.entries.iter_mut().find(|e| e.id == id) {
            entry.callbacks = Some(callbacks);
            None
        } else {
            Some(callbacks)
        }
    });
    drop(leftover);
}

/// Removes an in-flight entry if its closure unwinds so no zombie `callbacks: None` slot remains.
#[cfg(not(target_arch = "wasm32"))]
struct InFlightGuard {
    id: u64,
    armed: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl InFlightGuard {
    fn new(id: u64) -> Self {
        Self { id, armed: true }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if self.armed {
            unregister(self.id);
        }
    }
}

/// Dispatches every registered watch whose `pending()` returns `true`, on the UI thread.
///
/// Watches registered during a callback take effect on the next turn; watches dropped during a
/// callback (including their own) are not reinserted.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn dispatch_pending() {
    let ids: Vec<u64> = REGISTRY.with(|cell| {
        let reg = cell.borrow();
        if reg.closed || reg.entries.is_empty() {
            return Vec::new();
        }
        reg.entries.iter().map(|e| e.id).collect()
    });
    for id in ids {
        let Some(mut callbacks) = take_callbacks(id) else {
            continue;
        };
        let guard = InFlightGuard::new(id);
        let ready = (callbacks.pending)();
        if ready && is_live(id) {
            (callbacks.dispatch)();
        }
        guard.disarm();
        restore_or_drop(id, callbacks);
    }
}

/// Whether at least one display-queue watch is currently registered on the UI thread.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn has_watches() -> bool {
    REGISTRY.with(|cell| {
        let reg = cell.borrow();
        !reg.closed && !reg.entries.is_empty()
    })
}

/// Whether any registered display-queue watch currently reports pending events.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn has_pending() -> bool {
    let ids: Vec<u64> = REGISTRY.with(|cell| {
        let reg = cell.borrow();
        if reg.closed || reg.entries.is_empty() {
            return Vec::new();
        }
        reg.entries.iter().map(|e| e.id).collect()
    });
    for id in ids {
        let Some(mut callbacks) = take_callbacks(id) else {
            continue;
        };
        let guard = InFlightGuard::new(id);
        let ready = (callbacks.pending)();
        guard.disarm();
        restore_or_drop(id, callbacks);
        if ready {
            return true;
        }
    }
    false
}

/// Marks the UI-thread registry closed and immediately drains and drops all registered watch
/// closures. Invoked strictly before `App::on_display_close` on every exit path so no callback
/// can touch foreign display proxies during or after teardown.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn stop_all() {
    let drained = REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        reg.closed = true;
        std::mem::take(&mut reg.entries)
    });
    drop(drained);
}

/// Re-opens the UI-thread registry at the start of `run_configured` without clearing watches
/// registered before `App::run`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn reset_closed() {
    REGISTRY.with(|cell| {
        cell.borrow_mut().closed = false;
    });
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "display_queue_tests.rs"]
mod tests;
