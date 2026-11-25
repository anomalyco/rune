//! An overlay's lifetime: making its native window and GPU surface, showing, hiding and
//! fading it, and giving its GPU back while hidden.

use super::*;

impl OverlayCore {
    pub(super) fn show_fading(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop, fade: Duration) {
        if self.presence.shown() { return; }
        // Hidden, it missed the redraws a new accent or portal preference asks for.
        app.rt_mut().set_system_dark(crate::platform::preferences::dark().unwrap_or(self.dark));
        #[cfg(target_os = "linux")]
        {
            let rt = app.rt_mut();
            rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
            rt.double_click_interval = crate::platform::preferences::double_click_interval();
        }
        self.accent_changed(app.rt_mut(), crate::system_accent());
        self.scroll_bars_changed(app.rt_mut(), crate::platform::scroll_bars::current());
        // Shown again mid fade-out: the window is still up with current pixels, and its motion
        // carries on rather than settling.
        let fading = self.presence.fading();
        let size = self.at.size;
        self.resolve_size(event_loop);
        let was_live = self.surface.live().is_some();
        if !was_live {
            self.prepare(app, event_loop, true);
            if self.shared.failure.lock().unwrap().is_some() { return; }
        }
        if self.surface.live().is_none() { return; }
        self.resume_live(app);
        if size != self.at.size { self.resized(app, event_loop); }
        if self.surface.live().is_none() { return; }
        if !fading { self.settle_before_reveal(app.rt_mut()); }
        if let Some(Backing::Window(window)) = self.surface.live().map(|l| &l.backing) {
            let window = window.clone();
            if self.position_owner.is_managed() { self.place_window(app.rt_mut(), event_loop, &window); }
            else { self.sync_origin(app.rt_mut(), &window); }
            if let Some(gpu) = self.gpu.clone() { self.rescale(app, &gpu, crate::window::backing_scale(&window)); }
            if self.surface.live().is_none() { return; }
            // Retained alpha-zero hosts already have pixels. A hidden update must reach
            // the surface before exposing it, rather than flashing the old frame.
            if !fading { self.refresh_for_reveal_at(app, crate::time::now()); }
            self.show_window(app, &window, fade);
            // A retained macOS panel drew while hidden during prewarm and does not mark itself
            // dirty on reveal, so exposing its retained pixels is the first visible present.
            if frame::retained_reveal_presents(self.retained(), self.dirty, self.surface.live().is_some()) {
                frame::record_presented(false);
            }
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Xwayland(surface), .. }) = self.surface.live_mut() {
            if was_live && size == self.at.size { surface.place(self.at, &self.options); }
            surface.show();
        }
        let effects = self.presence.step(presence::Event::Show, self.release);
        self.carry_out(app, effects);
        self.hidden_pace.reset();
        self.backoff_step(crate::window::BackoffEvent::Revealed);
        // Shown onto sleeping displays or under another window, the panel stays occluded.
        #[cfg(target_os = "macos")]
        {
            self.occlusion_read = false;
            // AppKit updates `occlusionState` some milliseconds after the order-in.
            if self.panel.is_some() { self.prime.arm(); }
            self.sync_occlusion(app);
            if !self.presence.occluded() { self.ensure_pacing(); }
        }
        if !self.retained() { self.dirty = true; }
        self.idle &= !(app.rt().animating || app.compositor_animating());
    }

    /// Makes the overlay's native window and GPU surface: at startup for a prewarmed overlay
    /// (`showing` false), or for the show. A prewarmed `release_gpu_when_hidden` overlay makes
    /// only its window at startup and parks it; its surface, and the GPU device for the first,
    /// wait for the show.
    pub(super) fn prepare(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop, showing: bool) {
        if self.surface.live().is_some() { return; }
        let surface = showing || !self.defers_gpu();
        let wayland = crate::window::is_wayland(event_loop);
        // Only an X11 window waits parked for its surface. A Wayland overlay's window is its
        // surface, and a Windows or macOS one that gives its GPU back is made for each show
        // (`park`): both wait.
        let parks = cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))) && !wayland;
        if !surface && !parks { return; }
        self.resolve_size(event_loop);
        let made = if wayland {
            self.layer(app, event_loop).or_else(|| self.xwayland(app))
        } else if self.surface.parked().is_some() || self.window(app, event_loop).is_some() {
            // macOS made the surface with its panel; elsewhere the window waits parked.
            match self.surface.take_live() {
                Some(live) => Some(live),
                None if surface => self.unpark(app),
                None => return self.ready(app, showing),
            }
        } else {
            None
        };
        match made {
            Some(live) => {
                self.surface.made(live);
                // An earlier attempt's failure no longer holds.
                self.shared.failure.lock().unwrap().take();
                // The surface was made before the panel was adopted and placed; its display's
                // scale is known now, before `warm` draws.
                if let Some(Backing::Window(window)) = self.surface.live().map(|l| &l.backing) && let Some(gpu) = self.gpu.clone() {
                    let scale = crate::window::backing_scale(window);
                    self.rescale(app, &gpu, scale);
                    if self.shared.failure.lock().unwrap().is_some() { return; }
                }
                self.ready(app, showing);
            }
            None => {
                let has_failure = self.shared.failure.lock().unwrap().is_some();
                if !has_failure {
                    self.fail(app, "the platform could not create its window or surface".into());
                } else {
                    self.set_availability_notify(app, Availability::Unavailable);
                }
            }
        }
    }

    /// Updates availability via atomic swap and invokes `ConfiguredOverlay::on_availability`
    /// only when the value actually changed.
    pub(super) fn set_availability_notify(&mut self, app: &mut dyn OverlayApp, a: Availability) -> bool {
        let next = Self::availability_code(a);
        let prev = self.shared.availability.swap(next, Ordering::AcqRel);
        if prev == next {
            return false;
        }
        if app.notify_availability(a) {
            self.clipboard.flush_core(app.rt_mut());
            app.rt_mut().schedule_pending_wakes();
            self.dirty = true;
        }
        true
    }

    /// Marks the overlay `Unavailable` and keeps why, for `OverlayHandle::failure`, then fires
    /// `ConfiguredOverlay::on_availability` when availability changed.
    pub(super) fn fail(&mut self, app: &mut dyn OverlayApp, reason: String) {
        eprintln!("rune: overlay {} can't show: {reason}", self.name);
        *self.shared.failure.lock().unwrap() = Some(reason);
        self.set_availability_notify(app, Availability::Unavailable);
    }

    /// Handles a Wayland layer surface closed by the compositor (`zwlr_layer_surface_v1.closed`)
    /// or a fatal display connection failure: hides and drops the surface like a refused GPU
    /// surface, records why in `OverlayHandle::failure`, and notifies `on_availability`.
    #[cfg_attr(
        not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
        allow(dead_code)
    )]
    pub(super) fn layer_closed(&mut self, app: &mut dyn OverlayApp, reason: Option<&str>) {
        let reason = reason.unwrap_or("compositor closed layer surface").to_owned();
        self.surface_refused(app, reason);
    }

    /// The window exists, with its surface or parked: the overlay can show. A prewarmed one
    /// warms what it has (parked, only the view); one that makes its surface for each show
    /// draws its own first frame instead.
    pub(super) fn ready(&mut self, app: &mut dyn OverlayApp, showing: bool) {
        self.shared.failure.lock().unwrap().take();
        self.set_availability_notify(app, Availability::Available);
        if !self.animate_reveal { app.rt_mut().settle_all(); }
        self.dirty = true;
        self.idle = true;
        if self.options.prewarm && !(showing && self.defers_gpu()) { self.warm(app); }
        if !self.surface.announced() && let Some(created) = &mut self.on_created {
            self.surface.announce();
            let (window_number, capabilities) = *self.shared.native.lock().unwrap();
            created(crate::OverlayCreated { window_number, capabilities });
        }
    }

    // A retained host can be prepared while hidden. Resume sampling before warm/reveal so
    // hidden publications reach that first visible frame, without waking hidden hosts.
    pub(super) fn resume_live(&mut self, app: &mut dyn OverlayApp) {
        app.set_live_visible(true);
        if app.rt().live_pending() { self.dirty = true; }
    }

    pub(super) fn settle_before_reveal(&mut self, rt: &mut RuntimeCore) {
        // Initial retained pixels were settled by warm(). Leaving a settle flag pending here
        // would snap changes arriving after that frame, in the same turn as show().
        if !self.retained() && !self.animate_reveal { rt.settle_all(); }
    }

    /// macOS keeps a prewarmed host's alpha-zero surface, and its pixels, while hidden, unless
    /// it gives its GPU back.
    pub(super) fn retained(&self) -> bool { cfg!(target_os = "macos") && self.options.prewarm && !self.defers_gpu() }

    pub(super) fn refresh_for_reveal_at(&mut self, app: &mut dyn OverlayApp, now: Instant) {
        if self.retained() && (self.dirty || app.rt().animating) {
            self.commit_at(app, now);
            self.warm_frame(app);
        }
    }

    pub(super) fn hide(&mut self, app: &mut dyn OverlayApp) { self.hide_fading(app, Duration::ZERO) }

    pub(super) fn hide_fading(&mut self, app: &mut dyn OverlayApp, fade: Duration) {
        // Any Fn change the monitor saw reaches the view while it's still shown; the hide
        // then releases what it held.
        #[cfg(target_os = "macos")]
        { self.fn_key(app); self.media_keys(app); self.prime.stop(); }
        // Only macOS and Windows fade the window; elsewhere it goes at once, and so does drawing.
        let fade = if cfg!(any(target_os = "macos", target_os = "windows")) { fade } else { Duration::ZERO };
        let effects = self.presence.step(presence::Event::Hide { fade, now: crate::time::now() }, self.release);
        self.carry_out(app, effects);
    }

    /// A fade-out's end, or a hidden surface's release, may be due. Ending the fade finishes
    /// native hiding too: AppKit's completion may stall while displays sleep. On Windows, steps
    /// a fading window's alpha until then.
    pub(super) fn presence_due(&mut self, app: &mut dyn OverlayApp, now: Instant) {
        let effects = self.presence.step(presence::Event::Due { now }, self.release);
        self.carry_out(app, effects);
        #[cfg(target_os = "windows")]
        if let Presence::FadingOut { until, length, .. } = self.presence && let Some(native) = &self.native {
            native.fade(fade_alpha(until, length, now));
        }
    }

    pub(super) fn carry_out(&mut self, app: &mut dyn OverlayApp, effects: crate::machine::Effects<presence::Effect>) {
        for effect in effects {
            match effect {
                presence::Effect::StopDrawing => self.stop_drawing(app),
                presence::Effect::HideWindow { fade } => self.hide_window(app, fade),
                presence::Effect::ReleaseGpu => {
                    app.release_compositor_gpu();
                    self.park();
                }
            }
        }
    }

    /// A hidden overlay draws nothing more: its display link, timer frames and live values stop.
    fn stop_drawing(&mut self, app: &mut dyn OverlayApp) {
        #[cfg(target_os = "macos")]
        if let Some(live) = self.surface.live_mut() && let Some(pacing) = &mut live.pacing { pacing.stop(); }
        app.set_live_visible(false);
        self.backoff_step(crate::window::BackoffEvent::Lapsed);
        self.quiet.step(frame::QuietEvent::Forget);
        if self.release == Release::Now {
            self.rest = crate::window::Rest::Rested;
        } else {
            self.rest_step(app, crate::window::RestEvent::Unseen, crate::time::now());
        }
    }

    /// Records a visible frame reaching the screen so unused pools are given back once the
    /// overlay rests.
    pub(super) fn note_presented(&mut self, now: Instant) {
        self.note_presented_with(now, crate::window::rest::low_memory());
    }

    pub(super) fn note_presented_with(&mut self, now: Instant, low: bool) {
        let _ = crate::window::rest::step(&mut self.rest, crate::window::RestEvent::Drew { now, low });
    }

    pub(super) fn rest_step(&mut self, app: &mut dyn OverlayApp, event: crate::window::RestEvent, now: Instant) {
        for effect in crate::window::rest::step(&mut self.rest, event) {
            match effect {
                crate::window::RestEffect::GiveBackUnused => self.give_back(app, false, now),
                crate::window::RestEffect::GiveBackAll => self.give_back(app, true, now),
            }
        }
    }

    fn give_back(&mut self, app: &mut dyn OverlayApp, all: bool, now: Instant) {
        {
            let rt = app.rt_mut();
            rt.text.rest();
            rt.images.rest();
            if let Some(rewind) = rt.motion.rewind() { rewind.rest(); }
        }
        let gpu = self.gpu.clone();
        if let (Some(gpu), Some(renderer)) = (gpu, &mut self.renderer) {
            // Poll once (non-blocking) at idle rest only if recyclable oversized chunks are in
            // flight so their completed buffers return before `give_back(false)` trims them;
            // ordinary overlays skip polling to avoid the 4-9 ms Iris Xe post-poll penalty.
            if !all && renderer.has_oversized_uploads_in_flight() {
                rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
            }
            renderer.give_back(all, &mut app.rt_mut().text.atlas.resident_caches);
            if all {
                // Non-blocking retirement on shared devices: poll once so completed submissions
                // retire and hand back staging chunks before trimming, and schedule deferred
                // retries instead of blocking with `wait_indefinitely()` while work is in flight.
                self.retire.retire_now(&gpu, now);
                renderer.trim_uploads();
            }
        }
    }

    /// Input stops, and the native window hides over `fade`. A fading window stays up while
    /// `presence_due` steps its alpha (Windows), then hides it.
    fn hide_window(&mut self, app: &mut dyn OverlayApp, fade: Duration) {
        // A hide need not report a blur, and a hidden overlay hears none: what it held is
        // released.
        if self.options.keys == KeyPolicy::OnDemand { app.rt_mut().keys_lost(); }
        self.mods = Default::default();
        self.control_click = false;
        self.ime.clear();
        app.popup_cancel(&mut self.popup);
        app.rt_mut().cancel_pointer();
        self.apply_cursor(app.rt());
        self.hidden_input(app);
        let Some(live) = self.surface.live() else { return };
        match &live.backing {
            Backing::Window(window) => {
                if self.options.keys == KeyPolicy::OnDemand {
                    crate::platform::ime::cancel(window);
                    window.set_ime_allowed(false);
                }
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if let Some(x11) = &self.x11 { x11.hide(); }
                #[cfg(target_os = "windows")]
                {
                    let _ = (window, fade);
                    if let Some(native) = &mut self.native {
                        if self.presence.fading() { native.fade(255) } else { native.hide() }
                    }
                }
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel {
                    panel.hide(fade);
                }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                let _ = fade;
                #[cfg(not(target_os = "windows"))]
                window.set_visible(false);
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(_) | Backing::Xwayland(_) => {
                self.surface.take_live();
                self.drop_hidden_images(app);
            }
        }
    }

    /// Drops the last shown frame's `Pixels` references, CPU image atlas pages and GPU image
    /// textures when a Wayland overlay's surface is destroyed on hide. The next show rebuilds
    /// the view and uploads whatever images it still draws.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(super) fn drop_hidden_images(&mut self, app: &mut dyn OverlayApp) {
        app.rt_mut().drop_images();
        let dropped_gpu = self.renderer.as_mut().is_some_and(Renderer::clear_images);
        if dropped_gpu && self.release != Release::Now
            && let Some(gpu) = &self.gpu
        {
            self.retire.retire_now(gpu, crate::time::now());
        }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn layer(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop) -> Option<Live> {
        use winit::raw_window_handle::HasDisplayHandle;
        use winit::platform::wayland::MonitorHandleExtWayland;
        let display = event_loop.display_handle().ok()?.as_raw();
        let monitor = monitor(event_loop, &self.options.display);
        let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor()).ceil() as i32;
        // With no target, the compositor chooses an output; global pointer querying is forbidden.
        let output = match &self.options.display {
            DisplayTarget::Id(id) if event_loop.available_monitors().any(|m| m == id.0) => Some(id.0.native_id()),
            DisplayTarget::Native(crate::NativeDisplayId::Wayland(id)) if event_loop.available_monitors().any(|m| m.native_id() == *id) => Some(*id),
            _ => None,
        };
        let mut layer = crate::platform::layer_shell::LayerSurface::new(display, self.name, self.at, self.interactive, &self.options, output)?;
        layer.set_buffer_scale(scale);
        if let Some((w, h)) = layer.configured_size() {
            if w > 0 { self.at.size.0 = w as f32; }
            if h > 0 { self.at.size.1 = h as f32; }
        }
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
            capture_exclusion: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Unsupported,
            nonactivating_keys: if layer.supports_keys() { OverlaySupport::Supported } else { OverlaySupport::Unsupported },
        };
        self.attach(app, Backing::Layer(layer), scale as f32)
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    pub(super) fn layer(&mut self, _app: &mut dyn OverlayApp, _event_loop: &ActiveEventLoop) -> Option<Live> {
        None
    }

    /// Makes the native window: on macOS with its GPU surface (`live`), which binds winit's view
    /// before the view moves into the panel; elsewhere without one (`parked`).
    pub(super) fn window(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop) -> Option<()> {
        let mut attrs = Window::default_attributes()
            .with_title(self.name)
            .with_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64))
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_active(false)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        if let Some(p) = position(event_loop, self.at, &self.options) {
            attrs = attrs.with_position(p);
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            use winit::platform::x11::{WindowAttributesExtX11, WindowType};
            attrs = attrs.with_override_redirect(true).with_x11_window_type(vec![WindowType::Notification]);
            if let Some(id) = crate::tooling::app_id() {
                attrs = attrs.with_name(id, id);
            }
        }
        #[cfg(target_os = "windows")]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            // DirectComposition draws the overlay (`Gpu::instance`); without a redirection bitmap
            // nothing under it paints the window black.
            attrs = attrs.with_skip_taskbar(true).with_undecorated_shadow(false).with_no_redirection_bitmap(true);
        }
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_has_shadow(false);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(app, format!("could not create its window: {error}"));
                return None;
            }
        };
        #[cfg(target_os = "macos")]
        crate::native_menu::adopt_standard_view();
        #[cfg(all(target_os = "linux", feature = "dbus"))]
        crate::platform::portal_appearance::follow(&window);
        self.dark = window.theme().map_or_else(|| crate::platform::preferences::desktop_dark(&window).unwrap_or(true), |t| t == winit::window::Theme::Dark);
        app.rt_mut().set_system_dark(self.dark);
        if self.cursor.is_none() { self.cursor = Some(crate::cursor::os::OsCursor::new()); }
        // The surface first: on macOS it binds winit's view and its layer, which then move
        // into the panel.
        #[cfg(target_os = "macos")]
        let live = self.attach(app, Backing::Window(window.clone()), crate::window::backing_scale(&window))?;
        #[cfg(target_os = "macos")]
        {
            let wake = self.shared.wake.lock().unwrap().clone();
            self.panel = Some(crate::platform::macos::Panel::adopt(&window, self.at, self.interactive, &self.options, wake)?);
            let panel = self.panel.as_ref().unwrap();
            if let Some(level) = self.macos_level { panel.set_level(level); }
            let native = (Some(panel.window_number()), crate::OverlayCapabilities {
                capture_exclusion: panel.capture_exclusion(), nonactivating_keys: super::keys::keys_support(self.options.keys, true), global_pointer: OverlaySupport::Supported,
            });
            *self.shared.native.lock().unwrap() = native;
        }
        #[cfg(not(target_os = "macos"))]
        {
            window.set_cursor_hittest(self.options.input.resolved(self.interactive) == OverlayInput::All).ok();
            #[cfg(all(unix, not(target_os = "ios"), not(target_os = "android")))]
            {
                self.x11 = crate::platform::x11::overlay::InputWindow::new(&window, self.shared.wake.lock().unwrap().clone(), self.options.input.resolved(self.interactive), self.options.follow_pointer, self.options.keys);
                if self.options.input == OverlayInput::Regions && self.x11.is_none() { return None; }
                self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
                    capture_exclusion: OverlaySupport::Unsupported, nonactivating_keys: super::keys::keys_support(self.options.keys, self.x11.is_some()), global_pointer: OverlaySupport::Supported,
                };
            }
            #[cfg(target_os = "windows")]
            {
                let wake = self.shared.wake.lock().unwrap().clone();
                match crate::platform::win32::OverlayWindow::new(&window, self.interactive, &self.options, wake) {
                    Ok(native) => self.native = Some(native),
                    Err(error) => {
                        self.fail(app, error);
                        return None;
                    }
                }
                self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
                    capture_exclusion: self.native.as_ref().unwrap().capture_exclusion(), nonactivating_keys: super::keys::keys_support(self.options.keys, true), global_pointer: OverlaySupport::Supported,
                };
            }
        }
        self.place_window(app.rt_mut(), event_loop, &window);
        #[cfg(target_os = "macos")]
        self.surface.made(live);
        #[cfg(not(target_os = "macos"))]
        self.surface.park(Backing::Window(window));
        Some(())
    }

    /// The parked window's GPU surface, and the device for the first surface. Failing, the
    /// window stays parked for the next show to try again.
    pub(super) fn unpark(&mut self, app: &mut dyn OverlayApp) -> Option<Live> {
        let Some(Backing::Window(window)) = self.surface.take_parked() else { return None };
        let live = self.attach(app, Backing::Window(window.clone()), crate::window::backing_scale(&window));
        if live.is_none() { self.surface.park(Backing::Window(window)); }
        live
    }

    /// `release_gpu_when_hidden` or `release_gpu_after` (see `Release::new`).
    pub(super) fn defers_gpu(&self) -> bool { self.release != Release::Keep }

    /// `release_gpu_when_hidden`: the hidden overlay gives back its surface, swapchain and
    /// renderer, and its hold on the GPU device, which waits a while for the next surface
    /// (`render::park_gpu`) once no other surface draws with it. The
    /// window stays parked on X11 only. On Windows a hidden topmost window, often
    /// display-sized, in a process that draws with DXGI is enough for game capture tools such
    /// as NVIDIA's Instant Replay to take the app for the running game; on macOS 27 hidden
    /// screen-sized panels that join every Space empty Mission Control's Space thumbnails.
    /// There the window goes too, and the next show makes a new one.
    pub(super) fn park(&mut self) {
        self.rest = crate::window::Rest::Rested;
        if let Some(live) = self.surface.live() {
            if !matches!(live.backing, Backing::Window(_)) { return; }
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            self.release();
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                let Some(Live { alpha, surface, backing, .. }) = self.surface.take_live() else { return };
                // The surface goes before the device that made it and the window it draws into.
                drop(alpha);
                drop(surface);
                self.surface.park(backing);
            }
        }
        // The device goes to the next surface rather than with this one; one another
        // surface still draws with stays there, polled without waiting so finished frames
        // retire now and anything still in flight retires on a later turn.
        match (self.gpu.take(), self.renderer.take()) {
            (Some(gpu), Some(renderer)) => match Arc::try_unwrap(gpu) {
                Ok(gpu) => {
                    self.retire.clear();
                    crate::render::park_gpu(gpu, renderer);
                }
                Err(gpu) => {
                    drop(renderer);
                    self.retire.retire_now(&gpu, crate::time::now());
                }
            },
            (Some(gpu), None) => {
                if Arc::strong_count(&gpu) > 1 {
                    self.retire.retire_now(&gpu, crate::time::now());
                } else {
                    self.retire.clear();
                }
            }
            (None, renderer) => drop(renderer),
        }
        *self.shared.surface.lock().unwrap() = None;
    }
}
