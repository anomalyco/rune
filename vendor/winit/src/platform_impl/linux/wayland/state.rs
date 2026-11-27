use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use ahash::AHashMap;

use sctk::reexports::calloop::LoopHandle;
use sctk::reexports::client::backend::ObjectId;
use sctk::reexports::client::globals::GlobalList;
use sctk::reexports::client::protocol::wl_output::WlOutput;
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{Connection, Proxy, QueueHandle};

use sctk::compositor::{CompositorHandler, CompositorState};
use sctk::output::{OutputHandler, OutputState};
use sctk::registry::{ProvidesRegistryState, RegistryState};
use sctk::seat::pointer::ThemedPointer;
use sctk::seat::SeatState;
use sctk::shell::xdg::window::{Window, WindowConfigure, WindowHandler};
use sctk::shell::xdg::XdgShell;
use sctk::shell::WaylandSurface;
use sctk::shm::slot::SlotPool;
use sctk::shm::{Shm, ShmHandler};
use sctk::subcompositor::SubcompositorState;

use crate::platform_impl::wayland::event_loop::sink::EventSink;
use crate::platform_impl::wayland::output::MonitorHandle;
use crate::platform_impl::wayland::seat::{
    PointerConstraintsState, RelativePointerState, TextInputState, WinitPointerData,
    WinitPointerDataExt, WinitSeatState,
};
use crate::platform_impl::wayland::types::kwin_blur::KWinBlurManager;
use crate::platform_impl::wayland::types::wp_fractional_scaling::FractionalScalingManager;
use crate::platform_impl::wayland::types::wp_viewporter::ViewporterState;
use crate::platform_impl::wayland::types::xdg_activation::XdgActivationState;
use crate::platform_impl::wayland::types::xdg_toplevel_icon::XdgToplevelIconManagerState;
use crate::platform_impl::wayland::window::{WindowRequests, WindowState};
use crate::platform_impl::wayland::{WaylandError, WindowId};
use crate::platform_impl::OsError;

/// Winit's Wayland state.
pub struct WinitState {
    /// The WlRegistry.
    pub registry_state: RegistryState,

    /// The state of the WlOutput handling.
    pub output_state: OutputState,

    /// The compositor state which is used to create new windows and regions.
    pub compositor_state: Arc<CompositorState>,

    /// The state of the subcompositor.
    pub subcompositor_state: Option<Arc<SubcompositorState>>,

    /// The seat state responsible for all sorts of input.
    pub seat_state: SeatState,

    /// The shm for software buffers, such as cursors.
    pub shm: Shm,

    /// The pool where custom cursors are allocated.
    pub custom_cursor_pool: Arc<Mutex<SlotPool>>,

    /// The XDG shell that is used for windows.
    pub xdg_shell: XdgShell,

    /// The currently present windows.
    pub windows: RefCell<AHashMap<WindowId, Arc<Mutex<WindowState>>>>,

    /// The requests from the `Window` to EventLoop, such as close operations and redraw requests.
    pub window_requests: RefCell<AHashMap<WindowId, Arc<WindowRequests>>>,

    /// The events that were generated directly from the window.
    pub window_events_sink: Arc<Mutex<EventSink>>,

    /// The update for the `windows` coming from the compositor.
    pub window_compositor_updates: Vec<WindowCompositorUpdate>,

    /// Currently handled seats.
    pub seats: AHashMap<ObjectId, WinitSeatState>,

    /// Currently present cursor surfaces.
    pub pointer_surfaces: AHashMap<ObjectId, Arc<ThemedPointer<WinitPointerData>>>,

    /// The state of the text input on the client.
    pub text_input_state: Option<TextInputState>,

    /// Observed monitors.
    pub monitors: Arc<Mutex<Vec<MonitorHandle>>>,

    /// Sink to accumulate window events from the compositor, which is latter dispatched in
    /// event loop run.
    pub events_sink: EventSink,

    /// Xdg activation.
    pub xdg_activation: Option<XdgActivationState>,

    /// Xdg toplevel icon manager, for window icons.
    pub xdg_toplevel_icon_manager: Option<XdgToplevelIconManagerState>,

    /// Relative pointer.
    pub relative_pointer: Option<RelativePointerState>,

    /// Pointer constraints to handle pointer locking and confining.
    pub pointer_constraints: Option<Arc<PointerConstraintsState>>,

    /// Viewporter state on the given window.
    pub viewporter_state: Option<ViewporterState>,

