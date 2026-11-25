//! Revealing a launcher's first frame, placing the window, and asking for, stepping and
//! landing its frames.

use std::sync::atomic::Ordering;

#[cfg(not(target_os = "macos"))]
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;

use super::Host;
use crate::time::Instant;
#[cfg(not(target_os = "macos"))]
use crate::window::chrome;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
use crate::window::chrome::float_on_open;
use crate::window::chrome::sync_chrome_core;
use crate::window::is_wayland;
use crate::window::state::Reveal;
use crate::window::sync_screen_origin_core;
#[cfg(target_os = "windows")]
use crate::window::win32::post_frame;

/// The window's frame moving to where a handler asked (`Tx::resize_window`): a spring animation
/// over the frames macOS paints, or the size a jump lands at once elsewhere. One value, so a
/// jump can't sit loose beside an animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::window) enum FrameMove {
    /// The frame in flight: its animation on the host's frame clock, and the origin it last set,
    /// to notice a drag mid-flight. macOS only; elsewhere frames change at once.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Moving { anim: crate::window::frame::FrameAnim, origin: [f32; 2] },
    /// Elsewhere, a size a resize request jumps to: the coupon for the one `Resized` it causes,
    /// so that event lays out like a step of a resize animation rather than the user's drag.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Jumped { size: (f32, f32) },
}

impl FrameMove {
    /// The frame animating to a new size (macOS).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(in crate::window) fn animating(anim: crate::window::frame::FrameAnim, origin: [f32; 2]) -> Self {
        FrameMove::Moving { anim, origin }
    }

    /// The frame jumping to `size` at once (elsewhere), which its `Resized` announces.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(in crate::window) fn jumped(size: (f32, f32)) -> Self { FrameMove::Jumped { size } }

    /// Whether the frame is animating to a new frame. A jump names the one `Resized` it causes,
    /// which the draw schedules like any other event, not as motion.
    pub(in crate::window) fn moving(slot: &Option<Self>) -> bool {
        matches!(slot, Some(FrameMove::Moving { .. }))
    }

    /// Takes the jump a `Resized` laid out at `viewport` announces: spent by that event whether
    /// or not it matches, so a later resize is the user's again. `true` when this window asked
    /// for the size.
    pub(in crate::window) fn take_jump(slot: &mut Option<Self>, viewport: (f32, f32)) -> bool {
        let Some(FrameMove::Jumped { size }) = *slot else { return false };
        *slot = None;
        (size.0 - viewport.0).abs() < 1.0 && (size.1 - viewport.1).abs() < 1.0
    }

    /// Takes the animation in flight, for a window that stops painting mid-move. A jump coupon
    /// outlives it: its `Resized` still comes.
    pub(in crate::window) fn take_moving(slot: &mut Option<Self>) -> Option<crate::window::frame::FrameAnim> {
        let Some(FrameMove::Moving { anim, .. }) = *slot else { return None };
        *slot = None;
        Some(anim)
    }
}

/// Applies a size that `Window::request_inner_size` committed immediately (as on Wayland,
/// where no `WindowEvent::Resized` follows): spends any pending [`FrameMove::Jumped`] coupon,
/// resizes the surface via `resize_surface`, and updates the runtime viewport with
/// `step_viewport` when the window asked for this size or `set_viewport` otherwise. Returns
/// whether the surface or viewport changed.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(in crate::window) fn apply_immediate_resize(
    rt: &mut crate::runtime::RuntimeCore,
    frame_move: &mut Option<FrameMove>,
    size: (u32, u32),
    scale: f32,
    resize_surface: impl FnOnce((u32, u32)) -> (bool, (u32, u32)),
) -> bool {
    if size.0 == 0 || size.1 == 0 {
        FrameMove::take_jump(frame_move, (0.0, 0.0));
        return false;
    }
    let (resized, (width, height)) = resize_surface(size);
    let viewport = (width as f32 / scale, height as f32 / scale);
    let asked = FrameMove::take_jump(frame_move, viewport);
    if !resized && viewport == rt.window_viewport() && scale == rt.window_scale() {
        return false;
    }
    if asked { rt.step_viewport(viewport, scale) } else { rt.set_viewport(viewport, scale) }
    true
}

