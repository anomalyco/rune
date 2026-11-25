//! Native Wayland `wl_data_device` / `wl_data_offer` drop target and `wl_data_source` drag
//! source on winit's display connection.
//!
//! One shared controller per `wl_display` binds a single `wl_data_device` per `wl_seat` across all
//! windows (KWin only delivers drags to a client's first data device per seat), watches its
//! private event queue with zero idle polling via `watch::WaylandWatch`, reads `text/uri-list`
//! pipes and writes outgoing `wl_data_source.send` pipes on bounded cancellable worker threads,
//! and destroys every proxy before winit closes `wl_display`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::AsFd;
use std::sync::{Arc, mpsc};

use wayland_backend::client::Backend;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::{self, WlCompositor};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum};

use super::machine::{
    self, Effect, Event, MAX_URI_LIST_BYTES, OfferId, OfferSet, ReadOutcome, ReadPhase,
    SelectedAction, State, SurfaceId, URI_LIST_MIME, WindowMap, WindowToken,
};
use super::pipe::{CancelToken, DoneFlag, SignalOnDrop, THREAD_JOIN_BOUND, join_bounded, make_pipe, read_fd_bounded};
use super::watch::{WakePool, WaylandWatch, destroy_globals, wayland_ids, wayland_surface_id};
use crate::drag_out::DragOutcome;
use crate::machine::{Attempt, Seq};
use crate::platform::wayland::drag_source::icon::PreparedIcon;
use crate::platform::wayland::drag_source::machine::DragId;
use crate::platform::wayland::drag_source::mime::OfferedMime;
use crate::platform::wayland::serial;
use crate::platform::wayland::drag_source::wire::{Done, DragSourceWire};
use crate::runtime::Waker;
use crate::window::FileStep;

thread_local! {
    /// One shared Wayland data-device controller per `wl_display` pointer, always touched on the UI thread.
    static CONTROLLERS: RefCell<HashMap<usize, Controller>> = RefCell::new(HashMap::new());
}

/// Removes a window's surface from the shared display controller on drop, tearing down the
/// controller and its Wayland proxies when the last window on that display closes.
pub(crate) struct DropTarget {
    display: usize,
    surface: SurfaceId,
    token: WindowToken,
}

impl Drop for DropTarget {
    fn drop(&mut self) {
        CONTROLLERS.try_with(|map| {
            let mut map = map.borrow_mut();
            let remove = if let Some(controller) = map.get_mut(&self.display) {
                controller.unregister(self.surface, self.token);
                controller.is_empty()
            } else {
                false
            };
            if remove {
                map.remove(&self.display);
            }
        }).ok();
    }
}

/// Registers `window`'s `wl_surface` with the shared data-device controller for its `wl_display`,
/// starting the controller on the first Wayland window. Returns `None` off Wayland or when the
/// compositor lacks `wl_data_device_manager`.
#[allow(unsafe_code)]
pub(crate) fn install(window: &winit::window::Window, wake: Waker) -> Option<DropTarget> {
    let (display_ptr, surface_id) = wayland_ids(window)?;
    CONTROLLERS.with(|map| {
        let mut map = map.borrow_mut();
        if !map.contains_key(&display_ptr) {
            let controller = Controller::new(display_ptr as *mut std::ffi::c_void)?;
            map.insert(display_ptr, controller);
        }
        let controller = map.get_mut(&display_ptr)?;
        let token = controller.register(surface_id, wake);
        Some(DropTarget { display: display_ptr, surface: surface_id, token })
    })
}

