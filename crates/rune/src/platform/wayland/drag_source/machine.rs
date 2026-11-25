//! Explicit cross-platform state machine for a Wayland `wl_data_source` drag-out session.
//!
//! Tracks `Idle -> Active -> Dropped -> Idle`, guards `wl_data_source` v3-only requests
//! (`set_actions`, `dnd_drop_performed`, `dnd_finished`) by object version, enforces a per-drag
//! concurrent pipe-writer cap (`MAX_CONCURRENT_WRITERS`), and guarantees the `on_drag_end`
//! outcome callback fires exactly once across drop completion, cancellation, replacement,
//! window close, display loss, or shutdown.

use std::sync::Arc;

use super::mime::{DND_ACTION_COPY, DND_ACTION_MOVE, OfferedMime};
use crate::drag_out::DragOutcome;
use crate::platform::wayland::drop::machine::SurfaceId;

/// Maximum concurrent background pipe-writer threads permitted for one active drag session.
/// Excess `wl_data_source.send` file descriptors are closed immediately.
pub(crate) const MAX_CONCURRENT_WRITERS: usize = 8;

/// Monotonic identifier for one drag-source session on a display controller.
pub(crate) type DragId = u64;

/// Opaque handle indexing an incoming `wl_data_source.send` file descriptor owned by the wire
/// layer (or a fake test seam).
pub(crate) type FdId = u64;

/// Wayland `wl_data_device_manager::dnd_action` chosen by the compositor for this source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SourceAction {
    #[default]
    None,
    Copy,
    Move,
}

impl SourceAction {
    /// Decodes a raw `wl_data_source.action` bitmask into `SourceAction`.
    #[inline]
    pub(crate) const fn from_wayland_mask(mask: u32) -> Self {
        if mask & DND_ACTION_MOVE != 0 {
            Self::Move
        } else if mask & DND_ACTION_COPY != 0 {
            Self::Copy
        } else {
            Self::None
        }
    }

    #[inline]
    pub(crate) const fn to_outcome(self) -> DragOutcome {
        match self {
            Self::Copy => DragOutcome::Copied,
            Self::Move => DragOutcome::Moved,
            Self::None => DragOutcome::Cancelled,
        }
    }
}

/// Data live while a `wl_data_source` is either actively dragging (`Active`) or awaiting
/// `dnd_finished` / `cancelled` after the user released the pointer (`Dropped`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DragSession {
    pub drag_id: DragId,
    pub seat: u32,
    pub origin_surface: SurfaceId,
    pub version: u32,
    pub within_app: bool,
    pub offers: Vec<OfferedMime>,
    pub accepted_mime: Option<String>,
    pub selected_action: SourceAction,
    pub sent_any: bool,
    pub active_writers: usize,
}

/// Explicit lifecycle state of the display controller's outgoing Wayland drag source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    #[default]
    Idle,
    Active(DragSession),
    Dropped(DragSession),
}

impl State {
    /// Returns `(seat_name, within_app)` while a drag source is active or dropped, so the
    /// shared `wl_data_device` drop-in path can mute self-drops when `!within_app`.
    #[inline]
    pub(crate) fn active_seat_within_app(&self) -> Option<(u32, bool)> {
        match self {
            Self::Idle => None,
            Self::Active(s) | Self::Dropped(s) => Some((s.seat, s.within_app)),
        }
    }

    #[inline]
    pub(crate) fn active_drag_id(&self) -> Option<DragId> {
        match self {
            Self::Idle => None,
            Self::Active(s) | Self::Dropped(s) => Some(s.drag_id),
        }
    }
}

/// Inputs stepping [`State`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Start a new drag session on `seat` from `origin_surface` using `serial`.
    Start {
        drag_id: DragId,
        seat: u32,
        origin_surface: SurfaceId,
        serial: u32,
        version: u32,
        within_app: bool,
        offers: Vec<OfferedMime>,
        actions_mask: u32,
        with_icon: bool,
    },
    /// `wl_data_source.target(mime)`.
    Target { drag_id: DragId, mime: Option<String> },
    /// `wl_data_source.action(dnd_action)` (v3+).
    Action { drag_id: DragId, mask: u32 },
    /// `wl_data_source.send(mime, fd)`.
    Send { drag_id: DragId, mime: String, fd_id: FdId },
    /// Background pipe writer for `drag_id` finished.
    WriterFinished { drag_id: DragId },
    /// `wl_data_source.dnd_drop_performed` (v3+).
    DropPerformed { drag_id: DragId },
    /// `wl_data_source.dnd_finished` (v3+).
    Finished { drag_id: DragId },
    /// `wl_data_source.cancelled`.
    Cancelled { drag_id: DragId },
    /// Origin window closed or unregistered.
    WindowClosed { surface: SurfaceId },
    /// Wayland display connection failed.
    DisplayLost,
    /// Event loop exiting / shutting down.
    Shutdown,
}

/// Side effects returned by [`step`] for the wire layer to execute in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Create `wl_data_source`, call `offer` for each MIME, call `set_actions(actions_mask)`
    /// when `version >= 3`, attach optional icon, and call `wl_data_device.start_drag`.
    StartDrag {
        drag_id: DragId,
        seat: u32,
        origin_surface: SurfaceId,
        serial: u32,
        mimes: Vec<String>,
        actions_mask: Option<u32>,
        with_icon: bool,
    },
    /// Spawn a bounded non-blocking background writer for `fd_id` with `payload`.
    WritePipe { drag_id: DragId, fd_id: FdId, payload: Arc<[u8]> },
    /// Close `fd_id` immediately (unknown MIME, writer cap exceeded, or stale drag).
    CloseFd { fd_id: FdId },
    /// Cancel all in-flight pipe writers for the ending drag.
    CancelWriters { drag_id: DragId },
    /// Destroy the `wl_data_source` proxy and its drag icon surface.
    DestroySource { drag_id: DragId },
    /// Deliver the final [`DragOutcome`] to the app's `on_drag_end` sink (exactly once).
    Complete { drag_id: DragId, outcome: DragOutcome },
    /// Flush the Wayland queue.
    Flush,
}

