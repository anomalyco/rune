//! Which errors on a layer surface's Wayland connection mean the display is gone, and the
//! words a dead connection is reported with.

use wayland_backend::client::{Backend, WaylandError};
use wayland_client::DispatchError;
use winit::raw_window_handle::RawDisplayHandle;

use super::connection;

fn wayland_error_kind(err: &WaylandError) -> connection::ErrorKind {
    match err {
        WaylandError::Protocol(_) => connection::ErrorKind::Protocol,
        WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock => connection::ErrorKind::WouldBlock,
        WaylandError::Io(_) => connection::ErrorKind::Io,
    }
}

pub(super) fn format_wayland_error(err: &WaylandError) -> Option<String> {
    if !connection::is_fatal(wayland_error_kind(err)) { return None; }
    Some(match err {
        WaylandError::Protocol(protocol) => protocol.to_string(),
        WaylandError::Io(io) => io.to_string(),
    })
}

pub(super) fn classify_turn_error(
    dispatch: Option<DispatchError>,
    flush: Option<WaylandError>,
    last: Option<WaylandError>,
) -> (Option<connection::ErrorKind>, Option<String>) {
    if let Some(err) = last {
        let kind = wayland_error_kind(&err);
        if connection::is_fatal(kind) { return (Some(kind), format_wayland_error(&err)); }
    }
    if let Some(err) = dispatch {
        return match err {
            DispatchError::Backend(err) => (Some(wayland_error_kind(&err)), format_wayland_error(&err)),
            other => (Some(connection::ErrorKind::Protocol), Some(other.to_string())),
        };
    }
    if let Some(err) = flush {
        return (Some(wayland_error_kind(&err)), format_wayland_error(&err));
    }
    (None, None)
}

/// Returns the fatal protocol or socket error on a Wayland display connection, if it has died.
#[allow(unsafe_code)]
pub(crate) fn display_error(display: RawDisplayHandle) -> Option<String> {
    let RawDisplayHandle::Wayland(display) = display else { return None };
    // SAFETY: winit owns this `wl_display` while `ActiveEventLoop` is alive; a foreign backend
    // creates only a temporary empty event queue and never closes the display.
    let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
    if let Err(err) = backend.dispatch_inner_queue() { return format_wayland_error(&err); }
    if let Err(err) = backend.flush() { return format_wayland_error(&err); }
    backend.last_error().and_then(|err| format_wayland_error(&err))
}
