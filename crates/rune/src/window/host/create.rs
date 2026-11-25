//! Creating the native window: its attributes, frame, surface and GPU, accessibility adapter
//! and platform pieces, and the first frame it comes on screen with.

use std::sync::{Arc, Weak, atomic::Ordering};

use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use super::Host;
use crate::render::{Gpu, Renderer, SurfaceSetupError, format_window_setup_error};
#[cfg(target_os = "windows")]
use crate::window::chrome;
use crate::window::chrome::{frame_chrome, place_chrome, sync_chrome_core};
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
use crate::window::chrome::float_on_open;
#[cfg(target_os = "macos")]
use crate::window::chrome::{mac, uses_drag_regions};
use crate::window::handle::native_window_number;
use crate::window::state::{FirstFrame, apply_create_plan};
#[cfg(target_os = "macos")]
use crate::window::surface::LaunchDrawables;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::window::surface::native;
#[cfg(target_os = "windows")]
use crate::window::surface::win;
#[cfg(target_os = "windows")]
use crate::window::surface::configure;
use crate::window::surface::{
    Backoff, BackoffEvent, HiddenPace, LoopFrame, Native, Redraw, SurfaceRetries, SurfaceStep,
    SURFACE_TRIES, TimerFrame, Win, configure_checked, drawable_size, max_surface_dim,
    rebuild_renderer_core, surface_config,
};
#[cfg(target_os = "windows")]
use crate::window::win32::{covered_draw::CoveredDraw, cpu_frame, handover};
use crate::window::{WakeEvent, sync_screen_origin_core};

