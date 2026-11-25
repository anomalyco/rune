//! Shared per-seat Wayland input serial tracker on winit's `wl_display` without patching winit.
//!
//! Captures pointer button press/release, touch-down, and keyboard key press/release serials
//! per `wl_seat` on a single separate non-watched Wayland `Connection` (so pointer motion never
//! wakes `WaylandWatch` or `set_wake_on_wayland_read`).
//!
//! - Drag-out (`drag_source`) queries press-only serials matched to the origin `wl_surface`
//!   via [`SerialState::latest_for_surface`] / [`wire::latest_press`].
//! - Selection protocols (e.g. `zwp_primary_selection_v1`) can query the latest pointer,
//!   keyboard, or touch serial via [`SerialState::latest_input`].
//! - Every reader enforces the single dispatch-timing invariant: call non-blocking
//!   `queue.dispatch_pending()` on the UI thread immediately before reading the serial so
//!   already-read events left in the private queue after winit's read are never missed.

use std::collections::HashMap;

use crate::platform::wayland::drop::machine::SurfaceId;

/// Input device kind that produced a Wayland serial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputKind {
    Pointer,
    Touch,
    Keyboard,
}

/// Captured input serial with its owning `wl_seat` global name and device kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CapturedSerial {
    pub seat: u32,
    pub serial: u32,
    pub kind: InputKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PressEntry {
    surface: Option<SurfaceId>,
    serial: u32,
    order: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InputEntry {
    serial: u32,
    kind: InputKind,
    order: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SeatSerials {
    has_pointer: bool,
    has_touch: bool,
    has_keyboard: bool,
    pointer_focus: Option<SurfaceId>,
    pointer_press: Option<PressEntry>,
    /// Active touch points keyed by Wayland touch `id` -> `(surface, serial, order)`.
    touches: HashMap<i32, (SurfaceId, u32, u64)>,
    last_touch_press: Option<PressEntry>,
    /// Latest input serial of any kind (pointer press/release, keyboard press/release, touch down).
    last_input: Option<InputEntry>,
}

/// Pure per-display seat capability & serial state machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SerialState {
    seats: HashMap<u32, SeatSerials>,
    next_order: u64,
}

/// Inputs driving [`SerialState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SerialEvent {
    SeatAdded { seat: u32 },
    SeatRemoved { seat: u32 },
    Capabilities { seat: u32, pointer: bool, touch: bool, keyboard: bool },
    PointerEnter { seat: u32, surface: SurfaceId },
    PointerLeave { seat: u32, surface: SurfaceId },
    PointerButton { seat: u32, serial: u32, pressed: bool },
    TouchDown { seat: u32, surface: SurfaceId, id: i32, serial: u32 },
    TouchUp { seat: u32, id: i32 },
    TouchCancel { seat: u32 },
    KeyboardKey { seat: u32, serial: u32, pressed: bool },
}

/// Proxy lifecycle actions emitted by [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SerialEffect {
    GetPointer { seat: u32 },
    ReleasePointer { seat: u32 },
    GetTouch { seat: u32 },
    ReleaseTouch { seat: u32 },
    GetKeyboard { seat: u32 },
    ReleaseKeyboard { seat: u32 },
}

impl SerialState {
    fn bump_order(&mut self) -> u64 {
        self.next_order = self.next_order.wrapping_add(1);
        self.next_order
    }

