//! Events a host handles: resuming, input while hidden, the fn key, file drops, and user and
//! window events.

use super::*;

impl<S: 'static> Host<S> {
    pub(super) fn resume(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            return;
        }
        // Updates queued before launch (the link that launched the app) show in the first frame.
        for f in self.shared.take_updates() { self.rt.update_if(f); self.menus.drain(); }
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
    pub(super) fn hidden_input(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.life.finished() || self.win.as_ref().is_some_and(|_| self.life.shown() || self.launcher()) { return; }
        if !self.rt.keys_queued() { return; }
        self.rt.hidden_timer_frame();
        self.clipboard.flush(&mut self.rt);
        self.shell(event_loop);
    }

    #[cfg(target_os = "macos")]
    pub(super) fn fn_key(&mut self) {
        let Some(l) = &self.win else { return };
        let Some(changes) = l.native.fn_key.as_ref().map(|f| f.take()) else { return };
        let mut redraw = false;
        for pressed in changes {
            redraw |= self.rt.physical_with_fallback(crate::PhysicalInput::Key {
                key: winit::keyboard::KeyCode::Fn, mods: map_mods(self.mods), pressed, repeat: false,
            }, None);
        }
        if redraw { l.request_redraw(); }
    }

    /// Hands the runtime the file drag winit reported since the last pass, in order: the files
    /// that entered, as one drag, and those dropped, as one drop, each at a fresh pointer
    /// sample; and, while the drag is over the window, a new sample, which redraws only when it
    /// changes the drop target. Runs even while the window is covered, as queued input does.
    pub(super) fn flush_files(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "macos")]
        let appkit = self.win.as_ref().map(|l| crate::platform::macos::drop::take(&l.window)).unwrap_or_default();
        #[cfg(target_os = "windows")]
        let ole = self.win.as_ref().map(|l| crate::platform::win32::drop_target::take(&l.window)).unwrap_or_default();
        let Some(l) = self.win.as_ref().filter(|_| self.life.shown()) else {
            self.files = FileDrop::default();
            return;
        };
        let f = &mut self.files;
        let mut queued = false;
        // AppKit's drags say where they are at every step (`macos_drop`), so nothing samples.
        #[cfg(target_os = "macos")]
        for step in appkit {
            use crate::platform::macos::drop::Step;
            queued |= match step {
                Step::Enter(paths, at) => { self.rt.drag_files(paths, Some(at)); true }
                Step::Move(at) => self.rt.drag_files_to(Some(at)),
                Step::Drop(paths, at) => { self.rt.drop_files(paths, Some(at)); true }
                Step::Leave => { self.rt.drag_files_leave(); true }
            };
        }
        // Rune's OLE target says where each step is too (`platform::win32::drop_target`).
        #[cfg(target_os = "windows")]
        for step in ole {
            use crate::platform::win32::drop_target::Step;
            // Window points, as the pointer's samples are, in the zoomed app's points.
            let z = self.rt.zoom();
            let at = |(x, y): (f32, f32)| Some((x / z, y / z));
            queued |= match step {
                Step::Enter(paths, p) => { self.rt.drag_files(paths, at(p)); true }
                Step::Move(p) => self.rt.drag_files_to(at(p)),
                Step::Drop(paths, p) => { self.rt.drop_files(paths, at(p)); true }
                Step::Leave => { self.rt.drag_files_leave(); true }
            };
        }
        for step in std::mem::take(&mut f.steps) {
            queued = true;
            match step {
                FileStep::Enter(paths) => {
                    let pointer = crate::input::drop_pointer::DropPointer::new();
                    f.at = pointer.sample(&l.window, self.rt.zoom()).or_else(|| self.rt.pointer());
                    f.pointer = Some(pointer);
                    self.rt.drag_files(paths, f.at);
                }
                FileStep::Drop(paths) => {
                    // A drop with no drag before it (Windows' WM_DROPFILES) samples too.
                    let pointer = f.pointer.take().unwrap_or_else(crate::input::drop_pointer::DropPointer::new);
                    let at = pointer.sample(&l.window, self.rt.zoom()).or(f.at.take()).or_else(|| self.rt.pointer());
                    self.rt.drop_files(paths, at);
                }
                FileStep::Leave => {
                    f.pointer = None;
                    f.at = None;
                    self.rt.drag_files_leave();
                }
            }
        }
        // Sampled even when still, which also notices a target that unmounted under the drag.
        if !queued && let Some(at) = f.pointer.as_ref().and_then(|p| p.sample(&l.window, self.rt.zoom())) {
            f.at = Some(at);
            queued = self.rt.drag_files_to(Some(at));
        }
        if !queued { return; }
        if !self.life.covered() && !l.minimized() { l.request_redraw(); } else {
            self.covered_frame();
            self.shell(event_loop);
        }
    }

    pub(super) fn handle_user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        if !self.started { return; }
        // The display tick a timer's frame waits for draws that frame.
        if !matches!(event, WakeEvent::DisplayTick(_)) && let Some(w) = &mut self.win { w.timer_frame.no_longer_quiet(); }
        match event {
            WakeEvent::App(_) => {
                self.drain(event_loop, true);
            }
            WakeEvent::DisplayTick(_) => {
                if self.win.as_ref().is_some_and(Win::paced) {
                    self.redraw();
                    self.shell(event_loop);
                }
            }
            WakeEvent::Live(_) => {
                if self.rt.live_pending() && let Some(w) = self.win.as_ref().filter(|_| self.life.uncovered()) {
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
                request.spring = (request.animate && !self.rt.reduced_motion).then_some(crate::tok::motion::DEFAULT);
                self.rt.window_frame = Some(self.rt.window_frame.unwrap_or_default().merge(request));
                if let Some(w) = self.shown_win() { w.request_redraw(); }
                self.shell(event_loop);
            }
            WakeEvent::A11y(_) => self.a11y_receive(),
            WakeEvent::Covered(_, covered) => {
                if covered { self.land_frame(); }
                self.set_cover(Cover::Windows, covered);
            }
            WakeEvent::Reopen | WakeEvent::Turn | WakeEvent::Quit(_) => {}
        }
    }

    /// Runs what assistive clients asked; an action draws the frame that runs it.
    fn a11y_receive(&mut self) {
        let Some(w) = self.win.as_mut() else { return };
        let Some(adapter) = w.a11y.as_mut() else { return };
        if adapter.receive(&mut self.rt, self.spec.title) { w.request_redraw(); }
    }

    /// Sends the tree's changes at the end of the turn, after its frames.
    pub(super) fn a11y_turn(&mut self) {
        let Some(adapter) = self.win.as_mut().and_then(|w| w.a11y.as_mut()) else { return };
        adapter.sync(&mut self.rt);
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

    pub(super) fn handle_window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.win.as_ref().is_none_or(|w| w.window.id() != id) {
            return;
        }
        self.a11y_window_event(&event);
        if !matches!(event, WindowEvent::RedrawRequested) && let Some(w) = &mut self.win { w.timer_frame.no_longer_quiet(); }
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
                self.rt.window_focus(focused);
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
        if let WindowEvent::Occluded(occluded) = event {
            // Alpha-zero launcher panels remain renderable on explicit invalidation.
            // Their native occlusion transitions must not settle or redraw a reveal.
            if !(self.macos_window.launcher && cfg!(target_os = "macos")) { self.set_cover(Cover::System, occluded); }
            return;
        }
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. }) { self.sync_origin(); }
        let (Some(l), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
        let rt = &mut self.rt;
        let scale = l.window.scale_factor() as f32;
        let (chrome, titlebar) = (self.chrome, self.titlebar);
        // A finger presses through `WindowEvent::Touch`; the mouse events a system makes up
        // from it (Windows promotes touches) would press a second time.
        if matches!(event, WindowEvent::CursorMoved { .. } | WindowEvent::CursorLeft { .. } | WindowEvent::MouseInput { .. })
            && (rt.touching() || touch_promoted()) { return; }
        match event {
            WindowEvent::Moved(_) => {
                // winit queues `ScaleFactorChanged` in `kCFRunLoopDefaultMode`, which waits
                // until a window drag ends; resize the surface now when a drag crosses displays
                // so top-left gravity never shows the old scale's drawable at half or double size.
                if (scale - rt.window_scale()).abs() > f32::EPSILON {
                    let size = l.window.inner_size();
                    if size.width > 0 && size.height > 0 {
                        l.resize_surface(gpu, size);
                        rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
                        sync_chrome(rt, l, chrome, titlebar);
                        l.request_redraw();
                    }
                }
            }
            WindowEvent::Resized(size) => {
                sync_chrome(rt, l, chrome, titlebar);
                let was_live = rt.live_visible();
                rt.set_live_visible(self.life.uncovered() && size.width > 0 && size.height > 0 && !l.minimized());
                // Restored where no occlusion event says so (Windows): as uncovering does, the
                // first frame paints the settled state of what changed meanwhile.
                let restored = !was_live && rt.live_visible();
                if restored { rt.settle_all(); }
                if size.width == 0 || size.height == 0 {
                    self.secure_input.set(false);
                    l.stop_pacing();
                    l.presented = None;
                    rt.cancel_timer_frame();
                    self.idle = true; return;
                }
                let origin_moved = sync_screen_origin(rt, &l.window);
                // A frame Rune moved itself has its drawable and viewport already.
                let resized = l.resize_surface(gpu, size);
                let viewport = (size.width as f32 / scale, size.height as f32 / scale);
                if !resized && viewport == rt.window_viewport() && scale == rt.window_scale() {
                    if restored || origin_moved { l.request_redraw(); }
                    return;
                }
                let asked = self.frame_jump.take().is_some_and(|(w, h)| (w - viewport.0).abs() < 1.0 && (h - viewport.1).abs() < 1.0);
                if asked { rt.step_viewport(viewport, scale) } else { rt.set_viewport(viewport, scale) }
                // The user is dragging the edge. AppKit commits the new size when this returns,
                // and a frame drawn later would leave the old one stretched to it until then.
                #[cfg(target_os = "macos")]
                if crate::platform::macos::in_live_resize(&l.window) {
                    if self.frame_move.take().is_some() { l.present_with_transaction(false); }
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
                // Still the old scale's size: Windows resizes the window after this event.
                #[cfg(target_os = "windows")]
                let size = {
                    let (width, height) = crate::window::frame::rescaled((size.width, size.height), f64::from(rt.window_scale()), scale_factor);
                    winit::dpi::PhysicalSize::new(width, height)
                };
                if size.width > 0 && size.height > 0 {
                    let resized = l.resize_surface(gpu, size);
                    let viewport = (size.width as f32 / scale, size.height as f32 / scale);
                    if resized || viewport != rt.window_viewport() || scale != rt.window_scale() {
                        rt.set_viewport(viewport, scale);
                        sync_chrome(rt, l, chrome, titlebar);
                        l.request_redraw();
                    }
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                if self.args.theme.is_none() {
                    rt.set_system_dark(theme == Theme::Dark);
                    l.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                if rt.physical_observation(crate::PhysicalInput::Modifiers(map_mods(self.mods))) {
                    l.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
                rt.set_full_keyboard_access(crate::platform::preferences::full_keyboard_access());
                rt.set_system_scroll_bars_state(crate::platform::scroll_bars::reread().with_override(self.args.scroll_bars));
                self.secure_input.set(focused && self.life.uncovered() && rt.has_secure_focus());
                sync_chrome(rt, l, chrome, titlebar);
                if !focused {
                    rt.cancel_composition();
                    l.window.set_ime_allowed(false);
                    self.ime_allowed = false;
                    // A minimized or occluded window skips redraw.
                    l.apply_cursor(rt, false);
                } else if rt.stale() {
                    rt.ime_reset = true;
                } else {
                    let ime_allowed = rt.ime_area.is_some() && !rt.has_secure_focus();
                    if std::mem::take(&mut rt.ime_reset) || ime_allowed != self.ime_allowed {
                        l.window.set_ime_allowed(false);
                        l.window.set_ime_allowed(ime_allowed);
                        self.ime_allowed = ime_allowed;
                    }
                    if let Some(a) = rt.ime_area.map(|a| rt.to_window(a)) {
                        l.window.set_ime_cursor_area(
                            LogicalPosition::new(a[0] as f64, a[1] as f64),
                            LogicalSize::new(a[2] as f64, a[3] as f64),
                        );
                    }
                    l.apply_cursor(rt, true);
                }
                if rt.stale() { l.request_redraw(); }
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                let pressed = event.state == ElementState::Pressed;
                // A release's logical key only matters to `on_key_up` handlers.
                let logical = (pressed || rt.wants_key_up()).then(|| map_key_event(&event, self.mods)).flatten();
                let mut redraw = false;
                // A menu item handing Rune its key: it acts after, if no handler takes it. So does
                // the window menu's Alt+Space, posted rather than run inside the key's message.
                let unhandled = crate::native_menu::take_unhandled(logical.as_ref().filter(|_| pressed)).or_else(|| {
                    logical.as_ref().filter(|ev| pressed && ev.is_window_menu_chord() && chrome::has_window_menu(&l.window))
                        .map(|_| crate::runtime::Unhandled::WindowMenu)
                });
                if let winit::keyboard::PhysicalKey::Code(key) = event.physical_key {
                    redraw = rt.physical_then(crate::PhysicalInput::Key {
                        key, mods: map_mods(self.mods), pressed, repeat: event.repeat,
                    }, logical, key_base(&event), unhandled);
                } else if let Some(ev) = logical {
                    if !pressed { redraw = rt.key_up(ev); }
                    else if ev.edit_command() == Some('v') && rt.accepts_paste() { self.clipboard.read(rt); }
                    else if event.repeat { rt.repeat_key(ev); }
                    else { rt.key(ev); }
                    redraw |= pressed;
                }
                if redraw { l.request_redraw(); }
            }
            WindowEvent::Ime(ime) => {
                match ime {
                    WIme::Preedit(s, cur) => rt.ime(Ime::Preedit(s, cur)),
                    WIme::Commit(s) => rt.ime(Ime::Commit(s)),
                    WIme::Enabled => rt.ime(Ime::Enabled),
                    WIme::Disabled => rt.ime(Ime::Disabled),
                }
                l.request_redraw();
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let shift = crate::runtime::SHIFT_TURNS_WHEEL && self.mods.shift_key();
                // In window points: a wheel turns as far on screen at any zoom, as Chromium's does,
                // and a pinch is the same gesture whatever the zoom.
                let (dx, dy) = crate::runtime::wheel_delta(delta, scale, shift);
                let precise = matches!(delta, winit::event::MouseScrollDelta::PixelDelta(_));
                let phase = gesture_phase(phase);
                match rt.pointer() {
                    // Off macOS a trackpad pinch is a Ctrl + wheel.
                    Some((x, y)) if !cfg!(target_os = "macos") && self.mods.control_key() && rt.pinch_wheel(x, y, dy) => {}
                    Some((x, y)) => rt.wheel_event(crate::WheelEvent::new(dx / rt.zoom(), dy / rt.zoom()).at(x, y).mods(map_mods(self.mods))
                        .phase(phase).precise(precise)),
                    None if phase == crate::GesturePhase::Began => rt.wheel_began(),
                    None if phase == crate::GesturePhase::Ended => rt.wheel_ended(),
                    None => {}
                }
                l.request_redraw();
            }
            WindowEvent::PinchGesture { delta, phase, .. } => {
                if let Some((x, y)) = rt.pointer() {
                    rt.magnify_event(crate::MagnifyEvent::new(delta as f32).at(x, y).phase(gesture_phase(phase)));
                    l.request_redraw();
                }
            }
            WindowEvent::RotationGesture { delta, phase, .. } => {
                if let Some((x, y)) = rt.pointer() {
                    rt.rotate_event(crate::RotateEvent::new(delta).at(x, y).phase(gesture_phase(phase)));
                    l.request_redraw();
                }
            }
            WindowEvent::DoubleTapGesture { .. } => {
                if let Some((x, y)) = rt.pointer() {
                    rt.smart_magnify(x, y);
                    l.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                rt.pointer_move(position.x as f32 / rt.input_scale(), position.y as f32 / rt.input_scale());
                // AppKit needs the drag session started inside this mouse-dragged event.
                if let Some(start) = rt.take_drag_out() {
                    let image = self.gpu.as_ref().filter(|_| start.item.drag_image().is_none()).and_then(|(gpu, renderer)|
                        crate::drag_out::snapshot(gpu, renderer, (l.config.width, l.config.height), scale, rt.clear_color(), start.rect));
                    let (sink, path) = (rt.drag_end_sink(), start.path.clone());
                    let handle = self.native_handle.clone();
                    crate::drag_out::begin(&l.window, start, image, Box::new(move |outcome| {
                        sink.borrow_mut().push((path, outcome));
                        handle.send(WakeEvent::App);
                    }));
                    l.apply_cursor(rt, false);
                }
                l.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                rt.pointer_leave();
                l.request_redraw();
            }
            WindowEvent::Touch(touch) => {
                crate::window::touch(rt, &touch);
                l.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    self.control_click = cfg!(target_os = "macos") && self.mods.control_key();
                }
                if let Some((x, y)) = button_at(rt, &l.window) {
                    match (state, self.control_click) {
                        (ElementState::Pressed, true) => rt.secondary_down(x, y),
                        (ElementState::Released, true) => rt.secondary_up(x, y),
                        (ElementState::Pressed, false) => {
                            #[cfg(target_os = "macos")]
                            rt.set_native_titlebar_band(mac::native_titlebar_band(&l.window));
                            rt.pointer_down(x, y);
                            rt.physical_input(crate::PhysicalInput::Modifiers(map_mods(self.mods)));
                            // AppKit needs the drag started inside this mouse-down.
                            match rt.take_titlebar_action() {
                                Some(TitlebarAction::Drag) => {
                                    let _ = l.window.drag_window();
                                }
                                Some(TitlebarAction::DoubleClick) => titlebar_double_click(&l.window),
                                None => {}
                            }
                        }
                        (ElementState::Released, false) => rt.pointer_up(x, y),
                    }
                }
                if state == ElementState::Released { self.control_click = false; }
                l.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => {
                if let Some((x, y)) = button_at(rt, &l.window) {
                    match state {
                        ElementState::Pressed => rt.secondary_down(x, y),
                        ElementState::Released => {
                            rt.secondary_up(x, y);
                            // At the pointer, while it's still where the click was.
                            if let Some(menu) = rt.take_window_menu() { chrome::window_menu(&l.window, menu, Some((x * rt.zoom(), y * rt.zoom()))); }
                        }
                    }
                }
                l.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some((x, y)) = button_at(rt, &l.window) {
                    match state {
                        ElementState::Pressed => rt.button_down(x, y, pointer_button(button)),
                        ElementState::Released => rt.button_up(x, y, pointer_button(button)),
                    }
                }
                l.request_redraw();
            }
            _ => {}
        }
    }
}

/// Whether the mouse message being handled is one Windows made up from a touch or a pen
/// (`GetMessageExtraInfo` carries `MI_WP_SIGNATURE`). winit handles the message inside the
/// window procedure, so the extra info is still the message's own.
#[cfg(target_os = "windows")]
fn touch_promoted() -> bool {
    const MI_WP_SIGNATURE: u32 = 0xFF51_5700;
    // SAFETY: reads the calling thread's current message info; no pointers involved.
    let info = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo() } as u32;
    info & 0xFFFF_FF00 == MI_WP_SIGNATURE
}

#[cfg(not(target_os = "windows"))]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn touch_promoted() -> bool { false }

/// Where a mouse button acts. On Windows, the point its message carries
/// (`platform::win32::press_point`). Elsewhere, where the runtime last saw the pointer, else
/// where the system says it is: a menu that took the mouse (`release_pointer_for_popup`) leaves
/// the runtime no pointer, and winit reports no move for a cursor that hasn't moved since, so a
/// click at the same spot after the menu closes would otherwise be lost. The pointer moves
/// there first, so hover is under it as the press lands.
fn button_at<S: 'static>(rt: &mut Runtime<S>, window: &winit::window::Window) -> Option<(f32, f32)> {
    #[cfg(target_os = "windows")]
    if let Some((x, y)) = crate::platform::win32::press_point::current(window) {
        let at = (x as f32 / rt.input_scale(), y as f32 / rt.input_scale());
        if rt.pointer() != Some(at) { rt.pointer_move(at.0, at.1); }
        return Some(at);
    }
    if let Some(at) = rt.pointer() { return Some(at); }
    let (x, y) = crate::input::drop_pointer::DropPointer::new().sample(window, rt.zoom())?;
    rt.pointer_move(x, y);
    Some((x, y))
}

impl<S: 'static> Host<S> {
    /// The system's paint (`RedrawRequested`), or one the loop runs for a window whose paint
    /// the system keeps putting off (`Redraw`).
    pub(super) fn paint(&mut self, event_loop: &ActiveEventLoop) {
        // The frame a due timer asked for drew, or ran covered in `draw`.
        if let Some(l) = &mut self.win { l.timer_frame.drawn(); }
        // AppKit paints off its own display link; elsewhere a paint holds what it asks for.
        #[cfg(not(target_os = "macos"))]
        if let Some(l) = &self.win { l.redraw_step(RedrawEvent::Paint); }
        // A new accent, scroller style or light/dark preference asks every window to redraw.
        self.rt.set_system_accent(crate::system_accent());
        self.rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(self.args.scroll_bars));
        if self.args.theme.is_none() && let Some(dark) = self.win.as_ref().and_then(|l| crate::platform::preferences::desktop_dark(&l.window)) {
            self.rt.set_system_dark(dark);
        }
        #[cfg(target_os = "macos")]
        if let Some(l) = self.win.as_ref().filter(|l| l.native.full_screen.take()) {
            sync_chrome(&mut self.rt, l, self.chrome, self.titlebar);
        }
        // The window procedure asks for this paint when a caption button's hover or
        // press changes.
        #[cfg(target_os = "windows")]
        if let Some(l) = &self.win { sync_chrome(&mut self.rt, l, self.chrome, self.titlebar); }
        self.redraw();
        self.shell(event_loop);
        // What asked for a frame while it painted waits for the turn's end.
        #[cfg(not(target_os = "macos"))]
        if let Some(l) = &self.win { l.redraw_step(RedrawEvent::Painted); }
    }
}