/// Starts an outgoing Wayland `wl_data_source` drag sharing this display's `wl_data_device`.
/// Invokes `done` with `DragOutcome::Cancelled` immediately if off Wayland, no controller/seat
/// is bound, or no press/touch serial was captured for `window`'s surface.
pub(crate) fn begin_drag(
    window: &winit::window::Window,
    within_app: bool,
    offers: Vec<OfferedMime>,
    actions_mask: u32,
    icon: Option<PreparedIcon>,
    done: Done,
) {
    let Some((display_ptr, surface_id, surface_obj_id)) = wayland_surface_id(window) else {
        done(DragOutcome::Cancelled);
        return;
    };
    let Some(captured) = serial::wire::latest_press(display_ptr, surface_id) else {
        done(DragOutcome::Cancelled);
        return;
    };
    let mut done_opt = Some(done);
    CONTROLLERS.with(|map| {
        let mut map = map.borrow_mut();
        let Some(controller) = map.get_mut(&display_ptr) else {
            return;
        };
        if let Some(done) = done_opt.take() {
            controller.begin_drag(
                captured.seat,
                captured.serial,
                surface_id,
                surface_obj_id,
                within_app,
                offers,
                actions_mask,
                icon,
                done,
            );
        }
    });
    if let Some(done) = done_opt {
        done(DragOutcome::Cancelled);
    }
}

/// Whether `window`'s display controller currently has an outgoing Wayland drag in flight.
pub(crate) fn has_active_drag(window: &winit::window::Window) -> bool {
    let Some((display_ptr, _)) = wayland_ids(window) else {
        return false;
    };
    CONTROLLERS.try_with(|map| {
        map.borrow().get(&display_ptr).is_some_and(|c| c.state.drag_source.is_active())
    }).unwrap_or(false)
}

/// Pumps the display's shared Wayland data-device queue and worker replies, then drains any
/// `FileStep`s queued for `window`, plus whether the display connection failed. Returns empty off
/// Wayland or when no controller is bound.
pub(crate) fn take(window: &winit::window::Window) -> (Vec<FileStep>, bool) {
    let Some((display_ptr, surface_id)) = wayland_ids(window) else {
        return (Vec::new(), false);
    };
    CONTROLLERS.with(|map| {
        let mut map = map.borrow_mut();
        let Some(controller) = map.get_mut(&display_ptr) else {
            return (Vec::new(), false);
        };
        let lost = controller.pump();
        (controller.take(surface_id), lost)
    })
}

/// Tears down any remaining Wayland data-device controllers and serial listeners before exit.
pub(crate) fn shutdown() {
    CONTROLLERS.try_with(|map| {
        map.borrow_mut().clear();
    }).ok();
    serial::wire::shutdown();
}

struct WorkerReply {
    seat: u32,
    attempt: Attempt,
    outcome: ReadOutcome,
}

struct ActiveReader {
    cancel: Arc<CancelToken>,
    done: DoneFlag,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ActiveReader {
    fn cancel_and_join(self) {
        self.cancel.cancel();
        self.join();
    }

    fn join(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = join_bounded(thread, &self.done, THREAD_JOIN_BOUND);
        }
    }
}

struct SeatDevice {
    seat: WlSeat,
    device: WlDataDevice,
    pending: OfferSet,
    state: State,
}

struct QueueState {
    manager: Option<WlDataDeviceManager>,
    seats: HashMap<u32, SeatDevice>,
    offers: HashMap<OfferId, WlDataOffer>,
    offer_seat: HashMap<OfferId, u32>,
    windows: WindowMap,
    seq: Seq,
    readers: HashMap<Attempt, ActiveReader>,
    reply_tx: mpsc::Sender<WorkerReply>,
    wakers: WakePool,
    drag_source: DragSourceWire,
    needs_flush: bool,
    tearing_down: bool,
}

impl QueueState {
    fn new(reply_tx: mpsc::Sender<WorkerReply>, wakers: WakePool) -> Self {
        Self {
            manager: None,
            seats: HashMap::new(),
            offers: HashMap::new(),
            offer_seat: HashMap::new(),
            windows: WindowMap::default(),
            seq: Seq::default(),
            readers: HashMap::new(),
            reply_tx,
            wakers,
            drag_source: DragSourceWire::new(),
            needs_flush: false,
            tearing_down: false,
        }
    }

