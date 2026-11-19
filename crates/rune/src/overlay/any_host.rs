//! The overlay as a host on the shared event loop: its window events, wake deadline and
//! connection.

use super::*;

impl OverlayCore {
    /// Windows: its next frame waits for room in the swap chain, and comes when there is
    /// (`slot`).
    fn waits_for_slot(&self) -> bool {
        #[cfg(target_os = "windows")]
        return self.surface.live().is_some_and(|l| matches!(l.slot, Slot::Waiting(_)));
        #[cfg(not(target_os = "windows"))]
        false
    }

    pub(super) fn wake_deadline(&self, rt: &RuntimeCore) -> Option<Instant> {
        // A running display link consumes due runtime timers. Keep the independent
        // pointer deadline, but do not spin on an overdue animation timer. Without one (hidden,
        // covered, faded out or unpaced), a timer due sooner than a frame waits a frame.
        let deadline = crate::window::timer_deadline_core(rt);
        // A frame waiting for room in the swap chain comes when there is (`slot`): its timers
        // would turn the loop at once until then.
        let deadline = deadline.filter(|_| !self.waits_for_slot());
        #[cfg(target_os = "macos")]
        let deadline = deadline.filter(|_| !self.surface.live().and_then(|l| l.pacing.as_ref()).is_some_and(|p| p.running()));
        #[cfg(target_os = "macos")]
        let deadline = deadline.into_iter().chain(self.panel.as_ref().and_then(|p| p.pointer_deadline(self.presence.occluded()))).min();
        // A Windows fade-out steps its alpha once a frame.
        #[cfg(target_os = "windows")]
        let deadline = deadline.into_iter().chain(self.presence.fading().then(|| Instant::now() + crate::window::TIMER_FRAME)).min();
        deadline.into_iter().chain(self.backoff.deadline().filter(|_| self.presence.drawing())).chain(self.presence.deadline())
            .chain(self.hidden_pace.deadline().filter(|_| !self.presence.drawing())).min()
    }

    /// Answers what assistive clients asked of the overlay and sends its tree's changes. An
    /// action draws the frame that runs it.
    fn a11y_turn(&mut self, rt: &mut RuntimeCore) -> bool {
        let Some(adapter) = self.surface.live_mut().and_then(|l| l.a11y.as_mut()) else { return false };
        let queued = adapter.service_core(rt, self.name);
        if queued {
            self.dirty = true;
            self.idle = false;
        }
        queued
    }

    pub(super) fn connect(&mut self, app: &mut dyn OverlayApp, wake: Waker, _frame_wake: Waker, _slot_wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>) {
        #[cfg(target_os = "macos")]
        { self.frame_wake = Some(_frame_wake); }
        #[cfg(target_os = "windows")]
        { self.slot_wake = Some(_slot_wake); }
        // All live sources (including Pixels) must share this registration: a second
        // set_live_waker call replaces the wake lane and resets its subscriptions.
        {
            let rt = app.rt_mut();
            rt.set_live_waker(wake.clone());
            rt.open_urls_natively = true;
            rt.record_events = false;
        }
        self.clipboard.connect(wake.clone());
        self.popup.connect(wake.clone());
        self.gpu_pool = gpu;
        *self.shared.wake.lock().unwrap() = Some(wake.clone());
        let rerun = || -> Waker {
            let shared = self.shared.clone();
            Arc::new(move || {
                let first = !std::mem::replace(&mut shared.requests.lock().unwrap().woken, true);
                if first { shared.wake(); }
            })
        };
        // A live-edit patch re-runs the view like new data does, shown or hidden.
        #[cfg(feature = "hot")]
        if crate::hot::enabled() && app.rt().hot.is_none() {
            app.rt_mut().hot = Some(crate::hot::Session::connect(rerun()));
        }
        app.run_setup(rerun());
        // Requests made before the loop existed.
        wake();
    }
}