    /// Fractional scaling manager.
    pub fractional_scaling_manager: Option<FractionalScalingManager>,

    /// KWin blur manager.
    pub kwin_blur_manager: Option<KWinBlurManager>,

    /// Loop handle to re-register event sources, such as keyboard repeat.
    pub loop_handle: LoopHandle<'static, Self>,

    /// Whether we have dispatched events to the user thus we want to
    /// send `AboutToWait` and normally wakeup the user.
    pub dispatched_events: bool,

    /// Rune: when true, any successful Wayland display read marks `dispatched_events = true`
    /// even when winit's own event sinks are empty, so foreign queues sharing the display
    /// dispatch on that turn without background reader threads.
    pub wake_on_wayland_read: bool,
}

impl WinitState {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<Self>,
        loop_handle: LoopHandle<'static, WinitState>,
    ) -> Result<Self, OsError> {
        // A compositor without the decoration protocol answers a decorated window's first
        // configure with a client-side frame, and that frame reads the system's colour scheme and
        // button layout through two `dbus-send` processes, 100 ms of reply timeout each. Started
        // here, on threads of their own, they are done (or well on their way) when the configure
        // asks, instead of running one after the other on this thread before the first frame.
        #[cfg(feature = "sctk-adwaita")]
        if !globals
            .contents()
            .with_list(|list| list.iter().any(|global| global.interface == "zxdg_decoration_manager_v1"))
        {
            sctk_adwaita::read_system_config_early();
        }

        let registry_state = RegistryState::new(globals);
        let compositor_state =
            CompositorState::bind(globals, queue_handle).map_err(WaylandError::Bind)?;
        let subcompositor_state = match SubcompositorState::bind(
            compositor_state.wl_compositor().clone(),
            globals,
            queue_handle,
        ) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("Subcompositor protocol not available, ignoring CSD: {e:?}");
                None
            },
        };

        let output_state = OutputState::new(globals, queue_handle);
        let mut monitors = Vec::new();
        for output in output_state.outputs() {
            upsert_monitor(&mut monitors, MonitorHandle::new(output), same_monitor);
        }

        let seat_state = SeatState::new(globals, queue_handle);

        let mut seats = AHashMap::default();
        for seat in seat_state.seats() {
            seats.insert(seat.id(), WinitSeatState::new());
        }

        let (viewporter_state, fractional_scaling_manager) =
            if let Ok(fsm) = FractionalScalingManager::new(globals, queue_handle) {
                (ViewporterState::new(globals, queue_handle).ok(), Some(fsm))
            } else {
                (None, None)
            };

        let shm = Shm::bind(globals, queue_handle).map_err(WaylandError::Bind)?;
        let custom_cursor_pool = Arc::new(Mutex::new(SlotPool::new(2, &shm).unwrap()));

        Ok(Self {
            registry_state,
            compositor_state: Arc::new(compositor_state),
            subcompositor_state: subcompositor_state.map(Arc::new),
            output_state,
            seat_state,
            shm,
            custom_cursor_pool,

            xdg_shell: XdgShell::bind(globals, queue_handle).map_err(WaylandError::Bind)?,
            xdg_activation: XdgActivationState::bind(globals, queue_handle).ok(),
            xdg_toplevel_icon_manager: XdgToplevelIconManagerState::bind(globals, queue_handle)
                .ok(),

            windows: Default::default(),
            window_requests: Default::default(),
            window_compositor_updates: Vec::new(),
            window_events_sink: Default::default(),
            viewporter_state,
            fractional_scaling_manager,
            kwin_blur_manager: KWinBlurManager::new(globals, queue_handle).ok(),

            seats,
            text_input_state: TextInputState::new(globals, queue_handle).ok(),

            relative_pointer: RelativePointerState::new(globals, queue_handle).ok(),
            pointer_constraints: PointerConstraintsState::new(globals, queue_handle)
                .map(Arc::new)
                .ok(),
            pointer_surfaces: Default::default(),

            monitors: Arc::new(Mutex::new(monitors)),
            events_sink: EventSink::new(),
            loop_handle,
            // Make it true by default.
            dispatched_events: true,
            wake_on_wayland_read: false,
        })
    }

    pub fn scale_factor_changed(
        &mut self,
        surface: &WlSurface,
        scale_factor: f64,
        is_legacy: bool,
    ) {
        // Check if the cursor surface.
        let window_id = super::make_wid(surface);

        if let Some(window) = self.windows.get_mut().get(&window_id) {
            // Don't update the scaling factor, when legacy method is used.
            if is_legacy && self.fractional_scaling_manager.is_some() {
                return;
            }

            // The scale factor change is for the window.
            let pos = if let Some(pos) = self
                .window_compositor_updates
                .iter()
                .position(|update| update.window_id == window_id)
            {
                pos
            } else {
                self.window_compositor_updates.push(WindowCompositorUpdate::new(window_id));
                self.window_compositor_updates.len() - 1
            };

            // Update the scale factor right away.
            window.lock().unwrap().set_scale_factor(scale_factor);
            self.window_compositor_updates[pos].scale_changed = true;
        } else if let Some(pointer) = self.pointer_surfaces.get(&surface.id()) {
            // Get the window, where the pointer resides right now.
            let focused_window = match pointer.pointer().winit_data().focused_window() {
                Some(focused_window) => focused_window,
                None => return,
            };

            if let Some(window_state) = self.windows.get_mut().get(&focused_window) {
                window_state.lock().unwrap().reload_cursor_style()
            }
        }
    }

    pub fn queue_close(updates: &mut Vec<WindowCompositorUpdate>, window_id: WindowId) {
        let pos = if let Some(pos) = updates.iter().position(|update| update.window_id == window_id)
        {
            pos
        } else {
            updates.push(WindowCompositorUpdate::new(window_id));
            updates.len() - 1
        };

        updates[pos].close_window = true;
    }
}

