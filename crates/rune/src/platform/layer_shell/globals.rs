//! Per-display shared Wayland globals for layer-shell overlays.
//!
//! `wl_compositor` has no destructor request in Wayland, and `wl_registry` can only be destroyed on
//! the compositor when `wl_fixes` is advertised (which compositors such as niri do not). Binding a
//! fresh `wl_registry` and `wl_compositor` on every overlay show therefore leaks both objects on
//! the compositor for the life of the process.
//!
//! `SharedGlobals` creates one foreign `Connection`, one `wl_registry`, and one bound proxy per
//! singleton global (`wl_compositor`, `zwlr_layer_shell_v1`, `wp_cursor_shape_manager_v1`,
//! `zwp_text_input_manager_v3`, `xdg_activation_v1`) and per requested `wl_output` for each
//! `wl_display`, sharing them across all `LayerSurface`, `PendingActivationToken`, and overlay
//! holders on that display and tearing them down cleanly once the last holder drops.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use wayland_backend::client::{Backend, WaylandError};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use winit::raw_window_handle::WaylandDisplayHandle;

use super::super::layer_shell_machine::connection;
use super::super::layer_shell_machine::globals_pool::{DisplayPool, DrainTracker};
use super::dead::{classify_turn_error, format_wayland_error};
use super::State;

static POOL: OnceLock<DisplayPool<SharedGlobals>> = OnceLock::new();

fn pool() -> &'static DisplayPool<SharedGlobals> {
    POOL.get_or_init(DisplayPool::new)
}

#[derive(Default)]
struct GlobalsState {
    outputs: HashMap<u32, WlOutput>,
    cursor_shapes: Option<(u32, WpCursorShapeManagerV1)>,
    text_input_manager: Option<(u32, ZwpTextInputManagerV3)>,
    activation: Option<(u32, XdgActivationV1)>,
}

impl Dispatch<WlRegistry, GlobalListContents> for GlobalsState {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::GlobalRemove { name } = event {
            if let Some(output) = state.outputs.remove(&name) {
                release_output(conn, output);
            }
            if state.cursor_shapes.as_ref().is_some_and(|(n, _)| *n == name)
                && let Some((_, shapes)) = state.cursor_shapes.take()
            {
                shapes.destroy();
            }
            if state.text_input_manager.as_ref().is_some_and(|(n, _)| *n == name)
                && let Some((_, tim)) = state.text_input_manager.take()
            {
                tim.destroy();
            }
            if state.activation.as_ref().is_some_and(|(n, _)| *n == name)
                && let Some((_, act)) = state.activation.take()
            {
                act.destroy();
            }
        }
    }
}