    /// Returns the most recent press/touch-down serial matching `surface` for
    /// `wl_data_device.start_drag` (never returning key or button-release serials).
    pub(crate) fn latest_for_surface(&self, surface: SurfaceId) -> Option<CapturedSerial> {
        let mut exact: Option<(u64, CapturedSerial)> = None;
        let mut fallback: Option<(u64, CapturedSerial)> = None;
        let consider = |slot: &mut Option<(u64, CapturedSerial)>, order: u64, cap: CapturedSerial| {
            if slot.is_none_or(|(best, _)| order > best) {
                *slot = Some((order, cap));
            }
        };
        for (&seat, info) in &self.seats {
            if let Some(p) = info.pointer_press {
                let cap = CapturedSerial { seat, serial: p.serial, kind: InputKind::Pointer };
                if p.surface == Some(surface) {
                    consider(&mut exact, p.order, cap);
                } else if p.surface.is_none() {
                    consider(&mut fallback, p.order, cap);
                }
            }
            if let Some(t) = info.last_touch_press {
                let cap = CapturedSerial { seat, serial: t.serial, kind: InputKind::Touch };
                if t.surface == Some(surface) {
                    consider(&mut exact, t.order, cap);
                }
            }
        }
        exact.or(fallback).map(|(_, cap)| cap)
    }

    /// Returns the most recent input serial across all seats (pointer press/release, keyboard
    /// press/release, or touch down), for selection requests such as `set_selection`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn latest_input(&self) -> Option<CapturedSerial> {
        let mut best: Option<(u64, CapturedSerial)> = None;
        for (&seat, info) in &self.seats {
            if let Some(entry) = info.last_input {
                let cap = CapturedSerial { seat, serial: entry.serial, kind: entry.kind };
                if best.is_none_or(|(b, _)| entry.order > b) {
                    best = Some((entry.order, cap));
                }
            }
        }
        best.map(|(_, cap)| cap)
    }

    /// Drains already-read buffered events from `pending` into `self` without reading the socket
    /// or blocking, then returns the latest drag-start press serial for `surface`.
    #[cfg(test)]
    pub(crate) fn drain_pending_and_read(
        &mut self,
        surface: SurfaceId,
        pending: impl IntoIterator<Item = SerialEvent>,
    ) -> Option<CapturedSerial> {
        for ev in pending {
            let _ = step(self, ev);
        }
        self.latest_for_surface(surface)
    }
}

