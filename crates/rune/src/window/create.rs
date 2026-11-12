//! Creating the native window: its attributes, frame, surface and GPU, accessibility adapter
//! and platform pieces, and the first frame it comes on screen with.

use super::*;

impl<S: 'static> Host<S> {
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(unused_variables))]
    pub(super) fn try_create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) -> Result<(), String> {
        // A released window reopens at the height its content reached while it was gone.
        crate::marks::mark("window.create");
        let built = self.shown && self.build_for_show();
        let size = self.args.size.unwrap_or(self.spec.size);
        let launcher = self.launcher();
        let visible = !launcher && (self.started || self.options.visible);
        #[allow(unused_mut)]
        let mut attrs = self.options.attributes(self.rt.window_title.current().unwrap_or(self.spec.title), size, self.spec.min_size, visible).with_active(focus);
        #[cfg(target_os = "macos")]
        if self.macos_window.nonactivating_panel { attrs = attrs.with_visible(false).with_active(false); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if let Some(id) = crate::tooling::app_id() {
                attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, id, id);
                attrs = winit::platform::x11::WindowAttributesExtX11::with_name(attrs, id, id);
            }
            if self.float_on_open { attrs = float_on_open::fixed(attrs, &self.options, size); }
            if let Some(token) = token {
                let token = winit::window::ActivationToken::from_raw(token);
                attrs = winit::platform::startup_notify::WindowAttributesExtStartupNotify::with_activation_token(attrs, token);
            }
        }
        let open_as = if !self.shown && self.options.resizable { self.open_as } else { crate::windows::OpenAs::Normal };
        // Windows maximizes it once cloaked: maximizing shows a window.
        #[cfg(not(target_os = "windows"))]
        {
            attrs = attrs.with_maximized(open_as.maximizes());
            if open_as.full_screens() { attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None))); }
        }
        let chrome = frame_chrome(self.chrome, self.titlebar, self.rt.titlebar_set());
        #[cfg(target_os = "macos")]
        if !self.options.borderless && chrome.full_size_content {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true);
        }
        // Windows opens it hidden, then shows it cloaked until its first frame lands
        // (`Cloak`), so it never comes on screen empty. The show doesn't activate it: the cloak
        // does as it lifts, so the text services' start-up follows the frame.
        #[cfg(target_os = "windows")]
        let attrs = crate::platform::app_icon::with_icon(winit::platform::windows::WindowAttributesExtWindows::with_drag_and_drop(attrs.with_visible(false).with_active(false), false));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        crate::marks::mark("window.native");
        #[cfg(target_os = "windows")]
        crate::platform::win32::follow_suggested_rect(&window);
        #[cfg(target_os = "windows")]
        crate::platform::win32::press_point::install(&window);
        // Before it shows, so the title bar never appears.
        #[cfg(target_os = "windows")]
        let frame = self.titlebar.and_then(|t| crate::platform::win32::CustomFrame::install(&window, t.height()));
        crate::marks::mark("window.frame_installed");
        #[cfg(target_os = "windows")]
        let cloak = crate::platform::win32::Cloak::default();
        // Its first frame on the CPU renderer, painted before it shows (`cpu_frame`).
        #[cfg(target_os = "windows")]
        let cpu_first = visible && !launcher && cpu_frame::opens_on_cpu(self);
        #[cfg(target_os = "windows")]
        if visible && !cpu_first {
            // Unfocused (`App::open_inactive`), it never activates.
            cloak.hold(&window, Instant::now() + win::READY_FALLBACK, focus);
            window.set_visible(true);
            chrome::open_as(&window, open_as);
        }
        crate::marks::mark("window.framed");
        // First, before anything observes the view (macOS) or an assistive client asks. On Windows,
        // with no screen reader running, UI Automation's adapter (about 7,500 µs) waits for the
        // loop's first idle turn after the first frame: it subclasses the window, which works at
        // any time (`a11y_after_frame`).
        #[cfg(target_os = "windows")]
        let a11y_start = if crate::platform::win32::screen_reader() { native::A11yStart::Made } else { native::A11yStart::AfterFirstFrame };
        #[cfg(target_os = "windows")]
        let a11y = if a11y_start == native::A11yStart::Made { self.a11y_adapter(&window) } else { None };
        #[cfg(not(target_os = "windows"))]
        let a11y = self.a11y_adapter(&window);
        #[cfg(target_os = "macos")]
        crate::native_menu::adopt_standard_view();
        #[cfg(target_os = "macos")]
        let panel = if self.macos_window.nonactivating_panel {
            Some(crate::platform::macos::Panel::adopt_window(&window, size, &self.macos_window).ok_or("could not create nonactivating panel")?)
        } else { None };
        #[cfg(target_os = "macos")]
        crate::platform::macos::configure_window(&window, &self.macos_window);
        #[cfg(target_os = "macos")]
        crate::platform::macos::drop::install(&window);
        if self.options.click_through && let Err(error) = window.set_cursor_hittest(false) {
            eprintln!("click-through unavailable for {}: {error}", self.native_handle.key());
        }
        window.set_ime_allowed(false);
        self.rt.ime_reset = true;
        #[cfg(target_os = "macos")]
        if uses_drag_regions(chrome, self.options.borderless, false) {
            mac::own_window_dragging(&window);
        }
        self.rt.set_titlebar_inset(place_chrome(&window, chrome));
        #[cfg(target_os = "macos")]
        {
            self.rt.double_click_interval = mac::double_click_interval();
            self.rt.set_native_help(true);
        }
        #[cfg(windows)]
        {
            let ms = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
            self.rt.double_click_interval = f64::from(ms) / 1000.0;
            // A tooltip control's initial delay is the double-click time.
            self.rt.help_delay = self.rt.double_click_interval;
        }
        #[cfg(target_os = "windows")]
        if cpu_first { return self.open_on_cpu(event_loop, window, a11y, a11y_start, frame, cloak, focus, open_as, built); }
        let mut prewarmed = None;
        crate::marks::mark("gpu.begin");
        // A device that died while this window was closed draws nothing: make another.
        let (gpu, surface) = match self.gpu.take().filter(|(gpu, _)| !gpu.is_lost()) {
            Some((gpu, renderer)) => {
                let surface = gpu.instance.create_surface(window.clone());
                self.gpu = Some((gpu, renderer));
                (None, surface.map_err(|e| e.to_string())?)
            }
            None => {
                let features = self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
                let transparent = self.options.transparent;
                // A shared device that can't composite this window's translucent pixels gets its own.
                let cached = self.gpu_pool.borrow().upgrade().filter(|gpu| !gpu.is_lost()).and_then(|gpu| {
                    let surface = gpu.instance.create_surface(window.clone()).ok()?;
                    crate::render::translucency::serves(&gpu, &surface, features, transparent).then_some((gpu, surface))
                });
                let (gpu, surface) = if let Some(cached) = cached {
                    cached
                } else {
                    let (gpu, surface, renderer) = Gpu::for_surface(features, transparent, |instance| instance.create_surface(window.clone()).map_err(|e| e.to_string()))?;
                    prewarmed = renderer;
                    let gpu = Arc::new(gpu);
                    *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                    crate::render::surface_gpu(&gpu);
                    (gpu, surface)
                };
                (Some(gpu), surface)
            }
        };
        let adapter = &gpu.as_ref().or(self.gpu.as_ref().map(|g| &g.0)).unwrap().adapter;
        let phys = window.inner_size();
        let (mut config, translucent) = surface_config(&surface, adapter, (phys.width, phys.height), self.rt.srgb_blend())
            .ok_or("the GPU adapter can't present to this window")?;
        #[cfg(target_os = "macos")]
        let launch_drawables = LaunchDrawables::cap(&mut config);
        if self.options.transparent {
            config.alpha_mode = translucent.unwrap_or_else(|| {
                eprintln!("transparency unavailable for {}; using an opaque window", self.native_handle.key());
                wgpu::CompositeAlphaMode::Opaque
            });
            self.rt.transparent = config.alpha_mode != wgpu::CompositeAlphaMode::Opaque;
        }
        let target_format = if self.rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
        if let Some(gpu) = gpu {
            crate::marks::mark("renderer.begin");
            let mut renderer = prewarmed.unwrap_or_else(|| Renderer::new(&gpu.device, target_format));
            crate::marks::note("uber", renderer.uber_shader());
            crate::marks::mark("renderer.ready");
            renderer.set_srgb_blend(self.rt.srgb_blend());
            self.gpu = Some((gpu, renderer));
        }
        let (gpu, renderer) = self.gpu.as_mut().unwrap();
        rebuild_renderer(&gpu.device, renderer, target_format, &mut self.rt);
        // A refused configuration (the driver's, or a device that is gone) leaves a surface
        // wgpu panics on its next drawable: the window closes, and the next open makes both.
        let refused = configure(&surface, gpu, &config).err().map(|error| (gpu.is_lost(), error));
        if let Some((lost, error)) = refused {
            if lost {
                self.gpu = None;
                *self.gpu_pool.borrow_mut() = Weak::default();
            }
            return Err(format!("the window's GPU surface refused its configuration: {error}"));
        }
        crate::marks::mark("gpu.surface");
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface);
        #[cfg(target_os = "macos")]
        crate::platform::macos::window_shape::apply_corners(&window, &self.macos_window, self.options.transparent, self.options.borderless);
        // Metal's PostMultiplied mode only makes CAMetalLayer non-opaque; Core Animation
        // still composites premultiplied pixels. Other backends follow wgpu's straight-alpha contract.
        let alpha = alpha_pass(gpu, self.options.transparent, &config, target_format);
        let scale = window.scale_factor() as f32;
        self.rt.set_system_dark(crate::platform::preferences::window_dark(&window, self.args.theme.as_deref()));
        self.rt.set_viewport((phys.width as f32 / scale, phys.height as f32 / scale), scale);
        if (self.shown || launcher) && !built {
            // Reopening paints the settled state in the first frame (guarantee 8).
            self.rt.settle_all();
        }
        self.shown = true;
        if visible { self.life.show(); }
        self.idle = true;
        self.rt.set_live_visible(visible);
        *self.native_handle.native.window.lock().unwrap() = Arc::downgrade(&window);
        self.native_handle.native.number.store(native_window_number(&window).unwrap_or(0), Ordering::Release);
        #[cfg(target_os = "macos")]
        let resamples = crate::platform::macos::window_shape::resamples(&window, self.options.transparent);
        #[cfg(target_os = "macos")]
        let hook_log = self.present_log.clone()
            .or_else(|| resamples.then(|| Arc::new(crate::frame::present_time::metal::Log::untraced())));
        #[cfg(target_os = "macos")]
        let present = hook_log.clone().and_then(|log| crate::frame::present_time::metal::LayerHook::attach(&surface, log));
        #[cfg(target_os = "macos")]
        let shadow = hook_log.clone().filter(|_| resamples && present.is_some())
            .and_then(|log| crate::platform::macos::window_shape::Shadow::new(&window, log, visible || launcher));
        #[cfg(target_os = "macos")]
        if self.present_log.is_some() && present.is_none() { eprintln!("rune: trace: this surface reports no present times"); }
        #[cfg(target_os = "macos")]
        let fn_key = if self.rt.modifier_keys() { crate::input::fn_key::FnKey::new(&window) } else { None };
        #[cfg(target_os = "macos")]
        let full_screen = mac::FullScreenWatch::new(&window);
        #[cfg(target_os = "macos")]
        let prime = if visible && panel.is_none() { native::Prime::launching() } else { native::Prime::Steady };
        #[cfg(target_os = "macos")]
        let native = Native {
            pacing: crate::platform::display_link::DisplayLink::new(&window, self.frame_wake.clone()),
            present,
            present_log: hook_log,
            shadow,
            panel,
            materials: crate::system_material::macos::Materials::new(),
            help: Default::default(),
            full_screen,
            fn_key,
            prime,
            launch_drawables,
            _spot: crate::platform::macos::traffic_lights::Spot::new(&window),
        };
        // Making the GPU and surface took part of the cloak's wait; the frame gets all of it.
        #[cfg(target_os = "windows")]
        cloak.extend(Instant::now() + win::READY_FALLBACK);
        #[cfg(target_os = "windows")]
        let native = {
            let handle = self.native_handle.clone();
            let report = Arc::new(move |covered| handle.send(move |slot| WakeEvent::Covered(slot, covered)));
            Native { cloak, frame, _occlusion: crate::platform::win32::occlusion::Tracked::new(&window, report), mode: Default::default(), a11y_start, video_layers: Default::default() }
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let native = Native::default();
        self.win = Some(Win {
            a11y,
            native,
            first_frame: FirstFrame::opened(Instant::now()),
            surface: Some(surface),
            config,
            surface_ok: true,
            surface_retries: SurfaceRetries::default(),
            window,
            backoff: Backoff::default(),
            hidden_pace: HiddenPace::default(),
            cursor: crate::cursor::os::OsCursor::new(),
            alpha,
            timer_frame: TimerFrame::default(),
            presented: None,
            #[cfg(target_os = "windows")]
            steady: Steady::Rest,
            redraw: std::cell::Cell::new(Redraw::Idle),
        });
        if launcher { self.win.as_mut().unwrap().first_frame.prepare(Instant::now()); }
        self.apply_placement(event_loop);
        #[cfg(target_os = "macos")]
        if visible && let Some(w) = &self.win && w.native.panel.is_some() { w.show(focus); }
        crate::marks::mark("window.created");
        if let Some(created) = self.on_created.take() { created(self.native_handle.clone()); }
        if let Some(on_window) = self.on_window.as_mut() { on_window(&self.native_handle); }
        if let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) {
            #[cfg(target_os = "macos")]
            if let Some(pacing) = &mut w.native.pacing { pacing.update_display(&w.window); }
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            if size.width > 0 && size.height > 0 {
                w.resize_surface(Some(&**gpu), size);
                self.rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
            }
            sync_chrome(&mut self.rt, w, self.chrome, self.titlebar);
            sync_screen_origin(&mut self.rt, &w.window);
            if w.window.has_focus() {
                self.rt.window_focus(true);
            }
        }
        // The first frame, now, so the window comes on screen with it instead of empty for a
        // frame or two. AppKit has ordered the window in, but it reaches the screen with this
        // turn's Core Animation commit, so nothing reports it visible yet.
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut drew = false;
        #[cfg(target_os = "macos")]
        if visible && !self.rt.minimize.field() && !self.life.finished() && !event_loop.exiting()
            && self.win.as_ref().is_some_and(|w| w.native.panel.is_none() && w.window.is_minimized() != Some(true))
        {
            let presents = self.meter.totals().presents;
            self.redraw();
            drew = self.meter.totals().presents > presents;
            self.shell(event_loop);
        }
        if !drew && (visible || launcher) && let Some(w) = &self.win {
            w.request_redraw();
        }
        Ok(())
    }

    /// UI Automation's (or the platform's) accessibility adapter for the window.
    pub(super) fn a11y_adapter(&self, window: &Window) -> Option<crate::a11y::Adapter> {
        let handle = self.native_handle.clone();
        crate::a11y::Adapter::new(window, Arc::new(move || handle.send(WakeEvent::A11y)))
    }

    /// Windows: the accessibility adapter a window opened without, on the first idle turn after
    /// its first frame; assistive clients hear the window's content change so they ask again.
    #[cfg(target_os = "windows")]
    pub(super) fn a11y_after_frame(&mut self) {
        let Some(w) = self.win.as_ref() else { return };
        if w.native.a11y_start != native::A11yStart::AfterFirstFrame || !w.first_frame.ready() { return; }
        let adapter = self.a11y_adapter(&w.window);
        crate::marks::mark("window.a11y");
        let Some(w) = self.win.as_mut() else { return };
        w.native.a11y_start = native::A11yStart::Made;
        w.a11y = adapter;
        if let Some(a) = &w.a11y { a.notify_structure(); }
    }

    /// Replaces the window's surface and the device it draws with, keeping the window, its size
    /// and its place. A lost GPU configures nothing, and under it wgpu drops the error
    /// `Surface::configure` returns without recording the surface's error sink: the surface has
    /// no presentation, and its next `get_current_texture` panics ("Surface is not configured
    /// for presentation"). The same goes for a surface whose configuration a driver refused.
    /// The next frame draws with what this makes; `replace_lost_surface` gives the window up if
    /// this keeps failing.
    pub(super) fn reattach(&mut self) -> Result<(), String> {
        let Some(w) = self.win.as_mut() else { return Err("there is no window to draw into".into()) };
        let window = w.window.clone();
        // The old surface goes before the device that made it and before the new surface is
        // made: a DX12 surface takes the window's one DirectComposition target
        // (`Dx12SwapchainKind::DxgiFromVisual`), so the replacement can only be configured once
        // the old swapchain has let go of it.
        w.alpha = None;
        w.presented = None;
        drop(w.surface.take());
        self.gpu = None;
        self.rt.text.atlas.invalidate();
        self.rt.images.invalidate();
        let features = self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
        let transparent = self.options.transparent;
        // A window the CPU handed to the GPU keeps its DirectComposition target: the new swap
        // chain goes on the same visual, committed once it has presented (`SurfaceMode::rebind`).
        // wgpu has the composition-visual target only on Windows.
        #[cfg(target_os = "windows")]
        let visual = w.native.mode.visual();
        let (gpu, surface, prewarmed) = Gpu::for_surface(features, transparent, |instance| {
            #[cfg(target_os = "windows")]
            if let Some(visual) = visual {
                // SAFETY: the window keeps the visual's `Handover` as long as it keeps this surface.
                return unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual)) }.map_err(|e| e.to_string());
            }
            instance.create_surface(window.clone()).map_err(|e| e.to_string())
        })?;
        let gpu = Arc::new(gpu);
        let size = window.inner_size();
        let (mut config, translucent) = surface_config(&surface, &gpu.adapter, (size.width, size.height), self.rt.srgb_blend())
            .ok_or("the GPU adapter can't present to this window")?;
        if transparent {
            config.alpha_mode = translucent.unwrap_or(wgpu::CompositeAlphaMode::Opaque);
        }
        if let Err(error) = configure(&surface, &gpu, &config) {
            return Err(format!("the new GPU surface refused its configuration: {error}"));
        }
        let target_format = if self.rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
        let mut renderer = prewarmed.unwrap_or_else(|| Renderer::new(&gpu.device, target_format));
        renderer.set_srgb_blend(self.rt.srgb_blend());
        rebuild_renderer(&gpu.device, &mut renderer, target_format, &mut self.rt);
        let alpha = alpha_pass(&gpu, transparent, &config, target_format);
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface);
        // The new surface reports presents where the old one did, to the log the window kept
        // (`try_create`): the shadow built on that log keeps re-sampling.
        #[cfg(target_os = "macos")]
        let present = self.win.as_ref()
            .and_then(|w| w.native.present_log.clone())
            .and_then(|log| crate::frame::present_time::metal::LayerHook::attach(&surface, log));
        let Some(w) = self.win.as_mut() else { return Err("the window closed while it made a new surface".into()) };
        w.surface = Some(surface);
        w.config = config;
        w.surface_ok = true;
        w.alpha = alpha;
        #[cfg(target_os = "windows")]
        w.native.mode.rebind();
        #[cfg(target_os = "macos")]
        {
            w.native.present = present;
            w.native.prime.stop();
        }
        *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
        crate::render::surface_gpu(&gpu);
        self.gpu = Some((gpu, renderer));
        Ok(())
    }

    /// Makes a handed-over window's swap chain again with premultiplied alpha (a playing clip's
    /// own layer shows under the UI, `video_layer`) or opaque (none does): a new surface on the
    /// same visual, committed once its first frame has presented (`SurfaceMode::rebind`), as for
    /// a lost device. Premultiplied costs DWM a blend of every region the UI presents, so it lasts
    /// only while a layer shows. Ok(false) when there's nothing to do.
    #[cfg(target_os = "windows")]
    pub(super) fn rebind_alpha(&mut self, premultiplied: bool) -> Result<bool, String> {
        let Some((gpu, _)) = self.gpu.as_ref() else { return Ok(false) };
        let Some(w) = self.win.as_mut() else { return Ok(false) };
        let Some(visual) = w.native.mode.visual() else { return Ok(false) };
        if self.options.transparent || matches!(w.native.mode, super::cpu_frame::SurfaceMode::Rebinding { .. }) {
            return Ok(false);
        }
        let alpha = if premultiplied { wgpu::CompositeAlphaMode::PreMultiplied } else { wgpu::CompositeAlphaMode::Opaque };
        if w.config.alpha_mode == alpha {
            return Ok(false);
        }
        if premultiplied && w.surface.as_ref().is_none_or(|s| crate::render::translucency::mode(s, &gpu.adapter) != Some(alpha)) {
            return Err("this adapter can't composite a premultiplied swap chain".into());
        }
        // The old surface goes first: a DX12 surface takes the visual's content, and the
        // committed tree keeps showing its last frame until the new one commits.
        drop(w.surface.take());
        // SAFETY: the window keeps the visual's `Handover` as long as it keeps this surface.
        let surface = unsafe { gpu.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual)) }.map_err(|e| e.to_string())?;
        let mut config = w.config.clone();
        config.alpha_mode = alpha;
        configure(&surface, gpu, &config)?;
        w.surface = Some(surface);
        w.config = config;
        w.surface_ok = true;
        w.presented = None;
        w.native.mode.rebind();
        Ok(true)
    }

    /// Replaces the window's surface and device when either can't draw a frame: a device that is
    /// gone, or a surface whose last configuration failed (`Win::surface_ok`). Every replacement
    /// counts against `SURFACE_TRIES` (`SurfaceRetries`), whether it worked or not: only a frame
    /// that presents starts the count over (`Host::draw`), so a surface replaced and lost again
    /// before it presents still ends in the window being given up. A failed replacement waits a
    /// backoff before the next try.
    ///
    /// Giving the window up leaves it hidden (`Life::Hidden`, as `hide` does), gives back its
    /// device and surface, and records why (`AppHandle::window_error`); opening it again makes a
    /// window, a device and a surface. False when there is nothing to draw with this frame.
    pub(super) fn replace_lost_surface(&mut self) -> bool {
        let lost = self.gpu.as_ref().is_none_or(|(gpu, _)| gpu.is_lost());
        let surface_missing = self.win.as_ref().is_some_and(|w| !w.surface_ok || w.surface.is_none());
        if !lost && !surface_missing { return true; }
        let now = Instant::now();
        let Some(step) = self.win.as_mut().map(|w| w.surface_retries.attempt()) else { return false };
        if step == SurfaceStep::GiveUp {
            return self.give_up_surface(format!("the window's GPU surface was replaced {SURFACE_TRIES} times without a frame reaching the screen"));
        }
        match self.reattach() {
            Ok(()) => {
                if let Some(w) = self.win.as_mut() { w.backoff_step(BackoffEvent::Lapsed); }
                true
            }
            Err(error) => match self.win.as_ref().and_then(|w| w.surface_retries.retry(now)) {
                Some(at) => {
                    eprintln!("rune: could not replace the window's GPU surface, trying again: {error}");
                    if let Some(w) = self.win.as_mut() {
                        w.surface_ok = false;
                        w.backoff_step(BackoffEvent::At(at));
                    }
                    false
                }
                None => self.give_up_surface(format!("the window's GPU surface could not be replaced: {error}")),
            },
        }
    }

    /// Gives up the window and the device and surface it drew with, as `hide` does for an app
    /// that releases its GPU when closed: it leaves the screen as `hide` leaves it, stops input
    /// and hides the native window (`stop_showing`, `wind_down`), then the window, device and
    /// surface go. An app sees `window_error` and can `open` the window again.
    fn give_up_surface(&mut self, reason: String) -> bool {
        eprintln!("rune: {reason}");
        self.stop_showing();
        self.wind_down();
        self.release();
        self.idle = true;
        *self.shared.window_error.lock().unwrap() = Some(reason);
        false
    }
}

/// The pass that turns premultiplied pixels into straight alpha for a `PostMultiplied` surface on
/// a backend that composites premultiplied pixels (all but Metal).
fn alpha_pass(gpu: &Gpu, transparent: bool, config: &wgpu::SurfaceConfiguration, format: wgpu::TextureFormat)
    -> Option<crate::render::surface_alpha::AlphaSurface> {
    (transparent && config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied
        && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
        .then(|| crate::render::surface_alpha::AlphaSurface::new(&gpu.device, format, (config.width, config.height)))
}