impl Host {
    pub(in crate::window) fn launcher(&self) -> bool { cfg!(target_os = "macos") && self.macos_window.launcher }

    pub(in crate::window) fn reveal_launcher(&mut self, event_loop: &ActiveEventLoop) {
        self.apply_placement(event_loop);
        if self.win.is_none() { self.create(event_loop, None, false); }
        let stale = self.app.rt().needs_commit();
        let visible = self.life.shown();
        let Some(w) = &mut self.win else { return };
        w.hidden_pace.reset();
        if !w.first_frame.request_reveal(stale, visible, crate::time::now()) {
            w.request_redraw();
            return;
        }
        // Already showing: its next frame draws the new state, or a covered one's retry does.
        if visible && stale { w.request_redraw(); }
        self.reveal(event_loop, Reveal::Cached, None);
    }

    pub(in crate::window) fn first_frame_ready(&mut self, event_loop: &ActiveEventLoop) {
        let Some(w) = &mut self.win else { return };
        if !w.first_frame.pending() {
            // No first frame to wait for (`FirstFrame::skipped`, `READY_FALLBACK`): the app's live
            // work starts anyway. The window isn't ready (`is_ready`) until it draws.
            if w.first_frame.ready_due(crate::time::now()) && w.first_frame.take_ready() { self.ready_owed(); }
            return;
        }
        let reveal = w.first_frame.complete();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.float_on_open && !w.first_frame.notified() { float_on_open::release(&w.window, &self.options, self.geometry.min_size); }
        self.native_handle.native.ready.store(true, Ordering::Release);
        self.shared.set_window_error(None);
        crate::tooling::perf_warn::launch::first_frame();
        w.first_frame.notify();
        if w.first_frame.take_ready() { self.ready_owed(); }
        if reveal {
            self.reveal_launcher(event_loop);
            self.shell(event_loop);
        }
        self.publish();
    }

    /// The window is ready: on Windows its `on_ready` and hooks run in the turns after it, one
    /// piece a turn and none while input waits (`post_frame`); elsewhere now.
    fn ready_owed(&mut self) {
        #[cfg(target_os = "windows")]
        { self.chores = post_frame::Chores::Ready; }
        #[cfg(not(target_os = "windows"))]
        self.run_on_ready();
    }

    pub(in crate::window) fn run_on_ready(&mut self) {
        if let Some(ready) = self.on_ready.as_mut() {
            crate::tooling::perf_warn::timed("on_ready", || ready(&self.native_handle));
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            self.wayland_drop_ready();
            self.wayland_backdrop_ready();
        }
        #[cfg(not(target_arch = "wasm32"))]
        crate::platform::ready_signal::ready();
    }