/// Advances `state` by `event` and returns proxy creation/release effects.
pub(crate) fn step(state: &mut SerialState, event: SerialEvent) -> Vec<SerialEffect> {
    let mut effects = Vec::new();
    match event {
        SerialEvent::SeatAdded { seat } => {
            state.seats.entry(seat).or_default();
        }
        SerialEvent::SeatRemoved { seat } => {
            if let Some(info) = state.seats.remove(&seat) {
                if info.has_pointer {
                    effects.push(SerialEffect::ReleasePointer { seat });
                }
                if info.has_touch {
                    effects.push(SerialEffect::ReleaseTouch { seat });
                }
                if info.has_keyboard {
                    effects.push(SerialEffect::ReleaseKeyboard { seat });
                }
            }
        }
        SerialEvent::Capabilities { seat, pointer, touch, keyboard } => {
            let info = state.seats.entry(seat).or_default();
            if pointer && !info.has_pointer {
                info.has_pointer = true;
                effects.push(SerialEffect::GetPointer { seat });
            } else if !pointer && info.has_pointer {
                info.has_pointer = false;
                info.pointer_focus = None;
                info.pointer_press = None;
                if info.last_input.is_some_and(|e| e.kind == InputKind::Pointer) {
                    info.last_input = None;
                }
                effects.push(SerialEffect::ReleasePointer { seat });
            }
            if touch && !info.has_touch {
                info.has_touch = true;
                effects.push(SerialEffect::GetTouch { seat });
            } else if !touch && info.has_touch {
                info.has_touch = false;
                info.touches.clear();
                info.last_touch_press = None;
                if info.last_input.is_some_and(|e| e.kind == InputKind::Touch) {
                    info.last_input = None;
                }
                effects.push(SerialEffect::ReleaseTouch { seat });
            }
            if keyboard && !info.has_keyboard {
                info.has_keyboard = true;
                effects.push(SerialEffect::GetKeyboard { seat });
            } else if !keyboard && info.has_keyboard {
                info.has_keyboard = false;
                if info.last_input.is_some_and(|e| e.kind == InputKind::Keyboard) {
                    info.last_input = None;
                }
                effects.push(SerialEffect::ReleaseKeyboard { seat });
            }
        }
        SerialEvent::PointerEnter { seat, surface } => {
            let info = state.seats.entry(seat).or_default();
            info.pointer_focus = Some(surface);
        }
        SerialEvent::PointerLeave { seat, surface } => {
            if let Some(info) = state.seats.get_mut(&seat)
                && info.pointer_focus == Some(surface)
            {
                info.pointer_focus = None;
            }
        }
        SerialEvent::PointerButton { seat, serial, pressed } => {
            let order = state.bump_order();
            let info = state.seats.entry(seat).or_default();
            info.last_input = Some(InputEntry { serial, kind: InputKind::Pointer, order });
            if pressed {
                info.pointer_press = Some(PressEntry {
                    surface: info.pointer_focus,
                    serial,
                    order,
                });
            }
        }
        SerialEvent::TouchDown { seat, surface, id, serial } => {
            let order = state.bump_order();
            let info = state.seats.entry(seat).or_default();
            info.touches.insert(id, (surface, serial, order));
            info.last_touch_press = Some(PressEntry { surface: Some(surface), serial, order });
            info.last_input = Some(InputEntry { serial, kind: InputKind::Touch, order });
        }
        SerialEvent::TouchUp { seat, id } => {
            if let Some(info) = state.seats.get_mut(&seat) {
                info.touches.remove(&id);
            }
        }
        SerialEvent::TouchCancel { seat } => {
            if let Some(info) = state.seats.get_mut(&seat) {
                info.touches.clear();
            }
        }
        SerialEvent::KeyboardKey { seat, serial, pressed: _ } => {
            let order = state.bump_order();
            let info = state.seats.entry(seat).or_default();
            info.last_input = Some(InputEntry { serial, kind: InputKind::Keyboard, order });
        }
    }
    effects
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod wire {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use wayland_backend::client::Backend;
    use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
    use wayland_client::protocol::wl_pointer::{self, WlPointer};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
    use wayland_client::protocol::wl_touch::{self, WlTouch};
    use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum};

    use super::{CapturedSerial, SerialEffect, SerialEvent, SerialState, step};
    use crate::platform::wayland::drop::machine::SurfaceId;
    use crate::platform::wayland::drop::watch::{destroy_globals, wayland_ids};

    thread_local! {
        /// One unwatched serial listener per `wl_display`, ref-counted by active guards.
        static LISTENERS: RefCell<HashMap<usize, SerialListener>> = RefCell::new(HashMap::new());
    }

    /// RAII guard keeping the per-display serial listener alive while at least one subsystem
    /// (e.g. `drag_out` or primary selection) needs seat input serials.
    pub(crate) struct SerialGuard {
        display: usize,
    }

    impl Drop for SerialGuard {
        fn drop(&mut self) {
            LISTENERS.try_with(|map| {
                let mut map = map.borrow_mut();
                let remove = if let Some(listener) = map.get_mut(&self.display) {
                    listener.refs = listener.refs.saturating_sub(1);
                    listener.refs == 0
                } else {
                    false
                };
                if remove {
                    map.remove(&self.display);
                }
            }).ok();
        }
    }

    /// Ensures the unwatched serial listener exists for `window`'s `wl_display` and increments
    /// its ref count. Returns `None` off Wayland or if registry initialization fails.
    pub(crate) fn attach_window(window: &winit::window::Window) -> Option<SerialGuard> {
        let (display_ptr, _) = wayland_ids(window)?;
        attach(display_ptr)
    }

    /// Ensures the unwatched serial listener exists for `display_ptr` and increments its ref
    /// count. Returns `None` if registry initialization fails.
    pub(crate) fn attach(display_ptr: usize) -> Option<SerialGuard> {
        LISTENERS.with(|map| {
            let mut map = map.borrow_mut();
            if !map.contains_key(&display_ptr) {
                let listener = SerialListener::new(display_ptr as *mut std::ffi::c_void)?;
                map.insert(display_ptr, listener);
            }
            let listener = map.get_mut(&display_ptr)?;
            listener.refs += 1;
            Some(SerialGuard { display: display_ptr })
        })
    }

    /// Non-blocking drain of the unwatched serial event queue for `window`'s display.
    pub(crate) fn drain_window(window: &winit::window::Window) {
        let Some((display_ptr, _)) = wayland_ids(window) else { return };
        drain_display(display_ptr);
    }

    /// Non-blocking drain of the unwatched serial event queue for `display_ptr`.
    pub(crate) fn drain_display(display_ptr: usize) {
        LISTENERS.try_with(|map| {
            let mut map = map.borrow_mut();
            if let Some(listener) = map.get_mut(&display_ptr) {
                listener.drain_pending();
            }
        }).ok();
    }

    /// Dispatches already-read events sitting in the private serial queue (non-blocking
    /// `dispatch_pending`, never reading the socket or blocking) and returns the latest press or
    /// touch-down serial for `surface`.
    pub(crate) fn latest_press(display_ptr: usize, surface: SurfaceId) -> Option<CapturedSerial> {
        LISTENERS.try_with(|map| {
            let mut map = map.borrow_mut();
            let listener = map.get_mut(&display_ptr)?;
            listener.drain_pending();
            listener.state.tracker.latest_for_surface(surface)
        }).ok().flatten()
    }

    /// Dispatches already-read events sitting in the private serial queue (non-blocking
    /// `dispatch_pending`) and returns the latest pointer, keyboard, or touch serial.
    #[allow(dead_code)]
    pub(crate) fn latest_input(display_ptr: usize) -> Option<CapturedSerial> {
        LISTENERS.try_with(|map| {
            let mut map = map.borrow_mut();
            let listener = map.get_mut(&display_ptr)?;
            listener.drain_pending();
            listener.state.tracker.latest_input()
        }).ok().flatten()
    }

    /// Tears down all remaining serial listeners on event-loop exit.
    pub(crate) fn shutdown() {
        LISTENERS.try_with(|map| {
            map.borrow_mut().clear();
        }).ok();
    }

    struct SeatProxies {
        seat: WlSeat,
        pointer: Option<WlPointer>,
        touch: Option<WlTouch>,
        keyboard: Option<WlKeyboard>,
    }

    struct QueueState {
        seats: HashMap<u32, SeatProxies>,
        tracker: SerialState,
        needs_flush: bool,
        tearing_down: bool,
    }

    impl QueueState {
        fn new() -> Self {
            Self {
                seats: HashMap::new(),
                tracker: SerialState::default(),
                needs_flush: false,
                tearing_down: false,
            }
        }

        fn bind_seat(&mut self, registry: &WlRegistry, qh: &QueueHandle<Self>, name: u32, version: u32) {
            if self.tearing_down || self.seats.contains_key(&name) {
                return;
            }
            let seat = registry.bind::<WlSeat, _, _>(name, version.min(5), qh, name);
            self.seats.insert(name, SeatProxies { seat, pointer: None, touch: None, keyboard: None });
            let _ = step(&mut self.tracker, SerialEvent::SeatAdded { seat: name });
            self.needs_flush = true;
        }

        fn apply_effects(&mut self, effects: Vec<SerialEffect>, conn: &Connection, qh: &QueueHandle<Self>) {
            let backend = conn.backend();
            for effect in effects {
                match effect {
                    SerialEffect::GetPointer { seat } => {
                        if !self.tearing_down && let Some(p) = self.seats.get_mut(&seat) && p.pointer.is_none() {
                            p.pointer = Some(p.seat.get_pointer(qh, seat));
                            self.needs_flush = true;
                        }
                    }
                    SerialEffect::ReleasePointer { seat } => {
                        if let Some(p) = self.seats.get_mut(&seat) && let Some(ptr) = p.pointer.take() {
                            if ptr.version() >= 3 {
                                ptr.release();
                            } else {
                                backend.destroy_object(&ptr.id()).ok();
                            }
                            self.needs_flush = true;
                        }
                    }
                    SerialEffect::GetTouch { seat } => {
                        if !self.tearing_down && let Some(p) = self.seats.get_mut(&seat) && p.touch.is_none() {
                            p.touch = Some(p.seat.get_touch(qh, seat));
                            self.needs_flush = true;
                        }
                    }
                    SerialEffect::ReleaseTouch { seat } => {
                        if let Some(p) = self.seats.get_mut(&seat) && let Some(t) = p.touch.take() {
                            if t.version() >= 3 {
                                t.release();
                            } else {
                                backend.destroy_object(&t.id()).ok();
                            }
                            self.needs_flush = true;
                        }
                    }
                    SerialEffect::GetKeyboard { seat } => {
                        if !self.tearing_down && let Some(p) = self.seats.get_mut(&seat) && p.keyboard.is_none() {
                            p.keyboard = Some(p.seat.get_keyboard(qh, seat));
                            self.needs_flush = true;
                        }
                    }
                    SerialEffect::ReleaseKeyboard { seat } => {
                        if let Some(p) = self.seats.get_mut(&seat) && let Some(kb) = p.keyboard.take() {
                            if kb.version() >= 3 {
                                kb.release();
                            } else {
                                backend.destroy_object(&kb.id()).ok();
                            }
                            self.needs_flush = true;
                        }
                    }
                }
            }
        }

        fn remove_seat(&mut self, name: u32, conn: &Connection, qh: &QueueHandle<Self>) {
            let effects = step(&mut self.tracker, SerialEvent::SeatRemoved { seat: name });
            self.apply_effects(effects, conn, qh);
            if let Some(proxies) = self.seats.remove(&name) {
                let backend = conn.backend();
                if let Some(ptr) = proxies.pointer {
                    if ptr.version() >= 3 { ptr.release(); } else { backend.destroy_object(&ptr.id()).ok(); }
                }
                if let Some(touch) = proxies.touch {
                    if touch.version() >= 3 { touch.release(); } else { backend.destroy_object(&touch.id()).ok(); }
                }
                if let Some(kb) = proxies.keyboard {
                    if kb.version() >= 3 { kb.release(); } else { backend.destroy_object(&kb.id()).ok(); }
                }
                if proxies.seat.version() >= 5 {
                    proxies.seat.release();
                } else {
                    backend.destroy_object(&proxies.seat.id()).ok();
                }
                self.needs_flush = true;
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
                    state.remove_seat(name, conn, qh);
                }
                _ => {}
            }
        }
    }

    impl Dispatch<WlSeat, u32> for QueueState {
        fn event(
            state: &mut Self,
            _: &WlSeat,
            event: wl_seat::Event,
            seat_name: &u32,
            conn: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if state.tearing_down {
                return;
            }
            if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
                let effects = step(&mut state.tracker, SerialEvent::Capabilities {
                    seat: *seat_name,
                    pointer: caps.contains(Capability::Pointer),
                    touch: caps.contains(Capability::Touch),
                    keyboard: caps.contains(Capability::Keyboard),
                });
                state.apply_effects(effects, conn, qh);
            }
        }
    }

    impl Dispatch<WlPointer, u32> for QueueState {
        fn event(
            state: &mut Self,
            _: &WlPointer,
            event: wl_pointer::Event,
            seat_name: &u32,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if state.tearing_down {
                return;
            }
            let seat = *seat_name;
            match event {
                wl_pointer::Event::Enter { surface, .. } => {
                    let _ = step(&mut state.tracker, SerialEvent::PointerEnter {
                        seat,
                        surface: surface.id().protocol_id(),
                    });
                }
                wl_pointer::Event::Leave { surface, .. } => {
                    let _ = step(&mut state.tracker, SerialEvent::PointerLeave {
                        seat,
                        surface: surface.id().protocol_id(),
                    });
                }
                wl_pointer::Event::Button { serial, state: btn_state, .. } => {
                    let pressed = matches!(btn_state, WEnum::Value(wl_pointer::ButtonState::Pressed));
                    let _ = step(&mut state.tracker, SerialEvent::PointerButton { seat, serial, pressed });
                }
                _ => {}
            }
        }
    }

    impl Dispatch<WlTouch, u32> for QueueState {
        fn event(
            state: &mut Self,
            _: &WlTouch,
            event: wl_touch::Event,
            seat_name: &u32,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if state.tearing_down {
                return;
            }
            let seat = *seat_name;
            match event {
                wl_touch::Event::Down { serial, surface, id, .. } => {
                    let _ = step(&mut state.tracker, SerialEvent::TouchDown {
                        seat,
                        surface: surface.id().protocol_id(),
                        id,
                        serial,
                    });
                }
                wl_touch::Event::Up { id, .. } => {
                    let _ = step(&mut state.tracker, SerialEvent::TouchUp { seat, id });
                }
                wl_touch::Event::Cancel => {
                    let _ = step(&mut state.tracker, SerialEvent::TouchCancel { seat });
                }
                _ => {}
            }
        }
    }

    impl Dispatch<WlKeyboard, u32> for QueueState {
        fn event(
            state: &mut Self,
            _: &WlKeyboard,
            event: wl_keyboard::Event,
            seat_name: &u32,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if state.tearing_down {
                return;
            }
            if let wl_keyboard::Event::Key { serial, state: key_state, .. } = event {
                let pressed = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                let _ = step(&mut state.tracker, SerialEvent::KeyboardKey {
                    seat: *seat_name,
                    serial,
                    pressed,
                });
            }
        }
    }

    fn drain_queue(queue: &mut EventQueue<QueueState>, state: &mut QueueState) {
        loop {
            match queue.dispatch_pending(state) {
                Ok(0) => break,
                Ok(_) | Err(DispatchError::BadMessage { .. }) => {}
                Err(_) => break,
            }
        }
    }

    struct SerialListener {
        conn: Connection,
        globals: Option<GlobalList>,
        queue: EventQueue<QueueState>,
        state: QueueState,
        refs: usize,
    }

    impl SerialListener {
        #[allow(unsafe_code)]
        fn new(display_ptr: *mut std::ffi::c_void) -> Option<Self> {
            // SAFETY: winit owns `wl_display` while windows are open; `SerialGuard`s drop before
            // winit closes `wl_display` and `shutdown()` clears any remaining listeners on exit.
            let backend = unsafe { Backend::from_foreign_display(display_ptr.cast()) };
            let conn = Connection::from_backend(backend);
            let (globals, mut queue) = registry_queue_init::<QueueState>(&conn).ok()?;
            let qh = queue.handle();
            let mut state = QueueState::new();
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
            // One roundtrip at lazy bind so initial seat capabilities arrive and pointer /
            // touch / keyboard listeners are bound before the first user press.
            let _ = queue.roundtrip(&mut state);
            state.needs_flush = false;
            let _ = queue.flush();
            Some(Self { conn, globals: Some(globals), queue, state, refs: 0 })
        }

        fn drain_pending(&mut self) {
            drain_queue(&mut self.queue, &mut self.state);
            if self.state.needs_flush {
                self.state.needs_flush = false;
                let _ = self.queue.flush();
            }
        }
    }

    impl Drop for SerialListener {
        fn drop(&mut self) {
            self.state.tearing_down = true;
            drain_queue(&mut self.queue, &mut self.state);
            let qh = self.queue.handle();
            let seat_names: Vec<u32> = self.state.seats.keys().copied().collect();
            for name in seat_names {
                self.state.remove_seat(name, &self.conn, &qh);
            }
            if let Some(globals) = self.globals.take() {
                destroy_globals(&self.conn, globals);
            }
            drain_queue(&mut self.queue, &mut self.state);
            let _ = self.queue.flush();
        }
    }
}

#[cfg(test)]
#[path = "serial_tests.rs"]
mod tests;