    fn bind_seat(&mut self, registry: &WlRegistry, qh: &QueueHandle<Self>, name: u32, version: u32) {
        if self.tearing_down || self.seats.contains_key(&name) {
            return;
        }
        let Some(manager) = &self.manager else {
            return;
        };
        let seat = registry.bind::<WlSeat, _, _>(name, version.min(5), qh, name);
        let device = manager.get_data_device(&seat, qh, name);
        self.seats.insert(name, SeatDevice {
            seat,
            device,
            pending: OfferSet::default(),
            state: State::Idle,
        });
        self.needs_flush = true;
    }

    fn remove_seat(&mut self, name: u32, conn: &Connection) {
        let Some(mut seat_dev) = self.seats.remove(&name) else {
            return;
        };
        let effects = machine::step(&mut self.seq, &mut seat_dev.state, Event::Gone);
        self.apply_effects(name, effects);
        for offer_id in seat_dev.pending.drain_ids() {
            self.destroy_offer(offer_id);
        }
        let backend = conn.backend();
        if seat_dev.device.version() >= 2 {
            seat_dev.device.release();
        } else {
            backend.destroy_object(&seat_dev.device.id()).ok();
        }
        if seat_dev.seat.version() >= 5 {
            seat_dev.seat.release();
        } else {
            backend.destroy_object(&seat_dev.seat.id()).ok();
        }
        self.needs_flush = true;
    }

    fn step_seat(&mut self, seat_name: u32, event: Event) {
        let Some(seat_dev) = self.seats.get_mut(&seat_name) else {
            return;
        };
        let effects = machine::step(&mut self.seq, &mut seat_dev.state, event);
        self.apply_effects(seat_name, effects);
    }

    fn destroy_offer(&mut self, offer_id: OfferId) {
        self.offer_seat.remove(&offer_id);
        if let Some(offer) = self.offers.remove(&offer_id) {
            offer.destroy();
            self.needs_flush = true;
        }
    }

    fn apply_effects(&mut self, seat_name: u32, effects: crate::machine::Effects<Effect>) {
        for effect in effects {
            match effect {
                Effect::AcceptOffer { offer, serial, mime } => {
                    if let Some(wl_offer) = self.offers.get(&offer) {
                        wl_offer.accept(serial, mime.map(str::to_owned));
                    }
                }
                Effect::SetActions { offer, copy } => {
                    if let Some(wl_offer) = self.offers.get(&offer)
                        && wl_offer.version() >= 3
                    {
                        let action = if copy { DndAction::Copy } else { DndAction::None };
                        wl_offer.set_actions(action, action);
                    }
                }
                Effect::StartRead { offer, attempt, phase } => {
                    if self.tearing_down {
                        continue;
                    }
                    self.start_read(seat_name, offer, attempt, phase);
                }
                Effect::CancelRead(attempt) => {
                    if let Some(reader) = self.readers.remove(&attempt) {
                        reader.cancel_and_join();
                    }
                }
                Effect::FinishOffer(offer) => {
                    if let Some(wl_offer) = self.offers.get(&offer)
                        && wl_offer.version() >= 3
                    {
                        wl_offer.finish();
                    }
                }
                Effect::DestroyOffer(offer) => {
                    self.destroy_offer(offer);
                }
                Effect::Flush => {
                    self.needs_flush = true;
                }
                Effect::EnqueueStep { surface, token, step } => {
                    if !self.tearing_down {
                        self.windows.push(surface, token, step);
                    }
                }
            }
        }
    }

