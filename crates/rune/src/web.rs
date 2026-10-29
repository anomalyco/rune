//! Single-threaded browser shell. GPU initialization yields to the browser; winit owns
//! demand-driven rAF and its device-pixel-content-box ResizeObserver (with DPR fallback).
use std::{cell::RefCell, rc::Rc, sync::{Arc, atomic::Ordering}, time::Duration};
use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys, WindowExtWebSys}, window::{Window, WindowId}};
use crate::{App, Runtime, Env, render::{Gpu, Renderer}, time::Instant};
mod input;
pub(crate) mod platform;

thread_local! { static INSETS: std::cell::Cell<[f32; 4]> = const { std::cell::Cell::new([0.0; 4]) }; }

pub(crate) fn safe_area() -> [f32; 4] { INSETS.get() }

/// Open an HTTP(S) or mail link in a new browser tab. Call from a user input handler;
/// browser popup policy applies. The new page receives no opener or referrer.
pub fn open_url(url: &str) -> Result<(), String> {
    let url = crate::open_url::check(url).map_err(|e| e.to_string())?;
    platform::open_link(&url).map_err(|e| format!("{e:?}"))
}

#[derive(Clone, Copy)]
enum Wake { Ready, App }

pub(crate) fn run<S: 'static>(app: App<S>) -> Result<(), String> {
    if !app.overlays.is_empty() || !app.windows.is_empty() {
        return Err("web supports one canvas; overlays and multiple windows are unsupported".into());
    }
    if !app.open || app.on_window.is_some() || app.on_ready.is_some() {
        return Err("web does not support windowless apps or native window hooks".into());
    }
    console_error_panic_hook::set_once();
    let event_loop = EventLoop::<Wake>::with_user_event().build().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    event_loop.spawn_app(Browser { app: Some(app), proxy, window: None, pending: Rc::default(), running: Rc::default() });
    Ok(())
}

struct Browser<S: 'static> {
    app: Option<App<S>>,
    proxy: EventLoopProxy<Wake>,
    window: Option<Arc<Window>>,
    pending: Rc<RefCell<Option<Result<(Gpu, wgpu::Surface<'static>), String>>>>,
    running: Rc<RefCell<Option<Running<S>>>>,
}

struct Running<S: 'static> {
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    rt: Runtime<S>,
    shared: Arc<crate::app::Shared<S>>,
    clock: crate::frame_clock::FrameClock,
    compositor: Option<Box<dyn crate::Compositor<S>>>,
    suspended: bool,
    _menus: crate::native_menu::Guard<S>,
    shortcuts: crate::shortcuts::Guard,
    input: input::Bridge,
    clipboard: crate::clipboard::Client,
}

fn error(message: &str) {
    web_sys::console::error_1(&message.into());
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        if let (Some(body), Ok(label)) = (document.body(), document.create_element("pre")) {
            label.set_text_content(Some(message));
            let _ = label.set_attribute("style", "position:fixed;inset:0;z-index:1;padding:24px;white-space:pre-wrap;background:Canvas;color:CanvasText");
            let _ = body.append_child(&label);
        }
    }
}

async fn gpu(canvas: web_sys::HtmlCanvasElement, features: wgpu::Features) -> Result<(Gpu, wgpu::Surface<'static>), String> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::BROWSER_WEBGPU;
    let instance = wgpu::Instance::new(descriptor);
    let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas)).map_err(|e| e.to_string())?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower, compatible_surface: Some(&surface),
        force_fallback_adapter: false, apply_limit_buckets: false,
    }).await.map_err(|e| format!("WebGPU adapter unavailable (HTTPS and WebGPU are required): {e}"))?;
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rune web"), required_features: adapter.features() & features,
        required_limits: wgpu::Limits::default(), experimental_features: Default::default(),
        memory_hints: Default::default(), trace: Default::default(),
    }).await.map_err(|e| format!("WebGPU device: {e}"))?;
    Ok((Gpu { instance, adapter, device, queue }, surface))
}

