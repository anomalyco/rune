//! Wayland wire backend for `wp_presentation` (`presentation-time`) on winit's shared
//! `wl_display`.
//!
//! Shares one per-display `DisplayController` across all Wayland windows that registered
//! `App::on_frame_presented`, with zero background threads: `presented` and `discarded` events
//! arrive on the controller's private `EventQueue` and wake winit through `window::wayland_wake`
//! strictly while feedback objects are in flight.

use std::cell::RefCell;
use std::collections::HashMap;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::presentation_time::client::wp_presentation::{
    self, WpPresentation,
};
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};

use super::machine::{SurfacePresentationState, timespec_to_instant};
use crate::time::Instant;

thread_local! {
    /// One shared Wayland presentation controller per `wl_display` pointer, always touched on the
    /// UI thread and torn down before winit closes the display.
    static CONTROLLERS: RefCell<HashMap<usize, DisplayController>> = RefCell::new(HashMap::new());
}

#[allow(unsafe_code)]
fn read_posix_clock(clk_id: u32) -> Option<(i64, i64)> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid out-pointer; `clock_gettime` returns 0 on success and -1 on error.
    if unsafe { libc::clock_gettime(clk_id as libc::clockid_t, &mut ts) } == 0 {
        Some((ts.tv_sec as i64, ts.tv_nsec as i64))
    } else {
        None
    }
}

#[derive(Default)]
struct ControllerState {
    global_name: Option<u32>,
    clock_id: Option<u32>,
    next_surface_id: u64,
    surfaces: HashMap<u64, SurfacePresentationState>,
}

impl Dispatch<WlRegistry, GlobalListContents> for ControllerState {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::GlobalRemove { name } = event
            && state.global_name == Some(name)
        {
            state.global_name = None;
            state.clock_id = None;
        }
    }
}

impl Dispatch<WpPresentation, ()> for ControllerState {
    fn event(
        state: &mut Self,
        _: &WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock_id = Some(clk_id);
        }
    }
}

impl Dispatch<WpPresentationFeedback, (u64, u64)> for ControllerState {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        &(surface_id, seq): &(u64, u64),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::SyncOutput { .. } => {}
            wp_presentation_feedback::Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, .. } => {
                let at = state.clock_id.and_then(|clk_id| {
                    let now = crate::time::now();
                    let (sec, nsec) = read_posix_clock(clk_id)?;
                    timespec_to_instant(now, sec, nsec, tv_sec_hi, tv_sec_lo, tv_nsec)
                });
                if let Some(surface) = state.surfaces.get_mut(&surface_id) {
                    surface.on_presented(seq, at);
                }
            }
            wp_presentation_feedback::Event::Discarded => {
                if let Some(surface) = state.surfaces.get_mut(&surface_id) {
                    surface.on_discarded(seq);
                }
            }
            _ => {}
        }
    }
}

fn destroy_globals(conn: &Connection, globals: GlobalList) {
    let registry_id = globals.registry().id();
    globals.destroy();
    conn.backend().destroy_object(&registry_id).ok();
}

fn drain_queue(queue: &mut EventQueue<ControllerState>, state: &mut ControllerState) -> bool {
    loop {
        match queue.dispatch_pending(state) {
            Ok(0) => return true,
            Ok(_) | Err(DispatchError::BadMessage { .. }) => {}
            Err(_) => return false,
        }
    }
}

struct DisplayController {
    conn: Connection,
    globals: Option<GlobalList>,
    queue: EventQueue<ControllerState>,
    state: ControllerState,
    presentation: Option<WpPresentation>,
    surfaces_attached: usize,
    dead: bool,
}

impl DisplayController {
    #[allow(unsafe_code)]
    fn new(display_ptr: *mut std::ffi::c_void) -> Option<Self> {
        // SAFETY: winit owns this `wl_display` for the life of the event loop, and `shutdown()`
        // clears every controller before the display closes; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display_ptr.cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<ControllerState>(&conn).ok()?;
        let qh = queue.handle();
        let global_name = globals.contents().with_list(|list| {
            list.iter().find(|g| g.interface == "wp_presentation").map(|g| g.name)
        });
        let Some(name) = global_name else {
            destroy_globals(&conn, globals);
            let mut state = ControllerState::default();
            let _ = drain_queue(&mut queue, &mut state);
            queue.flush().ok();
            return None;
        };
        let Ok(presentation) = globals.bind::<WpPresentation, _, _>(&qh, 1..=2, ()) else {
            destroy_globals(&conn, globals);
            let mut state = ControllerState::default();
            let _ = drain_queue(&mut queue, &mut state);
            queue.flush().ok();
            return None;
        };
        let mut state = ControllerState { global_name: Some(name), ..Default::default() };
        // Receive the mandatory `wp_presentation.clock_id` event sent immediately on bind.
        if queue.roundtrip(&mut state).is_err() || state.clock_id.is_none() {
            presentation.destroy();
            destroy_globals(&conn, globals);
            let _ = drain_queue(&mut queue, &mut state);
            queue.flush().ok();
            return None;
        }
        Some(Self {
            conn,
            globals: Some(globals),
            queue,
            state,
            presentation: Some(presentation),
            surfaces_attached: 0,
            dead: false,
        })
    }

    fn dispatch(&mut self) {
        if self.dead {
            return;
        }
        if !drain_queue(&mut self.queue, &mut self.state) || self.conn.backend().last_error().is_some() {
            self.dead = true;
        }
    }

    fn flush(&mut self) {
        if self.dead {
            return;
        }
        if self.queue.flush().is_err() || self.conn.backend().last_error().is_some() {
            self.dead = true;
        }
    }
}

