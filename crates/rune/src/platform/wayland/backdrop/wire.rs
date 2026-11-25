//! Wayland wire backend for behind-window blur (`ext_background_effect_v1` and
//! `org_kde_kwin_blur_manager`) on winit's shared `wl_display`.
//!
//! Shares one per-display `DisplayController` across all transparent Wayland toplevel windows and
//! layer-shell overlays on the UI thread, with zero background threads: runtime `capabilities`
//! events arrive on the controller's private `EventQueue` and wake winit through
//! `window::wayland_wake`.

use std::cell::RefCell;
use std::collections::HashMap;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::{
    self, ExtBackgroundEffectManagerV1,
};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;
use wayland_protocols_plasma::blur::client::org_kde_kwin_blur::OrgKdeKwinBlur;
use wayland_protocols_plasma::blur::client::org_kde_kwin_blur_manager::OrgKdeKwinBlurManager;

use super::policy::{
    BlurSync, CompositorProtocol, SurfaceBlurTracker, WaylandBackdropInputs, WaylandBlurSeam,
    choose_protocol, ext_capabilities_has_blur,
};
use super::region::{BlurRegion, RegionRect, RegionSpec, build_elements_region};

thread_local! {
    /// One shared Wayland backdrop controller per `wl_display` pointer, always touched on the UI
    /// thread and torn down before winit closes the display.
    static CONTROLLERS: RefCell<HashMap<usize, DisplayController>> = RefCell::new(HashMap::new());
}

#[derive(Default)]
struct ControllerState {
    ext_global_name: Option<u32>,
    ext_blur_capable: bool,
    kde_global_name: Option<u32>,
    kde_removed: bool,
    capability_gen: u64,
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
        if let wl_registry::Event::GlobalRemove { name } = event {
            if state.ext_global_name == Some(name) {
                state.ext_global_name = None;
                if state.ext_blur_capable {
                    state.ext_blur_capable = false;
                    state.capability_gen = state.capability_gen.wrapping_add(1);
                }
            }
            if state.kde_global_name == Some(name) {
                state.kde_global_name = None;
                if !state.kde_removed {
                    state.kde_removed = true;
                    state.capability_gen = state.capability_gen.wrapping_add(1);
                }
            }
        }
    }
}