impl<S: 'static> ApplicationHandler<Wake> for Browser<S> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let app = self.app.as_ref().expect("app before canvas");
        let window = match event_loop.create_window(Window::default_attributes().with_title(app.spec.title)
            .with_append(true).with_prevent_default(false)) {
            Ok(window) => Arc::new(window),
            Err(e) => { error(&e.to_string()); event_loop.exit(); return; }
        };
        let canvas = window.canvas().expect("winit web canvas");
        // No border/padding: winit's physical input and resize coordinates must match pixels.
        canvas.style().set_css_text("position:fixed;inset:0;width:100%;height:100%;display:block;touch-action:none;outline:none;user-select:none;-webkit-user-select:none");
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            document.set_title(app.spec.title);
            if let Some(body) = document.body() { body.style().set_css_text("margin:0;overflow:hidden;overscroll-behavior:none"); }
        }
        let features = app.compositor.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
        let pending = self.pending.clone();
        let proxy = self.proxy.clone();
        self.window = Some(window);
        wasm_bindgen_futures::spawn_local(async move {
            *pending.borrow_mut() = Some(gpu(canvas, features).await);
            if proxy.send_event(Wake::Ready).is_err() { error("web event loop closed during GPU initialization"); }
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        let Some(window) = self.window.as_ref() else { return };
        if matches!(event, Wake::Ready) {
            let Some(result) = self.pending.borrow_mut().take() else { return };
            let (gpu, surface) = match result {
                Ok(gpu) => gpu,
                Err(e) => {
                    if let Some(app) = &self.app { *app.shared.window_error.lock().unwrap() = Some(e.clone()); }
                    error(&e); event_loop.exit(); return;
                }
            };
            let mut app = self.app.take().expect("one canvas initialization");
            let compositor = app.compositor.take();
            let (on_focus, modifier_keys, srgb_blend) = (app.on_focus, app.modifier_keys, app.srgb_blend);
            let playground = app.playground.take();
            let (spec, state, shared) = app.into_parts(None);
            let size = window.inner_size();
            let scale = window.scale_factor() as f32;
            let dark = web_sys::window().and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten()).is_some_and(|q| q.matches());
            let proxy = self.proxy.clone();
            let wake: crate::Waker = Arc::new(move || { let _ = proxy.send_event(Wake::App); });
            let mut rt = crate::tooling::runtime_for(&spec, state, (size.width as f32 / scale, size.height as f32 / scale), Env::new(dark, scale), wake.clone());
            crate::playground::attach(&mut rt, playground, &spec, None, None, true);
            rt.on_focus = on_focus;
            rt.set_modifier_keys(modifier_keys);
            rt.set_srgb_blend(srgb_blend);
            rt.set_os_drag(false);
            let now = Instant::now();
            rt.wall_start = Some(now);
            rt.open_urls_natively = true;
            rt.record_events = false;
            shared.connect(wake.clone());
            let menus = crate::native_menu::Guard::attach(&shared, false);
            let shortcuts = crate::shortcuts::Guard::attach(&shared, wake.clone(), false, false);
            let mut clipboard = crate::clipboard::Client::default();
            clipboard.connect(wake);
            shared.visible.store(true, Ordering::Release);
            let Some(mut config) = surface.get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1)) else {
                error("WebGPU canvas has no supported configuration"); event_loop.exit(); return;
            };
            config.present_mode = wgpu::PresentMode::Fifo;
            // Backdrop blurs and effect backdrops copy the canvas mid-frame; without it they fall back.
            config.usage |= surface.get_capabilities(&gpu.adapter).usages & wgpu::TextureUsages::COPY_SRC;
            let format = if srgb_blend { config.format.remove_srgb_suffix() } else { config.format.add_srgb_suffix() };
            if format != config.format { config.view_formats.push(format); }
            surface.configure(&gpu.device, &config);
            let mut renderer = Renderer::new(&gpu.device, format);
            renderer.set_srgb_blend(srgb_blend);
            let input = input::Bridge::new(&window.canvas().unwrap(), Rc::downgrade(&self.running), window.clone());
            *self.running.borrow_mut() = Some(Running { gpu, surface, config, renderer, rt, shared, input, clipboard,
                clock: crate::frame_clock::FrameClock::new(now), compositor, suspended: false, _menus: menus, shortcuts });
        }
        if let Some(r) = &mut *self.running.borrow_mut() {
            r.shortcuts.drain();
            r.clipboard.receive(&mut r.rt);
            let requests = r.shared.take();
            if requests.quit { event_loop.exit(); return; }
            if requests.window.as_ref().is_some_and(|r| !matches!(r, crate::app::WindowRequest::Open(_) | crate::app::WindowRequest::Show))
                || requests.position.is_some() || requests.place.is_some() || requests.minimize {
                web_sys::console::warn_1(&"Native window management is ignored on web.".into());
            }
            for update in requests.updates { r.rt.update_if(update); }
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        // Canvas/editor focus moves are internal, and may occur while a DOM callback owns
        // Running. Actual window focus comes from the DOM bridge, not the canvas listener.
        if matches!(event, WindowEvent::Focused(_)) { return; }
        let mut running = self.running.borrow_mut();
        let (Some(window), Some(r)) = (&self.window, &mut *running) else { return };
        let _input = crate::input_time::arrived(&event);
        if r.window_input(&event, window) { window.request_redraw(); return; }
        match event {
            WindowEvent::Resized(size) => {
                r.suspended = size.width == 0 || size.height == 0;
                if r.suspended { return; }
                r.config.width = size.width;
                r.config.height = size.height;
                r.surface.configure(&r.gpu.device, &r.config);
                let scale = window.scale_factor() as f32;
                r.rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
                window.request_redraw();
            }
            WindowEvent::ThemeChanged(theme) => { r.rt.set_system_dark(theme == winit::window::Theme::Dark); window.request_redraw(); }
            WindowEvent::RedrawRequested if !r.suspended => {
                let now = Instant::now();
                let (animating, dt) = r.clock.advance(&mut r.rt, now, r.compositor.as_ref().is_some_and(|c| c.animating()));
                r.clipboard.flush(&mut r.rt);
                if r.rt.popup_menu().is_some() {
                    web_sys::console::warn_1(&"Native popup menus are unsupported on web.".into());
                    r.rt.cancel_popups();
                }
                r.rt.refuse_file_dialog("native file dialogs are unsupported on web");
                r.sync_input(false);
                if !r.rt.surfaces.is_empty() {
                    error("native video surfaces are unsupported on web"); event_loop.exit(); return;
                }
                if r.renderer.srgb_blend() != r.rt.srgb_blend() {
                    // `Tx::set_srgb_blend`: the canvas keeps its format and views it the other way.
                    let format = if r.rt.srgb_blend() { r.config.format.remove_srgb_suffix() } else { r.config.format.add_srgb_suffix() };
                    if format != r.config.format && !r.config.view_formats.contains(&format) {
                        r.config.view_formats.push(format);
                        r.surface.configure(&r.gpu.device, &r.config);
                    }
                    crate::window::rebuild_renderer(&r.gpu.device, &mut r.renderer, format, &mut r.rt);
                }
                let frame = match r.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                    _ => { error("WebGPU canvas acquisition failed"); event_loop.exit(); return; }
                };
                let target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(r.renderer.format()), ..Default::default()
                });
                r.renderer.upload_images(&r.gpu.device, &r.gpu.queue, &mut r.rt.images);
                let mut encoder = r.gpu.device.create_command_encoder(&Default::default());
                if let Some(compositor) = &mut r.compositor {
                    compositor.render(&mut crate::Frame { device: &r.gpu.device, queue: &r.gpu.queue, encoder: &mut encoder,
                        target: &target, format: r.renderer.format(), size: (r.config.width, r.config.height), dt,
                        update_ms: now.elapsed().as_secs_f64() * 1000.0, rt: &mut r.rt, ui: &mut r.renderer });
                } else {
                    r.renderer.prepare(&r.gpu.device, &r.gpu.queue, &mut r.rt.text.atlas, &r.rt.instances, (r.config.width, r.config.height));
                    r.renderer.draw(&mut encoder, &target, r.rt.clear_color());
                }
                r.renderer.submit(&r.gpu.queue, encoder);
                window.pre_present_notify();
                r.gpu.queue.present(frame);
                if r.rt.shell_request.take().is_some() { event_loop.exit(); return; }
                if animating || r.rt.has_pending_input() || r.rt.live_pending() || r.compositor.as_ref().is_some_and(|c| c.animating()) {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // A synchronous textarea callback can focus the canvas, causing winit to finish
        // a nested focus turn here. That callback requests a redraw when it releases
        // Running; compute deadlines in that turn rather than reborrowing it now.
        let Ok(running) = self.running.try_borrow() else { return };
        let Some(r) = &*running else { return };
        event_loop.set_control_flow(match r.rt.wake_deadline() {
            Some(deadline) if deadline <= r.rt.wall_start.unwrap().elapsed().as_secs_f64() => {
                if let Some(window) = &self.window { window.request_redraw(); }
                ControlFlow::Wait
            }
            Some(deadline) => ControlFlow::WaitUntil(r.rt.wall_start.unwrap() + Duration::from_secs_f64(deadline.max(0.0))),
            None => ControlFlow::Wait,
        });
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: winit::event::StartCause) {
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. }) {
            if let Some(window) = &self.window { window.request_redraw(); }
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let Some(r) = self.running.borrow_mut().take() { r.shared.visible.store(false, Ordering::Release); }
        if let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) { canvas.remove(); }
    }
}