impl Drop for DisplayController {
    fn drop(&mut self) {
        let _ = drain_queue(&mut self.queue, &mut self.state);
        if let Some(pres) = self.presentation.take() {
            pres.destroy();
        }
        if let Some(globals) = self.globals.take() {
            destroy_globals(&self.conn, globals);
        }
        let _ = drain_queue(&mut self.queue, &mut self.state);
        self.queue.flush().ok();
    }
}

/// Per-surface Wayland `wp_presentation` handle attached to one live `wl_surface`.
///
/// Must be dropped strictly before the owning `wl_surface` drops (stored in `Win::native`).
pub(crate) struct SurfacePresentation {
    display: usize,
    surface_id: u64,
    surface: WlSurface,
}

impl SurfacePresentation {
    /// Attaches to a winit `Window`'s `wl_surface`, binding the shared per-display
    /// `wp_presentation` global if supported. Returns `None` off Wayland or when the compositor
    /// does not advertise `wp_presentation`.
    pub(crate) fn for_window(window: &winit::window::Window) -> Option<Self> {
        let display = window.display_handle().ok()?.as_raw();
        let handle = window.window_handle().ok()?.as_raw();
        Self::from_raw_handles(display, handle)
    }

    #[allow(unsafe_code)]
    fn from_raw_handles(display: RawDisplayHandle, window: RawWindowHandle) -> Option<Self> {
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) =
            (display, window)
        else {
            return None;
        };
        let display_ptr = display.display.as_ptr() as usize;
        CONTROLLERS.with(|map| {
            let mut map = map.borrow_mut();
            if !map.contains_key(&display_ptr) {
                let controller = DisplayController::new(display.display.as_ptr().cast())?;
                map.insert(display_ptr, controller);
            }
            let controller = map.get_mut(&display_ptr)?;
            if controller.dead || controller.state.clock_id.is_none() {
                return None;
            }
            // SAFETY: `handle.surface` is a live `wl_surface` owned by winit, and
            // `SurfacePresentation` drops strictly before that window drops.
            let id =
                unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }
                    .ok()?;
            let surface = WlSurface::from_id(&controller.conn, id).ok()?;
            controller.state.next_surface_id = controller.state.next_surface_id.wrapping_add(1).max(1);
            let surface_id = controller.state.next_surface_id;
            controller.state.surfaces.insert(surface_id, SurfacePresentationState::default());
            controller.surfaces_attached += 1;
            Some(Self { display: display_ptr, surface_id, surface })
        })
    }

    /// Requests `wp_presentation_feedback` for the next `wl_surface.commit` (called immediately
    /// before `gpu.queue.present`) and flushes the request onto the shared `wl_display`.
    pub(crate) fn request_feedback(&mut self) -> Option<u64> {
        CONTROLLERS.with(|map| {
            let mut map = map.borrow_mut();
            let controller = map.get_mut(&self.display)?;
            if controller.dead {
                return None;
            }
            let has_clock = controller.state.clock_id.is_some();
            let pres = controller.presentation.as_ref()?;
            let seq = controller
                .state
                .surfaces
                .get_mut(&self.surface_id)?
                .request_feedback(has_clock)?;
            let _feedback = pres.feedback(&self.surface, &controller.queue.handle(), (self.surface_id, seq));
            controller.flush();
            Some(seq)
        })
    }

    /// Dispatches pending presentation events non-blockingly and drains completed timestamps.
    pub(crate) fn poll(&mut self, out: &mut Vec<(u64, Option<Instant>)>) {
        out.clear();
        CONTROLLERS.with(|map| {
            let mut map = map.borrow_mut();
            let Some(controller) = map.get_mut(&self.display) else { return };
            controller.dispatch();
            if let Some(surface) = controller.state.surfaces.get_mut(&self.surface_id) {
                surface.drain(out);
            }
        });
    }

    /// Whether winit should wake on Wayland display reads for this surface (only while feedback
    /// objects are actually in flight).
    pub(crate) fn needs_wake_on_read(&self) -> bool {
        CONTROLLERS.with(|map| {
            let map = map.borrow();
            map.get(&self.display).is_some_and(|c| {
                !c.dead && c.state.surfaces.get(&self.surface_id).is_some_and(SurfacePresentationState::needs_wake)
            })
        })
    }

    /// Whether libwayland already buffered events for our presentation queue.
    pub(crate) fn has_pending_events(&self) -> bool {
        CONTROLLERS.with(|map| {
            let map = map.borrow();
            map.get(&self.display).is_some_and(|c| {
                !c.dead
                    && c.state.surfaces.get(&self.surface_id).is_some_and(SurfacePresentationState::needs_wake)
                    && c.queue.prepare_read().is_none()
            })
        })
    }

    /// Drops in-flight bookkeeping after a surface replacement, hide, or device loss.
    pub(crate) fn reset(&mut self) {
        CONTROLLERS.with(|map| {
            let mut map = map.borrow_mut();
            let Some(controller) = map.get_mut(&self.display) else { return };
            if let Some(surface) = controller.state.surfaces.get_mut(&self.surface_id) {
                surface.reset();
            }
        });
    }
}

impl Drop for SurfacePresentation {
    fn drop(&mut self) {
        CONTROLLERS
            .try_with(|map| {
                let mut map = map.borrow_mut();
                let remove = if let Some(controller) = map.get_mut(&self.display) {
                    controller.state.surfaces.remove(&self.surface_id);
                    controller.surfaces_attached = controller.surfaces_attached.saturating_sub(1);
                    controller.surfaces_attached == 0
                } else {
                    false
                };
                if remove {
                    map.remove(&self.display);
                }
            })
            .ok();
    }
}

/// Tears down any remaining per-display Wayland presentation controllers before the display closes.
pub(crate) fn shutdown() {
    CONTROLLERS
        .try_with(|map| {
            map.borrow_mut().clear();
        })
        .ok();
}
