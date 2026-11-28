//! Graceful Wayland teardown before process exit.
//!
//! Dropping a Rune window or overlay queues `xdg_toplevel.destroy`, `zwlr_layer_surface_v1.destroy`,
//! and `wl_surface.destroy` into libwayland's outgoing buffer, and winit holds its own
//! `WindowState` clones until closed windows are reaped. Ending the process with `_exit` (or
//! closing the display socket without a roundtrip) leaves those surfaces focused on the compositor
//! when the socket hits EOF, which triggers a `wayland-backend` panic on Smithay-based compositors.
//! Performing a bounded `wl_display.sync` roundtrip after releasing surfaces and reaping winit's
//! closed windows guarantees the compositor processes the destroys and sends `wl_keyboard.leave`
//! while the client connection is still alive.

use std::time::Duration;

/// Maximum time teardown waits for the compositor to acknowledge `wl_display.sync`.
/// Healthy local compositors reply in under a millisecond; the bound prevents a frozen compositor
/// from stalling process exit.
pub(crate) const SYNC_TIMEOUT: Duration = Duration::from_millis(250);

/// Result of a bounded Wayland teardown sync roundtrip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
pub(crate) enum SyncOutcome {
    /// The compositor answered `wl_callback.done`; every preceding destroy request was processed.
    Answered,
    /// The display handle is not a Wayland display (for example X11).
    NotWayland,
    /// The display connection had already failed or returned an I/O or protocol error.
    Failed,
    /// The compositor did not answer before the timeout expired.
    TimedOut,
}

/// Tears down every global Wayland helper Rune installed on the shared display connection.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) fn shutdown_globals() {
    crate::platform::wayland::primary_selection::wire::shutdown();
    crate::platform::wayland::drop::shutdown();
    crate::platform::wayland::backdrop::shutdown();
    crate::platform::wayland::presentation::shutdown();
    crate::platform::wayland::serial::wire::shutdown();
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod wire {
    use std::io::ErrorKind;
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    use winit::raw_window_handle::RawDisplayHandle;
    use wayland_backend::client::{Backend, WaylandError};
    use wayland_client::protocol::wl_callback::{self, WlCallback};
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

    use super::SyncOutcome;

    #[derive(Default)]
    struct SyncState {
        answered: bool,
    }

    impl Dispatch<WlCallback, ()> for SyncState {
        fn event(
            state: &mut Self,
            _: &WlCallback,
            event: wl_callback::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if matches!(event, wl_callback::Event::Done { .. }) {
                state.answered = true;
            }
        }
    }

    fn cleanup_unanswered(
        conn: &Connection,
        queue: &mut EventQueue<SyncState>,
        state: &mut SyncState,
        callback: WlCallback,
    ) {
        while matches!(queue.dispatch_pending(state), Ok(n) if n > 0) {}
        if !state.answered {
            conn.backend().destroy_object(&callback.id()).ok();
        }
        while matches!(queue.dispatch_pending(state), Ok(n) if n > 0) {}
        let _ = queue.flush();
    }

    /// Sends `wl_display.sync` on winit's `wl_display`, flushes all queued destroy requests, and
    /// waits up to `timeout` for `wl_callback.done`.
    #[allow(unsafe_code)]
    pub(crate) fn sync_display(display: RawDisplayHandle, timeout: Duration) -> SyncOutcome {
        let RawDisplayHandle::Wayland(handle) = display else {
            return SyncOutcome::NotWayland;
        };
        // SAFETY: winit owns this `wl_display` while `ActiveEventLoop` is alive; a foreign
        // backend uses only a private event queue and never closes the display.
        let backend = unsafe { Backend::from_foreign_display(handle.display.as_ptr().cast()) };
        sync_connection(&Connection::from_backend(backend), timeout)
    }

    /// Bounded `wl_display.sync` roundtrip on `conn` using a private event queue so winit's main
    /// queue handlers are never re-entered during exit.
    #[allow(unsafe_code)]
    pub(crate) fn sync_connection(conn: &Connection, timeout: Duration) -> SyncOutcome {
        if conn.backend().last_error().is_some() {
            return SyncOutcome::Failed;
        }
        let mut queue = conn.new_event_queue::<SyncState>();
        let callback = conn.display().sync(&queue.handle(), ());
        let mut state = SyncState::default();
        let deadline = Instant::now() + timeout;
        let mut want_write = false;

        let outcome = loop {
            match queue.dispatch_pending(&mut state) {
                Ok(_) if state.answered => break SyncOutcome::Answered,
                Ok(_) => {}
                Err(_) => break SyncOutcome::Failed,
            }

            match queue.flush() {
                Ok(()) => want_write = false,
                Err(WaylandError::Io(err)) if err.kind() == ErrorKind::WouldBlock => {
                    want_write = true;
                }
                Err(_) => break SyncOutcome::Failed,
            }

            if conn.backend().last_error().is_some() {
                break SyncOutcome::Failed;
            }

            let Some(guard) = queue.prepare_read() else {
                continue;
            };

            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                drop(guard);
                break SyncOutcome::TimedOut;
            };
            let wait_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
            let events = libc::POLLIN | if want_write { libc::POLLOUT } else { 0 };
            let mut pfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events,
                revents: 0,
            };
            // SAFETY: `pfd.fd` is the live display socket borrowed by `guard`.
            let ret = unsafe { libc::poll(&mut pfd, 1, wait_ms) };
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                drop(guard);
                if err.kind() == ErrorKind::Interrupted {
                    continue;
                }
                break SyncOutcome::Failed;
            }
            if ret == 0 {
                drop(guard);
                break SyncOutcome::TimedOut;
            }
            if pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(err))
                        if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                    Err(_) => break SyncOutcome::Failed,
                }
            } else {
                drop(guard);
            }
        };

        if outcome != SyncOutcome::Answered {
            cleanup_unanswered(conn, &mut queue, &mut state, callback);
        }
        outcome
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use wire::sync_display;
#[cfg(all(test, unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use wire::sync_connection;

#[cfg(test)]
#[path = "teardown_tests.rs"]
mod tests;