    fn start_read(&mut self, seat_name: u32, offer: OfferId, attempt: Attempt, phase: ReadPhase) {
        let Some(wl_offer) = self.offers.get(&offer) else {
            self.reply_tx.send(WorkerReply { seat: seat_name, attempt, outcome: ReadOutcome::Error }).ok();
            self.wakers.wake();
            return;
        };
        let (Some((read_fd, write_fd)), Some(cancel)) = (make_pipe(), CancelToken::new()) else {
            self.reply_tx.send(WorkerReply { seat: seat_name, attempt, outcome: ReadOutcome::Error }).ok();
            self.wakers.wake();
            return;
        };
        wl_offer.receive(URI_LIST_MIME.to_owned(), write_fd.as_fd());
        drop(write_fd);
        self.needs_flush = true;

        let cancel = Arc::new(cancel);
        let worker_cancel = cancel.clone();
        let (done, signal_done) = DoneFlag::new();
        let reply_tx = self.reply_tx.clone();
        let wakers = self.wakers.clone();
        let timeout = phase.timeout();
        match std::thread::Builder::new()
            .name("rune-wayland-drop-read".into())
            .spawn(move || {
                let _signal = SignalOnDrop(signal_done);
                if let Some(outcome) = read_fd_bounded(read_fd, &worker_cancel, MAX_URI_LIST_BYTES, timeout) {
                    if reply_tx.send(WorkerReply { seat: seat_name, attempt, outcome }).is_ok() {
                        wakers.wake();
                    }
                }
            }) {
            Ok(thread) => {
                self.readers.insert(attempt, ActiveReader { cancel, done, thread: Some(thread) });
            }
            Err(_) => {
                self.reply_tx.send(WorkerReply { seat: seat_name, attempt, outcome: ReadOutcome::Error }).ok();
                self.wakers.wake();
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
                if interface == WlSeat::interface().name =>
            {
                state.bind_seat(registry, qh, name, version);
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.remove_seat(name, conn);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for QueueState {
    fn event(_: &mut Self, _: &WlDataDeviceManager, _: <WlDataDeviceManager as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSeat, u32> for QueueState {
    fn event(_: &mut Self, _: &WlSeat, _: wl_seat::Event, _: &u32, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlCompositor, ()> for QueueState {
    fn event(_: &mut Self, _: &WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlShm, ()> for QueueState {
    fn event(_: &mut Self, _: &WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlShmPool, ()> for QueueState {
    fn event(_: &mut Self, _: &WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlBuffer, ()> for QueueState {
    fn event(_: &mut Self, _: &WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSurface, ()> for QueueState {
    fn event(_: &mut Self, _: &WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlDataSource, DragId> for QueueState {
    fn event(
        state: &mut Self,
        _: &WlDataSource,
        event: wl_data_source::Event,
        drag_id: &DragId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.tearing_down {
            return;
        }
        state.drag_source.on_source_event(*drag_id, event, &state.wakers, &mut state.needs_flush);
    }
}

impl Dispatch<WlDataDevice, u32> for QueueState {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        seat_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.tearing_down {
            if let wl_data_device::Event::DataOffer { id } = event {
                id.destroy();
            }
            return;
        }
        let seat_name = *seat_name;
        match event {
            wl_data_device::Event::DataOffer { id: offer } => {
                let offer_id = offer.id().protocol_id();
                let version = offer.version();
                state.offers.insert(offer_id, offer);
                state.offer_seat.insert(offer_id, seat_name);
                let evicted = state.seats.get_mut(&seat_name).and_then(|s| s.pending.insert(offer_id, version));
                if let Some(old_id) = evicted {
                    state.destroy_offer(old_id);
                }
            }
            wl_data_device::Event::Enter { serial, surface, x, y, id } => {
                let surface_id = surface.id().protocol_id();
                // Mute self-drops while our own outgoing drag on this seat has `!within_app`.
                let target = (!state.drag_source.mutes_drop_in(seat_name))
                    .then(|| state.windows.token(surface_id))
                    .flatten();
                let offer = id.and_then(|o| {
                    let offer_id = o.id().protocol_id();
                    state.seats.get_mut(&seat_name).and_then(|s| s.pending.take(offer_id))
                });
                state.step_seat(seat_name, Event::Enter {
                    serial,
                    surface: surface_id,
                    target,
                    x: x as f32,
                    y: y as f32,
                    offer,
                });
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                state.step_seat(seat_name, Event::Motion { x: x as f32, y: y as f32 });
            }
            wl_data_device::Event::Leave => {
                state.step_seat(seat_name, Event::Leave);
            }
            wl_data_device::Event::Drop => {
                state.step_seat(seat_name, Event::Drop);
            }
            wl_data_device::Event::Selection { id } => {
                // Rune uses arboard for clipboard; destroy clipboard offers immediately so proxies do not leak.
                if let Some(offer) = id {
                    let offer_id = offer.id().protocol_id();
                    if let Some(seat_dev) = state.seats.get_mut(&seat_name) {
                        seat_dev.pending.take(offer_id);
                    }
                    state.offer_seat.remove(&offer_id);
                    if let Some(stored) = state.offers.remove(&offer_id) {
                        stored.destroy();
                    } else {
                        offer.destroy();
                    }
                    state.needs_flush = true;
                }
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(QueueState, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataOffer, ()> for QueueState {
    fn event(
        state: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.tearing_down {
            return;
        }
        let offer_id = offer.id().protocol_id();
        let Some(&seat_name) = state.offer_seat.get(&offer_id) else {
            return;
        };
        match event {
            wl_data_offer::Event::Offer { mime_type } => {
                if let Some(seat_dev) = state.seats.get_mut(&seat_name) {
                    seat_dev.pending.add_mime(offer_id, &mime_type);
                }
            }
            wl_data_offer::Event::Action { dnd_action } => {
                let action = match dnd_action {
                    WEnum::Value(a) => SelectedAction::from_wayland_mask(a.bits()),
                    WEnum::Unknown(_) => SelectedAction::None,
                };
                if let Some(seat_dev) = state.seats.get_mut(&seat_name) {
                    seat_dev.pending.set_action(offer_id, action);
                }
                state.step_seat(seat_name, Event::OfferAction { offer: offer_id, action });
            }
            wl_data_offer::Event::SourceActions { .. } | _ => {}
        }
    }
}

fn drain_event_queue(queue: &mut EventQueue<QueueState>, state: &mut QueueState) {
    loop {
        match queue.dispatch_pending(state) {
            Ok(0) => break,
            Ok(_) | Err(DispatchError::BadMessage { .. }) => {}
            Err(_) => break,
        }
    }
}

struct Controller {
    conn: Connection,
    globals: Option<GlobalList>,
    queue: EventQueue<QueueState>,
    state: QueueState,
    reply_rx: mpsc::Receiver<WorkerReply>,
    watch: Option<WaylandWatch>,
}

impl Controller {
    #[allow(unsafe_code)]
    fn new(display_ptr: *mut std::ffi::c_void) -> Option<Self> {
        // SAFETY: winit owns `wl_display` for the life of the event loop, and this controller is
        // removed and dropped when the last window's `DropTarget` drops before `wl_display` closes.
        let backend = unsafe { Backend::from_foreign_display(display_ptr.cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<QueueState>(&conn).ok()?;
        let qh = queue.handle();
        let (reply_tx, reply_rx) = mpsc::channel();
        let wakers = WakePool::default();
        let mut state = QueueState::new(reply_tx, wakers.clone());
        let Ok(manager) = globals.bind::<WlDataDeviceManager, _, _>(&qh, 1..=3, ()) else {
            destroy_globals(&conn, globals);
            state.tearing_down = true;
            drain_event_queue(&mut queue, &mut state);
            return None;
        };
        state.manager = Some(manager);
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
        state.needs_flush = false;
        queue.flush().ok();
        let watch = WaylandWatch::spawn(conn.clone(), wakers);
        Some(Self { conn, globals: Some(globals), queue, state, reply_rx, watch })
    }

    fn register(&mut self, surface: SurfaceId, wake: Waker) -> WindowToken {
        let token = self.state.windows.register(surface);
        self.state.wakers.add(token, wake);
        token
    }

    fn unregister(&mut self, surface: SurfaceId, token: WindowToken) {
        self.state.wakers.remove(token);
        if !self.state.windows.unregister(surface, token) {
            return;
        }
        self.state.drag_source.on_window_closed(surface, &self.state.wakers, &mut self.state.needs_flush);
        let seat_names: Vec<u32> = self.state.seats.keys().copied().collect();
        for seat_name in seat_names {
            self.state.step_seat(seat_name, Event::UnregisterWindow { surface, token });
        }
        if self.state.needs_flush {
            self.state.needs_flush = false;
            self.queue.flush().ok();
        }
    }

    fn is_empty(&self) -> bool {
        self.state.windows.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_drag(
        &mut self,
        seat: u32,
        serial: u32,
        origin_surface: SurfaceId,
        origin_obj_id: wayland_backend::client::ObjectId,
        within_app: bool,
        offers: Vec<OfferedMime>,
        actions_mask: u32,
        icon: Option<PreparedIcon>,
        done: Done,
    ) {
        let Ok(origin_wl_surface) = WlSurface::from_id(&self.conn, origin_obj_id) else {
            done(DragOutcome::Cancelled);
            return;
        };
        let (Some(manager), Some(seat_dev)) = (&self.state.manager, self.state.seats.get(&seat)) else {
            done(DragOutcome::Cancelled);
            return;
        };
        let qh = self.queue.handle();
        self.state.drag_source.start_drag(
            manager,
            self.globals.as_ref(),
            &seat_dev.device,
            &origin_wl_surface,
            origin_surface,
            seat,
            serial,
            within_app,
            offers,
            actions_mask,
            icon,
            done,
            &qh,
            &self.state.wakers,
            &mut self.state.needs_flush,
        );
        if self.state.needs_flush {
            self.state.needs_flush = false;
            self.queue.flush().ok();
        }
    }

    /// Pumps Wayland events and worker replies; returns `true` if the display connection failed.
    fn pump(&mut self) -> bool {
        let dispatch_err = self.queue.dispatch_pending(&mut self.state).is_err();
        self.state.drag_source.pump_writers(&self.state.wakers, &mut self.state.needs_flush);
        for _ in 0..64 {
            let Ok(reply) = self.reply_rx.try_recv() else {
                break;
            };
            if let Some(reader) = self.state.readers.remove(&reply.attempt) {
                reader.join();
            }
            self.state.step_seat(reply.seat, Event::ReadComplete {
                attempt: reply.attempt,
                outcome: reply.outcome,
            });
        }
        let flush_err = if self.state.needs_flush {
            self.state.needs_flush = false;
            self.queue.flush().is_err()
        } else {
            false
        };
        let lost = dispatch_err || flush_err || self.conn.backend().last_error().is_some();
        if lost {
            self.watch = None;
            self.state.drag_source.on_display_lost(&self.state.wakers, &mut self.state.needs_flush);
        } else if let Some(watch) = &self.watch {
            watch.turned();
        }
        lost
    }

    fn take(&mut self, surface: SurfaceId) -> Vec<FileStep> {
        self.state.windows.take(surface)
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        // Stop and join the reader thread before draining or destroying proxies.
        self.watch = None;
        self.state.tearing_down = true;
        self.state.drag_source.on_shutdown(&self.conn, &self.state.wakers, &mut self.state.needs_flush);
        let readers: Vec<ActiveReader> = self.state.readers.drain().map(|(_, r)| r).collect();
        for reader in &readers {
            reader.cancel.cancel();
        }
        for reader in readers {
            reader.join();
        }
        drain_event_queue(&mut self.queue, &mut self.state);
        let seat_names: Vec<u32> = self.state.seats.keys().copied().collect();
        for seat_name in seat_names {
            self.state.remove_seat(seat_name, &self.conn);
        }
        for (_, offer) in self.state.offers.drain() {
            offer.destroy();
        }
        self.state.offer_seat.clear();
        let backend = self.conn.backend();
        if let Some(manager) = self.state.manager.take() {
            backend.destroy_object(&manager.id()).ok();
        }
        if let Some(globals) = self.globals.take() {
            destroy_globals(&self.conn, globals);
        }
        drain_event_queue(&mut self.queue, &mut self.state);
        self.queue.flush().ok();
    }
}