impl Dispatch<WlCompositor, ()> for ControllerState {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: <WlCompositor as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlRegion, ()> for ControllerState {
    fn event(
        _: &mut Self,
        _: &WlRegion,
        _: <WlRegion as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for ControllerState {
    fn event(
        state: &mut Self,
        _: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
            let raw_bits = match flags {
                WEnum::Value(caps) => caps.bits(),
                WEnum::Unknown(raw) => raw,
            };
            let next = ext_capabilities_has_blur(raw_bits);
            if state.ext_blur_capable != next {
                state.ext_blur_capable = next;
                state.capability_gen = state.capability_gen.wrapping_add(1);
            }
        }
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, ()> for ControllerState {
    fn event(
        _: &mut Self,
        _: &ExtBackgroundEffectSurfaceV1,
        _: <ExtBackgroundEffectSurfaceV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<OrgKdeKwinBlurManager, ()> for ControllerState {
    fn event(
        _: &mut Self,
        _: &OrgKdeKwinBlurManager,
        _: <OrgKdeKwinBlurManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<OrgKdeKwinBlur, ()> for ControllerState {
    fn event(
        _: &mut Self,
        _: &OrgKdeKwinBlur,
        _: <OrgKdeKwinBlur as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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
    compositor: WlCompositor,
    ext_manager: Option<ExtBackgroundEffectManagerV1>,
    kde_manager: Option<OrgKdeKwinBlurManager>,
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
        let Ok(compositor) = globals.bind::<WlCompositor, _, _>(&qh, 1..=6, ()) else {
            destroy_globals(&conn, globals);
            let mut state = ControllerState::default();
            let _ = drain_queue(&mut queue, &mut state);
            queue.flush().ok();
            return None;
        };

        let (ext_global, kde_global) = globals.contents().with_list(|list| {
            let ext = list
                .iter()
                .find(|g| g.interface == "ext_background_effect_manager_v1")
                .map(|g| g.name);
            let kde = list
                .iter()
                .find(|g| g.interface == "org_kde_kwin_blur_manager")
                .map(|g| g.name);
            (ext, kde)
        });

        let mut state = ControllerState::default();
        let mut dead = false;
        let mut ext_manager = None;
        let mut kde_manager = None;

        if let Some(name) = ext_global {
            // Staging protocol guard: bind strictly at version 1..=1.
            if let Ok(manager) = globals.bind::<ExtBackgroundEffectManagerV1, _, _>(&qh, 1..=1, ()) {
                state.ext_global_name = Some(name);
                ext_manager = Some(manager);
                // Receive the initial `capabilities` event advertised immediately on bind.
                if queue.roundtrip(&mut state).is_err() {
                    dead = true;
                }
            }
        } else if let Some(name) = kde_global {
            // Fall back to legacy KWin blur only when `ext_background_effect_manager_v1` is absent.
            if let Ok(manager) = globals.bind::<OrgKdeKwinBlurManager, _, _>(&qh, 1..=1, ()) {
                state.kde_global_name = Some(name);
                kde_manager = Some(manager);
            }
        }

        Some(Self {
            conn,
            globals: Some(globals),
            queue,
            state,
            compositor,
            ext_manager,
            kde_manager,
            surfaces_attached: 0,
            dead,
        })
    }

    fn protocol(&self) -> CompositorProtocol {
        if self.dead {
            return CompositorProtocol::None;
        }
        let has_ext = self.ext_manager.is_some() && self.state.ext_global_name.is_some();
        let has_kde = self.kde_manager.is_some() && !self.state.kde_removed;
        choose_protocol(has_ext, self.state.ext_blur_capable, has_kde)
    }

    fn dispatch(&mut self) {
        if self.dead {
            return;
        }
        if !drain_queue(&mut self.queue, &mut self.state) || self.conn.backend().last_error().is_some() {
            self.dead = true;
            if self.state.ext_blur_capable || !self.state.kde_removed {
                self.state.ext_blur_capable = false;
                self.state.kde_removed = true;
                self.state.capability_gen = self.state.capability_gen.wrapping_add(1);
            }
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

    fn needs_wake_on_read(&self) -> bool {
        !self.dead && self.surfaces_attached > 0 && self.ext_manager.is_some()
    }

    fn has_pending_events(&self) -> bool {
        !self.dead && self.surfaces_attached > 0 && self.queue.prepare_read().is_none()
    }
}

impl Drop for DisplayController {
    fn drop(&mut self) {
        let _ = drain_queue(&mut self.queue, &mut self.state);
        if let Some(ext) = self.ext_manager.take() {
            ext.destroy();
        }
        if let Some(kde) = self.kde_manager.take() {
            self.conn.backend().destroy_object(&kde.id()).ok();
        }
        self.conn.backend().destroy_object(&self.compositor.id()).ok();
        if let Some(globals) = self.globals.take() {
            destroy_globals(&self.conn, globals);
        }
        let _ = drain_queue(&mut self.queue, &mut self.state);
        self.queue.flush().ok();
    }
}

struct RealBlurSeam<'a> {
    controller: &'a mut DisplayController,
    surface: &'a WlSurface,
    ext_surface: &'a mut Option<ExtBackgroundEffectSurfaceV1>,
    kde_blur: &'a mut Option<OrgKdeKwinBlur>,
}

impl WaylandBlurSeam for RealBlurSeam<'_> {
    type Region = WlRegion;

    fn create_region(&mut self) -> Option<Self::Region> {
        if self.controller.dead {
            return None;
        }
        Some(self.controller.compositor.create_region(&self.controller.queue.handle(), ()))
    }

    fn region_add(&mut self, region: &Self::Region, rect: RegionRect) {
        region.add(rect.x, rect.y, rect.width, rect.height);
    }

    fn region_destroy(&mut self, region: Self::Region) {
        region.destroy();
    }

    fn ext_get_background_effect(&mut self) -> bool {
        if self.controller.dead {
            return false;
        }
        let Some(manager) = self.controller.ext_manager.as_ref() else {
            return false;
        };
        let effect =
            manager.get_background_effect(self.surface, &self.controller.queue.handle(), ());
        *self.ext_surface = Some(effect);
        true
    }

    fn ext_set_blur_region(&mut self, region: Option<&Self::Region>) {
        if let Some(effect) = self.ext_surface.as_ref() {
            effect.set_blur_region(region);
        }
    }

    fn ext_destroy_effect(&mut self) {
        if let Some(effect) = self.ext_surface.take() {
            effect.destroy();
        }
    }

    fn kde_create(&mut self) -> bool {
        if self.controller.dead {
            return false;
        }
        let Some(manager) = self.controller.kde_manager.as_ref() else {
            return false;
        };
        let blur = manager.create(self.surface, &self.controller.queue.handle(), ());
        *self.kde_blur = Some(blur);
        true
    }

    fn kde_set_region(&mut self, region: Option<&Self::Region>) {
        if let Some(blur) = self.kde_blur.as_ref() {
            blur.set_region(region);
        }
    }

    fn kde_commit(&mut self) {
        if let Some(blur) = self.kde_blur.as_ref() {
            blur.commit();
        }
    }

    fn kde_unset(&mut self) {
        if let Some(manager) = self.controller.kde_manager.as_ref() {
            manager.unset(self.surface);
        }
    }

    fn kde_release(&mut self) {
        if let Some(blur) = self.kde_blur.take() {
            blur.release();
        }
    }
}

/// Per-surface Wayland blur controller attached to one live `wl_surface` (either a winit
/// toplevel window or a `LayerSurface` overlay).
///
/// Must be dropped strictly before the owning `wl_surface` drops (stored in `Win::native` for
/// toplevel windows and in `Live` before `backing` for overlays) so per-surface blur proxies are
/// destroyed before `wl_surface.destroy()`.
pub(crate) struct SurfaceBlur {
    display: usize,
    surface: Option<WlSurface>,
    ext_surface: Option<ExtBackgroundEffectSurfaceV1>,
    kde_blur: Option<OrgKdeKwinBlur>,
    tracker: SurfaceBlurTracker,
    last_capability_gen: u64,
}

impl SurfaceBlur {
    /// Attaches to a winit `Window`'s `wl_surface`, binding the shared per-display blur
    /// controller if not yet bound. Returns `None` off Wayland or on display error.
    pub(crate) fn for_window(window: &winit::window::Window) -> Option<Self> {
        let display = window.display_handle().ok()?.as_raw();
        let handle = window.window_handle().ok()?.as_raw();
        Self::from_raw_handles(display, handle)
    }

    /// Attaches to raw Wayland `(wl_display, wl_surface)` handles (used by both winit windows
    /// and `LayerSurface` overlays).
    #[allow(unsafe_code)]
    pub(crate) fn from_raw_handles(
        display: RawDisplayHandle,
        window: RawWindowHandle,
    ) -> Option<Self> {
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
            // SAFETY: `handle.surface` is a live `wl_surface` owned by winit or `LayerSurface`,
            // and `SurfaceBlur` drops strictly before that owner drops.
            let id =
                unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }
                    .ok()?;
            let surface = WlSurface::from_id(&controller.conn, id).ok()?;
            controller.surfaces_attached += 1;
            let last_capability_gen = controller.state.capability_gen;
            Some(Self {
                display: display_ptr,
                surface: Some(surface),
                ext_surface: None,
                kde_blur: None,
                tracker: SurfaceBlurTracker::default(),
                last_capability_gen,
            })
        })
    }

    /// Dispatches any queued events on the shared display controller and returns `true` if the
    /// compositor's blur capability changed since this surface last synced.
    pub(crate) fn poll_capability_change(&mut self) -> bool {
        CONTROLLERS.with(|map| {
            let mut map = map.borrow_mut();
            let Some(controller) = map.get_mut(&self.display) else {
                return false;
            };
            controller.dispatch();
            controller.state.capability_gen != self.last_capability_gen
        })
    }

    /// Whether the shared display controller needs winit's `wake_on_wayland_read` flag enabled to
    /// deliver runtime `ext_background_effect_manager_v1.capabilities` updates.
    pub(crate) fn needs_wake_on_read(&self) -> bool {
        CONTROLLERS.with(|map| {
            map.borrow().get(&self.display).is_some_and(DisplayController::needs_wake_on_read)
        })
    }

    /// Syscall-free check for whether the shared display controller's event queue already has
    /// unread events from a mid-turn read.
    pub(crate) fn has_pending_events(&self) -> bool {
        CONTROLLERS.with(|map| {
            map.borrow().get(&self.display).is_some_and(DisplayController::has_pending_events)
        })
    }

    /// Synchronizes this surface's blur region and protocol state with the compositor
    /// (including any per-element `El::system_material` blur rects when `WindowBackdrop::None`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sync_with_materials(
        &mut self,
        requested: crate::WindowBackdrop,
        transparent: bool,
        alpha_mode: Option<wgpu::CompositeAlphaMode>,
        cpu_painter: bool,
        region_spec: RegionSpec,
        materials: &[crate::system_material::WaylandMaterialBlur],
        window_scale: f32,
        is_overlay: bool,
        force_reapply: bool,
    ) -> BlurSync {
        let fallback_sync = || BlurSync {
            active: crate::WindowBackdrop::None,
            supported_mask: 0,
            material_blur_enabled: false,
            opaque_fallback: super::policy::needs_opaque_fallback(
                is_overlay,
                transparent,
                requested,
                crate::WindowBackdrop::None,
            ),
            needs_commit: false,
        };
        let Some(surface) = self.surface.as_ref() else {
            return fallback_sync();
        };
        let env_mode = crate::system_material::Mode::from_env();
        let material_region = if requested == crate::WindowBackdrop::None
            && env_mode != crate::system_material::Mode::Off
        {
            build_elements_region(materials, window_scale, region_spec.logical_size)
        } else {
            BlurRegion::Empty
        };
        CONTROLLERS.with(|map| {
            let mut map = map.borrow_mut();
            let Some(controller) = map.get_mut(&self.display) else {
                return fallback_sync();
            };
            controller.dispatch();
            self.last_capability_gen = controller.state.capability_gen;
            let protocol = controller.protocol();
            let inputs = WaylandBackdropInputs {
                requested,
                is_wayland: true,
                transparent,
                alpha_mode,
                cpu_painter,
                protocol,
            };
            let mut seam = RealBlurSeam {
                controller,
                surface,
                ext_surface: &mut self.ext_surface,
                kde_blur: &mut self.kde_blur,
            };
            let outcome = self.tracker.sync_with_materials(
                inputs,
                region_spec,
                material_region,
                env_mode,
                is_overlay,
                force_reapply,
                &mut seam,
            );
            if outcome.needs_commit {
                controller.flush();
            }
            outcome
        })
    }

    /// Pre-present per-frame sync for `El::system_material` blur regions when
    /// `WindowBackdrop::None` is active. Zero allocations and zero Wayland calls when idle
    /// with no active blur or when the quantized logical region is unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sync_frame_materials(
        &mut self,
        requested: crate::WindowBackdrop,
        transparent: bool,
        alpha_mode: Option<wgpu::CompositeAlphaMode>,
        cpu_painter: bool,
        physical_size: (u32, u32),
        window_scale: f32,
        materials: &[crate::system_material::WaylandMaterialBlur],
        is_overlay: bool,
    ) {
        if requested != crate::WindowBackdrop::None {
            return;
        }
        if materials.is_empty() && !self.tracker.is_active() {
            return;
        }
        let region_spec = RegionSpec::from_surface(physical_size, window_scale, 0.0);
        let _ = self.sync_with_materials(
            requested,
            transparent,
            alpha_mode,
            cpu_painter,
            region_spec,
            materials,
            window_scale,
            is_overlay,
            false,
        );
    }

    /// Destroys any active per-surface blur object immediately and marks this surface closed so
    /// subsequent calls cannot send requests against a dead `wl_surface`.
    pub(crate) fn mark_closed(&mut self) {
        let Some(surface) = self.surface.take() else {
            return;
        };
        let destroyed_through_controller = CONTROLLERS
            .try_with(|map| {
                let mut map = map.borrow_mut();
                if let Some(controller) = map.get_mut(&self.display) {
                    let mut seam = RealBlurSeam {
                        controller,
                        surface: &surface,
                        ext_surface: &mut self.ext_surface,
                        kde_blur: &mut self.kde_blur,
                    };
                    if self.tracker.close(&mut seam) {
                        controller.flush();
                    }
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if !destroyed_through_controller {
            if let Some(effect) = self.ext_surface.take() {
                effect.destroy();
            }
            if let Some(blur) = self.kde_blur.take() {
                blur.release();
            }
        }
    }
}

impl Drop for SurfaceBlur {
    fn drop(&mut self) {
        self.mark_closed();
        CONTROLLERS
            .try_with(|map| {
                let mut map = map.borrow_mut();
                let remove = if let Some(controller) = map.get_mut(&self.display) {
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

/// Tears down any remaining per-display Wayland backdrop controllers before the display closes.
pub(crate) fn shutdown() {
    CONTROLLERS
        .try_with(|map| {
            map.borrow_mut().clear();
        })
        .ok();
}
