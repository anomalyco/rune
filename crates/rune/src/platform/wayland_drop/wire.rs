//! Native Wayland `wl_data_device` / `wl_data_offer` drop target on winit's display connection.
//!
//! One shared controller per `wl_display` binds a single `wl_data_device` per `wl_seat` across all
//! windows (KWin only delivers drags to a client's first data device per seat), watches its
//! private event queue with zero idle polling via `layer_shell_machine::watch_loop`, reads
//! `text/uri-list` pipes on bounded cancellable worker threads, and destroys every proxy before
//! winit closes `wl_display`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId, ReadEventsGuard, WaylandError};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum};

use super::machine::{
    self, Effect, Event, MAX_URI_LIST_BYTES, OfferId, OfferSet, ReadOutcome, ReadPhase,
    SelectedAction, State, SurfaceId, URI_LIST_MIME, WindowMap, WindowToken,
};
use super::pipe::{CancelToken, DoneFlag, SignalOnDrop, StopSignal, THREAD_JOIN_BOUND, join_bounded, make_pipe, read_fd_bounded};
use crate::machine::{Attempt, Seq};
use crate::platform::layer_shell_machine::watch_loop;
use crate::runtime::Waker;
use crate::window::FileStep;

thread_local! {
    /// One shared Wayland drop controller per `wl_display` pointer, always touched on the UI thread.
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

/// Registers `window`'s `wl_surface` with the shared drop controller for its `wl_display`,
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

/// Pumps the display's shared Wayland drop queue and worker replies, then drains any `FileStep`s
/// queued for `window`, plus whether the display connection failed. Returns empty off Wayland or
/// when no controller is bound.
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

/// Tears down any remaining Wayland drop controllers and joins their threads before exit.
pub(crate) fn shutdown() {
    CONTROLLERS.try_with(|map| {
        map.borrow_mut().clear();
    }).ok();
}

#[allow(unsafe_code)]
fn wayland_ids(window: &winit::window::Window) -> Option<(usize, SurfaceId)> {
    let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) = (
        window.display_handle().ok()?.as_raw(),
        window.window_handle().ok()?.as_raw(),
    ) else {
        return None;
    };
    // SAFETY: `handle.surface` is the live `wl_surface` proxy winit owns for `window`.
    let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }.ok()?;
    Some((display.display.as_ptr() as usize, id.protocol_id()))
}

/// Shared wake handle pointing to all currently registered windows on one display; calling `wake`
/// wakes the shared winit event loop once.
#[derive(Clone, Default)]
struct WakePool {
    wakers: Arc<Mutex<Vec<(WindowToken, Waker)>>>,
}

impl WakePool {
    fn add(&self, token: WindowToken, wake: Waker) {
        self.wakers.lock().unwrap_or_else(|e| e.into_inner()).push((token, wake));
    }

    fn remove(&self, token: WindowToken) {
        self.wakers.lock().unwrap_or_else(|e| e.into_inner()).retain(|(t, _)| *t != token);
    }

    fn wake(&self) {
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

struct WaylandWatch {
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
    fn spawn(conn: Connection, wake: WakePool) -> Option<Self> {
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

    fn turned(&self) {
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
    fn cancel_and_join(mut self) {
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
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, u32> for QueueState {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wl_seat::Event,
        _: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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
                let target = state.windows.token(surface_id);
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

fn destroy_globals(conn: &Connection, globals: GlobalList) {
    let registry_id = globals.registry().id();
    globals.destroy();
    conn.backend().destroy_object(&registry_id).ok();
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

    /// Pumps Wayland events and worker replies; returns `true` if the display connection failed.
    fn pump(&mut self) -> bool {
        let dispatch_err = self.queue.dispatch_pending(&mut self.state).is_err();
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