impl Dispatch<WlCompositor, ()> for GlobalsState {
    fn event(_: &mut Self, _: &WlCompositor, _: <WlCompositor as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrLayerShellV1, ()> for GlobalsState {
    fn event(_: &mut Self, _: &ZwlrLayerShellV1, _: <ZwlrLayerShellV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpCursorShapeManagerV1, ()> for GlobalsState {
    fn event(_: &mut Self, _: &WpCursorShapeManagerV1, _: <WpCursorShapeManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpTextInputManagerV3, ()> for GlobalsState {
    fn event(_: &mut Self, _: &ZwpTextInputManagerV3, _: <ZwpTextInputManagerV3 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationV1, ()> for GlobalsState {
    fn event(_: &mut Self, _: &XdgActivationV1, _: <XdgActivationV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlOutput, ()> for GlobalsState {
    fn event(_: &mut Self, _: &WlOutput, _: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

fn release_output(conn: &Connection, output: WlOutput) {
    if output.version() >= 3 {
        output.release();
    } else {
        conn.backend().destroy_object(&output.id()).ok();
    }
}

fn destroy_globals(conn: &Connection, globals: GlobalList) {
    let registry_id = globals.registry().id();
    globals.destroy();
    conn.backend().destroy_object(&registry_id).ok();
}

fn drain_queue(queue: &mut EventQueue<GlobalsState>, state: &mut GlobalsState) -> bool {
    let mut any = false;
    loop {
        match queue.dispatch_pending(state) {
            Ok(0) => return any,
            Ok(_) | Err(DispatchError::BadMessage { .. }) => any = true,
            Err(_) => return any,
        }
    }
}

struct GlobalsInner {
    globals: Option<GlobalList>,
    queue: EventQueue<GlobalsState>,
    state: GlobalsState,
}

/// Shared per-display Wayland registry, core layer-shell globals, optional managers, and bound
/// outputs. Held by every live `LayerSurface`, in-flight `PendingActivationToken`, and overlay
/// `SharedGlobalsHandle` on the same `wl_display`.
pub(super) struct SharedGlobals {
    display: usize,
    conn: Connection,
    compositor: WlCompositor,
    shell: ZwlrLayerShellV1,
    drain_tracker: DrainTracker,
    inner: Mutex<GlobalsInner>,
}

/// Keeps a display connection's shared layer-shell globals alive across overlay hide/show cycles
/// without retaining any per-surface Wayland objects.
#[derive(Clone)]
pub(crate) struct SharedGlobalsHandle(#[allow(dead_code)] Arc<SharedGlobals>);

impl SharedGlobals {
    pub(super) fn handle(self: &Arc<Self>) -> SharedGlobalsHandle {
        SharedGlobalsHandle(self.clone())
    }

    /// Acquires the shared globals for `display`, creating them with one `registry_queue_init`
    /// roundtrip on first use and reusing them on every subsequent show on the same `wl_display`.
    pub(super) fn acquire(display: WaylandDisplayHandle, namespace: &str) -> Option<Arc<Self>> {
        let key = display.display.as_ptr() as usize;
        let shared = pool().acquire(
            key,
            |existing| !existing.is_dead(),
            || Self::connect(display, namespace),
        )?;
        let mut inner = shared.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        shared.drain_globals_queue(&mut inner);
        drop(inner);
        if shared.is_dead() {
            if let Some(reason) = shared.conn.backend().last_error().and_then(|e| format_wayland_error(&e)) {
                eprintln!("rune: Wayland layer surface '{namespace}' failed: the display connection failed: {reason}");
            }
            return None;
        }
        Some(shared)
    }

    #[allow(unsafe_code)]
    fn connect(display: WaylandDisplayHandle, namespace: &str) -> Option<Self> {
        // SAFETY: winit owns this `wl_display` for the life of the event loop, which outlives
        // every overlay and shared-globals holder; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = match registry_queue_init::<GlobalsState>(&conn) {
            Ok(pair) => pair,
            Err(_) => {
                if let Some(reason) = conn.backend().last_error().and_then(|e| format_wayland_error(&e)) {
                    eprintln!("rune: Wayland layer surface '{namespace}' failed: the display connection failed: {reason}");
                }
                return None;
            }
        };
        let qh = queue.handle();
        let Ok(compositor) = globals.bind::<WlCompositor, _, _>(&qh, 1..=6, ()) else {
            destroy_globals(&conn, globals);
            drain_queue(&mut queue, &mut GlobalsState::default());
            queue.flush().ok();
            return None;
        };
        let Ok(shell) = globals.bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=4, ()) else {
            conn.backend().destroy_object(&compositor.id()).ok();
            destroy_globals(&conn, globals);
            drain_queue(&mut queue, &mut GlobalsState::default());
            queue.flush().ok();
            return None;
        };
        Some(Self {
            display: display.display.as_ptr() as usize,
            conn,
            compositor,
            shell,
            drain_tracker: DrainTracker::new(),
            inner: Mutex::new(GlobalsInner {
                globals: Some(globals),
                queue,
                state: GlobalsState::default(),
            }),
        })
    }

    pub(super) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(super) fn compositor(&self) -> &WlCompositor {
        &self.compositor
    }

    pub(super) fn shell(&self) -> &ZwlrLayerShellV1 {
        &self.shell
    }

    /// True while `wl_output` global `id` is still advertised and bound in the shared registry.
    pub(super) fn has_output(&self, id: u32) -> bool {
        let inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.state.outputs.contains_key(&id)
    }

    pub(super) fn inner_drain_gen(&self) -> u64 {
        self.drain_tracker.generation()
    }

    pub(super) fn has_pending_events(&self, seen_gen: u64, c_queue_pending: bool) -> bool {
        self.drain_tracker.has_pending(seen_gen, c_queue_pending)
    }

    fn is_dead(&self) -> bool {
        let (kind, _) = classify_turn_error(None, None, self.conn.backend().last_error());
        kind.is_some_and(connection::is_fatal)
    }

    /// Moves any events waiting in the shared libwayland queue into their target Rust
    /// `EventQueue`s, bumps the drain generation so peer queues stay wakeable, and drains any
    /// registry/output events routed to the shared globals queue.
    pub(super) fn dispatch_inner_queue(&self) -> Result<usize, WaylandError> {
        let moved = self.conn.backend().dispatch_inner_queue()?;
        self.drain_tracker.note_inner_drain(moved);
        if moved > 0 && let Ok(mut guard) = self.inner.try_lock() {
            let inner = &mut *guard;
            if drain_queue(&mut inner.queue, &mut inner.state) {
                inner.queue.flush().ok();
            }
        }
        Ok(moved)
    }

    fn drain_globals_queue(&self, inner: &mut GlobalsInner) {
        if let Ok(moved) = self.conn.backend().dispatch_inner_queue() {
            self.drain_tracker.note_inner_drain(moved);
        }
        if drain_queue(&mut inner.queue, &mut inner.state) {
            inner.queue.flush().ok();
        }
    }

    /// Returns the shared `wp_cursor_shape_manager_v1`, binding it at most once while advertised.
    pub(super) fn cursor_shape_manager(&self) -> Option<WpCursorShapeManagerV1> {
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.drain_globals_queue(&mut inner);
        if let Some((_, shapes)) = &inner.state.cursor_shapes {
            return Some(shapes.clone());
        }
        let globals = inner.globals.as_ref()?;
        let found = globals.contents().with_list(|list| {
            list.iter()
                .find(|g| g.interface == "wp_cursor_shape_manager_v1" && g.version >= 1)
                .map(|g| (g.name, g.version.min(1)))
        })?;
        let bound = globals
            .registry()
            .bind::<WpCursorShapeManagerV1, _, _>(found.0, found.1, &inner.queue.handle(), ());
        inner.state.cursor_shapes = Some((found.0, bound.clone()));
        Some(bound)
    }

    /// Returns the shared `zwp_text_input_manager_v3`, binding it at most once while advertised.
    pub(super) fn text_input_manager(&self) -> Option<ZwpTextInputManagerV3> {
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.drain_globals_queue(&mut inner);
        if let Some((_, manager)) = &inner.state.text_input_manager {
            return Some(manager.clone());
        }
        let globals = inner.globals.as_ref()?;
        let found = globals.contents().with_list(|list| {
            list.iter()
                .find(|g| g.interface == "zwp_text_input_manager_v3" && g.version >= 1)
                .map(|g| (g.name, g.version.min(1)))
        })?;
        let bound = globals
            .registry()
            .bind::<ZwpTextInputManagerV3, _, _>(found.0, found.1, &inner.queue.handle(), ());
        inner.state.text_input_manager = Some((found.0, bound.clone()));
        Some(bound)
    }

    /// Returns the shared `xdg_activation_v1`, binding it at most once while advertised.
    pub(super) fn activation(&self) -> Option<XdgActivationV1> {
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.drain_globals_queue(&mut inner);
        if let Some((_, activation)) = &inner.state.activation {
            return Some(activation.clone());
        }
        let globals = inner.globals.as_ref()?;
        let found = globals.contents().with_list(|list| {
            list.iter()
                .find(|g| g.interface == "xdg_activation_v1" && g.version >= 1)
                .map(|g| (g.name, g.version.min(1)))
        })?;
        let bound = globals
            .registry()
            .bind::<XdgActivationV1, _, _>(found.0, found.1, &inner.queue.handle(), ());
        inner.state.activation = Some((found.0, bound.clone()));
        Some(bound)
    }

    /// Returns the shared `wl_output` proxy for global name `id`, binding it at most once while
    /// advertised and returning `None` if the output was removed.
    pub(super) fn output(&self, id: u32) -> Option<WlOutput> {
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.drain_globals_queue(&mut inner);
        let version = inner.globals.as_ref()?.contents().with_list(|list| {
            list.iter()
                .find(|g| g.name == id && g.interface == "wl_output")
                .map(|g| g.version.min(4))
        });
        let Some(version) = version else {
            if let Some(stale) = inner.state.outputs.remove(&id) {
                release_output(&self.conn, stale);
                inner.queue.flush().ok();
            }
            return None;
        };
        if let Some(existing) = inner.state.outputs.get(&id) {
            return Some(existing.clone());
        }
        let globals = inner.globals.as_ref()?;
        let bound = globals
            .registry()
            .bind::<WlOutput, _, _>(id, version, &inner.queue.handle(), ());
        inner.state.outputs.insert(id, bound.clone());
        Some(bound)
    }

    /// Binds a per-surface `wl_seat` on the caller's `EventQueue` (released on surface drop).
    pub(super) fn bind_seat(&self, qh: &QueueHandle<State>) -> Option<WlSeat> {
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.drain_globals_queue(&mut inner);
        let globals = inner.globals.as_ref()?;
        globals.contents().with_list(|list| {
            list.iter()
                .find(|g| g.interface == "wl_seat")
                .map(|g| globals.registry().bind::<WlSeat, _, _>(g.name, g.version.min(8), qh, ()))
        })
    }

    #[cfg(test)]
    pub(super) fn registry_id(&self) -> wayland_backend::client::ObjectId {
        let inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.globals.as_ref().expect("live globals").registry().id()
    }

    #[cfg(test)]
    pub(super) fn registry_object_data(&self) -> Option<std::sync::Weak<dyn wayland_backend::client::ObjectData>> {
        let inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.globals.as_ref()?.registry().object_data().map(Arc::downgrade)
    }

    #[cfg(test)]
    pub(super) fn optional_object_data(&self) -> Vec<(&'static str, std::sync::Weak<dyn wayland_backend::client::ObjectData>)> {
        let inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::new();
        if let Some((_, p)) = &inner.state.cursor_shapes && let Some(d) = p.object_data() {
            out.push(("cursor_shape_manager", Arc::downgrade(d)));
        }
        if let Some((_, p)) = &inner.state.text_input_manager && let Some(d) = p.object_data() {
            out.push(("text_input_manager", Arc::downgrade(d)));
        }
        if let Some((_, p)) = &inner.state.activation && let Some(d) = p.object_data() {
            out.push(("xdg_activation", Arc::downgrade(d)));
        }
        for output in inner.state.outputs.values() {
            if let Some(d) = output.object_data() {
                out.push(("wl_output", Arc::downgrade(d)));
            }
        }
        out
    }
}

impl Drop for SharedGlobals {
    fn drop(&mut self) {
        pool().release(self.display, self);
        let inner = self.inner.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = self.conn.backend().dispatch_inner_queue();
        drain_queue(&mut inner.queue, &mut inner.state);
        let backend = self.conn.backend();
        if let Some((_, tim)) = inner.state.text_input_manager.take() {
            tim.destroy();
        }
        if let Some((_, shapes)) = inner.state.cursor_shapes.take() {
            shapes.destroy();
        }
        if let Some((_, act)) = inner.state.activation.take() {
            act.destroy();
        }
        for (_, output) in inner.state.outputs.drain() {
            release_output(&self.conn, output);
        }
        if self.shell.version() >= 3 {
            self.shell.destroy();
        } else {
            backend.destroy_object(&self.shell.id()).ok();
        }
        backend.destroy_object(&self.compositor.id()).ok();
        if let Some(globals) = inner.globals.take() {
            destroy_globals(&self.conn, globals);
        }
        let _ = self.conn.backend().dispatch_inner_queue();
        drain_queue(&mut inner.queue, &mut inner.state);
        inner.queue.flush().ok();
    }
}

#[cfg(test)]
#[path = "globals_tests.rs"]
mod tests;
