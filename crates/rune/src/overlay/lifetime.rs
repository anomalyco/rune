//! An overlay's lifetime: making its native window and GPU surface, showing, hiding and
//! fading it, and giving its GPU back while hidden.

use super::*;

impl<T: 'static> Overlay<T> {
    pub(super) fn show_fading(&mut self, event_loop: &ActiveEventLoop, fade: Duration) {
        if self.visible { return; }
        // Hidden, it missed the redraws a new accent asks for (`accent::watch`).
        self.rt.set_system_dark(crate::platform::preferences::dark().unwrap_or(self.dark));
        self.accent_changed(crate::system_accent());
        self.scroll_bars_changed(crate::platform::scroll_bars::current());
        // Shown again mid fade-out: the window is still up with current pixels, and its motion
        // carries on rather than settling.
        let fading = self.fading_out.take().is_some();
        let size = self.at.size;
        self.resolve_size(event_loop);
        if self.live.is_none() {
            self.prepare(event_loop, true);
            if self.shared.failure.lock().unwrap().is_some() { return; }
        }
        if self.live.is_none() { return; }
        self.resume_live();
        if size != self.at.size { self.resized(event_loop); }
        if self.live.is_none() { return; }
        if !fading { self.settle_before_reveal(); }
        if let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) {
            let window = window.clone();
            self.place_window(event_loop, &window);
            if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, crate::window::backing_scale(&window)); }
            if self.live.is_none() { return; }
            // Retained alpha-zero hosts already have pixels. A hidden update must reach
            // the surface before exposing it, rather than flashing the old frame.
            if !fading { self.refresh_for_reveal_at(Instant::now()); }
            self.show_window(&window, fade);
        }
        self.visible = true;
        self.release_at = None;
        self.hidden_pace.reset();
        self.occluded_wait = crate::window::OCCLUDED_RETRY;
        // Shown onto sleeping displays or under another window, the panel stays occluded.
        #[cfg(target_os = "macos")]
        {
            self.occluded = false;
            self.occlusion_read = false;
            self.sync_occlusion();
            if !self.occluded { self.ensure_pacing(); }
        }
        if !self.retained() { self.dirty = true; }
        self.idle &= !(self.rt.animating || self.compositor.as_ref().is_some_and(|c| c.animating()));
    }

    /// Makes the overlay's native window and GPU surface: at startup for a prewarmed overlay
    /// (`showing` false), or for the show. A prewarmed `release_gpu_when_hidden` overlay makes
    /// only its window at startup and parks it; its surface, and the GPU device for the first,
    /// wait for the show.
    pub(super) fn prepare(&mut self, event_loop: &ActiveEventLoop, showing: bool) {
        if self.live.is_some() { return; }
        let surface = showing || !self.defers_gpu();
        let wayland = crate::window::is_wayland(event_loop);
        // Only an X11 window waits parked for its surface. A Wayland overlay's window is its
        // surface, and a Windows or macOS one that gives its GPU back is made for each show
        // (`park`): both wait.
        let parks = cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))) && !wayland;
        if !surface && !parks { return; }
        self.resolve_size(event_loop);
        let made = if wayland {
            self.layer(event_loop).or_else(|| self.xwayland())
        } else if self.parked.is_some() || self.window(event_loop).is_some() {
            // macOS made the surface with its panel; elsewhere the window waits in `parked`.
            match self.live.take() {
                Some(live) => Some(live),
                None if surface => self.unpark(),
                None => return self.ready(showing),
            }
        } else {
            None
        };
        match made {
            Some(live) => {
                self.live = Some(live);
                // An earlier attempt's failure no longer holds.
                self.shared.failure.lock().unwrap().take();
                // The surface was made before the panel was adopted and placed; its display's
                // scale is known now, before `warm` draws.
                if let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) && let Some(gpu) = self.gpu.clone() {
                    let scale = crate::window::backing_scale(window);
                    self.rescale(&gpu, scale);
                    if self.shared.failure.lock().unwrap().is_some() { return; }
                }
                self.ready(showing);
            }
            None => if self.shared.failure.lock().unwrap().is_none() {
                self.fail("the platform could not create its window or surface".into());
            } else {
                self.set_availability(Availability::Unavailable);
            },
        }
    }

    /// The window exists, with its surface or parked: the overlay can show. A prewarmed one
    /// warms what it has (parked, only the view); one that makes its surface for each show
    /// draws its own first frame instead.
    pub(super) fn ready(&mut self, showing: bool) {
        self.set_availability(Availability::Available);
        if !self.animate_reveal { self.rt.settle_all(); }
        self.dirty = true;
        self.idle = true;
        if self.options.prewarm && !(showing && self.defers_gpu()) { self.warm(); }
        if !self.announced && let Some(created) = &mut self.on_created {
            self.announced = true;
            let (window_number, capabilities) = *self.shared.native.lock().unwrap();
            created(crate::OverlayCreated { window_number, capabilities });
        }
    }

    // A retained host can be prepared while hidden. Resume sampling before warm/reveal so
    // hidden publications reach that first visible frame, without waking hidden hosts.
    pub(super) fn resume_live(&mut self) {
        self.rt.set_live_visible(true);
        if self.rt.live_pending() { self.dirty = true; }
    }

    pub(super) fn settle_before_reveal(&mut self) {
        // Initial retained pixels were settled by warm(). Leaving a settle flag pending here
        // would snap changes arriving after that frame, in the same turn as show().
        if !self.retained() && !self.animate_reveal { self.rt.settle_all(); }
    }

    /// macOS keeps a prewarmed host's alpha-zero surface, and its pixels, while hidden, unless
    /// it gives its GPU back.
    pub(super) fn retained(&self) -> bool { cfg!(target_os = "macos") && self.options.prewarm && !self.defers_gpu() }

    pub(super) fn refresh_for_reveal_at(&mut self, now: Instant) {
        if self.retained() && (self.dirty || self.rt.animating) {
            self.commit_at(now);
            self.warm_frame();
        }
    }

    pub(super) fn hide(&mut self) { self.hide_fading(Duration::ZERO) }

    /// Shown, or fading out: frames still draw.
    pub(super) fn drawing(&self) -> bool { self.visible || self.fading_out.is_some() }

    /// A hidden overlay draws nothing more: its display link, timer frames and live values stop.
    pub(super) fn stop_drawing(&mut self) {
        self.fading_out = None;
        #[cfg(target_os = "macos")]
        if let Some(live) = &mut self.live && let Some(pacing) = &mut live.pacing { pacing.stop(); }
        self.rt.set_live_visible(false);
        self.retry = None;
        self.quiet = Default::default();
    }

    /// Finishes native hiding too: AppKit's completion may stall while displays sleep. On
    /// Windows, steps the window's alpha until then.
    pub(super) fn end_fade_out(&mut self, now: Instant) {
        let Some(end) = self.fading_out else { return };
        if end <= now { return self.hide(); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native { native.fade(fade_alpha(end, self.fade_length, now)); }
    }

    pub(super) fn hide_fading(&mut self, fade: Duration) {
        // A hide need not report a blur, and a hidden overlay hears none: what it held is
        // released, after any Fn change the monitor saw first.
        #[cfg(target_os = "macos")]
        self.fn_key();
        if self.options.keys == KeyPolicy::OnDemand { self.rt.keys_lost(); }
        let was_shown = self.visible || self.fading_out.is_some();
        // Only macOS and Windows fade the window; elsewhere it goes at once, and so does drawing.
        let fading = cfg!(any(target_os = "macos", target_os = "windows")) && !fade.is_zero() && (self.visible || self.fading_out.is_some());
        if fading {
            self.fading_out = Some(Instant::now() + fade);
            #[cfg(target_os = "windows")]
            { self.fade_length = fade; }
        } else {
            self.stop_drawing();
        }
        self.visible = false;
        self.popup.cancel(&mut self.rt);
        self.rt.cancel_pointer();
        self.apply_cursor();
        self.hidden_input();
        let Some(live) = &self.live else {
            if was_shown { self.hidden_release(Instant::now()); }
            return;
        };
        match &live.backing {
            Backing::Window(window) => {
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if let Some(x11) = &self.x11 { x11.hide(); }
                // A fading window stays up; `end_fade_out` steps its alpha, then hides it.
                #[cfg(target_os = "windows")]
                {
                    let _ = window;
                    if let Some(native) = &mut self.native {
                        if fading { native.fade(255) } else { native.hide() }
                    }
                }
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel {
                    panel.hide(fade);
                }
                #[cfg(not(target_os = "windows"))]
                window.set_visible(false);
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(_) | Backing::Xwayland(_) => { self.live.take(); }
        }
        if was_shown { self.hidden_release(Instant::now()); }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn layer(&mut self, event_loop: &ActiveEventLoop) -> Option<Live> {
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
        let wake = self.shared.wake.lock().unwrap().clone();
        let mut layer = crate::platform::layer_shell::LayerSurface::new(display, self.name, self.at, self.interactive, &self.options, output, wake)?;
        layer.set_buffer_scale(scale);
        if let Some((w, h)) = layer.configured_size() {
            if w > 0 { self.at.size.0 = w as f32; }
            if h > 0 { self.at.size.1 = h as f32; }
        }
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
            capture_exclusion: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Unsupported,
            nonactivating_keys: if layer.supports_keys() { OverlaySupport::Supported } else { OverlaySupport::Unsupported },
        };
        self.attach(Backing::Layer(layer), scale as f32)
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    pub(super) fn layer(&mut self, _event_loop: &ActiveEventLoop) -> Option<Live> {
        None
    }

    /// Makes the native window: on macOS with its GPU surface (`live`), which binds winit's view
    /// before the view moves into the panel; elsewhere without one (`parked`).
    pub(super) fn window(&mut self, event_loop: &ActiveEventLoop) -> Option<()> {
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
                self.fail(format!("could not create its window: {error}"));
                return None;
            }
        };
        #[cfg(target_os = "macos")]
        crate::native_menu::adopt_standard_view();
        self.dark = window.theme().map_or_else(|| crate::platform::preferences::desktop_dark(&window).unwrap_or(true), |t| t == winit::window::Theme::Dark);
        self.rt.set_system_dark(self.dark);
        if self.cursor.is_none() { self.cursor = Some(crate::cursor::os::OsCursor::new(event_loop)); }
        // The surface first: on macOS it binds winit's view and its layer, which then move
        // into the panel.
        #[cfg(target_os = "macos")]
        let live = self.attach(Backing::Window(window.clone()), crate::window::backing_scale(&window))?;
        #[cfg(target_os = "macos")]
        {
            let wake = self.shared.wake.lock().unwrap().clone();
            self.panel = Some(crate::platform::macos::Panel::adopt(&window, self.at, self.interactive, &self.options, wake)?);
            let panel = self.panel.as_ref().unwrap();
            if let Some(level) = self.macos_level { panel.set_level(level); }
            let native = (Some(panel.window_number()), crate::OverlayCapabilities {
                capture_exclusion: panel.capture_exclusion(), nonactivating_keys: OverlaySupport::Supported, global_pointer: OverlaySupport::Supported,
            });
            *self.shared.native.lock().unwrap() = native;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = window.set_cursor_hittest(self.options.input.resolved(self.interactive) == OverlayInput::All);
            #[cfg(all(unix, not(target_os = "ios"), not(target_os = "android")))]
            {
                self.x11 = crate::platform::x11::overlay::InputWindow::new(&window, self.shared.wake.lock().unwrap().clone(), self.options.input.resolved(self.interactive), self.options.follow_pointer);
                if self.options.input == OverlayInput::Regions && self.x11.is_none() { return None; }
                self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
                    capture_exclusion: OverlaySupport::Unsupported, nonactivating_keys: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Supported,
                };
            }
            #[cfg(target_os = "windows")]
            {
                let wake = self.shared.wake.lock().unwrap().clone();
                match crate::platform::win32::OverlayWindow::new(&window, self.interactive, &self.options, wake) {
                    Ok(native) => self.native = Some(native),
                    Err(error) => {
                        self.fail(error);
                        return None;
                    }
                }
                self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
                    capture_exclusion: self.native.as_ref().unwrap().capture_exclusion(), nonactivating_keys: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Supported,
                };
            }
        }
        self.place_window(event_loop, &window);
        #[cfg(target_os = "macos")]
        { self.live = Some(live); }
        #[cfg(not(target_os = "macos"))]
        { self.parked = Some(Backing::Window(window)); }
        Some(())
    }

    /// The parked window's GPU surface, and the device for the first surface. Failing, the
    /// window stays parked for the next show to try again.
    pub(super) fn unpark(&mut self) -> Option<Live> {
        let Some(Backing::Window(window)) = self.parked.take() else { return None };
        let live = self.attach(Backing::Window(window.clone()), crate::window::backing_scale(&window));
        if live.is_none() { self.parked = Some(Backing::Window(window)); }
        live
    }

    /// `release_gpu_when_hidden` or `release_gpu_after`. macOS honours only the first: a
    /// prewarmed panel with `release_gpu_after` (a HUD) retains its pixels.
    pub(super) fn defers_gpu(&self) -> bool {
        if cfg!(target_os = "macos") { self.release_after == Some(Duration::ZERO) } else { self.release_after.is_some() }
    }

    /// Just hidden: gives the GPU surface back now, or schedules it for `release_gpu_after`. A
    /// fade-out keeps it until the fade ends (`end_fade_out` hides again), so the exit plays
    /// out on the window and its last frames reach the screen first.
    pub(super) fn hidden_release(&mut self, now: Instant) {
        if !self.defers_gpu() || self.fading_out.is_some() { return; }
        match self.release_after {
            Some(after) if !after.is_zero() => self.release_at = Some(now + after),
            _ => self.park(),
        }
    }

    /// `release_gpu_after`'s time came while the overlay stayed hidden.
    pub(super) fn release_due(&mut self, now: Instant) {
        if self.release_at.is_some_and(|at| at <= now) && !self.drawing() {
            self.release_at = None;
            self.park();
        }
    }

    /// `release_gpu_when_hidden`: the hidden overlay gives back its surface, swapchain and
    /// renderer, and its hold on the GPU device, which waits a while for the next surface
    /// (`render::park_gpu`) once no other surface draws with it. The
    /// window stays parked on X11 only. On Windows a hidden topmost window, often
    /// display-sized, in a process that draws with DXGI is enough for game capture tools such
    /// as NVIDIA's Instant Replay to take the app for the running game; on macOS 27 hidden
    /// screen-sized panels that join every Space empty Mission Control's Space thumbnails.
    /// There the window goes too, and the next show makes a new one.
    pub(super) fn park(&mut self) {
        if !self.defers_gpu() || !matches!(self.live.as_ref().map(|l| &l.backing), Some(Backing::Window(_))) { return; }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.release();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let Some(Live { alpha, surface, backing, .. }) = self.live.take() else { return };
            // The surface goes before the device that made it and the window it draws into.
            drop(alpha);
            drop(surface);
            self.parked = Some(backing);
        }
        // The device goes to the next surface rather than with this one; one another
        // surface still draws with stays there.
        if let (Some(gpu), Some(renderer)) = (self.gpu.take(), self.renderer.take()) && let Ok(gpu) = Arc::try_unwrap(gpu) {
            crate::render::park_gpu(gpu, renderer);
        }
        *self.shared.surface.lock().unwrap() = None;
    }
}