impl ShmHandler for WinitState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl WindowHandler for WinitState {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window) {
        let window_id = super::make_wid(window.wl_surface());
        Self::queue_close(&mut self.window_compositor_updates, window_id);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let window_id = super::make_wid(window.wl_surface());

        let pos = if let Some(pos) =
            self.window_compositor_updates.iter().position(|update| update.window_id == window_id)
        {
            pos
        } else {
            self.window_compositor_updates.push(WindowCompositorUpdate::new(window_id));
            self.window_compositor_updates.len() - 1
        };

        // Populate the configure to the window.
        self.window_compositor_updates[pos].resized |= self
            .windows
            .get_mut()
            .get_mut(&window_id)
            .expect("got configure for dead window.")
            .lock()
            .unwrap()
            .configure(configure, &self.shm, &self.subcompositor_state);

        // NOTE: configure demands wl_surface::commit, however winit doesn't commit on behalf of the
        // users, since it can break a lot of things, thus it'll ask users to redraw instead.
        self.window_requests
            .get_mut()
            .get(&window_id)
            .unwrap()
            .redraw_requested
            .store(true, Ordering::Relaxed);

        // Manually mark that we've got an event, since configure may not generate a resize.
        self.dispatched_events = true;
    }
}

impl OutputHandler for WinitState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        let mut monitors = self.monitors.lock().unwrap();
        let output = MonitorHandle::new(output);
        // Rune: `WinitState::new` seeds `monitors` from `output_state.outputs()`, and sctk's
        // initial `wl_output.done` invokes `new_output` for that same output. Deduplicate so
        // `monitors` never holds duplicate entries for one `wl_output`.
        upsert_monitor(&mut monitors, output, same_monitor);
        // Rune: wake `AboutToWait` in case `wl_surface.enter` arrived before `wl_output.done`.
        self.dispatched_events = true;
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, updated: WlOutput) {
        let mut monitors = self.monitors.lock().unwrap();
        let updated = MonitorHandle::new(updated);
        upsert_monitor(&mut monitors, updated, same_monitor);
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, removed: WlOutput) {
        let mut monitors = self.monitors.lock().unwrap();
        let removed = MonitorHandle::new(removed);
        // Rune: remove every matching entry with `retain` so a duplicate can never leave a stale
        // monitor behind, and wake `AboutToWait` even without `wl_surface.leave`.
        if remove_monitor(&mut monitors, &removed, same_monitor) {
            self.dispatched_events = true;
        }
    }
}