impl Host {
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(unused_variables))]
    pub(in crate::window) fn try_create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) -> Result<(), String> {
        if self.poll_gpu_loss() {
            return Err(self.terminal_loss.reason().unwrap_or_else(|| "the GPU device was lost".into()));
        }
        // A released window reopens at the height its content reached while it was gone.
        crate::marks::mark("window.create");
        let built = self.shown && self.build_for_show();
        let size = self.args.size.unwrap_or(self.geometry.size);
        let launcher = self.launcher();
        let visible = !launcher && (self.started || self.options.visible);
        let effective_theme = crate::window::window_theme_for_platform(
            self.color_scheme,
            crate::window::app_color_scheme(),
            cfg!(target_os = "macos"),
        );
        #[allow(unused_mut)]
        let mut attrs = self.options.attributes_with_theme(
            self.app.rt().host.window_title.current().unwrap_or(self.geometry.title),
            size,
            self.geometry.min_size,
            visible,
            effective_theme,
        ).with_active(focus);
        #[cfg(target_os = "macos")]
        if self.macos_window.nonactivating_panel { attrs = attrs.with_visible(false).with_active(false); }
        let panel = cfg!(target_os = "macos") && self.macos_window.nonactivating_panel;
        let plan = self.reopen.create_plan(self.options.resizable, panel);
        #[cfg(target_os = "windows")]
        let open_as = plan.open_as;
        attrs = apply_create_plan(attrs, plan, cfg!(target_os = "windows"));
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if let Some(id) = crate::tooling::app_id() {
                attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, id, id);
                attrs = winit::platform::x11::WindowAttributesExtX11::with_name(attrs, id, id);
            }
            attrs = crate::platform::app_icon::with_icon(attrs);
            if self.float_on_open { attrs = float_on_open::fixed(attrs, &self.options, size); }
            if let Some(token) = token {
                let token = winit::window::ActivationToken::from_raw(token);
                attrs = winit::platform::startup_notify::WindowAttributesExtStartupNotify::with_activation_token(attrs, token);
            }
        }
        // Where the Wayland compositor draws no frame, a titlebar's window opens without winit's
        // own, so its grey bar never shows above the app's: Rune draws the caption buttons and
        // the resize edges (`titlebar::linux`).
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.titlebar.is_some() && crate::titlebar::linux::choose(event_loop) { attrs = attrs.with_decorations(false); }
        let chrome = frame_chrome(self.chrome, self.titlebar, self.app.rt().titlebar_set());
        #[cfg(target_os = "macos")]
        let attrs = self.macos_window.apply_attributes(attrs, self.options.borderless, chrome.full_size_content);
        // Windows opens it hidden, then shows it cloaked until its first frame lands
        // (`Cloak`), so it never comes on screen empty. The show doesn't activate it: the cloak
        // does as it lifts, so the text services' start-up follows the frame.
        #[cfg(target_os = "windows")]
        let focus = focus && !self.no_activate();
        #[cfg(target_os = "windows")]
        let attrs = self.windows_window.apply_attributes(crate::platform::app_icon::with_icon(winit::platform::windows::WindowAttributesExtWindows::with_drag_and_drop(attrs.with_visible(false).with_active(false), false)));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        crate::window::displays::track(&window);
        self.applied_theme = effective_theme;
        crate::marks::mark("window.native");
        // Before first show or CPU paint so no capture sees a frame and no taskbar button flashes.
        #[cfg(target_os = "windows")]
        crate::platform::win32::configure_window(&window, &self.windows_window, self.exclude_from_capture);
        #[cfg(target_os = "windows")]
        crate::platform::win32::follow_suggested_rect(&window);
        #[cfg(target_os = "windows")]
        self.register_resize_sync(&window);
        #[cfg(target_os = "windows")]
        crate::platform::win32::press_point::install(&window);
        #[cfg(target_os = "windows")]
        crate::platform::win32::dark_mode::install_theme_refresh(&window);
        // Before it shows, so the title bar never appears.
        #[cfg(target_os = "windows")]
        let frame = self.titlebar.map(|t| self.app.rt().titlebar_set().over(t))
            .and_then(|t| crate::platform::win32::CustomFrame::install(&window, t.height(), t.has_controls()));
        crate::marks::mark("window.frame_installed");
        #[cfg(target_os = "windows")]
        let cloak = crate::platform::win32::Cloak::default();
        // Its first frame on the CPU renderer, painted before it shows (`cpu_frame`).
        #[cfg(target_os = "windows")]
        let cpu_first = visible && !launcher && !self.is_terminal_gpu_policy() && cpu_frame::opens_on_cpu(self);
        #[cfg(target_os = "windows")]
        if visible && !cpu_first {
            // Unfocused (`App::open_inactive` or `WindowsWindowOptions::no_activate`), it never activates.
            cloak.hold(&window, crate::time::now() + win::READY_FALLBACK, focus);
            window.set_visible(true);
            chrome::open_as(&window, open_as);
            if open_as != crate::windows::OpenAs::Normal {
                crate::platform::win32::apply_window_styles(&window, &self.windows_window);
            }
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
        let mac_opts = self
            .macos_window
            .with_portable_exclude(self.exclude_from_capture)
            .with_runtime_document(
                self.app.rt().host.represented_file.current(),
                self.app.rt().host.document_edited.current(),
            );
        #[cfg(target_os = "macos")]
        let panel = if mac_opts.nonactivating_panel {
            Some(crate::platform::macos::Panel::adopt_window(&window, size, &mac_opts).ok_or("could not create nonactivating panel")?)
        } else { None };
        #[cfg(target_os = "macos")]
        if let Some(panel) = &panel {
            if self.color_scheme != crate::ColorScheme::System {
                panel.set_color_scheme(self.color_scheme);
            }
        } else {
            crate::platform::macos::configure_window(&window, &mac_opts);
        }
        #[cfg(target_os = "macos")]
        {
            let rt = self.app.rt_mut();
            rt.host.represented_file.mark_applied();
            rt.host.document_edited.mark_applied();
        }
        #[cfg(target_os = "macos")]
        let drop_target = crate::platform::macos::drop::install(&window);
        if self.options.click_through {
            if let Err(error) = window.set_cursor_hittest(false) {
                eprintln!("click-through unavailable for {}: {error}", self.native_handle.key());
            }
            #[cfg(target_os = "windows")]
            crate::platform::win32::apply_window_styles(&window, &self.windows_window);
        }
        window.set_ime_allowed(false);
        self.ime.clear();
        self.app.rt_mut().edit.ime_reset = true;
        #[cfg(target_os = "macos")]
        if uses_drag_regions(chrome, self.options.borderless, false) {
            mac::own_window_dragging(&window);
        }
        #[cfg(target_os = "macos")]
        if let Some(t) = self.titlebar.map(|t| self.app.rt().titlebar_set().over(t)) {
            crate::platform::macos::traffic_lights::set_hidden(&window, !t.has_controls());
        }
        self.app.rt_mut().set_titlebar_inset(place_chrome(&window, chrome));
        #[cfg(target_os = "macos")]
        {
            let rt = self.app.rt_mut();
            rt.double_click_interval = mac::double_click_interval();
            rt.set_native_help(true);
        }
        #[cfg(windows)]
        #[allow(unsafe_code)]
        {
            // SAFETY: `GetDoubleClickTime` takes no arguments and reads the system setting.
            let ms = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
            let rt = self.app.rt_mut();
            rt.double_click_interval = f64::from(ms) / 1000.0;
            // A tooltip control's initial delay is the double-click time.
            rt.help_delay = rt.double_click_interval;
        }
        #[cfg(target_os = "linux")]
        {
            self.app.rt_mut().double_click_interval = crate::platform::preferences::double_click_interval();
        }
        #[cfg(target_os = "windows")]
        if cpu_first { return self.open_on_cpu(event_loop, window, a11y, a11y_start, frame, cloak, focus, open_as, built); }
        // Linux: its first frame on the CPU renderer too, sent before any GPU is made
        // (`linux::cpu_frame`); a display that takes no CPU frames gives the window back.
        #[cfg(target_os = "linux")]
        let (window, a11y) = if visible && !self.is_terminal_gpu_policy() && crate::window::linux::cpu_frame::opens_on_cpu(self) {
            match self.open_on_cpu(event_loop, window, a11y, built) {
                Ok(()) => return Ok(()),
                Err(back) => back,
            }
        } else { (window, a11y) };
        crate::marks::mark("gpu.begin");
        let phys = window.inner_size();
        let srgb_blend = self.app.rt().srgb_blend();
        let prewarm_effects = self.prewarm_effects;
        let frame_latency = crate::window::frame_latency(self.frame_latency);
        let features = self.app.compositor_features();
        let transparent = self.options.transparent;
        // A window with a client-side frame keeps its pixels' alpha for the frame's corner arcs;
        // one under a compositor that draws the decorations itself keeps the surface it always had.
        let client_frame = self.client_frame(&window);
        #[cfg(target_os = "windows")]
        let dcomp_handover = if ((self.resize_sync_enabled() && !transparent && !launcher)
            || (cfg!(feature = "windows-system-material") && transparent))
            && matches!(Gpu::dx12_swapchain_kind(), wgpu::Dx12SwapchainKind::DxgiFromVisual)
        {
            crate::platform::win32::hwnd(&window)
                .and_then(|hwnd| crate::platform::win32::dcomp_handover::Handover::new(hwnd as isize).ok())
        } else {
            None
        };
        #[cfg(target_os = "windows")]
        let dcomp_visual = dcomp_handover.as_ref().map(|h| h.visual());
        #[allow(unsafe_code)]
        let make_surface = |instance: &wgpu::Instance| {
            #[cfg(target_os = "windows")]
            if let Some(visual) = dcomp_visual {
                // SAFETY: `dcomp_handover` lives in `Win::native.mode` as long as this surface.
                return unsafe {
                    instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual))
                }
                .map_err(|e| e.to_string());
            }
            instance.create_surface(window.clone()).map_err(|e| e.to_string())
        };
        let setup_surface = |gpu: &Gpu, surface: &wgpu::Surface<'_>, prewarmed: Option<Renderer>| -> Result<_, SurfaceSetupError> {
            let initial = drawable_size(None, (phys.width, phys.height), max_surface_dim(Some(gpu)));
            let (mut config, translucent) = surface_config(surface, &gpu.adapter, initial, srgb_blend)
                .ok_or_else(|| SurfaceSetupError::unsupported("the GPU adapter can't present to this window"))?;
            config.desired_maximum_frame_latency = frame_latency;
            #[cfg(target_os = "macos")]
            let launch_drawables = LaunchDrawables::cap(&mut config);
            if transparent || client_frame {
                config.alpha_mode = alpha_mode(transparent, client_frame, translucent);
            }
            // A refused configuration (the driver's, or a device that is gone) leaves a surface
            // wgpu panics on its next drawable; on a live device with an automatic pick, the
            // fallback chain moves on to the next adapter.
            configure_checked(surface, gpu, &config)
                .map_err(|error| SurfaceSetupError::from_configure("the window's GPU surface refused its configuration", error))?;
            crate::marks::mark("gpu.surface");
            let target_format = if srgb_blend { config.format.remove_srgb_suffix() } else { config.format };
            crate::marks::mark("renderer.begin");
            let mut renderer = match prewarmed {
                Some(mut renderer) => {
                    renderer.try_set_format(&gpu.device, target_format).map_err(SurfaceSetupError::from_shader)?;
                    renderer
                }
                None => Renderer::try_new_classified(&gpu.device, target_format).map_err(SurfaceSetupError::from_shader)?,
            };
            crate::marks::note("uber", renderer.uber_shader());
            crate::marks::mark("renderer.ready");
            renderer.attach_gpu(gpu);
            renderer.set_srgb_blend(srgb_blend);
            renderer.set_prewarm_effects(prewarm_effects);
            #[cfg(target_os = "macos")]
            return Ok((config, launch_drawables, renderer));
            #[cfg(not(target_os = "macos"))]
            Ok((config, renderer))
        };
        // A device that died while this window was closed draws nothing: release it and any
        // compositor resources on it before making another (D3D12 keeps a removed device alive per
        // adapter until every resource made on it is dropped).
        if self.gpu.as_ref().is_some_and(|(gpu, _)| gpu.is_lost()) {
            self.gpu = None;
            self.app.release_compositor_gpu();
        }
        let (gpu, surface, configured) = match self.gpu.take() {
            Some((gpu, renderer)) => {
                let surface = match make_surface(&gpu.instance) {
                    Ok(surface) => surface,
                    Err(error) => {
                        drop((renderer, gpu));
                        self.app.release_compositor_gpu();
                        return Err(error);
                    }
                };
                match setup_surface(&gpu, &surface, Some(renderer)) {
                    Ok(configured) => (gpu, surface, configured),
                    Err(err) => {
                        use crate::gpu::fallback::Decision;
                        match crate::gpu::fallback::decide(Gpu::env_fallback_policy(), err.kind) {
                            Decision::Stop => {
                                drop((surface, gpu));
                                self.app.release_compositor_gpu();
                                if err.kind == crate::gpu::fallback::Failure::DeviceLost {
                                    *self.gpu_pool.borrow_mut() = Weak::default();
                                }
                                return Err(format_window_setup_error(err));
                            }
                            Decision::MoveOn => {
                                if self.is_terminal_gpu_policy() {
                                    drop((surface, gpu));
                                    self.app.release_compositor_gpu();
                                    *self.gpu_pool.borrow_mut() = Weak::default();
                                    return Err(format_window_setup_error(err));
                                }
                                let instance = gpu.instance.clone();
                                let first_info = gpu.adapter.get_info();
                                let excluded = Gpu::candidate_info(0, &gpu.adapter, Some(&surface));
                                drop((surface, gpu));
                                self.app.release_compositor_gpu();
                                *self.gpu_pool.borrow_mut() = Weak::default();
                                let (gpu, surface, configured) = Gpu::for_surface_setup_excluding(
                                    instance, excluded, first_info, err.kind, err.message, features, transparent, make_surface,
                                    |gpu, surface| setup_surface(gpu, surface, None),
                                ).map_err(format_window_setup_error)?;
                                let gpu = Arc::new(gpu);
                                *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                                crate::render::surface_gpu(&gpu);
                                (gpu, surface, configured)
                            }
                        }
                    }
                }
            }
            None => {
                // A shared device that can't composite this window's translucent pixels gets its own.
                let cached = self.gpu_pool.borrow().upgrade().filter(|gpu| !gpu.is_lost()).and_then(|gpu| {
                    let surface = make_surface(&gpu.instance).ok()?;
                    crate::render::translucency::serves(&gpu, &surface, features, transparent).then_some((gpu, surface))
                });
                if let Some((gpu, surface)) = cached {
                    match setup_surface(&gpu, &surface, None) {
                        Ok(configured) => (gpu, surface, configured),
                        Err(err) => {
                            use crate::gpu::fallback::Decision;
                            match crate::gpu::fallback::decide(Gpu::env_fallback_policy(), err.kind) {
                                Decision::Stop => {
                                    if err.kind == crate::gpu::fallback::Failure::DeviceLost {
                                        *self.gpu_pool.borrow_mut() = Weak::default();
                                    }
                                    return Err(format_window_setup_error(err));
                                }
                                Decision::MoveOn => {
                                    if self.is_terminal_gpu_policy() {
                                        drop((surface, gpu));
                                        *self.gpu_pool.borrow_mut() = Weak::default();
                                        return Err(format_window_setup_error(err));
                                    }
                                    let instance = gpu.instance.clone();
                                    let first_info = gpu.adapter.get_info();
                                    let excluded = Gpu::candidate_info(0, &gpu.adapter, Some(&surface));
                                    drop((surface, gpu));
                                    *self.gpu_pool.borrow_mut() = Weak::default();
                                    let (gpu, surface, configured) = Gpu::for_surface_setup_excluding(
                                        instance, excluded, first_info, err.kind, err.message, features, transparent, make_surface,
                                        |gpu, surface| setup_surface(gpu, surface, None),
                                    ).map_err(format_window_setup_error)?;
                                    let gpu = Arc::new(gpu);
                                    *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                                    crate::render::surface_gpu(&gpu);
                                    (gpu, surface, configured)
                                }
                            }
                        }
                    }
                } else {
                    #[cfg(target_os = "linux")]
                    let (gpu, surface, configured) = match Gpu::for_surface_setup_with_latch(
                        self.terminal_loss.latch(), features, transparent, make_surface, setup_surface,
                    ) {
                        Ok(ok) => ok,
                        Err(err) => {
                            if !self.is_terminal_gpu_policy() && self.effective_cpu_first().transparent_gpu_fallback(
                                visible,
                                transparent,
                                crate::gpu::pick::env_policy(),
                                err.kind,
                            ) {
                                if self.open_on_cpu_fallback(event_loop, window, a11y, built).is_ok() {
                                    crate::marks::mark("cpu_first.transparent_fallback");
                                    return Ok(());
                                }
                            }
                            return Err(format_window_setup_error(err));
                        }
                    };
                    #[cfg(not(target_os = "linux"))]
                    let (gpu, surface, configured) = Gpu::for_surface_setup_with_latch(
                        self.terminal_loss.latch(), features, transparent, make_surface, setup_surface,
                    ).map_err(format_window_setup_error)?;
                    let gpu = Arc::new(gpu);
                    *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                    crate::render::surface_gpu(&gpu);
                    (gpu, surface, configured)
                }
            }
        };
        #[cfg(target_os = "macos")]
        let (config, launch_drawables, renderer) = configured;
        #[cfg(not(target_os = "macos"))]
        let (config, renderer) = configured;
        if transparent {
            if config.alpha_mode == wgpu::CompositeAlphaMode::Opaque {
                eprintln!("transparency unavailable for {}; using an opaque window", self.native_handle.key());
            }
            self.app.rt_mut().transparent = config.alpha_mode != wgpu::CompositeAlphaMode::Opaque;
        }
        let target_format = if srgb_blend { config.format.remove_srgb_suffix() } else { config.format };
        self.terminal_loss.attach_gpu(&gpu);
        self.gpu = Some((gpu, renderer));
        let (gpu, renderer) = self.gpu.as_mut().unwrap();
        rebuild_renderer_core(&gpu.device, renderer, target_format, self.app.rt_mut());
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface, self.macos_window.color_space);
        #[cfg(target_os = "macos")]
        crate::platform::macos::window_shape::apply_corners(&window, &self.macos_window, self.options.transparent, self.options.borderless);
        // Metal's PostMultiplied mode only makes CAMetalLayer non-opaque; Core Animation
        // still composites premultiplied pixels. Other backends follow wgpu's straight-alpha contract.
        let alpha = alpha_pass(gpu, self.options.transparent, &config, target_format);
        let scale = window.scale_factor() as f32;
        {
            let rt = self.app.rt_mut();
            rt.set_system_dark(crate::platform::preferences::window_dark(&window, self.args.theme.as_deref()));
            rt.set_viewport((config.width as f32 / scale, config.height as f32 / scale), scale);
            if (self.shown || launcher) && !built {
                // Reopening paints the settled state in the first frame (guarantee 8).
                rt.settle_all();
            }
        }
        self.shown = true;
        if visible { self.life.show(); }
        self.idle = true;
        self.app.set_live_visible(visible);
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
        if self.args.trace.is_some() && present.is_none() { eprintln!("rune: trace: this surface reports no present times"); }
        #[cfg(target_os = "macos")]
        let fn_key = if self.app.rt().modifier_keys() { crate::input::fn_key::FnKey::new(&window) } else { None };
        #[cfg(target_os = "macos")]
        let media_keys = if self.app.rt().media_keys() {
            let handle = self.native_handle.clone();
            crate::input::media_keys::MediaKeys::new(&window, Arc::new(move || handle.send(WakeEvent::App)))
        } else {
            None
        };
        #[cfg(target_os = "macos")]
        let full_screen = mac::FullScreenWatch::new(&window);
        #[cfg(target_os = "macos")]
        let prime = if visible && panel.is_none() { native::Prime::launching() } else { native::Prime::Steady };
        #[cfg(target_os = "macos")]
        let native = Native {
            // A window whose first frame is drawn here, at once (`prime`), gets its display link
            // after that frame. `RUNE_PACING_BEFORE_FRAME` keeps the old order.
            pacing: if prime.presumes() && std::env::var_os("RUNE_PACING_BEFORE_FRAME").is_none() { None } else { crate::platform::display_link::DisplayLink::new(&window, self.frame_wake.clone()).map(|p| p.with_ambient_period(self.ambient_period)) },
            present,
            present_log: hook_log,
            shadow,
            panel,
            materials: crate::system_material::macos::Materials::new(),
            help: Default::default(),
            full_screen,
            fn_key,
            media_keys,
            prime,
            launch_drawables,
            _spot: crate::platform::macos::traffic_lights::Spot::new(&window),
            _drop: drop_target,
        };
        // Making the GPU and surface took part of the cloak's wait; the frame gets all of it.
        #[cfg(target_os = "windows")]
        cloak.extend(crate::time::now() + win::READY_FALLBACK);
        #[cfg(target_os = "windows")]
        let native = {
            let mode = match dcomp_handover {
                Some(handover) => handover::SurfaceMode::Rebinding { handover },
                None => handover::SurfaceMode::Gpu,
            };
            let last_maximized = window.is_maximized();
            Native { cloak, frame, mode, a11y_start, last_maximized, ..Default::default() }
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let native = Native::default();
        let initial_mode = if self.gpu.as_ref().is_some_and(|(gpu, _)| gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu) {
            crate::gpu::fallback::GpuMode::Software
        } else {
            crate::gpu::fallback::GpuMode::Hardware
        };
        self.win = Some(Win {
            a11y,
            native,
            first_frame: FirstFrame::opened(crate::time::now()),
            surface: Some(surface),
            config,
            surface_ok: true,
            surface_retries: SurfaceRetries::for_mode(initial_mode),
            window,
            backoff: Backoff::default(),
            hidden_pace: HiddenPace::default(),
            cursor: crate::cursor::os::OsCursor::new(),
            alpha,
            corners: None,
            timer_frame: TimerFrame::default(),
            loop_frame: LoopFrame::default(),
            presented: None,
            #[cfg(target_os = "windows")]
            covered_draw: CoveredDraw::Current,
            redraw: std::cell::Cell::new(Redraw::Idle),
        });
        if launcher { self.win.as_mut().unwrap().first_frame.prepare(crate::time::now()); }
        self.app.rt_mut().set_renderer_kind(crate::RendererKind::Gpu);
        self.sync_window_backdrop(false);
        self.apply_placement(event_loop);
        #[cfg(target_os = "macos")]
        if visible && let Some(w) = &mut self.win && w.native.panel.is_some() { w.show(focus); }
        if !self.announce_created(true) { return Ok(()); }
        if let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) {
            #[cfg(target_os = "macos")]
            if let Some(pacing) = &mut w.native.pacing { pacing.update_display(&w.window); }
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            if size.width > 0 && size.height > 0 {
                w.resize_surface(Some(&**gpu), size);
                self.app.rt_mut().set_viewport((w.config.width as f32 / scale, w.config.height as f32 / scale), scale);
            }
            sync_chrome_core(self.app.rt_mut(), w, self.chrome, self.titlebar);
            sync_screen_origin_core(self.app.rt_mut(), &w.window);
            if w.window.has_focus() {
                self.app.window_focus(true);
            }
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.wayland && self.options.transparent {
            self.sync_window_backdrop(false);
        }
        // The first frame, now, so the window comes on screen with it instead of empty for a
        // frame or two. AppKit has ordered the window in, but it reaches the screen with this
        // turn's Core Animation commit, so nothing reports it visible yet.
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut drew = false;
        #[cfg(target_os = "macos")]
        if visible && !self.closing_at_creation() && !self.life.finished() && !event_loop.exiting()
            && self.win.as_ref().is_some_and(|w| w.native.panel.is_none() && w.window.is_minimized() != Some(true))
        {
            let presents = self.meter.totals().presents;
            self.ime_waits = true;
            self.pacing_waits = self.win.as_ref().is_some_and(|w| w.native.pacing.is_none());
            let opened_with = self.app.rt().titlebar_set();
            self.redraw();
            // The view's first run may set the titlebar (`Cx::set_titlebar`, `Cx::titlebar_height`):
            // the traffic lights move and the view's inset with them. The shell would do that
            // after this turn, but while AppKit launches the app the next frame is 50,000 µs
            // away and the first one would stand on screen with the lights where `App::chrome`
            // put them. So the frame is drawn again before either reaches the screen: at once,
            // as a frame for a new size is (the frame pacing would put a plain redraw off).
            // A view that set the titlebar the window opened with (the app gave `App::titlebar`
            // the one its view sets) moved nothing: that frame is drawn once.
            if crate::platform::macos::launching() && self.app.rt_mut().take_titlebar_changed()
                && self.app.rt().titlebar_set().changes(opened_with, self.titlebar)
            {
                if let Some(w) = &self.win { sync_chrome_core(self.app.rt_mut(), w, self.chrome, self.titlebar); }
                if self.app.rt().stale() {
                    self.draw(true);
                    if let Some(l) = self.win.as_mut() && !crate::platform::macos::in_live_resize(&l.window) { l.set_live_resize(false); }
                }
            }
            self.ime_waits = false;
            self.pacing_waits = false;
            drew = self.meter.totals().presents > presents;
            if drew { crate::platform::macos::flush_while_launching(); }
            // The text input the first frame left out, now that the frame is on its way.
            if let Some(l) = self.win.as_ref() {
                crate::input::host_events::sync_ime(&l.window, self.app.rt_mut(), &mut self.ime, l.window.has_focus());
                static FIRST_IME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                crate::marks::mark_once(&FIRST_IME, "frame.first.ime");
            }
            self.shell(event_loop);
        }
        // The launch's menu bar, which waited for that frame: winit's default menu, the
        // standard items, then this window's own.
        #[cfg(target_os = "macos")]
        {
            if crate::native_menu::bar_ready() && visible && let Some(w) = &self.win { crate::native_menu::list_window(&w.window); }
            self.menus.drain();
            // The display link that waited for the first frame, or for a window not drawn here.
            if let Some(w) = self.win.as_mut() && w.native.pacing.is_none() {
                w.native.pacing = crate::platform::display_link::DisplayLink::new(&w.window, self.frame_wake.clone())
                    .map(|p| p.with_ambient_period(self.ambient_period));
            }
        }
        if !drew && (visible || launcher) && let Some(w) = &self.win {
            w.request_redraw();
        }
        Ok(())
    }

    /// UI Automation's (or the platform's) accessibility adapter for the window.
    pub(in crate::window) fn a11y_adapter(&self, window: &Window) -> Option<crate::a11y::Adapter> {
        let handle = self.native_handle.clone();
        crate::a11y::Adapter::new(window, Arc::new(move || handle.send(WakeEvent::A11y)))
    }

    /// Whether the window opened without its accessibility adapter and its first frame is up.
    #[cfg(target_os = "windows")]
    pub(in crate::window) fn a11y_owed(&self) -> bool {
        self.win.as_ref().is_some_and(|w| w.native.a11y_start == native::A11yStart::AfterFirstFrame && w.first_frame.ready())
    }

    /// Windows: the accessibility adapter a window opened without, in a turn after its first
    /// frame (`post_frame`); assistive clients hear the window's content change so they ask again.
    #[cfg(target_os = "windows")]
    pub(in crate::window) fn a11y_after_frame(&mut self) {
        if !self.a11y_owed() { return; }
        let Some(w) = self.win.as_ref() else { return };
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
    #[allow(unsafe_code)]
    pub(in crate::window) fn reattach(&mut self, mode: crate::gpu::fallback::GpuMode) -> Result<(), ReattachError> {
        self.assert_no_live_frame_texture();
        if self.is_terminal_gpu_policy() {
            return Err(ReattachError::other("terminal GPU loss policy refuses device replacement"));
        }
        let Some(w) = self.win.as_mut() else { return Err(ReattachError::other("there is no window to draw into")) };
        let window = w.window.clone();
        // The old surface goes before the device that made it and before the new surface is
        // made: a DX12 surface takes the window's one DirectComposition target
        // (`Dx12SwapchainKind::DxgiFromVisual`), so the replacement can only be configured once
        // the old swapchain has let go of it.
        w.alpha = None;
        // A pass made on the device that went with it draws nothing: it goes too.
        w.corners = None;
        w.presented = None;
        // Unregister any swap-chain wait before dropping the surface, even if replacement fails.
        #[cfg(target_os = "windows")]
        {
            #[cfg(feature = "windows-system-material")]
            {
                let native = &mut w.native;
                native.materials.on_device_loss(native.mode.handover_mut());
            }
            w.native.slot.reset();
            w.native.damage.reset();
            self.on_resize_sync_event(crate::window::win32::resize_sync::Event::DeviceLost);
        }
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        self.app.rt_mut().paint.set_windows_acrylic_materials(false);
        let Some(w) = self.win.as_mut() else { return Err(ReattachError::other("there is no window to draw into")) };
        drop(w.surface.take());
        #[cfg(target_os = "macos")]
        self.frame_presented.reset();
        self.gpu = None;
        self.app.release_compositor_gpu();
        {
            let rt = self.app.rt_mut();
            rt.text.atlas.invalidate();
            rt.images.invalidate();
        }
        let features = self.app.compositor_features();
        let transparent = self.options.transparent;
        // A window the CPU handed to the GPU keeps its DirectComposition target: the new swap
        // chain goes on the same visual, committed once it has presented (`SurfaceMode::rebind`).
        // wgpu has the composition-visual target only on Windows.
        #[cfg(target_os = "windows")]
        let visual = w.native.mode.visual();
        let raw_size = window.inner_size();
        let prev = self.win.as_ref().map(|w| (w.config.width, w.config.height));
        let srgb_blend = self.app.rt().srgb_blend();
        let prewarm_effects = self.prewarm_effects;
        let frame_latency = crate::window::frame_latency(self.frame_latency);
        let client_frame = self.client_frame(&window);
        let make_surface = |instance: &wgpu::Instance| {
            #[cfg(target_os = "windows")]
            if let Some(visual) = visual {
                // SAFETY: the window keeps the visual's `Handover` as long as it keeps this surface.
                return unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual)) }.map_err(|e| e.to_string());
            }
            instance.create_surface(window.clone()).map_err(|e| e.to_string())
        };
        let setup_surface = |gpu: &Gpu, surface: &wgpu::Surface<'_>, prewarmed: Option<Renderer>| {
            let size = drawable_size(prev, (raw_size.width, raw_size.height), max_surface_dim(Some(gpu)));
            let (mut config, translucent) = surface_config(surface, &gpu.adapter, size, srgb_blend)
                .ok_or_else(|| SurfaceSetupError::unsupported("the GPU adapter can't present to this window"))?;
            config.desired_maximum_frame_latency = frame_latency;
            if transparent || client_frame {
                config.alpha_mode = alpha_mode(transparent, client_frame, translucent);
            }
            configure_checked(surface, gpu, &config)
                .map_err(|error| SurfaceSetupError::from_configure("the new GPU surface refused its configuration", error))?;
            let target_format = if srgb_blend { config.format.remove_srgb_suffix() } else { config.format };
            let mut renderer = match prewarmed {
                Some(mut renderer) => {
                    renderer.try_set_format(&gpu.device, target_format).map_err(SurfaceSetupError::from_shader)?;
                    renderer
                }
                None => Renderer::try_new_classified(&gpu.device, target_format).map_err(SurfaceSetupError::from_shader)?,
            };
            renderer.attach_gpu(gpu);
            renderer.set_srgb_blend(srgb_blend);
            renderer.set_prewarm_effects(prewarm_effects);
            Ok((config, renderer))
        };
        let (gpu, surface, (config, mut renderer)) = match mode {
            crate::gpu::fallback::GpuMode::Hardware => Gpu::for_surface_setup(
                features,
                transparent,
                make_surface,
                setup_surface,
            )
            .map_err(|e| ReattachError::other(format_window_setup_error(e)))?,
            crate::gpu::fallback::GpuMode::Software => Gpu::for_surface_setup_software(
                features,
                make_surface,
                setup_surface,
            )
            .map_err(|e| ReattachError {
                no_software: e.kind == crate::gpu::fallback::Failure::SurfaceUnsupported,
                message: format_window_setup_error(e),
            })?,
        };
        let gpu = Arc::new(gpu);
        // A surface that fell back to opaque draws an opaque window, as the first one did.
        self.app.rt_mut().transparent = transparent && config.alpha_mode != wgpu::CompositeAlphaMode::Opaque;
        let target_format = if srgb_blend { config.format.remove_srgb_suffix() } else { config.format };
        rebuild_renderer_core(&gpu.device, &mut renderer, target_format, self.app.rt_mut());
        let alpha = alpha_pass(&gpu, transparent, &config, target_format);
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface, self.macos_window.color_space);
        // The new surface reports presents where the old one did, to the log the window kept
        // (`try_create`): the shadow built on that log keeps re-sampling.
        #[cfg(target_os = "macos")]
        let present = self.win.as_ref()
            .and_then(|w| w.native.present_log.clone())
            .and_then(|log| crate::frame::present_time::metal::LayerHook::attach(&surface, log));
        let Some(w) = self.win.as_mut() else { return Err(ReattachError::other("the window closed while it made a new surface")) };
        w.surface = Some(surface);
        w.config = config;
        w.surface_ok = true;
        w.alpha = alpha;
        #[cfg(target_os = "windows")]
        { w.native.slot.reset(); w.native.damage.reset(); }
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
        self.sync_window_backdrop(true);
        Ok(())
    }

    /// Makes a handed-over window's swap chain again with premultiplied alpha (a playing clip's
    /// own layer shows under the UI, `video_layer`) or opaque (none does): a new surface on the
    /// same visual, committed once its first frame has presented (`SurfaceMode::rebind`), as for
    /// a lost device. Premultiplied costs DWM a blend of every region the UI presents, so it lasts
    /// only while a layer shows. Ok(false) when there's nothing to do.
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    pub(in crate::window) fn rebind_alpha(&mut self, premultiplied: bool) -> Result<bool, String> {
        let Some((gpu, _)) = self.gpu.as_ref() else { return Ok(false) };
        let Some(w) = self.win.as_mut() else { return Ok(false) };
        let Some(visual) = w.native.mode.visual() else { return Ok(false) };
        if self.options.transparent || matches!(w.native.mode, handover::SurfaceMode::Rebinding { .. }) {
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
        w.native.slot.reset();
        w.native.damage.reset();
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
        w.native.slot.reset();
        w.native.damage.reset();
        w.native.mode.rebind();
        Ok(true)
    }

    /// Replaces the window's surface and device when either can't draw a frame: a device that is
    /// gone, or a surface whose last configuration failed (`Win::surface_ok`). Follows Chromium's
    /// repeated-loss policy via `SurfaceRetries` (`rune_gpu::fallback::LossState`): up to
    /// `SURFACE_TRIES` (3) losses per mode with one forgiven per 5 minutes (a single healthy frame
    /// does not reset the count), switching an automatic pick from hardware to software (WARP /
    /// `DeviceType::Cpu`) before giving up after 3 software losses. A pinned adapter
    /// (`WGPU_ADAPTER_NAME`) or a platform without a software adapter gives up immediately.
    ///
    /// Giving the window up leaves it hidden (`Life::Hidden`, as `hide` does), gives back its
    /// device and surface, and records why (`AppHandle::window_error`); opening it again makes a
    /// window, a device and a surface. False when there is nothing to draw with this frame.
    pub(in crate::window) fn replace_lost_surface(&mut self) -> bool {
        let lost = self.gpu.as_ref().is_none_or(|(gpu, _)| gpu.is_lost());
        let surface_missing = self.win.as_ref().is_some_and(|w| !w.surface_ok || w.surface.is_none());
        match super::gpu_loss::decide_lost_surface(
            if self.is_terminal_gpu_policy() { crate::GpuLossPolicy::Terminal } else { crate::GpuLossPolicy::Recover },
            self.gpu_loss_failed(),
            lost,
            surface_missing,
        ) {
            super::gpu_loss::LostSurfaceAction::Healthy => return true,
            super::gpu_loss::LostSurfaceAction::LatchTerminal => {
                if !self.poll_gpu_loss() {
                    self.latch_loss_latch(crate::render::loss_latch::GpuFailureKind::DeviceLost, "the GPU device was lost");
                }
                return false;
            }
            super::gpu_loss::LostSurfaceAction::RecreateSurfaceSameDevice => {
                return self.recreate_surface_same_device();
            }
            super::gpu_loss::LostSurfaceAction::Recover => {}
        }
        let now = crate::time::now();
        let policy = Gpu::env_fallback_policy();
        let software_available = crate::gpu::fallback::software_fallback_available();
        let Some(step) = self.win.as_mut().map(|w| w.surface_retries.attempt(now, policy, software_available)) else { return false };
        let mode = match step {
            SurfaceStep::Replace { mode, switched_to_software } => {
                if switched_to_software {
                    eprintln!(
                        "rune: the GPU device was lost {SURFACE_TRIES} times within {} minutes; falling back to the software adapter",
                        crate::gpu::fallback::FORGIVE_LOSS.as_secs() / 60,
                    );
                }
                mode
            }
            SurfaceStep::GiveUp => {
                return self.give_up_surface(format!("the window's GPU surface was lost {SURFACE_TRIES} times; giving up"));
            }
        };
        match self.reattach(mode) {
            Ok(()) => {
                let on_software = self.gpu.as_ref().is_some_and(|(gpu, _)| gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu);
                if let Some(w) = self.win.as_mut() {
                    w.backoff_step(BackoffEvent::Lapsed);
                    if on_software { w.surface_retries.note_software(); }
                }
                true
            }
            Err(err) => {
                if err.no_software {
                    if let Some(w) = self.win.as_mut() { w.surface_retries.note_no_software(); }
                    return self.give_up_surface(format!(
                        "the window's GPU surface was lost {SURFACE_TRIES} times and no software adapter is available: {}",
                        err.message
                    ));
                }
                if let Some(w) = self.win.as_mut() { w.surface_retries.note_replace_failed(now); }
                match self.win.as_ref().and_then(|w| w.surface_retries.retry(now, policy, software_available)) {
                    Some(at) => {
                        eprintln!("rune: could not replace the window's GPU surface, trying again: {}", err.message);
                        if let Some(w) = self.win.as_mut() {
                            w.surface_ok = false;
                            w.backoff_step(BackoffEvent::At(at));
                        }
                        false
                    }
                    None => self.give_up_surface(format!("the window's GPU surface could not be replaced: {}", err.message)),
                }
            }
        }
    }

    /// Gives up the window and the device and surface it drew with, as `hide` does for an app
    /// that releases its GPU when closed: it leaves the screen as `hide` leaves it, stops input
    /// and hides the native window (`stop_showing`, `wind_down`), then the window, device and
    /// surface go. An app sees `window_error` and can `open` the window again.
    pub(in crate::window) fn give_up_surface(&mut self, reason: String) -> bool {
        self.assert_no_live_frame_texture();
        eprintln!("rune: {reason}");
        self.record_live_window_state();
        self.stop_showing();
        self.wind_down();
        self.release();
        self.idle = true;
        self.shared.set_window_error(Some(reason));
        false
    }
}

