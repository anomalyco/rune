//! Wayland wire backend for `zwp_primary_selection_unstable_v1` on winit's shared `wl_display`.
//!
//! Lazily initialized once per `wl_display` strictly after frame 1, with zero background
//! `WaylandWatch` threads: incoming `send` and `cancelled` events wake winit through
//! `window::wayland_wake` strictly while our own `zwp_primary_selection_source_v1` is owned.
//! Reuses the shared unwatched serial tracker (`platform::wayland::serial`), bounded pipe
//! reader (`platform::wayland::pipe`), and non-blocking pipe writer (`platform::wayland::pipe_write`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::{Arc, mpsc};

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_backend::client::Backend;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1;
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::{
    self, ZwpPrimarySelectionDeviceV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::{
    self, ZwpPrimarySelectionOfferV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::{
    self, ZwpPrimarySelectionSourceV1,
};

use super::machine::{
    self, Availability, Effect, Event, MAX_PRIMARY_BYTES, OFFERED_MIMES, OfferId,
    PRIMARY_READ_TIMEOUT, SeatId, SourceId, State, WriteFdId,
};
use crate::Waker;
use crate::machine::Attempt;
use crate::platform::wayland::drop::watch::destroy_globals;
use crate::platform::wayland::pipe::ReadOutcome;
use crate::platform::wayland::pipe::unix::{
    CancelToken, DoneFlag, SignalOnDrop, THREAD_JOIN_BOUND, join_bounded, make_pipe, read_fd_bounded,
};
use crate::platform::wayland::pipe_write::PIPE_WRITE_TIMEOUT;
use crate::platform::wayland::pipe_write::unix::{WriteCancel, write_fd_bounded};
use crate::platform::wayland::serial::wire::{SerialGuard, attach as attach_serial, latest_input};

thread_local! {
    /// One per-`wl_display` primary-selection slot on the UI thread, torn down before `wl_display` closes.
    static CONTROLLERS: RefCell<HashMap<usize, DisplaySlot>> = RefCell::new(HashMap::new());
}

/// Result of attempting a primary-selection write through `zwp_primary_selection_v1`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteDispatch { Handled, Fallback }

/// Result of attempting a primary-selection read through `zwp_primary_selection_v1`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) enum ReadDispatch { Handled, Fallback(String) }

/// UI-thread delivery callback: `(target_field_path, text, wake_needed)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) type DeliverFn = Box<dyn FnOnce(String, String, bool)>;

enum DisplaySlot {
    Unavailable,
    Live(Box<Controller>),
}

struct ActiveWriter {
    cancel: Arc<WriteCancel>,
    done: DoneFlag,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ActiveWriter {
    fn cancel_and_join(mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = join_bounded(thread, &self.done, THREAD_JOIN_BOUND);
        }
    }
}

struct ActiveReader {
    attempt: Attempt,
    cancel: Arc<CancelToken>,
    done: DoneFlag,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ActiveReader {
    fn cancel_and_join(mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = join_bounded(thread, &self.done, THREAD_JOIN_BOUND);
        }
    }
}

struct QueueState {
    machine: State,
    manager: Option<ZwpPrimarySelectionDeviceManagerV1>,
    manager_global_name: Option<u32>,
    seats: HashMap<SeatId, WlSeat>,
    devices: HashMap<SeatId, ZwpPrimarySelectionDeviceV1>,
    offers: HashMap<OfferId, ZwpPrimarySelectionOfferV1>,
    sources: HashMap<SourceId, ZwpPrimarySelectionSourceV1>,
    next_fd_id: WriteFdId,
    pending_fds: HashMap<WriteFdId, OwnedFd>,
    writers: Vec<ActiveWriter>,
    writer_done_tx: mpsc::Sender<()>,
    writer_done_rx: mpsc::Receiver<()>,
    reader: Option<ActiveReader>,
    cancelled_readers: Vec<ActiveReader>,
    read_done_tx: mpsc::Sender<(Attempt, ReadOutcome)>,
    read_done_rx: mpsc::Receiver<(Attempt, ReadOutcome)>,
    pending_deliveries: HashMap<Attempt, DeliverFn>,
    selection_lost: bool,
    needs_flush: bool,
    tearing_down: bool,
}

impl QueueState {
    fn new(manager: ZwpPrimarySelectionDeviceManagerV1, manager_global_name: u32) -> Self {
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        let (read_done_tx, read_done_rx) = mpsc::channel();
        Self {
            machine: State::new(Availability::Available),
            manager: Some(manager),
            manager_global_name: Some(manager_global_name),
            seats: HashMap::new(),
            devices: HashMap::new(),
            offers: HashMap::new(),
            sources: HashMap::new(),
            next_fd_id: 1,
            pending_fds: HashMap::new(),
            writers: Vec::new(),
            writer_done_tx,
            writer_done_rx,
            reader: None,
            cancelled_readers: Vec::new(),
            read_done_tx,
            read_done_rx,
            pending_deliveries: HashMap::new(),
            selection_lost: false,
            needs_flush: false,
            tearing_down: false,
        }
    }

    fn bind_seat(&mut self, registry: &WlRegistry, qh: &QueueHandle<Self>, name: u32, version: u32) {
        if self.tearing_down || self.seats.contains_key(&name) { return; }
        let seat = registry.bind::<WlSeat, _, _>(name, version.min(5), qh, name);
        self.seats.insert(name, seat);
        let effects = machine::step(&mut self.machine, Event::SeatAdded { seat: name });
        self.apply_effects(effects, qh, None, None, true);
        self.needs_flush = true;
    }

    fn remove_seat(&mut self, name: u32, conn: &Connection, qh: &QueueHandle<Self>) {
        let effects = machine::step(&mut self.machine, Event::SeatRemoved { seat: name });
        self.apply_effects(effects, qh, None, None, true);
        if let Some(seat) = self.seats.remove(&name) {
            if seat.version() >= 5 { seat.release(); } else { conn.backend().destroy_object(&seat.id()).ok(); }
            self.needs_flush = true;
        }
    }

    fn reap_finished_workers(&mut self) {
        while self.writer_done_rx.try_recv().is_ok() {
            let _ = machine::step(&mut self.machine, Event::WriterFinished);
        }
        let mut i = 0;
        while i < self.writers.len() {
            if self.writers[i].done.is_done() {
                if let Some(thread) = self.writers.swap_remove(i).thread { thread.join().ok(); }
            } else { i += 1; }
        }
        let mut j = 0;
        while j < self.cancelled_readers.len() {
            if self.cancelled_readers[j].done.is_done() {
                if let Some(thread) = self.cancelled_readers.swap_remove(j).thread { thread.join().ok(); }
            } else { j += 1; }
        }
        if self.reader.as_ref().is_some_and(|r| r.done.is_done())
            && let Some(mut r) = self.reader.take()
            && let Some(thread) = r.thread.take()
        {
            thread.join().ok();
        }
    }

    fn drain_completed_reads(&mut self, qh: &QueueHandle<Self>, wake_needed: bool) {
        self.reap_finished_workers();
        let finished: Vec<(Attempt, ReadOutcome)> = self.read_done_rx.try_iter().collect();
        for (attempt, outcome) in finished {
            let effects = machine::step(&mut self.machine, Event::ReadFinished { attempt, outcome });
            self.apply_effects(effects, qh, None, None, wake_needed);
            self.pending_deliveries.remove(&attempt);
        }
    }

    fn spawn_writer(&mut self, fd_id: WriteFdId, bytes: Arc<[u8]>) {
        let (Some(fd), Some(cancel)) = (self.pending_fds.remove(&fd_id), WriteCancel::new().map(Arc::new)) else {
            let _ = machine::step(&mut self.machine, Event::WriterFinished);
            return;
        };
        let (done_wait, done_signal) = DoneFlag::new();
        let worker_cancel = Arc::clone(&cancel);
        let done_tx = self.writer_done_tx.clone();
        let spawned = std::thread::Builder::new().name("rune-primary-write".into()).spawn(move || {
            let _signal = SignalOnDrop(done_signal);
            let _ = write_fd_bounded(fd, &bytes, &worker_cancel, MAX_PRIMARY_BYTES, PIPE_WRITE_TIMEOUT);
            let _ = done_tx.send(());
        });
        match spawned {
            Ok(thread) => self.writers.push(ActiveWriter { cancel, done: done_wait, thread: Some(thread) }),
            Err(_) => { let _ = machine::step(&mut self.machine, Event::WriterFinished); }
        }
    }

    fn start_read(
        &mut self,
        attempt: Attempt,
        offer_id: OfferId,
        mime: String,
        deliver: Option<DeliverFn>,
        wake: Option<Waker>,
    ) {
        let Some(offer) = self.offers.get(&offer_id) else {
            let _ = machine::step(&mut self.machine, Event::ReadFinished { attempt, outcome: ReadOutcome::Error });
            return;
        };
        let (Some((read_fd, write_fd)), Some(cancel)) = (make_pipe(), CancelToken::new().map(Arc::new)) else {
            let _ = machine::step(&mut self.machine, Event::ReadFinished { attempt, outcome: ReadOutcome::Error });
            return;
        };
        offer.receive(mime, write_fd.as_fd());
        drop(write_fd);
        self.needs_flush = true;
        if let Some(d) = deliver { self.pending_deliveries.insert(attempt, d); }
        let (done_wait, done_signal) = DoneFlag::new();
        let worker_cancel = Arc::clone(&cancel);
        let done_tx = self.read_done_tx.clone();
        let spawned = std::thread::Builder::new().name("rune-primary-read".into()).spawn(move || {
            let _signal = SignalOnDrop(done_signal);
            if let Some(outcome) = read_fd_bounded(read_fd, &worker_cancel, MAX_PRIMARY_BYTES, PRIMARY_READ_TIMEOUT)
                && done_tx.send((attempt, outcome)).is_ok()
                && let Some(wake) = wake
            {
                wake();
            }
        });
        match spawned {
            Ok(thread) => {
                if let Some(prev) = self.reader.replace(ActiveReader { attempt, cancel, done: done_wait, thread: Some(thread) }) {
                    prev.cancel.cancel();
                    self.cancelled_readers.push(prev);
                }
            }
            Err(_) => {
                self.pending_deliveries.remove(&attempt);
                let _ = machine::step(&mut self.machine, Event::ReadFinished { attempt, outcome: ReadOutcome::Error });
            }
        }
    }

    fn apply_effects(
        &mut self,
        effects: Vec<Effect>,
        qh: &QueueHandle<Self>,
        mut immediate_deliver: Option<DeliverFn>,
        read_wake: Option<Waker>,
        wake_needed: bool,
    ) {
        for effect in effects {
            match effect {
                Effect::GetDevice { seat } => {
                    if !self.tearing_down
                        && !self.devices.contains_key(&seat)
                        && let Some(manager) = self.manager.as_ref()
                        && let Some(wl_seat) = self.seats.get(&seat)
                    {
                        self.devices.insert(seat, manager.get_device(wl_seat, qh, seat));
                        self.needs_flush = true;
                    }
                }
                Effect::DestroyDevice { seat } => {
                    if let Some(dev) = self.devices.remove(&seat) { dev.destroy(); self.needs_flush = true; }
                }
                Effect::DestroyOffer { offer_id } => {
                    if let Some(offer) = self.offers.remove(&offer_id) { offer.destroy(); self.needs_flush = true; }
                }
                Effect::SetSelection { seat, source_id, serial } => {
                    if !self.tearing_down
                        && let Some(manager) = self.manager.as_ref()
                        && let Some(device) = self.devices.get(&seat)
                    {
                        let source = manager.create_source(qh, source_id);
                        for mime in OFFERED_MIMES { source.offer(mime.to_owned()); }
                        device.set_selection(Some(&source), serial);
                        self.sources.insert(source_id, source);
                        self.needs_flush = true;
                    }
                }
                Effect::DestroySource { source_id } => {
                    if let Some(src) = self.sources.remove(&source_id) { src.destroy(); self.needs_flush = true; }
                }
                Effect::SelectionLost => { self.selection_lost = true; }
                Effect::SpawnWriter { fd_id, bytes } => self.spawn_writer(fd_id, bytes),
                Effect::CloseFd { fd_id } => { let _ = self.pending_fds.remove(&fd_id); }
                Effect::StartRead { attempt, offer_id, mime } => {
                    self.start_read(attempt, offer_id, mime, immediate_deliver.take(), read_wake.clone());
                }
                Effect::CancelRead { attempt } => {
                    self.pending_deliveries.remove(&attempt);
                    if self.reader.as_ref().is_some_and(|r| r.attempt == attempt)
                        && let Some(prev) = self.reader.take()
                    {
                        prev.cancel.cancel();
                        self.cancelled_readers.push(prev);
                    }
                }
                Effect::DeliverText { attempt, target, text } => {
                    let deliver = match attempt {
                        None => immediate_deliver.take(),
                        Some(att) => self.pending_deliveries.remove(&att),
                    };
                    if let Some(deliver) = deliver { deliver(target, text.to_string(), wake_needed); }
                }
            }
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for QueueState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version }
                if interface == WlSeat::interface().name => state.bind_seat(registry, qh, name, version),
            wl_registry::Event::GlobalRemove { name } => {
                if state.manager_global_name == Some(name) {
                    state.manager_global_name = None;
                    let effects = machine::step(&mut state.machine, Event::DisplayLost);
                    state.apply_effects(effects, qh, None, None, true);
                } else if state.seats.contains_key(&name) {
                    state.remove_seat(name, conn, qh);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, u32> for QueueState {
    fn event(_: &mut Self, _: &WlSeat, _: wl_seat::Event, _: &u32, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpPrimarySelectionDeviceManagerV1, ()> for QueueState {
    fn event(
        _: &mut Self,
        _: &ZwpPrimarySelectionDeviceManagerV1,
        _: <ZwpPrimarySelectionDeviceManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<ZwpPrimarySelectionDeviceV1, u32> for QueueState {
    fn event(
        state: &mut Self,
        _: &ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        seat_name: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_device_v1::Event::DataOffer { offer } => {
                if state.tearing_down {
                    offer.destroy();
                    state.needs_flush = true;
                    return;
                }
                let offer_id = offer.id().protocol_id();
                state.offers.insert(offer_id, offer);
                let effects = machine::step(&mut state.machine, Event::DataOffer { seat: *seat_name, offer_id });
                state.apply_effects(effects, qh, None, None, true);
            }
            zwp_primary_selection_device_v1::Event::Selection { id } => {
                if state.tearing_down {
                    if let Some(offer) = id { offer.destroy(); state.needs_flush = true; }
                    return;
                }
                let offer_id = id.map(|offer| {
                    let pid = offer.id().protocol_id();
                    state.offers.entry(pid).or_insert(offer);
                    pid
                });
                let effects = machine::step(&mut state.machine, Event::Selection { seat: *seat_name, offer_id });
                state.apply_effects(effects, qh, None, None, true);
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(QueueState, ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<ZwpPrimarySelectionOfferV1, ()> for QueueState {
    fn event(
        state: &mut Self,
        offer: &ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.tearing_down { return; }
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            let _ = machine::step(&mut state.machine, Event::OfferMime { offer_id: offer.id().protocol_id(), mime_type });
        }
    }
}

impl Dispatch<ZwpPrimarySelectionSourceV1, SourceId> for QueueState {
    fn event(
        state: &mut Self,
        _: &ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        source_id: &SourceId,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if state.tearing_down { return; }
        match event {
            zwp_primary_selection_source_v1::Event::Send { mime_type, fd } => {
                state.reap_finished_workers();
                let fd_id = state.next_fd_id;
                state.next_fd_id = state.next_fd_id.wrapping_add(1).max(1);
                state.pending_fds.insert(fd_id, fd);
                let effects = machine::step(&mut state.machine, Event::SourceSend {
                    source_id: *source_id,
                    mime_type,
                    fd_id,
                });
                state.apply_effects(effects, qh, None, None, true);
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                let effects = machine::step(&mut state.machine, Event::SourceCancelled { source_id: *source_id });
                state.apply_effects(effects, qh, None, None, true);
            }
            _ => {}
        }
    }
}

fn drain_queue(queue: &mut EventQueue<QueueState>, state: &mut QueueState) -> bool {
    loop {
        match queue.dispatch_pending(state) {
            Ok(0) => return true,
            Ok(_) | Err(DispatchError::BadMessage { .. }) => {}
            Err(_) => return false,
        }
    }
}

struct Controller {
    display_ptr: usize,
    conn: Connection,
    globals: Option<GlobalList>,
    queue: EventQueue<QueueState>,
    state: QueueState,
    _serial_guard: Option<SerialGuard>,
}

impl Controller {
    #[allow(unsafe_code)]
    fn open(display_ptr: usize) -> Option<DisplaySlot> {
        // SAFETY: winit owns `wl_display` while the event loop runs, and `shutdown()` clears
        // every controller strictly before winit closes `wl_display`.
        let backend = unsafe { Backend::from_foreign_display((display_ptr as *mut std::ffi::c_void).cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<QueueState>(&conn).ok()?;
        let qh = queue.handle();

        let manager_global = globals.contents().with_list(|list| {
            list.iter()
                .find(|g| g.interface == ZwpPrimarySelectionDeviceManagerV1::interface().name)
                .map(|g| g.name)
        });
        let Some(manager_name) = manager_global else {
            destroy_globals(&conn, globals);
            let _ = queue.flush();
            return Some(DisplaySlot::Unavailable);
        };
        let Ok(manager) = globals.bind::<ZwpPrimarySelectionDeviceManagerV1, _, _>(&qh, 1..=1, ()) else {
            destroy_globals(&conn, globals);
            let _ = queue.flush();
            return Some(DisplaySlot::Unavailable);
        };

        let serial_guard = attach_serial(display_ptr);
        let mut state = QueueState::new(manager, manager_name);
        let initial_seats: Vec<(u32, u32)> = globals
            .contents()
            .clone_list()
            .into_iter()
            .filter(|g| g.interface == WlSeat::interface().name)
            .map(|g| (g.name, g.version))
            .collect();
        for (name, version) in initial_seats {
            state.bind_seat(globals.registry(), &qh, name, version);
        }
        if queue.roundtrip(&mut state).is_err() {
            let effects = machine::step(&mut state.machine, Event::DisplayLost);
            state.apply_effects(effects, &qh, None, None, true);
        }
        state.needs_flush = false;
        let _ = queue.flush();
        Some(DisplaySlot::Live(Box::new(Self {
            display_ptr,
            conn,
            globals: Some(globals),
            queue,
            state,
            _serial_guard: serial_guard,
        })))
    }

    fn dispatch_pending(&mut self) {
        if self.state.machine.availability() == Availability::Dead { return; }
        let ok = drain_queue(&mut self.queue, &mut self.state);
        if !ok || self.conn.backend().last_error().is_some() {
            let qh = self.queue.handle();
            let effects = machine::step(&mut self.state.machine, Event::DisplayLost);
            self.state.apply_effects(effects, &qh, None, None, true);
            return;
        }
        let qh = self.queue.handle();
        self.state.drain_completed_reads(&qh, true);
        if self.state.needs_flush {
            self.state.needs_flush = false;
            if self.queue.flush().is_err() || self.conn.backend().last_error().is_some() {
                let effects = machine::step(&mut self.state.machine, Event::DisplayLost);
                self.state.apply_effects(effects, &qh, None, None, true);
            }
        }
    }

    fn needs_wake_on_read(&self) -> bool { self.state.machine.needs_wake_on_read() }

    fn has_pending_events(&self) -> bool {
        self.state.machine.needs_wake_on_read() && self.queue.prepare_read().is_none()
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.state.tearing_down = true;
        self.state.pending_deliveries.clear();
        self.state.pending_fds.clear();
        if let Some(reader) = self.state.reader.take() { reader.cancel_and_join(); }
        for reader in self.state.cancelled_readers.drain(..) { reader.cancel_and_join(); }
        for writer in self.state.writers.drain(..) { writer.cancel_and_join(); }
        let _ = drain_queue(&mut self.queue, &mut self.state);
        let qh = self.queue.handle();
        let effects = machine::step(&mut self.state.machine, Event::Shutdown);
        self.state.apply_effects(effects, &qh, None, None, false);
        if let Some(manager) = self.state.manager.take() { manager.destroy(); }
        let backend = self.conn.backend();
        for (_, seat) in self.state.seats.drain() {
            if seat.version() >= 5 { seat.release(); } else { backend.destroy_object(&seat.id()).ok(); }
        }
        if let Some(globals) = self.globals.take() { destroy_globals(&self.conn, globals); }
        let _ = drain_queue(&mut self.queue, &mut self.state);
        let _ = self.queue.flush();
    }
}

/// Lazily binds `zwp_primary_selection_device_manager_v1` for `window`'s `wl_display` after frame 1.
pub(crate) fn ensure_for_window(window: &winit::window::Window) {
    if let Ok(handle) = window.display_handle() { ensure_for_display(handle.as_raw()); }
}

/// Lazily binds `zwp_primary_selection_device_manager_v1` for `event_loop`'s `wl_display`
/// (for overlay-only apps after their first frame).
pub(crate) fn ensure_for_event_loop(event_loop: &winit::event_loop::ActiveEventLoop) {
    if let Ok(handle) = event_loop.display_handle() { ensure_for_display(handle.as_raw()); }
}

/// Lazily binds `zwp_primary_selection_device_manager_v1` for a raw Wayland display handle.
pub(crate) fn ensure_for_display(display: RawDisplayHandle) {
    let RawDisplayHandle::Wayland(display) = display else { return };
    let display_ptr = display.display.as_ptr() as usize;
    CONTROLLERS.with(|map| {
        let mut map = map.borrow_mut();
        if map.contains_key(&display_ptr) { return; }
        if let Some(slot) = Controller::open(display_ptr) { map.insert(display_ptr, slot); }
    });
}

/// Publishes `text` via `zwp_primary_selection_v1`, or returns `Fallback` off Wayland / when absent.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn try_write(text: &str) -> WriteDispatch {
    CONTROLLERS.try_with(|map| {
        let mut map = map.borrow_mut();
        let Some(DisplaySlot::Live(controller)) = map.values_mut().next() else {
            return WriteDispatch::Fallback;
        };
        controller.dispatch_pending();
        if controller.state.machine.availability() == Availability::Dead { return WriteDispatch::Handled; }
        let serial = latest_input(controller.display_ptr).map(|c| (c.seat, c.serial));
        let qh = controller.queue.handle();
        let effects = machine::step(&mut controller.state.machine, Event::RequestWrite {
            text: text.to_owned(),
            serial,
        });
        let set_new = effects.iter().any(|e| matches!(e, Effect::SetSelection { .. }));
        controller.state.apply_effects(effects, &qh, None, None, true);
        if set_new { controller.state.selection_lost = false; }
        if controller.state.needs_flush {
            controller.state.needs_flush = false;
            let _ = controller.queue.flush();
        }
        WriteDispatch::Handled
    }).unwrap_or(WriteDispatch::Fallback)
}

/// Reads the Wayland primary selection via `zwp_primary_selection_v1`, or returns `Fallback`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn try_read(target: String, wake: Waker, deliver: DeliverFn) -> ReadDispatch {
    let mut target = Some(target);
    let mut deliver = Some(deliver);
    let mut wake = Some(wake);
    CONTROLLERS.try_with(|map| {
        let mut map = map.borrow_mut();
        let Some(DisplaySlot::Live(controller)) = map.values_mut().next() else {
            return ReadDispatch::Fallback(target.take().unwrap());
        };
        controller.dispatch_pending();
        if controller.state.machine.availability() == Availability::Dead { return ReadDispatch::Handled; }
        let preferred_seat = latest_input(controller.display_ptr).map(|c| c.seat);
        let qh = controller.queue.handle();
        let effects = machine::step(&mut controller.state.machine, Event::RequestRead {
            target: target.take().unwrap(),
            preferred_seat,
        });
        controller.state.apply_effects(effects, &qh, deliver.take(), wake.take(), true);
        if controller.state.needs_flush {
            controller.state.needs_flush = false;
            let _ = controller.queue.flush();
        }
        ReadDispatch::Handled
    }).unwrap_or_else(|_| ReadDispatch::Fallback(target.take().unwrap()))
}

/// Drains completed off-thread pipe reads into `Client` reply channels on the UI thread.
pub(crate) fn drain_completed_reads(wake_needed: bool) {
    CONTROLLERS.try_with(|map| {
        for slot in map.borrow_mut().values_mut() {
            if let DisplaySlot::Live(controller) = slot {
                let qh = controller.queue.handle();
                controller.state.drain_completed_reads(&qh, wake_needed);
            }
        }
    }).ok();
}

/// Dispatches already-read Wayland events and completed pipe workers on the UI thread.
/// Returns `true` if our owned primary-selection source was cancelled since the last check.
pub(crate) fn dispatch_pending() -> bool {
    CONTROLLERS.try_with(|map| {
        let mut lost = false;
        for slot in map.borrow_mut().values_mut() {
            if let DisplaySlot::Live(controller) = slot {
                controller.dispatch_pending();
                if std::mem::take(&mut controller.state.selection_lost) { lost = true; }
            }
        }
        lost
    }).unwrap_or(false)
}

/// True strictly while a live controller owns a `zwp_primary_selection_source_v1`.
pub(crate) fn needs_wake_on_read() -> bool {
    CONTROLLERS.try_with(|map| {
        map.borrow().values().any(|slot| matches!(slot, DisplaySlot::Live(c) if c.needs_wake_on_read()))
    }).unwrap_or(false)
}

/// Syscall-free check for whether an owned primary-selection queue has unread events.
pub(crate) fn has_pending_events() -> bool {
    CONTROLLERS.try_with(|map| {
        map.borrow().values().any(|slot| matches!(slot, DisplaySlot::Live(c) if c.has_pending_events()))
    }).unwrap_or(false)
}

/// Tears down all primary-selection controllers strictly before winit closes `wl_display`.
pub(crate) fn shutdown() {
    CONTROLLERS.try_with(|map| { map.borrow_mut().clear(); }).ok();
}
