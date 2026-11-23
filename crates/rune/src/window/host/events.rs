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
use crate::runtime::TitlebarAction;
use crate::window::chrome::{self, sync_chrome_core, titlebar_double_click};
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
        if self.launcher() {
            self.create(event_loop, None, false);
            if self.open_at_start && self.options.visible { self.reveal_launcher(event_loop); }
        } else if self.open_at_start {
            self.open(event_loop, None);
        }
        self.started = true;
        self.drain(event_loop, false);
    }

    /// A hidden window delivers queued input without drawing, such as the releases of the
    /// modifier keys held when it hid, rather than when it next shows. A shown window's
    /// redraw delivers it, as does a hidden launcher's.
    pub(in crate::window) fn hidden_input(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.life.finished() || self.win.as_ref().is_some_and(|_| self.life.shown() || self.launcher()) { return; }
        if !self.app.rt().keys_queued() { return; }
        self.app.hidden_timer_frame();
        self.clipboard.flush_core(self.app.rt_mut());
        self.shell(event_loop);
    }

    #[cfg(target_os = "macos")]
    pub(in crate::window) fn fn_key(&mut self) {
        let Some(l) = &self.win else { return };
        let Some(changes) = l.native.fn_key.as_ref().map(|f| f.take()) else { return };
        let mut redraw = false;
        for pressed in changes {
            redraw |= self.app.physical_with_fallback(crate::PhysicalInput::Key {
                key: winit::keyboard::KeyCode::Fn, mods: map_mods(self.mods), pressed, repeat: false,
            }, None);
        }
        if redraw { l.request_redraw(); }
    }

    /// Hands the runtime the file drag winit reported since the last pass, in order: the files
    /// that entered, as one drag, and those dropped, as one drop, each at a fresh pointer
    /// sample; and, while the drag is over the window, a new sample, which redraws only when it
    /// changes the drop target. Runs even while the window is covered, as queued input does.
    pub(in crate::window) fn flush_files(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "macos")]
        let native = self.win.as_ref().map(|l| crate::platform::macos::drop::take(&l.window)).unwrap_or_default();
        #[cfg(target_os = "windows")]
        let native = self.win.as_ref().map(|l| crate::platform::win32::drop_target::take(&l.window)).unwrap_or_default();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let native: Vec<FileStep> = Vec::new();
        let Some(l) = self.win.as_ref().filter(|_| self.life.shown()) else {
            self.files = FileDrop::default();
            return;
        };
        let f = &mut self.files;
        let rt = self.app.rt_mut();
        let zoom = rt.zoom();
        let view_at = |(x, y): (f32, f32)| (x / zoom, y / zoom);
        let mut queued = false;
        // Native drop targets (AppKit, Win32 OLE) report each step in window points; winit's
        // fallback events omit the position, so the host samples the pointer for them.
        for step in native.into_iter().chain(std::mem::take(&mut f.steps)) {
            queued |= match step {
                FileStep::Enter(paths, Some(p)) => { rt.drag_files(paths, Some(view_at(p))); true }
                FileStep::Enter(paths, None) => {
                    let pointer = crate::input::drop_pointer::DropPointer::new();
                    f.at = pointer.sample(&l.window, zoom).or_else(|| rt.pointer());
                    f.pointer = Some(pointer);
                    rt.drag_files(paths, f.at);
                    true
                }
                FileStep::Move(p) => rt.drag_files_to(Some(view_at(p))),
                FileStep::Drop(paths, Some(p)) => { rt.drop_files(paths, Some(view_at(p))); true }
                FileStep::Drop(paths, None) => {
                    // A drop with no drag before it (Windows' WM_DROPFILES) samples too.
                    let pointer = f.pointer.take().unwrap_or_else(crate::input::drop_pointer::DropPointer::new);
                    let at = pointer.sample(&l.window, zoom).or(f.at.take()).or_else(|| rt.pointer());
                    rt.drop_files(paths, at);
                    true
                }
                FileStep::Leave => {
                    f.pointer = None;
                    f.at = None;
                    rt.drag_files_leave();
                    true
                }
            };
        }
        // Sampled even when still, which also notices a target that unmounted under the drag.
        if !queued && let Some(at) = f.pointer.as_ref().and_then(|p| p.sample(&l.window, zoom)) {
            f.at = Some(at);
            queued = rt.drag_files_to(Some(at));
        }
        if !queued { return; }
        if !self.life.covered() && !l.minimized() { l.request_redraw(); } else {
            self.covered_frame();
            self.shell(event_loop);
        }
    }

    pub(in crate::window) fn handle_user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        if !self.started { return; }
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
            WakeEvent::Frame(_, mut request) => {
                let rt = self.app.rt_mut();
                request.spring = (request.animate && !rt.reduced_motion).then_some(crate::tok::motion::DEFAULT);
                rt.host.window_frame = Some(rt.host.window_frame.unwrap_or_default().merge(request));
                if let Some(w) = self.shown_win() { w.request_redraw(); }
                self.shell(event_loop);
            }
            WakeEvent::A11y(_) => self.a11y_receive(),
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
            #[cfg(target_os = "windows")]
            WakeEvent::GpuReady(_) => self.gpu_ready(),
            #[cfg(not(target_os = "windows"))]
            WakeEvent::GpuReady(_) => {}
            WakeEvent::Reopen | WakeEvent::Turn | WakeEvent::Quit(_) => {}
        }
    }

    /// Runs what assistive clients asked; an action draws the frame that runs it.
    fn a11y_receive(&mut self) {
        let Some(w) = self.win.as_mut() else { return };
        let Some(adapter) = w.a11y.as_mut() else { return };
        if adapter.receive_core(self.app.rt_mut(), self.geometry.title) { w.request_redraw(); }
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
                self.close(event_loop);
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
        match event {
            WindowEvent::Moved(_) => {
                // winit queues `ScaleFactorChanged` in `kCFRunLoopDefaultMode`, which waits
                // until a window drag ends; resize the surface and draw synchronously inside the
                // current Core Animation transaction when a drag crosses displays so the new
                // `contentsScale` and new-scale drawable land together with no flash.
                if (scale - app.rt().window_scale()).abs() > f32::EPSILON {
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
                    self.idle = true; return;
                }
                let origin_moved = sync_screen_origin_core(app.rt_mut(), &l.window);
                // A frame Rune moved itself has its drawable and viewport already.
                let resized = l.resize_surface(gpu, size);
                #[cfg(target_os = "macos")]
                if let Some(surface) = &l.surface && (scale - app.rt().window_scale()).abs() > f32::EPSILON {
                    crate::platform::macos::sync_contents_scale(surface, scale);
                }
                let viewport = (l.config.width as f32 / scale, l.config.height as f32 / scale);
                if !resized && viewport == app.rt().window_viewport() && scale == app.rt().window_scale() {
                    if restored || origin_moved { l.request_redraw(); }
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
                        l.window.is_maximized(),
                        l.window.fullscreen().is_some(),
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
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                if self.args.theme.is_none() {
                    app.rt_mut().set_system_dark(theme == Theme::Dark);
                    l.request_redraw();
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
                if app.keyboard(&self.clipboard, &event, self.mods, unhandled) {
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
                let input_scale = app.rt().input_scale();
                let (redraw, drag_out) = cursor_moved_core(
                    app,
                    &mut l.timer_frame,
                    &mut l.loop_frame,
                    position.x as f32 / input_scale,
                    position.y as f32 / input_scale,
                );
                // AppKit needs the drag session started inside this mouse-dragged event.
                if let Some(start) = drag_out {
                    let rt = app.rt();
                    let image = self.gpu.as_ref().filter(|_| start.item.drag_image().is_none()).and_then(|(gpu, renderer)|
                        crate::drag_out::snapshot(gpu, renderer, (l.config.width, l.config.height), scale, rt.clear_color(), start.rect));
                    let (sink, path) = (rt.drag_end_sink(), start.path.clone());
                    let handle = self.native_handle.clone();
                    crate::drag_out::begin(&l.window, start, image, Box::new(move |outcome| {
                        sink.borrow_mut().push((path, outcome));
                        handle.send(WakeEvent::App);
                    }));
                    l.apply_cursor_core(rt, false);
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
                app.touch(&touch);
                l.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
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
                            match app.rt_mut().take_titlebar_action() {
                                Some(TitlebarAction::Drag) => {
                                    l.window.drag_window().ok();
                                }
                                Some(TitlebarAction::DoubleClick) => titlebar_double_click(&l.window),
                                None => {}
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
                        ElementState::Released => {
                            app.secondary_up(x, y);
                            // At the pointer, while it's still where the click was.
                            let rt = app.rt_mut();
                            if let Some(menu) = rt.take_window_menu() { chrome::window_menu(&l.window, menu, Some((x * rt.zoom(), y * rt.zoom()))); }
                        }
                    }
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
        // A new accent, scroller style or light/dark preference asks every window to redraw.
        {
            let rt = self.app.rt_mut();
            rt.set_system_accent(crate::system_accent());
            rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(self.args.scroll_bars));
            if self.args.theme.is_none() && let Some(dark) = self.win.as_ref().and_then(|l| crate::platform::preferences::desktop_dark(&l.window)) {
                rt.set_system_dark(dark);
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(l) = self.win.as_ref().filter(|l| l.native.full_screen.take()) {
            sync_chrome_core(self.app.rt_mut(), l, self.chrome, self.titlebar);
        }
        // The window procedure asks for this paint when a caption button's hover or
        // press changes.
        #[cfg(target_os = "windows")]
        if let Some(l) = &self.win { sync_chrome_core(self.app.rt_mut(), l, self.chrome, self.titlebar); }
        self.redraw();
        self.shell(event_loop);
        // What asked for a frame while it painted waits for the turn's end.
        #[cfg(not(target_os = "macos"))]
        if let Some(l) = &self.win { l.redraw_step(RedrawEvent::Painted); }
    }
}