/// The surface alpha mode for a window that asked for `transparent`, and/or has a client-side frame
/// whose corner arcs the content fades out of, given the modes the surface `supports` (the first
/// that composites translucent pixels, or none).
///
/// An app that asked for a translucent window takes whatever the surface offers, straight alpha
/// included: that is the mode it had before the frame existed, and Metal offers only
/// `PostMultiplied`, which `alpha_pass` converts. A window that only needs its corner pixels
/// transparent takes `PreMultiplied` alone, the one mode the in-place fade can scale, and is
/// `Opaque` — square corners — when the surface has nothing else.
pub(crate) fn alpha_mode(
    transparent: bool,
    client_frame: bool,
    supports: Option<wgpu::CompositeAlphaMode>,
) -> wgpu::CompositeAlphaMode {
    if transparent {
        return supports.unwrap_or(wgpu::CompositeAlphaMode::Opaque);
    }
    if !client_frame {
        return wgpu::CompositeAlphaMode::Opaque;
    }
    match supports {
        Some(wgpu::CompositeAlphaMode::PreMultiplied) => wgpu::CompositeAlphaMode::PreMultiplied,
        _ => wgpu::CompositeAlphaMode::Opaque,
    }
}

pub(in crate::window) struct ReattachError {
    pub(in crate::window) message: String,
    no_software: bool,
}

impl ReattachError {
    fn other(message: impl Into<String>) -> Self {
        Self { message: message.into(), no_software: false }
    }
}

/// The pass that turns premultiplied pixels into straight alpha for a `PostMultiplied` surface on
/// a backend that composites premultiplied pixels (all but Metal).
pub(super) fn alpha_pass(gpu: &Gpu, transparent: bool, config: &wgpu::SurfaceConfiguration, format: wgpu::TextureFormat)
    -> Option<crate::render::surface_alpha::AlphaSurface> {
    (transparent && config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied
        && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
        .then(|| crate::render::surface_alpha::AlphaSurface::new(&gpu.device, format, (config.width, config.height)))
}