/// Advances `state` by `event` and returns the ordered effects to execute.
pub(crate) fn step(state: &mut State, event: Event) -> Vec<Effect> {
    let mut out = Vec::new();
    match event {
        Event::Start {
            drag_id,
            seat,
            origin_surface,
            serial,
            version,
            within_app,
            offers,
            actions_mask,
            with_icon,
        } => {
            // Replace any prior active/dropped session cleanly once.
            if let Some(prev) = take_session(state) {
                push_teardown(&mut out, prev.drag_id, DragOutcome::Cancelled, true);
            }
            if offers.is_empty() {
                out.push(Effect::Complete { drag_id, outcome: DragOutcome::Cancelled });
                return out;
            }
            let mimes: Vec<String> = offers.iter().map(|o| o.mime.clone()).collect();
            let actions_mask = (version >= 3).then_some(actions_mask);
            *state = State::Active(DragSession {
                drag_id,
                seat,
                origin_surface,
                version,
                within_app,
                offers,
                accepted_mime: None,
                selected_action: SourceAction::None,
                sent_any: false,
                active_writers: 0,
            });
            out.push(Effect::StartDrag {
                drag_id,
                seat,
                origin_surface,
                serial,
                mimes,
                actions_mask,
                with_icon,
            });
            out.push(Effect::Flush);
        }
        Event::Target { drag_id, mime } => {
            if let Some(s) = session_mut(state, drag_id) {
                s.accepted_mime = mime;
            }
        }
        Event::Action { drag_id, mask } => {
            if let Some(s) = session_mut(state, drag_id)
                && s.version >= 3
            {
                s.selected_action = SourceAction::from_wayland_mask(mask);
            }
        }
        Event::Send { drag_id, mime, fd_id } => {
            let Some(s) = session_mut(state, drag_id) else {
                out.push(Effect::CloseFd { fd_id });
                return out;
            };
            if s.active_writers >= MAX_CONCURRENT_WRITERS {
                out.push(Effect::CloseFd { fd_id });
                return out;
            }
            let Some(payload) = s.offers.iter().find(|o| o.mime == mime).map(|o| o.bytes.clone()) else {
                out.push(Effect::CloseFd { fd_id });
                return out;
            };
            s.sent_any = true;
            s.active_writers += 1;
            out.push(Effect::WritePipe { drag_id, fd_id, payload });
        }
        Event::WriterFinished { drag_id } => {
            if let Some(s) = session_mut(state, drag_id) {
                s.active_writers = s.active_writers.saturating_sub(1);
            }
        }
        Event::DropPerformed { drag_id } => {
            if matches!(state, State::Active(s) if s.drag_id == drag_id && s.version >= 3) {
                if let State::Active(s) = std::mem::take(state) {
                    *state = State::Dropped(s);
                }
            }
        }
        Event::Finished { drag_id } => {
            if state.active_drag_id() == Some(drag_id) {
                if let Some(s) = take_session(state) {
                    let outcome = if s.version >= 3 {
                        s.selected_action.to_outcome()
                    } else {
                        DragOutcome::Copied
                    };
                    push_teardown(&mut out, s.drag_id, outcome, true);
                }
            }
        }
        Event::Cancelled { drag_id } => {
            if state.active_drag_id() == Some(drag_id) {
                if let Some(s) = take_session(state) {
                    let outcome = if s.version < 3 && s.sent_any {
                        DragOutcome::Copied
                    } else {
                        DragOutcome::Cancelled
                    };
                    push_teardown(&mut out, s.drag_id, outcome, true);
                }
            }
        }
        Event::WindowClosed { surface } => {
            let matches_origin = match state {
                State::Active(s) | State::Dropped(s) => s.origin_surface == surface,
                State::Idle => false,
            };
            if matches_origin && let Some(s) = take_session(state) {
                push_teardown(&mut out, s.drag_id, DragOutcome::Cancelled, true);
            }
        }
        Event::DisplayLost => {
            if let Some(s) = take_session(state) {
                push_teardown(&mut out, s.drag_id, DragOutcome::Cancelled, false);
            }
        }
        Event::Shutdown => {
            if let Some(s) = take_session(state) {
                push_teardown(&mut out, s.drag_id, DragOutcome::Cancelled, true);
            }
        }
    }
    out
}

fn session_mut(state: &mut State, drag_id: DragId) -> Option<&mut DragSession> {
    match state {
        State::Active(s) | State::Dropped(s) if s.drag_id == drag_id => Some(s),
        _ => None,
    }
}

fn take_session(state: &mut State) -> Option<DragSession> {
    match std::mem::take(state) {
        State::Active(s) | State::Dropped(s) => Some(s),
        State::Idle => None,
    }
}

fn push_teardown(out: &mut Vec<Effect>, drag_id: DragId, outcome: DragOutcome, flush: bool) {
    out.push(Effect::CancelWriters { drag_id });
    out.push(Effect::DestroySource { drag_id });
    out.push(Effect::Complete { drag_id, outcome });
    if flush {
        out.push(Effect::Flush);
    }
}