impl AnyHost for OverlayHost {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        self.core.meter.turn(matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll), crate::frame::surface_stats::slept(cause));
        #[cfg(target_os = "macos")]
        { self.core.occlusion_read = false; }
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll) {
            let now = Instant::now();
            self.core.wake_due(&mut *self.app, now);
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        self.core.wake_deadline(self.app.rt())
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.core.sync(&mut *self.app, event_loop);
    }

    fn vanish(&mut self) { self.core.hide(&mut *self.app); }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        self.core.meter.waited();
        #[cfg(target_os = "windows")]
        if matches!(event, WakeEvent::FrameSlot(_)) {
            // The frame that waited for room in the swap chain draws now, after what changed
            // meanwhile. It waited from a turn `frame::turn` said had a frame to draw, and that
            // turn left `dirty` and `idle` as they were, so the gate still does.
            if let Some(live) = self.core.surface.live_mut() && matches!(live.slot, Slot::Waiting(_)) {
                live.slot.came_back();
            }
        }
        #[cfg(target_os = "macos")]
        if matches!(event, WakeEvent::DisplayTick(_))
            && self.core.surface.live().and_then(|l| l.pacing.as_ref()).is_some_and(|p| p.running()) {
            self.core.draw(&mut *self.app);
        }
        if let WakeEvent::App(_) = event {
            self.core.sync(&mut *self.app, event_loop);
        }
    }

    fn owns(&self, id: WindowId) -> bool {
        let window = match self.core.surface.backing() {
            Some(Backing::Window(w)) => Some(w),
            _ => None,
        };
        window.is_some_and(|w| w.id() == id)
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let core = &mut self.core;
        let app = &mut *self.app;
        if !matches!(event, WindowEvent::RedrawRequested) { core.meter.waited(); }
        #[cfg(target_os = "macos")]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Occluded(_))
            && let Some(live) = core.surface.live_mut() && let Some(pacing) = &mut live.pacing {
            let Backing::Window(window) = &live.backing;
            pacing.update_display(window);
        }
        // The panel's own state, which winit's source window forwards; kept while hidden too.
        #[cfg(target_os = "macos")]
        if let WindowEvent::Occluded(occluded) = event {
            let occluded = core.panel.as_ref().map_or(occluded, |p| p.occluded());
            core.occluded(app, occluded);
            return;
        }
        if let WindowEvent::Focused(focused) = &event {
            #[cfg(target_os = "macos")]
            if core.panel.as_ref().is_some_and(|p| p.is_key() != *focused) { return; }
            app.window_focus(*focused);
            app.rt_mut().ignore_minimize("an overlay");
            match app.rt_mut().shell_request.take() {
                Some(crate::el::Shell::Close) => core.hide(app),
                Some(crate::el::Shell::Quit) => event_loop.exit(),
                Some(crate::el::Shell::Relaunch) => {
                    let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after: crate::quit::After::Relaunch });
                    event_loop.exit();
                }
                None => {}
            }
        }
        // Hidden too, so the retained surface redraws in the new theme before it's revealed.
        if let WindowEvent::ThemeChanged(theme) = event {
            return core.theme_changed(app.rt_mut(), theme == winit::window::Theme::Dark);
        }
        // Hidden too: the queued rects are delivered with the next shown frame.
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
            core.sync_live_origin(app.rt_mut());
        }
        if !core.presence.shown() { return; }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. })
            && let Some(x11) = &core.x11 { x11.update_geometry(); }
        match event {
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(gpu) = core.gpu.clone() {
                    core.rescale(app, &gpu, scale_factor as f32);
                }
            }
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => {
                // A new accent or scroller style asks every window to redraw (`accent::watch`, `scroll_bars::watch`).
                core.accent_changed(app.rt_mut(), crate::system_accent());
                core.scroll_bars_changed(app.rt_mut(), crate::platform::scroll_bars::current());
                core.dirty = true;
            }
            WindowEvent::CursorMoved { position, .. } if core.options.input.resolved(core.interactive) != OverlayInput::None => {
                let scale = core.surface.live().map_or(1.0, |l| l.scale);
                let (x, y) = (position.x as f32 / scale, position.y as f32 / scale);
                #[cfg(target_os = "macos")]
                if let Some(pointer) = core.native_pointer() { return core.native_moved(app, &pointer, x, y); }
                core.cursor_moved(app, x, y);
            }
            WindowEvent::CursorLeft { .. } if core.options.input.resolved(core.interactive) != OverlayInput::None => {
                app.rt_mut().pointer_leave();
                core.dirty = true;
            }
            WindowEvent::ModifiersChanged(m) => {
                core.mods = m.state();
                core.dirty |= app.physical_observation(crate::PhysicalInput::Modifiers(crate::window::map_mods(core.mods)));
            }
            WindowEvent::Focused(_) => {
                app.rt_mut().set_system_reduced_motion(crate::platform::preferences::reduced_motion());
                core.scroll_bars_changed(app.rt_mut(), crate::platform::scroll_bars::reread());
                core.dirty = true;
            }
            WindowEvent::KeyboardInput { event, .. } if core.options.keys == KeyPolicy::OnDemand => {
                let pressed = event.state.is_pressed();
                let fallback = (pressed || app.rt().wants_key_up()).then(|| crate::window::map_key_event(&event, core.mods)).flatten();
                if let winit::keyboard::PhysicalKey::Code(key) = event.physical_key {
                    core.dirty |= app.physical_then(crate::PhysicalInput::Key {
                        key, mods: crate::window::map_mods(core.mods), pressed, repeat: event.repeat,
                    }, fallback, crate::window::key_base(&event), None);
                } else if let Some(key) = fallback {
                    if !pressed { core.dirty |= app.key_up(key); }
                    else if event.repeat { app.rt_mut().repeat_key(key); } else { app.rt_mut().key(key); }
                    core.dirty |= pressed;
                }
            }
            WindowEvent::MouseInput { state, button: winit::event::MouseButton::Left, .. } => {
                #[cfg(target_os = "macos")]
                {
                    match core.native_pointer() {
                        Some(pointer) => core.native_left(app, &pointer, state.is_pressed()),
                        None => core.left_edge(app, None, state.is_pressed(), None),
                    }
                    if !state.is_pressed() && let Some(panel) = &core.panel { panel.pointer_released(); }
                }
                #[cfg(not(target_os = "macos"))]
                if core.options.input.resolved(core.interactive) != OverlayInput::None {
                    #[cfg(target_os = "windows")]
                    let point = core.native.as_ref().and_then(|p| p.take_click(state.is_pressed()));
                    #[cfg(not(target_os = "windows"))]
                    let point: Option<(f32, f32)> = None;
                    core.left_button(app, state.is_pressed(), point);
                }
                core.apply_input(app.rt());
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) -> bool {
        let core = &mut self.core;
        let app = &mut *self.app;
        #[cfg(target_os = "macos")]
        core.fn_key(app);
        // A window switched the font families (`Tx::set_font_families`).
        core.dirty |= app.rt_mut().follow_font_families();
        // A fading-out overlay takes no input; it only draws its exit.
        if !core.presence.shown() {
            core.hidden_input(app);
            let more = core.draw(app);
            return core.a11y_turn(app.rt_mut()) || more;
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(live) = core.surface.live() && let Some(x11) = &core.x11 && let Some((point, desktop)) = x11.take_pointer(live.scale) {
            core.observe_pointer(app, point.0, point.1);
            core.follow_pointer(app, _event_loop, desktop);
        }
        #[cfg(target_os = "windows")]
        if core.surface.live().is_some() && let Some(point) = core.native.as_ref().and_then(|p| p.take_pointer()) && app.rt().pointer() != Some(point) {
            core.observe_pointer(app, point.0, point.1);
            if let Some(point) = crate::platform::win32::pointer_position() { core.follow_pointer(app, _event_loop, point); }
        }
        // Covering or sleeping displays can come without an event the overlay sees.
        #[cfg(target_os = "macos")]
        if core.surface.live().is_some() { core.sync_occlusion(app); }
        #[cfg(target_os = "macos")]
        if core.surface.live().is_some() && let Some(panel) = &core.panel {
            // Covered or asleep, the 120 Hz regions poll rests; input events still sample.
            panel.observe_pointer(Instant::now(), !core.presence.occluded());
            let pointer = panel.pointer_state();
            if let Some((x, y)) = core.turn_position(app, &pointer) && app.rt().pointer() != Some((x, y)) {
                core.observe_pointer(app, x, y);
                if core.options.follow_pointer && core.options.display == DisplayTarget::UnderPointer {
                    let size = core.at.size;
                    core.resolve_size(_event_loop);
                    if size != core.at.size { core.resized(app, _event_loop); }
                    else if let Some(panel) = &core.panel { panel.place(core.at, &core.options); }
                    core.sync_live_origin(app.rt_mut());
                }
            }
        }
        let more = core.draw(app);
        core.apply_cursor(app.rt());
        core.a11y_turn(app.rt_mut()) || more
    }

    fn keeps_loop(&self) -> bool {
        false
    }

    fn cancel_menus(&mut self) { self.app.popup_cancel(&mut self.core.popup); }
}

impl PendingOverlay for OverlayHost {
    fn connect(&mut self, wake: Waker, frame_wake: Waker, slot_wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>) {
        self.core.connect(&mut *self.app, wake, frame_wake, slot_wake, gpu);
    }
}
