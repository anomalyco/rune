//! Wayland wire driver for `wl_data_source` sharing `platform::wayland::drop`'s per-display
//! `wl_data_device` per `wl_seat`.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::{Arc, mpsc};

use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device::WlDataDevice;
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

use super::icon::PreparedIcon;
use super::icon::shm::{DragIconSurface, build_icon_surface};
use super::machine::{self, DragId, Effect, Event, FdId};
use super::mime::OfferedMime;
use crate::platform::wayland::pipe_write::{MAX_PIPE_WRITE_BYTES, PIPE_WRITE_TIMEOUT};
use crate::platform::wayland::pipe_write::unix::{WriteCancel, write_fd_bounded};
use crate::drag_out::DragOutcome;
use crate::platform::wayland::drop::machine::SurfaceId;
use crate::platform::wayland::pipe::unix::{DoneFlag, SignalOnDrop, THREAD_JOIN_BOUND, join_bounded};
use crate::platform::wayland::drop::watch::WakePool;

pub(in crate::platform::wayland) type Done = Box<dyn FnOnce(DragOutcome)>;

fn report(done: Done, outcome: DragOutcome) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(outcome))).is_err() {
        eprintln!("rune: drag end callback panicked");
    }
}

struct ActiveWriter {
    cancel: Arc<WriteCancel>,
    done: DoneFlag,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ActiveWriter {
    fn join(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = join_bounded(thread, &self.done, THREAD_JOIN_BOUND);
        }
    }
}

struct LiveSource {
    source: WlDataSource,
    icon: Option<DragIconSurface>,
    writers: Vec<ActiveWriter>,
}

/// Drag-source wire state embedded in the per-display `drop::wire::QueueState`.
pub(in crate::platform::wayland) struct DragSourceWire {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    state: machine::State,
    next_drag_id: DragId,
    next_fd_id: FdId,
    pending_fds: HashMap<FdId, OwnedFd>,
    pending_done: HashMap<DragId, Done>,
    sources: HashMap<DragId, LiveSource>,
    writer_done_tx: mpsc::Sender<DragId>,
    writer_done_rx: mpsc::Receiver<DragId>,
}

impl DragSourceWire {
    pub(in crate::platform::wayland) fn new() -> Self {
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        Self {
            compositor: None,
            shm: None,
            state: machine::State::Idle,
            next_drag_id: 1,
            next_fd_id: 1,
            pending_fds: HashMap::new(),
            pending_done: HashMap::new(),
            sources: HashMap::new(),
            writer_done_tx,
            writer_done_rx,
        }
    }

    /// True while our own outgoing drag on `seat` forbids dropping onto this app's windows.
    #[inline]
    pub(in crate::platform::wayland) fn mutes_drop_in(&self, seat: u32) -> bool {
        self.state.active_seat_within_app() == Some((seat, false))
    }

    /// True while a drag source is active or awaiting `dnd_finished` on this display.
    #[inline]
    pub(in crate::platform::wayland) fn is_active(&self) -> bool {
        self.state.active_drag_id().is_some()
    }