impl CompositorHandler for WinitState {
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wayland_client::protocol::wl_output::Transform,
    ) {
        // TODO(kchibisov) we need to expose it somehow in winit.
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
        // Rune: wake `AboutToWait` so output-presence hooks see entry at scale 1 without resize.
        self.dispatched_events = true;
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
        // Rune: wake `AboutToWait` so output-presence hooks see leave without resize.
        self.dispatched_events = true;
    }

    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &WlSurface,
        scale_factor: i32,
    ) {
        self.scale_factor_changed(surface, scale_factor as f64, true)
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        let window_id = super::make_wid(surface);
        let window = match self.windows.get_mut().get(&window_id) {
            Some(window) => window,
            None => return,
        };

        // In case we have a redraw requested we must indicate the wake up.
        if self
            .window_requests
            .get_mut()
            .get(&window_id)
            .unwrap()
            .redraw_requested
            .load(Ordering::Relaxed)
        {
            self.dispatched_events = true;
        }

        window.lock().unwrap().frame_callback_received();
    }
}

impl ProvidesRegistryState for WinitState {
    sctk::registry_handlers![OutputState, SeatState];

    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
}

// The window update coming from the compositor.
#[derive(Debug, Clone, Copy)]
pub struct WindowCompositorUpdate {
    /// The id of the window this updates belongs to.
    pub window_id: WindowId,

    /// New window size.
    pub resized: bool,

    /// New scale factor.
    pub scale_changed: bool,

    /// Close the window.
    pub close_window: bool,
}

impl WindowCompositorUpdate {
    fn new(window_id: WindowId) -> Self {
        Self { window_id, resized: false, scale_changed: false, close_window: false }
    }
}

sctk::delegate_subcompositor!(WinitState);
sctk::delegate_compositor!(WinitState);
sctk::delegate_output!(WinitState);
sctk::delegate_registry!(WinitState);
sctk::delegate_shm!(WinitState);
sctk::delegate_xdg_shell!(WinitState);
sctk::delegate_xdg_window!(WinitState);

/// Rune: compares two Wayland monitor handles by proxy identity first and global `wl_output`
/// identifier second, so duplicate entries for one output always match.
fn same_monitor(a: &MonitorHandle, b: &MonitorHandle) -> bool {
    a.proxy == b.proxy || a == b
}

/// Rune: inserts `item` into `monitors` or replaces its first match in place, removing any extra
/// duplicates so `WinitState.monitors` never holds duplicate entries for one `wl_output`.
fn upsert_monitor<T>(monitors: &mut Vec<T>, item: T, mut same: impl FnMut(&T, &T) -> bool) {
    if let Some(pos) = monitors.iter().position(|existing| same(existing, &item)) {
        monitors[pos] = item;
        let mut idx = pos + 1;
        while idx < monitors.len() {
            if same(&monitors[idx], &monitors[pos]) {
                monitors.remove(idx);
            } else {
                idx += 1;
            }
        }
    } else {
        monitors.push(item);
    }
}

/// Rune: removes every entry in `monitors` matching `removed`, returning whether any entry was
/// removed.
fn remove_monitor<T>(
    monitors: &mut Vec<T>,
    removed: &T,
    mut same: impl FnMut(&T, &T) -> bool,
) -> bool {
    let before = monitors.len();
    monitors.retain(|existing| !same(existing, removed));
    monitors.len() != before
}

#[cfg(test)]
mod rune_monitor_tests {
    use super::{remove_monitor, upsert_monitor};

    #[test]
    fn new_output_deduplicates_seeded_monitor_and_destroy_empties_list() {
        // Mirrors startup on Sway 1.12: WinitState::new seeds [58], then sctk calls new_output(58).
        let mut monitors = vec![58u32];
        upsert_monitor(&mut monitors, 58, |a, b| a == b);
        assert_eq!(monitors, vec![58]);

        assert!(remove_monitor(&mut monitors, &58, |a, b| a == b));
        assert!(monitors.is_empty());
        assert!(!remove_monitor(&mut monitors, &58, |a, b| a == b));
    }

    #[test]
    fn destroy_retains_only_other_monitors_even_with_duplicates() {
        let mut monitors = vec![58u32, 59, 58];
        assert!(remove_monitor(&mut monitors, &58, |a, b| a == b));
        assert_eq!(monitors, vec![59]);

        let mut dup_only = vec![58u32, 58];
        assert!(remove_monitor(&mut dup_only, &58, |a, b| a == b));
        assert!(dup_only.is_empty());
    }

    #[test]
    fn upsert_strips_any_extra_duplicates_in_place() {
        let mut monitors = vec![58u32, 59, 58];
        upsert_monitor(&mut monitors, 58, |a, b| a == b);
        assert_eq!(monitors, vec![58, 59]);

        upsert_monitor(&mut monitors, 60, |a, b| a == b);
        assert_eq!(monitors, vec![58, 59, 60]);
    }
}