    /// Wayland: lazily binds the shared per-display backdrop controller after the first frame for
    /// transparent windows opened with `WindowBackdrop::None`, populating
    /// `Cx::window_backdrop_supported(WindowBackdrop::Blur)` without adding a startup round-trip
    /// or creating any per-surface blur object.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn wayland_backdrop_ready(&mut self) {
        if !self.wayland || !self.options.transparent {
            return;
        }
        let needs_attach = self.win.as_ref().is_some_and(|w| {
            #[cfg(target_os = "linux")]
            let cpu = matches!(w.native.mode, crate::window::linux::handover::SurfaceMode::Cpu { .. });
            #[cfg(not(target_os = "linux"))]
            let cpu = false;
            w.native.wayland_backdrop.is_none()
                && !cpu
                && w.surface.is_some()
                && w.config.alpha_mode != wgpu::CompositeAlphaMode::Opaque
        });
        if needs_attach && let Some(w) = self.win.as_mut() {
            w.native.wayland_backdrop =
                crate::platform::wayland::backdrop::SurfaceBlur::for_window(&w.window);
            if w.native.wayland_backdrop.is_some() {
                self.sync_window_backdrop(false);
            }
        }
    }

    /// Wayland: if `El::system_material` elements mounted on a transparent window with
    /// `WindowBackdrop::None` before the shared blur controller bound (Frame 1), bind now so
    /// `Cx::window_backdrop_supported(WindowBackdrop::Blur)` and `wayland_blur_materials` take
    /// effect on Frame 1 before encoding/presenting. Returns `true` when the runtime became
    /// stale and must rebuild before encoding.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(in crate::window) fn prepare_wayland_material_blur(&mut self) -> bool {
        if !self.wayland
            || !self.options.transparent
            || self.window_backdrop != crate::WindowBackdrop::None
            || self.app.rt().paint.materials().is_empty()
        {
            return false;
        }
        let needs_attach = self.win.as_ref().is_some_and(|w| {
            #[cfg(target_os = "linux")]
            let cpu = matches!(w.native.mode, crate::window::linux::handover::SurfaceMode::Cpu { .. });
            #[cfg(not(target_os = "linux"))]
            let cpu = false;
            w.native.wayland_backdrop.is_none()
                && !cpu
                && w.surface.is_some()
                && w.config.alpha_mode != wgpu::CompositeAlphaMode::Opaque
        });
        if !needs_attach {
            return false;
        }
        if let Some(w) = self.win.as_mut() {
            w.native.wayland_backdrop =
                crate::platform::wayland::backdrop::SurfaceBlur::for_window(&w.window);
        }
        if self.win.as_ref().is_some_and(|w| w.native.wayland_backdrop.is_some()) {
            self.sync_window_backdrop(false);
            return self.app.rt().stale();
        }
        false
    }

    /// Wayland: dispatches queued runtime capability events on the shared display controller and
    /// re-syncs this window if `Capability::Blur` was revoked or re-granted.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(in crate::window) fn poll_wayland_backdrop(&mut self) {
        if !self.wayland {
            return;
        }
        let changed = self
            .win
            .as_mut()
            .and_then(|w| w.native.wayland_backdrop.as_mut())
            .is_some_and(|b| b.poll_capability_change());
        if changed {
            self.sync_window_backdrop(false);
        }
    }

    /// Wayland: marks the window ready for lazy `wl_data_device` binding and syncs drop targets.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn wayland_drop_ready(&mut self) {
        if !self.wayland { return; }
        self.step_wayland_drop_binding(crate::platform::wayland::drop::binding::Event::WindowReady);
        self.sync_wayland_drop();
    }

    /// Wayland: lazily binds or schedules teardown of `wl_data_device` (and the unwatched
    /// press/touch serial listener for `drag_out`) when `(commit_generation, active_modal)` or
    /// active-drag state changes. Zero work before frame 1 or on steady/animation frames.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(in crate::window) fn sync_wayland_drop(&mut self) {
        if !self.wayland { return; }
        let key = {
            let rt = self.app.rt();
            let active_drag = rt.has_active_drag_out()
                || self.win.as_ref().is_some_and(|w| crate::platform::wayland::drop::has_active_drag(&w.window));
            crate::platform::wayland::drop::binding::SyncKey {
                commit: rt.commit_generation(),
                modal: rt.active_modal(),
                active_drag,
            }
        };
        if !self.win.as_ref().is_some_and(|w| w.native.drop_binding.needs_sync(key)) {
            return;
        }
        let (has_drop, has_drag_out) = {
            let rt = self.app.rt();
            (rt.has_drop_targets(), rt.has_drag_out())
        };
        let active_drag = key.active_drag;
        let has_targets = has_drop || has_drag_out || active_drag;
        self.step_wayland_drop_binding(crate::platform::wayland::drop::binding::Event::TreeSynced {
            key,
            has_targets,
            now: crate::time::now(),
        });
        // Keep the unwatched press/touch serial listener bound only while `drag_out` is mounted
        // (or an outgoing drag is in flight) and the window's `wl_data_device` is bound.
        if let Some(w) = &mut self.win {
            let wants_serial = w.native.drop_target.is_some() && (has_drag_out || active_drag);
            if wants_serial && w.native.drag_serial.is_none() {
                w.native.drag_serial = crate::platform::wayland::serial::wire::attach_window(&w.window);
            } else if !wants_serial && w.native.drag_serial.is_some() {
                w.native.drag_serial = None;
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(in crate::window) fn wayland_drop_closed(&mut self) {
        if !self.wayland { return; }
        self.step_wayland_drop_binding(crate::platform::wayland::drop::binding::Event::WindowClosed);
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(in crate::window) fn wayland_drop_display_lost(&mut self) {
        if !self.wayland { return; }
        self.step_wayland_drop_binding(crate::platform::wayland::drop::binding::Event::DisplayLost);
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(in crate::window) fn step_wayland_drop_binding(
        &mut self,
        event: crate::platform::wayland::drop::binding::Event,
    ) {
        use crate::platform::wayland::drop::binding::{self, Effect};
        let Some(w) = &mut self.win else { return };
        let effects = binding::step(&mut w.native.drop_seq, &mut w.native.drop_binding, event);
        for effect in effects {
            match effect {
                Effect::Bind => {
                    let Some(w) = &mut self.win else { return };
                    if w.native.drop_target.is_none() {
                        let handle = self.native_handle.clone();
                        let target = crate::platform::wayland::drop::install(
                            &w.window,
                            std::sync::Arc::new(move || handle.send(|_| crate::window::WakeEvent::FileDrop)),
                        );
                        if target.is_some() {
                            w.native.drop_target = target;
                            if w.native.drag_serial.is_none()
                                && (self.app.rt().has_drag_out() || self.app.rt().has_active_drag_out())
                            {
                                w.native.drag_serial = crate::platform::wayland::serial::wire::attach_window(&w.window);
                            }
                        } else {
                            let _ = binding::step(
                                &mut w.native.drop_seq,
                                &mut w.native.drop_binding,
                                binding::Event::BindFailed,
                            );
                        }
                    }
                }
                Effect::LeaveRuntimeDrag => {
                    let rt = self.app.rt_mut();
                    if rt.dragging_files() {
                        rt.drag_files_leave();
                    }
                }
                Effect::WakeAt(_) => {}
                Effect::Unbind => {
                    if let Some(w) = &mut self.win {
                        w.native.drag_serial = None;
                        w.native.drop_target = None;
                    }
                }
            }
        }
    }

    /// Windows: the hooks a shown window takes once it's ready.
    #[cfg(target_os = "windows")]
    pub(in crate::window) fn run_ready_hooks(&mut self) {
        // After the first frame: files can only be dragged onto a window a person has seen.
        if let Some(w) = &mut self.win {
            if w.native.drop_target.is_none() {
                w.native.drop_target = crate::platform::win32::drop_target::install(&w.window);
            }
            // Win+L wipes a secret left on the clipboard.
            crate::platform::win32::session_lock::install(&w.window);
        }
    }

    pub(in crate::window) fn place(&mut self, event_loop: &ActiveEventLoop, request: crate::windows::PlaceRequest) {
        self.options.position = None;
        self.placement = Some(request);
        self.reopen.clear_position();
        self.apply_placement(event_loop);
    }

    pub(in crate::window) fn apply_placement(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(request), Some(w)) = (&self.placement, &self.win) else { return };
        if is_wayland(event_loop) { return; }
        #[cfg(target_os = "macos")]
        {
            let _ = request.pointer;
            crate::platform::macos::place_window(&w.window, self.app.rt().window_viewport(), request);
        }
        #[cfg(not(target_os = "macos"))]
        if let Some(monitor) = crate::overlay::monitor_at(event_loop, &request.display, request.pointer) {
            let scale = monitor.scale_factor();
            let area = monitor.size();
            let rect = crate::overlay::options::local_rect(crate::windows::PlaceRequest::at(self.app.rt().window_viewport()), Some(request.placement),
                (area.width as f32 / scale as f32, area.height as f32 / scale as f32));
            let origin = monitor.position();
            let applied = chrome::request_inner_size(&w.window, (rect[2], rect[3]));
            w.window.set_outer_position(PhysicalPosition::new(origin.x + (f64::from(rect[0]) * scale).round() as i32,
                origin.y + (f64::from(rect[1]) * scale).round() as i32));
            if let Some(size) = applied { self.apply_immediate_resize(size, true); }
        }
        self.sync_origin();
    }

    /// Applies a size that `Window::request_inner_size` committed immediately (Wayland):
    /// reconfigures the surface, updates the runtime viewport, and requests a redraw unless this
    /// frame is already about to draw it.
    #[cfg(not(target_os = "macos"))]
    fn apply_immediate_resize(&mut self, size: PhysicalSize<u32>, redraw: bool) {
        let Some(w) = self.win.as_mut() else { return };
        let gpu = self.gpu.as_ref().map(|(gpu, _)| &**gpu);
        let rt = self.app.rt_mut();
        sync_chrome_core(rt, w, self.chrome, self.titlebar);
        let scale = w.window.scale_factor() as f32;
        let origin_moved = sync_screen_origin_core(rt, &w.window);
        let changed = apply_immediate_resize(rt, &mut self.frame_move, (size.width, size.height), scale, |(width, height)| {
            let resized = w.resize_surface(gpu, PhysicalSize::new(width, height));
            (resized, (w.config.width, w.config.height))
        });
        if redraw && self.life.shown() && (changed || origin_moved) { w.request_redraw(); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.wayland && self.options.transparent {
            self.sync_window_backdrop(false);
        }
    }

    /// Seconds on the host's frame clock, which frame animations sample.
    pub(in crate::window) fn frame_secs(&self, at: Instant) -> f64 {
        self.app.rt().wall_start.map_or(0.0, |start| at.saturating_duration_since(start).as_secs_f64())
    }

    /// Takes the frame change handlers asked for. A painting macOS window moves over its next
    /// frames on the request's spring, from `start`; anywhere else the frame jumps. Sizes stay
    /// inside the window's minimum and maximum. A size outlives the window: one created later
    /// opens at it. Returns whether macOS resized the surface immediately.
    pub(in crate::window) fn take_frame_request(&mut self, start: f64, painting: bool) -> bool {
        let rt = self.app.rt_mut();
        rt.host.fit.take_held();
        let Some(mut request) = rt.host.window_frame.take() else { return false };
        let (min, max) = self.options.size_limits(self.geometry.min_size);
        request.size = request.size.map(|size| crate::window::frame::clamp_size(size, min, max));
        if let Some(size) = request.size {
            self.geometry.size = size;
            self.args.size = None;
            self.options.physical_size = None;
            self.reopen.clear_size();
        }
        if let Some(edge) = request.resize_drag
            && self.win.as_ref().is_some_and(|w| w.drag_resize_window(edge))
        {
            self.app.rt_mut().cancel_pointer();
        }
        if request.size.is_none() && !request.center {
            return false;
        }
        let Some(w) = &self.win else { return false };
        #[cfg(target_os = "macos")]
        {
            use crate::window::frame::{FrameAnim, snap, target};
            let Some((frame, visible, scale)) = crate::platform::macos::window_geometry(&w.window) else { return false };
            // Resizing again mid-flight keeps the edge the first resize keeps.
            let base = match &self.frame_move {
                Some(FrameMove::Moving { anim, .. }) => anim.to,
                _ => frame,
            };
            let outer = request.size.and_then(|size| crate::platform::macos::outer_size(&w.window, size));
            let to = snap(target(base, outer, request.center, visible), scale);
            // Centring alone carries on with a resize already moving.
            let spring = request.spring
                .or(match &self.frame_move {
                    Some(FrameMove::Moving { anim, .. }) => anim.spring,
                    _ => None,
                }.filter(|_| request.size.is_none()));
            if painting {
                let previous = match &self.frame_move {
                    Some(FrameMove::Moving { anim, .. }) => Some(anim),
                    _ => None,
                };
                let anim = FrameAnim::retarget(previous, spring, start, frame, to);
                self.frame_move = Some(FrameMove::animating(anim, [frame[0], frame[1]]));
                false
            } else {
                self.frame_move = None;
                crate::platform::macos::set_window_frame(&w.window, to);
                self.sync_origin();
                self.sync_surface_size()
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = start;
            let window = &w.window;
            // Read before the resize request, which may apply at once.
            let geometry = (window.current_monitor(), window.outer_position(), window.outer_size(), window.inner_size());
            let applied = if let Some((width, height)) = request.size {
                self.frame_move = Some(FrameMove::jumped((width, height)));
                chrome::request_inner_size(window, (width, height))
            } else {
                None
            };
            if let Some(size) = applied { self.apply_immediate_resize(size, !painting); }
            // Wayland windows can't see or set their position.
            if self.wayland { return false; }
            let Some(w) = &self.win else { return false };
            let window = &w.window;
            let (Some(monitor), Ok(position), outer, inner) = geometry else { return false };
            let scale = window.scale_factor() as f32;
            let size = request.size.map(|(width, height)| (
                width * scale + outer.width.saturating_sub(inner.width) as f32,
                height * scale + outer.height.saturating_sub(inner.height) as f32,
            ));
            let current = [position.x as f32, position.y as f32, outer.width as f32, outer.height as f32];
            let (origin, area) = (monitor.position(), monitor.size());
            let visible = [origin.x as f32, origin.y as f32, area.width as f32, area.height as f32];
            let [x, y, ..] = crate::window::frame::target(current, size, request.center, visible);
            if [x, y] != [current[0], current[1]] {
                window.set_outer_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
                self.sync_origin();
            }
            false
        }
    }

    /// Matches the surface and runtime viewport to the window's inner size after a synchronous
    /// macOS frame jump (`take_frame_request` with `painting = false`). Returns whether the
    /// surface size changed.
    #[cfg(target_os = "macos")]
    fn sync_surface_size(&mut self) -> bool {
        let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) else { return false };
        let size = w.window.inner_size();
        let resized = w.resize_surface(Some(&**gpu), size);
        let scale = w.window.scale_factor() as f32;
        self.app.rt_mut().set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
        resized
    }

    /// Moves the frame one display frame along, before the frame's drawable is acquired, and
    /// lays the content out at the new size: content follows the edge in the same frame, with
    /// no stretched or blank strip.
    pub(in crate::window) fn step_frame(&mut self, at: Instant) {
        let now = self.frame_secs(at);
        self.take_frame_request(now, true);
        #[cfg(target_os = "macos")]
        {
            use crate::window::frame::snap;
            // The window server shows a move on the first frame after an idle spell before the
            // frame drawn for it, leaving a stale strip. A request taken then starts here and
            // moves from the next frame.
            let resting = self.idle;
            let moving = FrameMove::moving(&self.frame_move);
            let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
            w.present_with_transaction(moving);
            if resting { return; }
            let Some(FrameMove::Moving { anim, origin }) = self.frame_move.as_mut() else { return };
            let Some((frame, _, scale)) = crate::platform::macos::window_geometry(&w.window) else { return };
            // Someone dragged the window mid-flight: the drag wins. The window stays where they
            // put it and the size carries on from there.
            let [x, y] = *origin;
            if (frame[0] - x).abs() > 0.5 || (frame[1] - y).abs() > 0.5 {
                anim.shift(frame[0] - x, frame[1] - y);
            }
            let rect = snap(anim.sample(now).0, scale);
            crate::platform::macos::set_window_frame(&w.window, rect);
            *origin = [rect[0], rect[1]];
            if anim.done(now) { self.frame_move = None; }
            let rt = self.app.rt_mut();
            sync_chrome_core(rt, w, self.chrome, self.titlebar);
            let size = w.window.inner_size();
            w.resize_surface(Some(&**gpu), size);
            let scale = w.window.scale_factor() as f32;
            rt.step_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
            // In this frame's layout, not the next one when `Moved` arrives.
            sync_screen_origin_core(rt, &w.window);
        }
    }

    /// A window that stops painting mid-move lands where it was going. A jump coupon outlives
    /// it: its `Resized` still comes.
    pub(in crate::window) fn land_frame(&mut self) {
        let moving = FrameMove::take_moving(&mut self.frame_move);
        #[cfg(target_os = "macos")]
        if let Some(w) = &mut self.win {
            w.present_with_transaction(false);
            w.set_live_resize(false);
            if let Some(anim) = moving { crate::platform::macos::set_window_frame(&w.window, anim.to); }
        }
        #[cfg(target_os = "macos")]
        self.sync_origin();
        #[cfg(not(target_os = "macos"))]
        let _ = moving;
    }
}
