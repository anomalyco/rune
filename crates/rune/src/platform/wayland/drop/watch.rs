//! Shared per-display Wayland watch thread, wake pool, and display/surface handle helpers
//! shared by `platform::wayland::drop` and `platform::wayland::drag_source`.

use std::os::fd::AsRawFd;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{ObjectId, ReadEventsGuard, WaylandError};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy};

use super::machine::{SurfaceId, WindowToken};
use super::pipe::{DoneFlag, SignalOnDrop, StopSignal, THREAD_JOIN_BOUND, join_bounded};
use crate::platform::layer_shell_machine::watch_loop;
use crate::runtime::Waker;

/// Extracts `(wl_display_ptr, surface_protocol_id, surface_object_id)` for a Wayland window.
#[allow(unsafe_code)]
pub(in crate::platform::wayland) fn wayland_surface_id(
    window: &winit::window::Window,
) -> Option<(usize, SurfaceId, ObjectId)> {
    let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) = (
        window.display_handle().ok()?.as_raw(),
        window.window_handle().ok()?.as_raw(),
    ) else {
        return None;
    };
    // SAFETY: `handle.surface` is the live `wl_surface` proxy winit owns for `window`.
    let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }.ok()?;
    Some((display.display.as_ptr() as usize, id.protocol_id(), id))
}

/// Extracts `(wl_display_ptr, surface_protocol_id)` for a Wayland window.
pub(in crate::platform::wayland) fn wayland_ids(
    window: &winit::window::Window,
) -> Option<(usize, SurfaceId)> {
    let (display, surface, _) = wayland_surface_id(window)?;
    Some((display, surface))
}

/// Destroys a `GlobalList`'s `wl_registry` proxy on `conn` cleanly.
pub(in crate::platform::wayland) fn destroy_globals(conn: &Connection, globals: GlobalList) {
    let registry_id = globals.registry().id();
    globals.destroy();
    conn.backend().destroy_object(&registry_id).ok();
}

/// Shared wake handle pointing to all currently registered windows on one display; calling `wake`
/// wakes the shared winit event loop once.
#[derive(Clone, Default)]
pub(in crate::platform::wayland) struct WakePool {
    wakers: Arc<Mutex<Vec<(WindowToken, Waker)>>>,
}

impl WakePool {
    pub(in crate::platform::wayland) fn add(&self, token: WindowToken, wake: Waker) {
        self.wakers.lock().unwrap_or_else(|e| e.into_inner()).push((token, wake));
    }

    pub(in crate::platform::wayland) fn remove(&self, token: WindowToken) {
        self.wakers.lock().unwrap_or_else(|e| e.into_inner()).retain(|(t, _)| *t != token);
    }

    pub(in crate::platform::wayland) fn wake(&self) {
        let waker = self.wakers.lock().unwrap_or_else(|e| e.into_inner()).first().map(|(_, w)| w.clone());
        if let Some(waker) = waker {
            waker();
        }
    }
}

struct WatchTurns {
    count: u64,
    stopped: bool,
}

/// Zero-idle-polling background reader watching one `wl_display` queue via `watch_loop`.
pub(in crate::platform::wayland) struct WaylandWatch {
    stop: Arc<StopSignal>,
    turns: Arc<(Mutex<WatchTurns>, Condvar)>,
    done: DoneFlag,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct WaylandWatchDriver {
    conn: Connection,
    stop: Arc<StopSignal>,
    turns: Arc<(Mutex<WatchTurns>, Condvar)>,
    wake: WakePool,
}

fn map_watch_dispatch(result: Result<usize, WaylandError>) -> watch_loop::Dispatch {
    match result {
        Ok(n) => watch_loop::Dispatch::Events(n),
        Err(WaylandError::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {
            watch_loop::Dispatch::WouldBlock
        }
        Err(_) => watch_loop::Dispatch::Error,
    }
}

impl watch_loop::Driver for WaylandWatchDriver {
    type Guard = ReadEventsGuard;

    fn stopped(&self) -> bool {
        self.turns.0.lock().unwrap_or_else(|e| e.into_inner()).stopped
    }

    fn prepare_read(&mut self) -> Option<ReadEventsGuard> {
        self.conn.prepare_read()
    }

    fn dispatch_inner_queue(&mut self) -> watch_loop::Dispatch {
        map_watch_dispatch(self.conn.backend().dispatch_inner_queue())
    }

    #[allow(unsafe_code)]
    fn poll(&mut self, guard: &ReadEventsGuard) -> watch_loop::Poll {
        let mut pfds = [
            libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.stop.poll_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: two open descriptors: the live display socket borrowed by `guard` and `stop`.
        let ret = unsafe { libc::poll(pfds.as_mut_ptr(), 2, -1) };
        if ret < 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                watch_loop::Poll::Interrupted
            } else {
                watch_loop::Poll::Error
            };
        }
        if ret == 0 {
            return watch_loop::Poll::Interrupted;
        }
        if pfds[1].revents != 0 {
            return watch_loop::Poll::Stop;
        }
        if pfds[0].revents & libc::POLLNVAL != 0 {
            return watch_loop::Poll::Error;
        }
        if pfds[0].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0 {
            return watch_loop::Poll::Readable;
        }
        watch_loop::Poll::Error
    }

    fn read(&mut self, guard: ReadEventsGuard) -> watch_loop::Dispatch {
        map_watch_dispatch(guard.read())
    }

    fn wake_and_wait(&mut self) {
        let (lock, cond) = &*self.turns;
        let before = {
            let state = lock.lock().unwrap_or_else(|e| e.into_inner());
            if state.stopped {
                return;
            }
            state.count
        };
        self.wake.wake();
        let state = lock.lock().unwrap_or_else(|e| e.into_inner());
        cond.wait_timeout_while(state, Duration::from_millis(100), |s| s.count == before && !s.stopped).ok();
    }
}

impl WaylandWatch {
    pub(in crate::platform::wayland) fn spawn(conn: Connection, wake: WakePool) -> Option<Self> {
        let stop = Arc::new(StopSignal::new()?);
        let turns = Arc::new((Mutex::new(WatchTurns { count: 0, stopped: false }), Condvar::new()));
        let (done, signal_done) = DoneFlag::new();
        let mut driver = WaylandWatchDriver { conn, stop: stop.clone(), turns: turns.clone(), wake };
        let thread = std::thread::Builder::new()
            .name("rune-wayland-drop".into())
            .spawn(move || {
                let _signal = SignalOnDrop(signal_done);
                watch_loop::run(&mut driver);
            })
            .ok()?;
        Some(Self { stop, turns, done, thread: Some(thread) })
    }

    pub(in crate::platform::wayland) fn turned(&self) {
        let (lock, cond) = &*self.turns;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        state.count = state.count.wrapping_add(1);
        cond.notify_all();
    }
}

impl Drop for WaylandWatch {
    fn drop(&mut self) {
        {
            let (lock, cond) = &*self.turns;
            lock.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
            cond.notify_all();
        }
        self.stop.signal();
        if let Some(thread) = self.thread.take() {
            let _ = join_bounded(thread, &self.done, THREAD_JOIN_BOUND);
        }
    }
}
