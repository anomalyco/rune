//! Events a host handles: resuming, input while hidden, the fn key, file drops, and user and
//! window events.

use std::sync::atomic::Ordering;

use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Theme, Window, WindowId};

use super::stepping::FrameMove;
use super::{Host, WindowApp};
use crate::app::Shared;
use crate::input::host_events;
use crate::window::chrome::{self, sync_chrome_core};
#[cfg(target_os = "macos")]
use crate::window::chrome::mac;
use crate::window::file_drop::{FileDrop, FileStep};
use crate::window::keys::{map_key_event, map_mods};
use crate::window::state::{Cover, RestEvent};
#[cfg(not(target_os = "macos"))]
use crate::window::surface::RedrawEvent;
use crate::window::surface::Win;
use crate::window::{WakeEvent, gesture_phase, pointer_button, sync_screen_origin_core};

impl Host {
    pub(in crate::window) fn resume(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            return;
        }
        // Updates queued before launch (the link that launched the app) show in the first frame.
        self.app.drain_startup(&self.shortcuts, &mut self.menus);
        self.apply_tx_color_scheme();
        self.apply_tx_window_backdrop();
        if let Some(scheme) = self.shared.take_color_scheme() {
            self.set_app_color_scheme(scheme);
        }
        if self.launcher() {
            self.create(event_loop, None, false);
            if !self.life.finished() && self.open_at_start && self.options.visible { self.reveal_launcher(event_loop); }
        } else if self.open_at_start {
            self.open(event_loop, None);
        }
        self.started = true;
        if !self.life.finished() { self.drain(event_loop, false); }
    }

    /// A hidden window delivers queued input without drawing, such as the releases of the
    /// modifier keys held when it hid, rather than when it next shows. A shown window's
    /// redraw delivers it, as does a hidden launcher's.
    pub(in crate::window) fn hidden_input(&mut self, event_loop: &ActiveEventLoop) {
        if self.poll_gpu_loss() { return; }
        if !self.started || self.life.finished() || self.win.as_ref().is_some_and(|_| self.life.shown() || self.launcher()) { return; }
        if !(self.app.rt().has_input_waiting() || self.app.rt().keys_queued() || self.app.rt().drag_ends_pending()) { return; }
        self.app.hidden_wake();
        self.clipboard.flush_core(self.app.rt_mut());
        self.shell(event_loop);
    }

    #[cfg(target_os = "macos")]
    pub(in crate::window) fn fn_key(&mut self) {
        if self.poll_gpu_loss() { return; }
        let changes = {
            let Some(l) = &self.win else { return };
            let Some(changes) = l.native.fn_key.as_ref().map(|f| f.take()) else { return };
            changes
        };
        if !changes.is_empty() {
            self.note_input(crate::time::now());
        }
        let mut redraw = false;
        for pressed in changes {
            redraw |= self.app.physical_with_fallback(crate::PhysicalInput::Key {
                key: winit::keyboard::KeyCode::Fn, mods: map_mods(self.mods), pressed, repeat: false, text: None, consumed_mods: Default::default(),
            }, None);
        }
        if redraw && let Some(l) = &self.win { l.request_redraw(); }
    }

    #[cfg(target_os = "macos")]
    pub(in crate::window) fn media_keys(&mut self) {
        if self.poll_gpu_loss() { return; }
        let Some(l) = &self.win else { return };
        let Some(events) = l.native.media_keys.as_ref().map(|m| m.take()) else { return };
        let mut redraw = false;
        for (decoded, mods) in events {
            redraw |= self.app.media_key(decoded, mods);
        }
        if redraw { l.request_redraw(); }
    }

    /// Hands the runtime the file drag winit reported since the last pass, in order: the files
    /// that entered, as one drag, and those dropped, as one drop, each at a fresh pointer
    /// sample; and, while the drag is over the window, a new sample, which redraws only when it
    /// changes the drop target. Runs even while the window is covered, as queued input does.
    pub(in crate::window) fn flush_files(&mut self, event_loop: &ActiveEventLoop) {
        if self.poll_gpu_loss() { self.files = FileDrop::default(); return; }
        #[cfg(target_os = "macos")]
        let native = self.win.as_ref().map(|l| crate::platform::macos::drop::take(&l.window)).unwrap_or_default();
        #[cfg(target_os = "windows")]
        let native = self.win.as_ref().map(|l| crate::platform::win32::drop_target::take(&l.window)).unwrap_or_default();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let (native, wayland_drop_lost) = match self.win.as_ref().filter(|l| l.native.drop_target.is_some()) {
            Some(l) => crate::platform::wayland::drop::take(&l.window),
            None => (Vec::new(), false),
        };
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if wayland_drop_lost {
            self.wayland_drop_display_lost();
        }
        #[cfg(not(any(target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))))]
        let native: Vec<FileStep> = Vec::new();
        let Some(l) = self.win.as_ref().filter(|_| self.life.shown()) else {
            self.files = FileDrop::default();
            return;
        };
        let f = &mut self.files;
        let rt = self.app.rt_mut();
        let zoom = rt.zoom();
        // Native drop targets (AppKit, Win32 OLE, Wayland) report each step in window points;
        // winit's fallback events omit the position, so the host samples the pointer for them.
        let steps: Vec<_> = native.into_iter().chain(std::mem::take(&mut f.steps)).collect();
        let mut queued = f.dispatch_steps(
            rt,
            steps,
            |f| {
                let pointer = crate::input::drop_pointer::DropPointer::new();
                f.at = pointer.sample(&l.window, zoom);
                f.pointer = Some(pointer);
                f.at
            },
            |f| {
                // A drop with no drag before it (Windows' WM_DROPFILES) samples too.
                let pointer = f.pointer.take().unwrap_or_else(crate::input::drop_pointer::DropPointer::new);
                pointer.sample(&l.window, zoom).or(f.at.take())
            },
        );
        // Sampled even when still, which also notices a target that unmounted under the drag.
        if !queued && let Some(at) = f.pointer.as_ref().and_then(|p| p.sample(&l.window, zoom)) {
            f.at = Some(at);
            let now = crate::time::now();
            let _input = crate::input::time::scope(Some(now));
            queued = rt.drag_files_to(Some(at));
            if queued { crate::input::time::record_last(now); }
        }
        if !queued { return; }
        let covered = self.life.covered() || l.minimized();
        let _ = l;
        self.note_input(crate::time::now());
        if !covered {
            if let Some(l) = &self.win { l.request_redraw(); }
        } else {
            self.covered_frame();
            self.shell(event_loop);
        }
    }

    pub(in crate::window) fn handle_user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        if !self.started { return; }
        if self.poll_gpu_loss() {
            match super::gpu_loss::classify_latched_wake_event(&event) {
                super::gpu_loss::LatchedWakeAction::DrainApp => {
                    self.app_woken.store(false, Ordering::Relaxed);
                    self.drain(event_loop, true);
                }
                super::gpu_loss::LatchedWakeAction::CloseOrHide => {
                    if let WakeEvent::Window(_, req) = event {
                        self.window_request(event_loop, req);
                        self.shell(event_loop);
                    }
                }
                super::gpu_loss::LatchedWakeAction::NonGpuShell => match event {
                    WakeEvent::Focus(_) => self.focus(event_loop),
                    WakeEvent::Position(_, position) => self.set_position(event_loop, position),
                    WakeEvent::Place(_, place) => self.place(event_loop, place),
                    WakeEvent::Level(_, on) => self.set_always_on_top(event_loop, on),
                    WakeEvent::ExcludeFromCapture(_, exclude) => self.set_exclude_from_capture(exclude),
                    WakeEvent::ColorScheme(_, scheme) => self.set_color_scheme(scheme),
                    _ => {}
                },
                super::gpu_loss::LatchedWakeAction::Drop => {}
            }
            return;
        }
        // The display tick a timer's frame waits for draws that frame.
        if !matches!(event, WakeEvent::DisplayTick(_)) && let Some(w) = &mut self.win { w.timer_frame.no_longer_quiet(); }
        if !matches!(event, WakeEvent::DisplayTick(_) | WakeEvent::FrameSlot(_)) && let Some(w) = &mut self.win { w.loop_frame.touched(); }
        match event {
            WakeEvent::App(_) => {
                self.app_woken.store(false, Ordering::Relaxed);
                self.drain(event_loop, true);
            }
            WakeEvent::DisplayTick(_) => {
                if self.win.as_ref().is_some_and(Win::paced) {
                    self.redraw();
                    self.shell(event_loop);
                } else {
                    #[cfg(target_os = "macos")]
                    if let Some(w) = &mut self.win {
                        w.stop_expired_pacing();
                    }
                }
            }
            WakeEvent::Live(_) => {
                if self.app.rt().live_pending() && let Some(w) = self.win.as_ref().filter(|_| self.life.uncovered()) {
                    w.request_redraw();
                }
            }
            WakeEvent::Window(_, request) => {
                self.window_request(event_loop, request);
                self.shell(event_loop);
            }
            WakeEvent::Focus(_) => self.focus(event_loop),
            WakeEvent::Position(_, position) => self.set_position(event_loop, position),
            WakeEvent::Place(_, place) => self.place(event_loop, place),
            WakeEvent::Level(_, on) => self.set_always_on_top(event_loop, on),
            WakeEvent::ExcludeFromCapture(_, exclude) => self.set_exclude_from_capture(exclude),
            WakeEvent::ColorScheme(_, scheme) => self.set_color_scheme(scheme),
            WakeEvent::WindowBackdrop(_, backdrop) => self.set_window_backdrop(backdrop),
            WakeEvent::Frame(_, mut request) => {
                let rt = self.app.rt_mut();
                request.spring = (request.animate && !rt.reduced_motion).then_some(crate::tok::motion::DEFAULT);
                rt.host.window_frame = Some(rt.host.window_frame.unwrap_or_default().merge(request));
                if let Some(w) = self.shown_win() { w.request_redraw(); }
                self.shell(event_loop);
            }
            WakeEvent::A11y(_) => self.a11y_receive(event_loop),
            #[cfg(target_os = "windows")]
            WakeEvent::FrameSlot(_) => {
                // The frame that waited for room in the swap chain draws now, after the input
                // that came meanwhile.
                if let Some(w) = &mut self.win && w.native.slot.waiting() {
                    w.native.slot.came_back();
                    w.request_redraw();
                }
            }
            #[cfg(not(target_os = "windows"))]
            WakeEvent::FrameSlot(_) => {}
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            WakeEvent::GpuReady(_) => self.gpu_ready(),
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            WakeEvent::GpuReady(_) => {}
            #[cfg(target_os = "windows")]
            WakeEvent::ResizeSync(_) => self.handle_resize_sync_wake(),
            #[cfg(not(target_os = "windows"))]
            WakeEvent::ResizeSync(_) => {}
            #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
            WakeEvent::MaterialPolicyChanged => {}
            WakeEvent::Reopen | WakeEvent::Turn | WakeEvent::Quit(_) | WakeEvent::QuitRequest | WakeEvent::FileDrop | WakeEvent::LayerPending | WakeEvent::AppColorScheme => {}
        }
    }

    /// Runs what assistive clients asked; an action or stale first activation builds or draws
    /// the frame that runs it.
    fn a11y_receive(&mut self, event_loop: &ActiveEventLoop) {
        if self.poll_gpu_loss() { return; }
        let Some(w) = self.win.as_mut() else { return };
        let Some(adapter) = w.a11y.as_mut() else { return };
        if !adapter.receive_core(self.app.rt_mut(), self.geometry.title) { return; }
        if self.life.shown() && (self.life.covered() || w.minimized()) {
            self.covered_frame();
            self.shell(event_loop);
        } else {
            w.request_redraw();
        }
    }

    /// Sends the tree's changes at the end of the turn, after its frames.
    pub(in crate::window) fn a11y_turn(&mut self) {
        let Some(adapter) = self.win.as_mut().and_then(|w| w.a11y.as_mut()) else { return };
        adapter.sync_core(self.app.rt_mut());
    }

    /// The window's focus and place, which the accessibility adapter reports.
    fn a11y_window_event(&mut self, event: &WindowEvent) {
        let Some(w) = self.win.as_mut() else { return };
        let Some(adapter) = w.a11y.as_mut() else { return };
        match event {
            WindowEvent::Focused(focused) => adapter.focus(*focused),
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                let rect = |p: winit::dpi::PhysicalPosition<i32>, s: winit::dpi::PhysicalSize<u32>| [f64::from(p.x), f64::from(p.y), f64::from(s.width), f64::from(s.height)];
                let (Ok(outer), Ok(inner)) = (w.window.outer_position(), w.window.inner_position()) else { return };
                adapter.bounds(rect(outer, w.window.outer_size()), rect(inner, w.window.inner_size()));
            }
            _ => {}
        }
    }

    pub(in crate::window) fn handle_window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.win.as_ref().is_none_or(|w| w.window.id() != id) {
            return;
        }
        // A window replaying `--window-script` takes its pointer from the script alone.
        if super::window_script::drops(self.window_script.is_some(), &event) {
            return;
        }
        if self.poll_gpu_loss() {
            match super::gpu_loss::classify_latched_window_event(&event) {
                super::gpu_loss::LatchedEvent::Close => self.close_requested(event_loop),
                super::gpu_loss::LatchedEvent::ModifiersChanged(state) => { self.mods = state; }
                super::gpu_loss::LatchedEvent::Keyboard => {
                    if let WindowEvent::KeyboardInput { ref event, .. } = event {
                        self.handle_latched_keyboard(event_loop, event);
                    }
                }
                super::gpu_loss::LatchedEvent::Drop => {}
            }
            return;
        }
        if crate::input::time::current().is_some() {
            self.note_input(crate::time::now());
        }
        self.a11y_window_event(&event);
        // What says whether other windows cover it (`look`): the pointer reaching it shows some
        // of it; its own move, size or focus makes the next frame look.
        #[cfg(target_os = "windows")]
        match &event {
            WindowEvent::CursorMoved { .. } | WindowEvent::CursorEntered { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => self.sighted(),
            WindowEvent::Focused(true) | WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.look_stale(),
            _ => {}
        }
        if !matches!(event, WindowEvent::RedrawRequested | WindowEvent::CursorMoved { .. }) && let Some(w) = &mut self.win {
            w.timer_frame.no_longer_quiet();
            w.loop_frame.touched();
        }
        #[cfg(target_os = "windows")]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. })
            && let Some(w) = &mut self.win
        {
            let _ = w.native.poll_pace.step(crate::window::poll_pace::PollPaceEvent::DisplayChanged);
        }
        #[cfg(target_os = "macos")]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Occluded(_))
            && let Some(w) = &mut self.win && let Some(pacing) = &mut w.native.pacing {
            pacing.update_display(&w.window);
        }
        if matches!(&event, WindowEvent::ScaleFactorChanged { .. }) { self.apply_placement(event_loop); }
        match event {
            WindowEvent::Focused(focused) => {
                // winit queues an initial false from its hidden source window before
                // adoption. It must not dismiss an already-key panel on first open.
                #[cfg(target_os = "macos")]
                let stale = self.win.as_ref().and_then(|w| w.native.panel.as_ref()).is_some_and(|p| p.is_key() != focused);
                #[cfg(not(target_os = "macos"))]
                let stale = false;
                // Before any return below: a window that goes to the background (deactivated,
                // hidden or another window made key) must not keep blocking other apps' keys.
                if !stale { self.secure_input.window_focus(focused); }
                if self.launcher() && self.win.as_ref().is_some_and(|w| !w.first_frame.notified()) { return; }
                if stale { return; }
                #[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
                if let Some(w) = &self.win { crate::file_dialog::window_focus(&w.window, focused && self.life.shown()); }
                // Deliver independently of drawable availability, including hide/occlusion.
                self.app.window_focus(focused);
                // The key window's App takes the menu bar, so its items act in this window.
                if focused { self.menus.claim(); }
                self.shell(event_loop);
                if self.life.finished() || self.win.is_none() { return; }
            }
            WindowEvent::CloseRequested => {
                self.close_requested(event_loop);
                return;
            }
            WindowEvent::HoveredFile(path) => { self.files.file(path, false); return; }
            WindowEvent::DroppedFile(path) => { self.files.file(path, true); return; }
            WindowEvent::HoveredFileCancelled => { self.files.steps.push(FileStep::Leave); return; }
            WindowEvent::RedrawRequested => {
                self.paint(event_loop);
                return;
            }
            _ => {}
        }
        // Occluded or minimised mid-move: land, so the window comes back at its new size.
        let stops = match &event {
            WindowEvent::Occluded(occluded) => *occluded && !(self.macos_window.launcher && cfg!(target_os = "macos")),
            WindowEvent::Resized(size) => size.width == 0 || size.height == 0,
            _ => false,
        };
        if stops { self.land_frame(); }
        // Minimised on Windows, where no occlusion event need say so.
        if matches!(event, WindowEvent::Resized(size) if size.width == 0 || size.height == 0) { self.rest_step(RestEvent::Unseen); }
        if let WindowEvent::Occluded(occluded) = event {
            crate::marks::mark(if occluded { "window.covered" } else { "window.uncovered" });
            // Alpha-zero launcher panels remain renderable on explicit invalidation.
            // Their native occlusion transitions must not settle or redraw a reveal.
            if !(self.macos_window.launcher && cfg!(target_os = "macos")) { self.set_cover(Cover::System, occluded); }
            return;
        }
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. }) { self.sync_origin(); }
        // A window the CPU renderer draws has no GPU yet; its input goes on all the same.
        let Some(l) = self.win.as_mut() else { return };
        let gpu = self.gpu.as_ref().map(|(gpu, _)| &**gpu);
        let app = &mut *self.app;
        let scale = l.window.scale_factor() as f32;
        let (chrome, titlebar) = (self.chrome, self.titlebar);
        // A finger presses through `WindowEvent::Touch`; the mouse events a system makes up
        // from it (Windows promotes touches) would press a second time.
        if matches!(event, WindowEvent::CursorMoved { .. } | WindowEvent::CursorLeft { .. } | WindowEvent::MouseInput { .. })
            && (app.rt().touching() || host_events::touch_promoted()) { return; }
        let record_reopen = |reopen: &mut crate::window::state::ReopenState, w: &Win, fit_height: bool| {
            if self.life.shown() {
                reopen.record_live(
                    w.live_state(self.wayland),
                    crate::window::state::reopen::use_physical_size(self.wayland),
                    self.placement.is_none() && !self.wayland,
                    !fit_height,
                );
            }
        };
        match event {
            WindowEvent::Moved(_) => {
                record_reopen(&mut self.reopen, l, app.rt().host.fit_height());
                let scale_changed = (scale - app.rt().window_scale()).abs() > f32::EPSILON;
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if !self.wayland && !scale_changed && chrome::sync_wm_state_core(app.rt_mut(), l) {
                    l.request_redraw();
                }
                // winit queues `ScaleFactorChanged` in `kCFRunLoopDefaultMode`, which waits
                // until a window drag ends; resize the surface and draw synchronously inside the
                // current Core Animation transaction when a drag crosses displays so the new
                // `contentsScale` and new-scale drawable land together with no flash.
                if scale_changed {
                    let size = l.window.inner_size();
                    if size.width > 0 && size.height > 0 {
                        l.resize_surface(gpu, size);
                        #[cfg(target_os = "macos")]
                        if let Some(surface) = &l.surface { crate::platform::macos::sync_contents_scale(surface, scale); }
                        let rt = app.rt_mut();
                        rt.set_viewport((l.config.width as f32 / scale, l.config.height as f32 / scale), scale);
                        sync_chrome_core(rt, l, chrome, titlebar);
                        #[cfg(target_os = "macos")]
                        {
                            self.draw(true);
                            if let Some(l) = self.win.as_mut() && !crate::platform::macos::in_live_resize(&l.window) {
                                l.set_live_resize(false);
                            }
                            self.shell(event_loop);
                            return;
                        }
                        #[cfg(not(target_os = "macos"))]
                        l.request_redraw();
                    }
                }
            }
            WindowEvent::Resized(size) => {
                sync_chrome_core(app.rt_mut(), l, chrome, titlebar);
                record_reopen(&mut self.reopen, l, app.rt().host.fit_height());
                #[cfg(target_os = "windows")]
                let maximize_changed = {
                    let maximized = l.is_maximized();
                    let changed = maximized != l.native.last_maximized;
                    l.native.last_maximized = maximized;
                    changed
                };
                let was_live = app.rt().live_visible();
                let now_live = self.life.uncovered() && size.width > 0 && size.height > 0 && !l.minimized();
                app.set_live_visible(now_live);
                // Restored where no occlusion event says so (Windows): as uncovering does, the
                // first frame paints the settled state of what changed meanwhile.
                let restored = !was_live && app.rt().live_visible();
                if restored { app.rt_mut().settle_all(); }
                if size.width == 0 || size.height == 0 {
                    self.secure_input.set(false);
                    l.stop_pacing();
                    l.presented = None;
                    app.rt_mut().cancel_timer_frame();
                    self.idle = true;
                    #[cfg(target_os = "windows")]
                    self.on_resize_sync_event(crate::window::win32::resize_sync::Event::Hide);
                    return;
                }
                let origin_moved = sync_screen_origin_core(app.rt_mut(), &l.window);
                #[cfg(target_os = "windows")]
                if maximize_changed {
                    self.sync_window_backdrop(true);
                }
                #[cfg(target_os = "windows")]
                if self.try_handle_drag_resize(size, scale, event_loop) {
                    return;
                }
                let Some(l) = self.win.as_mut() else { return };
                let gpu = self.gpu.as_ref().map(|(gpu, _)| &**gpu);
                let app = &mut *self.app;
                // A frame Rune moved itself has its drawable and viewport already.
                let resized = l.resize_surface(gpu, size);
                #[cfg(target_os = "macos")]
                if let Some(surface) = &l.surface && (scale - app.rt().window_scale()).abs() > f32::EPSILON {
                    crate::platform::macos::sync_contents_scale(surface, scale);
                }
                let viewport = (l.config.width as f32 / scale, l.config.height as f32 / scale);
                if !resized && viewport == app.rt().window_viewport() && scale == app.rt().window_scale() {
                    if restored || origin_moved || app.rt().stale() { l.request_redraw(); }
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    if self.wayland && self.options.transparent {
                        self.sync_window_backdrop(false);
                    }
                    return;
                }
                let asked = FrameMove::take_jump(&mut self.frame_move, viewport);
                if asked { app.rt_mut().step_viewport(viewport, scale) } else { app.rt_mut().set_viewport(viewport, scale) }
                // The user is dragging the edge. AppKit commits the new size when this returns,
                // and a frame drawn later would leave the old one stretched to it until then.
                #[cfg(target_os = "macos")]
                if crate::platform::macos::in_live_resize(&l.window) {
                    if FrameMove::take_moving(&mut self.frame_move).is_some() { l.present_with_transaction(false); }
                    self.draw(true);
                    self.shell(event_loop);
                    return;
                }
                // Windows sizes the window in its own loop and shows the new size at once: the
                // swap chain just took it, so a frame drawn later would leave the old pixels
                // stretched over it until then.
                #[cfg(target_os = "windows")]
                if crate::platform::win32::in_size_move(&l.window) {
                    self.draw(true);
                    self.shell(event_loop);
                    return;
                }
                l.request_redraw();
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if self.wayland && self.options.transparent {
                    self.sync_window_backdrop(false);
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                #[cfg(target_os = "macos")]
                if let Some(shadow) = &l.native.shadow { shadow.redraw(); }
                let scale = scale_factor as f32;
                let size = l.window.inner_size();
                // Still the old scale's size for a normal window: Windows resizes it after this
                // event. A maximised or full-screen window is not resized by winit and already
                // carries the new monitor's size from the preceding `Resized`.
                #[cfg(target_os = "windows")]
                let size = {
                    let (width, height) = crate::window::frame::scale_change_size(
                        (size.width, size.height),
                        f64::from(app.rt().window_scale()),
                        scale_factor,
                        l.is_maximized(),
                        l.is_full_screen(),
                    );
                    winit::dpi::PhysicalSize::new(width, height)
                };
                if size.width > 0 && size.height > 0 {
                    let resized = l.resize_surface(gpu, size);
                    #[cfg(target_os = "macos")]
                    if let Some(surface) = &l.surface { crate::platform::macos::sync_contents_scale(surface, scale); }
                    let viewport = (l.config.width as f32 / scale, l.config.height as f32 / scale);
                    let rt = app.rt_mut();
                    if resized || viewport != rt.window_viewport() || scale != rt.window_scale() {
                        rt.set_viewport(viewport, scale);
                        sync_chrome_core(rt, l, chrome, titlebar);
                        #[cfg(target_os = "macos")]
                        {
                            self.draw(true);
                            if let Some(l) = self.win.as_mut() && !crate::platform::macos::in_live_resize(&l.window) {
                                l.set_live_resize(false);
                            }
                            self.shell(event_loop);
                            return;
                        }
                        #[cfg(not(target_os = "macos"))]
                        l.request_redraw();
                    }
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    if self.wayland && self.options.transparent {
                        self.sync_window_backdrop(false);
                    }
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                if self.args.theme.is_none() {
                    app.rt_mut().set_system_dark(theme == Theme::Dark);
                    l.request_redraw();
                    #[cfg(target_os = "windows")]
                    {
                        // winit's `set_theme` does not update `preferred_theme`, so
                        // `WM_SETTINGCHANGE` resets a runtime `ColorScheme::Light`/`Dark`
                        // override to the OS theme; restore the explicit override if one is set.
                        if let Some(explicit) = self
                            .color_scheme
                            .effective_for_window(crate::window::app_color_scheme())
                            .winit_theme()
                        {
                            l.window.set_theme(Some(explicit));
                        }
                        self.sync_window_backdrop(true);
                    }
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                if app.modifiers(&mut self.mods, m.state()) {
                    l.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                let rt = app.rt_mut();
                host_events::refresh_focus_preferences(rt, self.args.scroll_bars);
                self.secure_input.set(focused && self.life.uncovered() && rt.has_secure_focus());
                sync_chrome_core(rt, l, chrome, titlebar);
                if !focused {
                    // `window_focus` wrote the composition into its field: the input method drops its copy.
                    crate::platform::ime::cancel(&l.window);
                    l.window.set_ime_allowed(false);
                    self.ime.clear();
                    // A minimized or occluded window skips redraw.
                    l.apply_cursor_core(rt, false);
                    l.apply_pointer_lock_core(rt);
                } else if rt.stale() {
                    rt.edit.ime_reset = true;
                } else {
                    host_events::sync_ime(&l.window, rt, &mut self.ime, true);
                    l.apply_cursor_core(rt, true);
                }
                if rt.stale() { l.request_redraw(); }
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if pressed { crate::marks::input_arrived(true); }
                if pressed && let Some(text) = crate::platform::ime::take_direct_commit(&l.window) {
                    host_events::ime(app.rt_mut(), &l.window, &mut self.ime, winit::event::Ime::Commit(text));
                    l.request_redraw();
                    return;
                }
                // A menu item handing Rune its key: it acts after, if no handler takes it. So does
                // the window menu's Alt+Space, posted rather than run inside the key's message.
                let logical = pressed.then(|| map_key_event(&event, self.mods)).flatten();
                let unhandled = crate::native_menu::take_unhandled(logical.as_ref()).or_else(|| {
                    logical.as_ref().filter(|ev| ev.is_window_menu_chord() && chrome::has_window_menu(&l.window))
                        .map(|_| crate::runtime::Unhandled::WindowMenu)
                });
                if app.keyboard(&event, self.mods, unhandled) {
                    l.request_redraw();
                }
            }
            WindowEvent::Ime(ime) => {
                host_events::ime(app.rt_mut(), &l.window, &mut self.ime, ime);
                l.request_redraw();
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                host_events::wheel(app.rt_mut(), delta, gesture_phase(phase), scale, self.mods);
                l.request_redraw();
            }
            WindowEvent::PinchGesture { delta, phase, .. } => {
                if host_events::magnify(app.rt_mut(), delta, gesture_phase(phase)) {
                    l.request_redraw();
                }
            }
            WindowEvent::RotationGesture { delta, phase, .. } => {
                if host_events::rotate(app.rt_mut(), delta, gesture_phase(phase)) {
                    l.request_redraw();
                }
            }
            WindowEvent::DoubleTapGesture { .. } => {
                if host_events::smart_magnify(app.rt_mut()) {
                    l.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if l.native.drag_serial.is_some() {
                    crate::platform::wayland::serial::wire::drain_window(&l.window);
                }
                let input_scale = app.rt().input_scale();
                let (redraw, drag_out) = cursor_moved_core(
                    app,
                    &mut l.timer_frame,
                    &mut l.loop_frame,
                    position.x as f32 / input_scale,
                    position.y as f32 / input_scale,
                );
                // Deferred mouse drags on Linux cross the drag threshold during motion while the
                // original press serial is still current on Wayland.
                if let Some(action) = app.rt_mut().take_titlebar_action() {
                    l.run_titlebar_action(app.rt_mut(), action);
                }
                // AppKit needs the drag session started inside this mouse-dragged event.
                if let Some(start) = drag_out {
                    begin_host_drag_out(self.gpu.as_ref(), l, app.rt(), &self.native_handle, scale, start);
                }
                if redraw {
                    l.request_redraw();
                } else {
                    l.apply_cursor_core(app.rt(), l.window.has_focus());
                }
            }
            WindowEvent::CursorLeft { .. } => {
                app.rt_mut().pointer_leave();
                l.request_redraw();
            }
            WindowEvent::Touch(touch) => {
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if l.native.drag_serial.is_some() {
                    crate::platform::wayland::serial::wire::drain_window(&l.window);
                }
                #[cfg(target_os = "macos")]
                if touch.phase == winit::event::TouchPhase::Started {
                    app.rt_mut().set_native_titlebar_band(mac::native_titlebar_band(&l.window));
                }
                app.touch(&touch);
                if let Some(action) = app.rt_mut().take_titlebar_action() {
                    l.run_titlebar_action(app.rt_mut(), action);
                }
                if let Some(edge) = app.rt_mut().take_resize_action() {
                    let _ = l.drag_resize_window(edge);
                }
                if let Some(start) = app.rt_mut().take_drag_out() {
                    begin_host_drag_out(self.gpu.as_ref(), l, app.rt(), &self.native_handle, scale, start);
                }
                l.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if l.native.drag_serial.is_some() {
                    crate::platform::wayland::serial::wire::drain_window(&l.window);
                }
                if state == ElementState::Pressed {
                    crate::marks::input_arrived(false);
                    self.control_click = cfg!(target_os = "macos") && self.mods.control_key();
                }
                if let Some((x, y)) = button_at(app, &l.window) {
                    match (state, self.control_click) {
                        (ElementState::Pressed, true) => app.rt_mut().secondary_down(x, y),
                        (ElementState::Released, true) => app.secondary_up(x, y),
                        (ElementState::Pressed, false) => {
                            #[cfg(target_os = "macos")]
                            app.rt_mut().set_native_titlebar_band(mac::native_titlebar_band(&l.window));
                            app.pointer_down(x, y);
                            crate::marks::input_handled(false);
                            app.rt_mut().physical_input(crate::PhysicalInput::Modifiers(map_mods(self.mods)));
                            // AppKit needs the drag started inside this mouse-down.
                            if let Some(action) = app.rt_mut().take_titlebar_action() {
                                l.run_titlebar_action(app.rt_mut(), action);
                            }
                            if let Some(edge) = app.rt_mut().take_resize_action() {
                                let _ = l.drag_resize_window(edge);
                            }
                        }
                        (ElementState::Released, false) => app.pointer_up(x, y),
                    }
                }
                if state == ElementState::Released { self.control_click = false; }
                l.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => {
                if let Some((x, y)) = button_at(app, &l.window) {
                    match state {
                        ElementState::Pressed => app.rt_mut().secondary_down(x, y),
                        ElementState::Released => app.secondary_up(x, y),
                    }
                    // At the pointer, while it's still where the click was (on press for Wayland's
                    // button-press serial, on release for Windows and X11).
                    let rt = app.rt_mut();
                    if let Some(menu) = rt.take_window_menu() { chrome::window_menu(&l.window, menu, Some((x * rt.zoom(), y * rt.zoom()))); }
                }
                l.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some((x, y)) = button_at(app, &l.window) {
                    match state {
                        ElementState::Pressed => app.button_down(x, y, pointer_button(button)),
                        ElementState::Released => app.button_up(x, y, pointer_button(button)),
                    }
                }
                l.request_redraw();
            }
            _ => {}
        }
    }
}

/// Starts an armed OS drag-out session from a pointer or touch move event.
fn begin_host_drag_out(
    gpu: Option<&(std::sync::Arc<crate::render::Gpu>, crate::render::Renderer)>,
    l: &mut crate::window::surface::Win,
    rt: &crate::runtime::RuntimeCore,
    native_handle: &crate::window::handle::WindowHandle,
    scale: f32,
    start: crate::drag_out::DragOutStart,
) {
    let image = gpu.filter(|_| start.item.drag_image().is_none()).and_then(|(gpu, renderer)| {
        crate::drag_out::snapshot(gpu, renderer, (l.config.width, l.config.height), scale, rt.clear_color(), start.rect)
    });
    let (sink, path) = (rt.drag_end_sink(), start.path.clone());
    let handle = native_handle.clone();
    crate::drag_out::begin(&l.window, start, image, Box::new(move |outcome| {
        sink.borrow_mut().push((path, outcome));
        handle.send(WakeEvent::App);
    }));
    l.apply_cursor_core(rt, false);
}

/// Applies a pointer move in view points, returning whether the host must request a redraw and
/// any OS drag-out session the move armed. Leaves `timer_frame` quiet and `loop_frame` untouched
/// when the move changed nothing the next frame would run or paint.
pub(super) fn cursor_moved_core(
    app: &mut dyn WindowApp,
    timer_frame: &mut crate::window::surface::TimerFrame,
    loop_frame: &mut crate::window::surface::LoopFrame,
    x: f32,
    y: f32,
) -> (bool, Option<crate::drag_out::DragOutStart>) {
    let changed = app.observe_pointer(x, y);
    let drag_out = app.rt_mut().take_drag_out();
    let redraw = changed || drag_out.is_some();
    if redraw {
        timer_frame.no_longer_quiet();
        loop_frame.touched();
    }
    (redraw, drag_out)
}

/// Runs the updates queued before launch with `run`, as the host `shortcuts` guards, as every
/// later drain does, so an activation queued before launch runs in its place among them.
pub(in crate::window) fn startup_updates<S: 'static>(shared: &Shared<S>, shortcuts: &crate::shortcuts::Guard, run: impl FnMut(crate::app::Update<S>)) {
    let _receiving = shortcuts.receiving();
    shared.take_updates().into_iter().for_each(run);
}

/// Where a mouse button acts. On Windows, the point its message carries
/// (`platform::win32::press_point`). Elsewhere, where the runtime last saw the pointer, else
/// where the system says it is: a menu that took the mouse (`release_pointer_for_popup`) leaves
/// the runtime no pointer, and winit reports no move for a cursor that hasn't moved since, so a
/// click at the same spot after the menu closes would otherwise be lost. The pointer moves
/// there first, so hover is under it as the press lands.
fn button_at(app: &mut dyn WindowApp, window: &Window) -> Option<(f32, f32)> {
    #[cfg(target_os = "windows")]
    if let Some((x, y)) = crate::platform::win32::press_point::current(window) {
        let input_scale = app.rt().input_scale();
        let at = (x as f32 / input_scale, y as f32 / input_scale);
        if app.rt().pointer() != Some(at) { app.pointer_move(at.0, at.1); }
        return Some(at);
    }
    if let Some(at) = app.rt().pointer() { return Some(at); }
    let (x, y) = crate::input::drop_pointer::DropPointer::new().sample(window, app.rt().zoom())?;
    app.pointer_move(x, y);
    Some((x, y))
}

impl Host {
    /// The system's paint (`RedrawRequested`), or one the loop runs for a window whose paint
    /// the system keeps putting off (`Redraw`).
    pub(in crate::window) fn paint(&mut self, event_loop: &ActiveEventLoop) {
        // The frame a due timer asked for drew, or ran covered in `draw`.
        if let Some(l) = &mut self.win { l.timer_frame.drawn(); }
        // AppKit paints off its own display link; elsewhere a paint holds what it asks for.
        #[cfg(not(target_os = "macos"))]
        if let Some(l) = &self.win { l.redraw_step(RedrawEvent::Paint); }
        #[cfg(target_os = "macos")]
        let satisfied = self.win.as_ref().is_some_and(Win::take_redraw_on_paint);
        // A new accent, scroller style, light/dark, reduced-motion or double-click preference asks
        // every window to redraw.
        {
            let rt = self.app.rt_mut();
            rt.set_system_accent(crate::system_accent());
            rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(self.args.scroll_bars));
            #[cfg(target_os = "linux")]
            {
                rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
                rt.double_click_interval = crate::platform::preferences::double_click_interval();
            }
            if self.args.theme.is_none() && let Some(dark) = self.win.as_ref().and_then(|l| crate::platform::preferences::desktop_dark(&l.window)) {
                rt.set_system_dark(dark);
            }
        }
        #[cfg(target_os = "macos")]
        if self.win.as_ref().is_some_and(|l| l.native.full_screen.take()) {
            if let Some(l) = self.win.as_ref() {
                sync_chrome_core(self.app.rt_mut(), l, self.chrome, self.titlebar);
            }
            self.record_live_window_state();
        }
        // A `DisplayTick` that already drew and settled the queued work leaves a stale
        // `RedrawRequested` in winit's queue. Skip it while resting with nothing changed so it
        // cannot bump a coasting display link back to `Active` for a duplicate frame.
        #[cfg(target_os = "macos")]
        if satisfied && self.win.as_ref().is_some_and(|l| l.presented.is_some()) {
            let rt = self.app.rt();
            if !rt.stale() && !rt.live_pending() && !rt.has_pending_input() && rt.host.window_frame.is_none() {
                return;
            }
        }
        // The window procedure asks for this paint when a caption button's hover or
        // press changes.
        #[cfg(target_os = "windows")]
        if let Some(l) = &self.win { sync_chrome_core(self.app.rt_mut(), l, self.chrome, self.titlebar); }
        // A Linux frame of Rune's own takes the desktop's button layout, which the settings
        // portal asks for this paint to show, and whether the window is maximized.
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if crate::titlebar::linux::chosen() && let Some(l) = &self.win { sync_chrome_core(self.app.rt_mut(), l, self.chrome, self.titlebar); }
        // X11 `PropertyNotify` on EWMH/GTK WM-state atoms sets a per-window flag and asks for
        // `RedrawRequested`; sync chrome only when that flag fired (zero IPC on ordinary redraws).
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if !self.wayland && let Some(l) = &self.win
            && winit::platform::x11::WindowExtX11::take_wm_state_changed(&*l.window)
        {
            sync_chrome_core(self.app.rt_mut(), l, self.chrome, self.titlebar);
        }
        self.redraw();
        self.shell(event_loop);
        // What asked for a frame while it painted waits for the turn's end.
        #[cfg(not(target_os = "macos"))]
        if let Some(l) = &self.win { l.redraw_step(RedrawEvent::Painted); }
    }
}