    /// Starts a new Wayland `wl_data_source` drag session on `device`.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::platform::wayland) fn start_drag<D>(
        &mut self,
        manager: &WlDataDeviceManager,
        globals: Option<&GlobalList>,
        device: &WlDataDevice,
        origin_wl_surface: &WlSurface,
        origin_surface: SurfaceId,
        seat: u32,
        serial: u32,
        within_app: bool,
        offers: Vec<OfferedMime>,
        actions_mask: u32,
        mut icon: Option<PreparedIcon>,
        done: Done,
        qh: &QueueHandle<D>,
        wakers: &WakePool,
        needs_flush: &mut bool,
    ) where
        D: Dispatch<WlDataSource, DragId>
            + Dispatch<WlCompositor, ()>
            + Dispatch<WlShm, ()>
            + Dispatch<WlShmPool, ()>
            + Dispatch<WlBuffer, ()>
            + Dispatch<WlSurface, ()>
            + 'static,
    {
        let drag_id = self.next_drag_id;
        self.next_drag_id = self.next_drag_id.wrapping_add(1).max(1);
        self.pending_done.insert(drag_id, done);
        let version = manager.version();
        let with_icon = icon.is_some();
        let effects = machine::step(&mut self.state, Event::Start {
            drag_id,
            seat,
            origin_surface,
            serial,
            version,
            within_app,
            offers,
            actions_mask,
            with_icon,
        });
        for effect in effects {
            match effect {
                Effect::StartDrag {
                    drag_id,
                    serial,
                    mimes,
                    actions_mask,
                    with_icon,
                    ..
                } => {
                    let source = manager.create_data_source(qh, drag_id);
                    for mime in mimes {
                        source.offer(mime);
                    }
                    if let Some(mask) = actions_mask {
                        source.set_actions(DndAction::from_bits_truncate(mask));
                    }
                    let icon_surface = if with_icon && let Some(prep) = icon.take() {
                        self.build_icon(globals, qh, &prep)
                    } else {
                        None
                    };
                    device.start_drag(
                        Some(&source),
                        origin_wl_surface,
                        icon_surface.as_ref().map(|i| &i.surface),
                        serial,
                    );
                    self.sources.insert(drag_id, LiveSource {
                        source,
                        icon: icon_surface,
                        writers: Vec::new(),
                    });
                }
                other => self.apply_effect(other, wakers, needs_flush),
            }
        }
    }

    fn build_icon<D>(
        &mut self,
        globals: Option<&GlobalList>,
        qh: &QueueHandle<D>,
        icon: &PreparedIcon,
    ) -> Option<DragIconSurface>
    where
        D: Dispatch<WlCompositor, ()>
            + Dispatch<WlShm, ()>
            + Dispatch<WlShmPool, ()>
            + Dispatch<WlBuffer, ()>
            + Dispatch<WlSurface, ()>
            + 'static,
    {
        let globals = globals?;
        if self.compositor.is_none() {
            self.compositor = globals.bind::<WlCompositor, _, _>(qh, 1..=6, ()).ok();
        }
        if self.shm.is_none() {
            self.shm = globals.bind::<WlShm, _, _>(qh, 1..=1, ()).ok();
        }
        let (Some(comp), Some(shm)) = (&self.compositor, &self.shm) else {
            return None;
        };
        build_icon_surface(comp, shm, qh, icon)
    }

    /// Handles one `wl_data_source` event dispatched on the shared Wayland event queue.
    pub(in crate::platform::wayland) fn on_source_event(
        &mut self,
        drag_id: DragId,
        event: wl_data_source::Event,
        wakers: &WakePool,
        needs_flush: &mut bool,
    ) {
        let ev = match event {
            wl_data_source::Event::Target { mime_type } => Event::Target { drag_id, mime: mime_type },
            wl_data_source::Event::Action { dnd_action } => {
                let mask = match dnd_action {
                    WEnum::Value(a) => a.bits(),
                    WEnum::Unknown(v) => v,
                };
                Event::Action { drag_id, mask }
            }
            wl_data_source::Event::Send { mime_type, fd } => {
                let fd_id = self.next_fd_id;
                self.next_fd_id = self.next_fd_id.wrapping_add(1).max(1);
                self.pending_fds.insert(fd_id, fd);
                Event::Send { drag_id, mime: mime_type, fd_id }
            }
            wl_data_source::Event::DndDropPerformed => Event::DropPerformed { drag_id },
            wl_data_source::Event::DndFinished => Event::Finished { drag_id },
            wl_data_source::Event::Cancelled => Event::Cancelled { drag_id },
            _ => return,
        };
        let effects = machine::step(&mut self.state, ev);
        for effect in effects {
            self.apply_effect(effect, wakers, needs_flush);
        }
    }

    /// Reaps finished background pipe-writer threads and updates writer slot accounting.
    pub(in crate::platform::wayland) fn pump_writers(&mut self, wakers: &WakePool, needs_flush: &mut bool) {
        for _ in 0..64 {
            let Ok(drag_id) = self.writer_done_rx.try_recv() else {
                break;
            };
            if let Some(live) = self.sources.get_mut(&drag_id) {
                let mut i = 0;
                while i < live.writers.len() {
                    if live.writers[i].done.is_done() {
                        let w = live.writers.swap_remove(i);
                        w.join();
                    } else {
                        i += 1;
                    }
                }
            }
            let effects = machine::step(&mut self.state, Event::WriterFinished { drag_id });
            for effect in effects {
                self.apply_effect(effect, wakers, needs_flush);
            }
        }
    }

    pub(in crate::platform::wayland) fn on_window_closed(
        &mut self,
        surface: SurfaceId,
        wakers: &WakePool,
        needs_flush: &mut bool,
    ) {
        let effects = machine::step(&mut self.state, Event::WindowClosed { surface });
        for effect in effects {
            self.apply_effect(effect, wakers, needs_flush);
        }
    }

    pub(in crate::platform::wayland) fn on_display_lost(&mut self, wakers: &WakePool, needs_flush: &mut bool) {
        let effects = machine::step(&mut self.state, Event::DisplayLost);
        for effect in effects {
            self.apply_effect(effect, wakers, needs_flush);
        }
    }

    pub(in crate::platform::wayland) fn on_shutdown(
        &mut self,
        conn: &Connection,
        wakers: &WakePool,
        needs_flush: &mut bool,
    ) {
        let effects = machine::step(&mut self.state, Event::Shutdown);
        for effect in effects {
            self.apply_effect(effect, wakers, needs_flush);
        }
        // Drain any remaining sources or pending fds/callbacks defensively.
        self.pending_fds.clear();
        for (_, mut live) in self.sources.drain() {
            for w in &live.writers {
                w.cancel.cancel();
            }
            for w in live.writers.drain(..) {
                w.join();
            }
            drop(live.icon);
            live.source.destroy();
        }
        for (_, done) in self.pending_done.drain() {
            report(done, DragOutcome::Cancelled);
        }
        let backend = conn.backend();
        if let Some(shm) = self.shm.take() {
            backend.destroy_object(&shm.id()).ok();
        }
        if let Some(comp) = self.compositor.take() {
            backend.destroy_object(&comp.id()).ok();
        }
    }

    fn apply_effect(&mut self, effect: Effect, wakers: &WakePool, needs_flush: &mut bool) {
        match effect {
            Effect::StartDrag { .. } => {}
            Effect::WritePipe { drag_id, fd_id, payload } => {
                let Some(fd) = self.pending_fds.remove(&fd_id) else { return };
                let Some(cancel) = WriteCancel::new() else {
                    drop(fd);
                    let _ = self.writer_done_tx.send(drag_id);
                    wakers.wake();
                    return;
                };
                let cancel = Arc::new(cancel);
                let worker_cancel = cancel.clone();
                let (done, signal_done) = DoneFlag::new();
                let done_tx = self.writer_done_tx.clone();
                let wake_pool = wakers.clone();
                match std::thread::Builder::new()
                    .name("rune-wayland-drag-write".into())
                    .spawn(move || {
                        let _signal = SignalOnDrop(signal_done);
                        let _ = write_fd_bounded(
                            fd,
                            &payload,
                            &worker_cancel,
                            MAX_PIPE_WRITE_BYTES,
                            PIPE_WRITE_TIMEOUT,
                        );
                        let _ = done_tx.send(drag_id);
                        wake_pool.wake();
                    })
                {
                    Ok(thread) => {
                        if let Some(live) = self.sources.get_mut(&drag_id) {
                            live.writers.push(ActiveWriter { cancel, done, thread: Some(thread) });
                        } else {
                            cancel.cancel();
                            let _ = join_bounded(thread, &done, THREAD_JOIN_BOUND);
                        }
                    }
                    Err(_) => {
                        let _ = self.writer_done_tx.send(drag_id);
                        wakers.wake();
                    }
                }
            }
            Effect::CloseFd { fd_id } => {
                let _ = self.pending_fds.remove(&fd_id);
            }
            Effect::CancelWriters { drag_id } => {
                if let Some(live) = self.sources.get_mut(&drag_id) {
                    for w in &live.writers {
                        w.cancel.cancel();
                    }
                    for w in live.writers.drain(..) {
                        w.join();
                    }
                }
            }
            Effect::DestroySource { drag_id } => {
                if let Some(mut live) = self.sources.remove(&drag_id) {
                    for w in &live.writers {
                        w.cancel.cancel();
                    }
                    for w in live.writers.drain(..) {
                        w.join();
                    }
                    drop(live.icon);
                    live.source.destroy();
                }
            }
            Effect::Complete { drag_id, outcome } => {
                if let Some(done) = self.pending_done.remove(&drag_id) {
                    report(done, outcome);
                }
            }
            Effect::Flush => {
                *needs_flush = true;
            }
        }
    }
}
