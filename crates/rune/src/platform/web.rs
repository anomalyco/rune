//! Single-threaded browser shell. GPU initialization yields to the browser; winit owns
//! demand-driven rAF and its device-pixel-content-box ResizeObserver (with DPR fallback).
use std::{cell::{Cell, RefCell}, rc::Rc, sync::{Arc, atomic::Ordering}, time::Duration};
use wasm_bindgen::JsCast;
use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys, WindowExtWebSys}, window::{Window, WindowId}};
use crate::{App, Runtime, Env, render::{Gpu, Renderer}, time::Instant};
use super::web_math;

/// The overscan a page's canvas carries above and below the window, in CSS points: the distance
/// the browser's compositor can scroll before Rune has to draw a new band.
const SKIRT: f32 = 256.0;
mod cpu_canvas;
mod full_faces;
mod page_fonts;
pub(crate) mod health;
mod input;
mod page;
pub(crate) mod platform;
pub(crate) mod segmenter;

thread_local! {
    static INSETS: std::cell::Cell<[f32; 4]> = const { std::cell::Cell::new([0.0; 4]) };
    /// The largest viewport's height in CSS points (`100lvh`): a page's band is sized from it so
    /// a phone's toolbar coming and going never changes the band.
    static LARGE_VIEWPORT: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
    /// The page's primary pointer can't hover (CSS `(hover: none)`): a phone's or tablet's touch screen.
    static PRIMARY_POINTER: std::cell::Cell<PrimaryPointer> = const { std::cell::Cell::new(PrimaryPointer::Hover) };
}

pub(crate) fn safe_area() -> [f32; 4] { INSETS.get() }

/// The largest viewport's height in CSS points, or 0 before the browser has reported one.
pub(crate) fn large_viewport() -> f32 { LARGE_VIEWPORT.get() }

#[derive(Clone, Copy, PartialEq, Eq)]
enum PrimaryPointer { Hover, NoHover, Touch }

pub(crate) fn hover_none() -> bool { PRIMARY_POINTER.get() != PrimaryPointer::Hover }
pub(crate) fn touch_controls() -> bool { PRIMARY_POINTER.get() == PrimaryPointer::Touch }

/// Fetches the font at `url` now, for something about to draw in it (a terminal's face, say),
/// as `App::font_url_later` fetches its fonts after the first frame: every window takes it up
/// on its next frame once it arrives; until then text in its family sets in the slot's fallback.
/// Asked before the first frame, it waits for it. Each URL loads once.
pub fn font_url_now(url: &str) {
    full_faces::now(url);
}

/// Open an HTTP(S) or mail link in a new browser tab. Call from a user input handler;
/// browser popup policy applies. The new page receives no opener or referrer.
pub fn open_url(url: &str) -> Result<(), String> {
    let url = crate::open_url::check(url).map_err(|e| e.to_string())?;
    platform::open_link(&url).map_err(|e| format!("{e:?}"))
}

/// Open a link in this tab, as `Tx::open_url_same_tab` does: a link on the page's origin goes
/// to the app's `App::same_tab_router` first, and the browser loads it with `location.assign`
/// unless the router handled it. A link without a scheme (`/notes/42`) is relative to the page.
pub fn open_url_same_tab(url: &str) -> Result<(), String> {
    let url = crate::open_url::check_same_tab(url).map_err(|e| e.to_string())?;
    let resolved = platform::resolve_link(&url).map_err(|e| format!("{e:?}"))?;
    let href = resolved.get(0).as_string().unwrap_or_default();
    if resolved.get(1).is_truthy() && page::route(&href) { return Ok(()); }
    platform::assign_link(&href).map_err(|e| format!("{e:?}"))
}


#[derive(Clone, Copy)]
enum Wake {
    /// The app's `font_url` fonts are in: the app starts (on the CPU, or with the GPU).
    Start,
    /// The GPU came, or couldn't.
    Ready,
    /// Time to see whether the page is quiet enough to start the GPU (`Browser::maybe_start_gpu`).
    StartGpu,
    App,
}

/// How a page's frames start: on the CPU until WebGPU takes over (a full-window app, the
/// default), or on the GPU from the first frame (a page that is the document or is inline, an
/// app with a compositor). `globalThis.RUNE_CPU_FIRST` set to `"off"` starts every app on the
/// GPU; `"only"` keeps it on the CPU for good.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    GpuFirst,
    CpuFirst,
    CpuOnly,
}

fn cpu_first_asked() -> Option<String> {
    js_sys::Reflect::get(&js_sys::global(), &"RUNE_CPU_FIRST".into()).ok().and_then(|v| v.as_string())
}

pub(crate) fn run<S: 'static>(app: App<S>) -> Result<(), String> {
    if !app.overlays.is_empty() || !app.windows.is_empty() {
        return Err("web supports one canvas; overlays and multiple windows are unsupported".into());
    }
    if !app.open || app.on_window.is_some() {
        return Err("web does not support windowless apps or native window hooks".into());
    }
    console_error_panic_hook::set_once();
    segmenter::install();
    text_probe();
    page::start(quitting(&app.shared), Box::new(move || spawn(app)))
}

/// For a measuring harness that sets `globalThis.__runeTextProbe` before the module runs:
/// `globalThis.__runeLines(text, size, weight, width)` gives the UTF-16 offsets where Rune starts
/// each line of `text` in the sans family (points), to compare with the browser's own lines.
fn text_probe() {
    let global = js_sys::global();
    if !js_sys::Reflect::get(&global, &"__runeTextProbe".into()).is_ok_and(|v| v.is_truthy()) { return; }
    let lines = wasm_bindgen::closure::Closure::<dyn Fn(String, f32, f32, f32) -> Vec<u32>>::new(|text: String, size: f32, weight: f32, width: f32| {
        crate::text::line_starts(&text, size, weight, width).into_iter().map(|b| text[..b].encode_utf16().count() as u32).collect::<Vec<u32>>()
    });
    let _ = js_sys::Reflect::set(&global, &"__runeLines".into(), lines.as_ref());
    lines.forget();
}

fn quitting<S: 'static>(shared: &Arc<crate::app::Shared<S>>) -> Box<dyn Fn() -> bool> {
    let shared = shared.clone();
    Box::new(move || shared.quitting.load(Ordering::Acquire))
}

fn spawn<S: 'static>(mut app: App<S>) -> Result<(), String> {
    if app.shared.quitting.load(Ordering::Acquire) { return Ok(()); }
    let event_loop = EventLoop::<Wake>::with_user_event().build().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let shared = app.shared.clone();
    let wake = proxy.clone();
    shared.connect(Arc::new(move || { let _ = wake.send_event(Wake::App); }));
    page::occupy(quitting(&shared), app.same_tab_router.take());
    event_loop.spawn_app(Browser { app: Some(app), shared, proxy, window: None, embedded: false, document: false, owns_canvas: true,
        borrowed: page::Borrowed::default(), start: Start::GpuFirst, fonts: Rc::default(), pending: Rc::default(), running: Rc::default(),
        lost: Rc::default(), waiting: None, first_at: None, drawn_at: None, gpu_asked: false });
    Ok(())
}

struct Browser<S: 'static> {
    app: Option<App<S>>,
    shared: Arc<crate::app::Shared<S>>,
    proxy: EventLoopProxy<Wake>,
    window: Option<Arc<Window>>,
    /// Inline in the page at its parent's width, rather than covering the viewport.
    embedded: bool,
    /// The app is the page (`App::web_document`): the browser scrolls it.
    document: bool,
    /// Rune made the canvas (no `App::canvas_id`), so it removes it when the app ends.
    owns_canvas: bool,
    borrowed: page::Borrowed,
    start: Start,
    /// The bytes of `App::font_url`'s fonts that loaded, once all have answered.
    fonts: Rc<RefCell<Option<Vec<Vec<u8>>>>>,
    /// The GPU, once the browser answered for it.
    pending: Rc<RefCell<Option<Result<(Gpu, wgpu::Surface<'static>), String>>>>,
    running: Rc<RefCell<Option<Running<S>>>>,
    /// Why the browser lost the GPU device, until the page goes back to the CPU for it.
    lost: Rc<RefCell<Option<String>>>,
    /// The deadline the event loop was last told to wait for (`None`: none). winit schedules a
    /// browser task for each new control flow and aborts the last one's, so the same deadline
    /// isn't set again.
    waiting: Option<u64>,
    /// For a page that starts on the CPU: when its first frame was presented, when it last drew
    /// one that changed more than an ambient loop (a spinner's step), and whether the GPU was
    /// asked for.
    first_at: Option<Instant>,
    drawn_at: Option<Instant>,
    gpu_asked: bool,
}

/// The GPU's side of the page: its device, the canvas's surface and Rune's renderer.
struct GpuSide {
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

/// Who draws the page's frames, and how far the GPU is (the native CPU-first window's
/// `SurfaceMode`, with two canvases for its two surfaces).
enum Draw {
    /// The CPU paints into its canvas under the GPU's, which stays see-through and takes the
    /// input. `failed`: the GPU won't come (no WebGPU, or it was lost), so this stays.
    Cpu { cpu: cpu_canvas::CpuCanvas, failed: bool },
    /// The GPU is made and its canvas configured, still see-through. `built` turns true once
    /// the queue has answered a submit made after the renderer's pipelines, so the browser has
    /// compiled them; the CPU paints on meanwhile. Then one frame is drawn by both from the same
    /// instances, and nothing new until the GPU's is done (`done`): its canvas then shows over
    /// the CPU's, which goes.
    HandingOver { cpu: cpu_canvas::CpuCanvas, side: GpuSide, built: Rc<Cell<bool>>, done: Option<Rc<Cell<bool>>>, since: Instant },
    Gpu(GpuSide),
    /// Only while one state is taken apart for the next: no frame sees it.
    Moving,
}

impl Draw {
    fn gpu(&self) -> Option<&GpuSide> {
        match self {
            Draw::HandingOver { side, .. } | Draw::Gpu(side) => Some(side),
            Draw::Cpu { .. } | Draw::Moving => None,
        }
    }

    fn gpu_mut(&mut self) -> Option<&mut GpuSide> {
        match self {
            Draw::HandingOver { side, .. } | Draw::Gpu(side) => Some(side),
            Draw::Cpu { .. } | Draw::Moving => None,
        }
    }

    /// The GPU's hand-over frame is drawn and not finished: nothing new is drawn until it is.
    fn waiting_for_gpu(&self) -> bool {
        matches!(self, Draw::HandingOver { done: Some(done), .. } if !done.get())
    }
}

struct Running<S: 'static> {
    draw: Draw,
    rt: Runtime<S>,
    shared: Arc<crate::app::Shared<S>>,
    clock: crate::frame::clock::FrameClock,
    compositor: Option<Box<dyn crate::Compositor<S>>>,
    suspended: bool,
    embedded: bool,
    document: bool,
    /// The first frame is on screen.
    presented: bool,
    /// `App::on_ready`, which runs once that first frame is presented, as a window's does.
    on_ready: Option<crate::app::OnWindow>,
    /// `App::font_url_later`'s fonts, fetched once that first frame is presented.
    late_fonts: Vec<String>,
    /// The app's fonts, which the page takes as its own once that first frame is presented
    /// (`page_fonts`).
    fonts: &'static [&'static [u8]],
    /// Where the canvas was last put in the document (a page's band top), in points.
    band: Option<f32>,
    /// The canvas's size in physical pixels: the CPU paints at it, the GPU's surface is it.
    size: (u32, u32),
    cursor: crate::cursor::os::OsCursor,
    _menus: crate::native_menu::Guard<S>,
    shortcuts: crate::shortcuts::Guard,
    input: input::Bridge,
    clipboard: crate::clipboard::Client,
}

/// The largest texture side a canvas may have before a GPU says: WebGPU's default limit.
const CPU_MAX_DIM: u32 = 8192;

impl<S: 'static> Running<S> {
    /// Ends the app: listeners off the page, then the GPU device, which the WebGPU backend
    /// doesn't destroy on drop.
    fn close(self) {
        self.shared.visible.store(false, Ordering::Release);
        let device = self.draw.gpu().map(|side| side.gpu.device.clone());
        drop(self);
        if let Some(device) = device { device.destroy(); }
    }

    fn max_dim(&self) -> u32 {
        self.draw.gpu().map_or(CPU_MAX_DIM, |side| side.gpu.device.limits().max_texture_dimension_2d.max(1))
    }

    pub(super) fn resize_surface(&mut self, window: &Window, size: winit::dpi::PhysicalSize<u32>, scale: f32) {
        let max_dim = self.max_dim();
        if self.document {
            // The canvas covers a band of the page: size it before the box is measured.
            let (view, band) = band_css(window, scale, max_dim);
            set_band_height(window, band);
            self.rt.set_band(band, view);
        }
        let size = window.canvas().map_or(size, |c| canvas_physical(&c, size, scale));
        self.suspended = size.width == 0 || size.height == 0;
        if self.suspended { return; }
        let next = (size.width.min(max_dim), size.height.min(max_dim));
        let document = self.document;
        if let Some(side) = self.draw.gpu_mut() {
            let (kept, reconfigure) = web_math::should_reconfigure_surface((side.config.width, side.config.height), next, document);
            if reconfigure {
                side.config.width = kept.0;
                side.config.height = kept.1;
                side.surface.configure(&side.gpu.device, &side.config);
            }
            self.size = (side.config.width, side.config.height);
        } else {
            self.size = next;
        }
        self.rt.set_viewport((next.0 as f32 / scale, next.1 as f32 / scale), scale);
    }

    /// Puts a page's canvas where the band it just drew starts in the document. The browser's
    /// compositor scrolls those pixels itself, at its own rate, until the window nears the band's
    /// edge and the next frame draws a new one.
    fn sync_band(&mut self, window: &Window) {
        if !self.document { return; }
        let top = self.rt.page_offset();
        if self.band == Some(top) { return; }
        self.band = Some(top);
        if let Some(canvas) = window.canvas() {
            let _ = canvas.style().set_property("transform", &format!("translate3d(0,{top}px,0)"));
        }
    }

    pub(super) fn apply_frame_requests(&mut self, window: &Window) {
        if !self.embedded
            && let Some(title) = self.rt.window_title.take_change()
            && let Some(doc) = web_sys::window().and_then(|w| w.document())
        {
            doc.set_title(title);
        }
        // A page's height is the mirror's, which `sync_input` keeps; the canvas stays the band.
        if self.document {
            self.rt.window_frame = None;
        } else if let Some(req) = self.rt.window_frame.take()
            && let Some((_, h)) = req.size
            && let Some(canvas) = window.canvas()
        {
            let _ = canvas.style().set_property("height", &format!("{h}px"));
        }
    }
}

/// A page's band in CSS points: `view`, how much of the document the window shows at once, and
/// `band`, the canvas that covers it. The view height is the *largest* viewport (`100lvh`), so a
/// phone's toolbar coming and going never changes the band. The skirt is clamped so the texture
/// stays inside the device's limit; a viewport already at the limit has no skirt.
fn band_css(window: &Window, scale: f32, max_dim: u32) -> (f32, f32) {
    let scale = scale.max(1e-3);
    let reported = large_viewport();
    let view = if reported > 0.0 { reported } else { window.inner_size().height as f32 / scale };
    let max = max_dim as f32 / scale;
    let band = (view + 2.0 * SKIRT).min(max.max(view));
    (view, band)
}

/// The canvas's CSS height is its band, in points.
fn set_band_height(window: &Window, band: f32) {
    if let Some(canvas) = window.canvas() {
        let height = format!("{band}px");
        if canvas.style().get_property_value("height").unwrap_or_default() != height {
            let _ = canvas.style().set_property("height", &height);
        }
    }
}

/// The canvas's size in device pixels: its CSS box times `scale` when the browser's own
/// `devicePixelContentBoxSize` (what winit reports) disagrees, as Chromium's emulated device
/// scale does by reporting CSS pixels. Trusting it would lay out at a fraction of the box.
fn canvas_physical(canvas: &web_sys::HtmlCanvasElement, reported: winit::dpi::PhysicalSize<u32>, scale: f32) -> winit::dpi::PhysicalSize<u32> {
    let rect = canvas.get_bounding_client_rect();
    let (w, h) = (rect.width() as f32, rect.height() as f32);
    if w <= 0.0 || h <= 0.0 { return reported; }
    let fix = |got: u32, css: f32| { let want = (css * scale).round().max(1.0); if (got as f32 - want).abs() > scale.ceil() + 1.0 { want as u32 } else { got } };
    winit::dpi::PhysicalSize::new(fix(reported.width, w), fix(reported.height, h))
}

/// Reports `message` to the console, and on screen unless the app is the page: a document's
/// static HTML is a working page, and an opaque overlay would cover the page a reader can still
/// read and scroll. Ending the app leaves that page as the browser found it.
fn error(message: &str, target: Option<&web_sys::HtmlCanvasElement>, embedded: bool, document: bool) {
    web_sys::console::error_1(&message.into());
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    if document && doc.query_selector("[data-rune-static]").ok().flatten().is_some() { return; }
    if let Ok(label) = doc.create_element("pre") {
        label.set_text_content(Some(message));
        if embedded && let Some(canvas) = target && let Some(parent) = canvas.parent_element() {
            let _ = label.set_attribute("style", "margin:0;padding:16px;white-space:pre-wrap;background:Canvas;color:CanvasText;border:1px solid #444;border-radius:8px");
            let _ = parent.insert_before(&label, Some(canvas));
            return;
        }
        if let Some(body) = doc.body() {
            let _ = label.set_attribute("style", "position:fixed;inset:0;z-index:1;padding:24px;white-space:pre-wrap;background:Canvas;color:CanvasText");
            let _ = body.append_child(&label);
        }
    }
}

async fn try_backend(canvas: &web_sys::HtmlCanvasElement, backends: wgpu::Backends, features: wgpu::Features, webgl: bool)
    -> Result<(Gpu, wgpu::Surface<'static>), String> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    let instance = wgpu::Instance::new(descriptor);
    let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone())).map_err(|e| e.to_string())?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower, compatible_surface: Some(&surface),
        force_fallback_adapter: false, apply_limit_buckets: false,
    }).await.map_err(|e| e.to_string())?;
    let required_limits = if webgl {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
    } else {
        wgpu::Limits::default().using_resolution(adapter.limits())
    };
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rune web"), required_features: adapter.features() & features,
        required_limits, experimental_features: Default::default(),
        memory_hints: Default::default(), trace: Default::default(),
    }).await.map_err(|e| format!("GPU device: {e}"))?;
    Ok((Gpu::from_device(instance, adapter, device, queue), surface))
}

async fn gpu(canvas: web_sys::HtmlCanvasElement, features: wgpu::Features) -> Result<(Gpu, wgpu::Surface<'static>), String> {
    match try_backend(&canvas, wgpu::Backends::BROWSER_WEBGPU, features, false).await {
        Ok(pair) => Ok(pair),
        #[cfg(feature = "webgl")]
        Err(webgpu_err) => try_backend(&canvas, wgpu::Backends::GL, features, true)
            .await
            .map_err(|gl_err| format!("WebGPU ({webgpu_err}) and WebGL2 ({gl_err}) unavailable")),
        #[cfg(not(feature = "webgl"))]
        Err(e) => Err(format!("WebGPU adapter unavailable (HTTPS and WebGPU are required): {e}")),
    }
}


/// The GPU's side for a canvas of `size`: the surface configured, the renderer made, and the
/// text system told its masks are the renderer's to fill from now on. `lost` runs if the device
/// is lost.
fn gpu_side<S: 'static>(gpu: Gpu, surface: wgpu::Surface<'static>, size: (u32, u32), rt: &mut Runtime<S>, lost: impl Fn(String) + 'static) -> Result<GpuSide, String> {
    let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
    let Some(mut config) = surface.get_default_config(&gpu.adapter, size.0.clamp(1, max_dim), size.1.clamp(1, max_dim)) else {
        return Err("WebGPU canvas has no supported configuration".into());
    };
    config.present_mode = wgpu::PresentMode::Fifo;
    let caps = surface.get_capabilities(&gpu.adapter);
    config.usage |= caps.usages & wgpu::TextureUsages::COPY_SRC;
    // A canvas can always be copied from; the hand-over's drift count needs that.
    if drift_wanted() { config.usage |= wgpu::TextureUsages::COPY_SRC; }
    let wanted = if rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format.add_srgb_suffix() };
    let is_gl = gpu.adapter.get_info().backend == wgpu::Backend::Gl;
    let format = if wanted != config.format && !is_gl {
        config.view_formats.push(wanted);
        wanted
    } else {
        config.format
    };
    surface.configure(&gpu.device, &config);
    health::watch(&gpu.device, lost);
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(!format.is_srgb());
    renderer.attach_text(&mut rt.text.atlas);
    Ok(GpuSide { gpu, surface, config, renderer })
}

/// A frame drawn on the GPU from the runtime's instances, presented; `Err` when the canvas gave
/// no texture to draw into.
fn gpu_frame<S: 'static>(side: &mut GpuSide, rt: &mut Runtime<S>, compositor: &mut Option<Box<dyn crate::Compositor<S>>>, now: Instant, dt: f64, readback: bool) -> Result<Option<Readback>, String> {
    if side.gpu.adapter.get_info().backend != wgpu::Backend::Gl && side.renderer.srgb_blend() != rt.srgb_blend() {
        let format = if rt.srgb_blend() { side.config.format.remove_srgb_suffix() } else { side.config.format.add_srgb_suffix() };
        if format != side.config.format && !side.config.view_formats.contains(&format) {
            side.config.view_formats.push(format);
            side.surface.configure(&side.gpu.device, &side.config);
        }
        crate::window::rebuild_renderer(&side.gpu.device, &mut side.renderer, format, rt);
    }
    let frame = match side.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        _ => return Err("WebGPU canvas acquisition failed".into()),
    };
    let target = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(side.renderer.format()), ..Default::default() });
    side.renderer.upload_images(&side.gpu.device, &side.gpu.queue, &mut rt.images);
    let mut encoder = side.gpu.device.create_command_encoder(&Default::default());
    if let Some(compositor) = compositor {
        compositor.render(&mut crate::Frame { device: &side.gpu.device, queue: &side.gpu.queue, encoder: &mut encoder,
            target: &target, format: side.renderer.format(), size: (side.config.width, side.config.height), dt,
            update_ms: now.elapsed().as_secs_f64() * 1000.0, rt, ui: &mut side.renderer });
    } else {
        side.renderer.prepare(&side.gpu.device, &side.gpu.queue, &mut rt.text.atlas, &rt.instances, (side.config.width, side.config.height));
        side.renderer.draw(&mut encoder, &target, rt.clear_color());
    }
    // A copy of the frame for counting how many of its pixels the CPU's differ in (drift).
    let copy = (readback && side.config.usage.contains(wgpu::TextureUsages::COPY_SRC)).then(|| {
        let (width, height) = (side.config.width, side.config.height);
        let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = side.gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("rune hand-over readback"), size: u64::from(row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        encoder.copy_texture_to_buffer(frame.texture.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
        Readback { buffer, row, size: (width, height), rgba: matches!(side.config.format, wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb) }
    });
    side.renderer.submit(&side.gpu.queue, encoder);
    side.gpu.queue.present(frame);
    Ok(copy)
}

/// The GPU's hand-over frame, copied where the CPU can read it.
struct Readback {
    buffer: wgpu::Buffer,
    row: u32,
    size: (u32, u32),
    /// The canvas's format is RGBA (the CPU paints BGRA).
    rgba: bool,
}

impl Readback {
    /// Counts the pixels of `cpu` (BGRA, `size.0 * 4` bytes a row) this frame drew otherwise, and
    /// appends `[differing, of, max]` to `globalThis.__runeDrift` once the copy is mapped.
    fn count(self, cpu: Vec<u8>) {
        let Readback { buffer, row, size: (w, h), rgba } = self;
        let buffer = Rc::new(buffer);
        let mapped = buffer.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if result.is_err() { return; }
            let (mut differing, mut max) = (0u32, 0u8);
            let mut bbox = [u32::MAX, u32::MAX, 0u32, 0u32];
            let mut worst = [0u32; 8];
            {
                let Ok(gpu) = mapped.slice(..).get_mapped_range() else { return };
                for y in 0..h as usize {
                    let g = &gpu[y * row as usize..y * row as usize + w as usize * 4];
                    let c = &cpu[y * w as usize * 4..(y + 1) * w as usize * 4];
                    for (x, (p, q)) in c.chunks_exact(4).zip(g.chunks_exact(4)).enumerate() {
                        let q = if rgba { [q[2], q[1], q[0]] } else { [q[0], q[1], q[2]] };
                        let d = p[..3].iter().zip(&q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
                        if d > 1 {
                            let (x, y) = (x as u32, y as u32);
                            bbox = [bbox[0].min(x), bbox[1].min(y), bbox[2].max(x + 1), bbox[3].max(y + 1)];
                        }
                        if d > max { worst = [x as u32, y as u32, u32::from(p[2]), u32::from(p[1]), u32::from(p[0]), u32::from(q[2]), u32::from(q[1]), u32::from(q[0])]; }
                        if d > 0 { differing += 1; max = max.max(d); }
                    }
                }
            }
            mapped.unmap();
            let Ok(list) = js_sys::Reflect::get(&js_sys::global(), &"__runeDrift".into()) else { return };
            if !js_sys::Array::is_array(&list) { return; }
            let list: js_sys::Array = list.unchecked_into();
            let row = js_sys::Array::of3(&differing.into(), &(w * h).into(), &max.into());
            for v in bbox.into_iter().chain(worst) { row.push(&v.into()); }
            list.push(&row);
        });
    }
}

/// Whether the page asked for the hand-over's drift (`globalThis.__runeDrift`, an array).
fn drift_wanted() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &"__runeDrift".into()).is_ok_and(|v| js_sys::Array::is_array(&v))
}

/// A flag the GPU's queue sets once the work submitted so far is done, waking the loop.
fn when_done(queue: &wgpu::Queue, proxy: &EventLoopProxy<Wake>) -> Rc<Cell<bool>> {
    let flag = Rc::new(Cell::new(false));
    let set = PageOnly((flag.clone(), proxy.clone()));
    queue.on_submitted_work_done(move || {
        let PageOnly((flag, proxy)) = &set;
        flag.set(true);
        let _ = proxy.send_event(Wake::App);
    });
    flag
}

/// A value moved into a callback wgpu wants `Send`. In a page that callback runs on the page's
/// one thread, from a promise.
struct PageOnly<T>(T);
// SAFETY: wasm32 without threads: there is one thread, and every callback runs on it.
unsafe impl<T> Send for PageOnly<T> {}

/// How long the GPU's hand-over frame may take to finish before the page stays on the CPU.
const FINISH_LIMIT: Duration = Duration::from_secs(5);

impl<S: 'static> Browser<S> {
    fn end(&self, event_loop: &ActiveEventLoop) {
        self.shared.quitting.store(true, Ordering::Release);
        event_loop.exit();
    }

    /// The browser lost the GPU device. A page with a CPU renderer goes back to it: the next
    /// frame paints whole, and the glyph masks the device held fill again on the CPU. A page that
    /// started on the GPU ends, as nothing more can draw. True when the app ended.
    fn lost_device(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(why) = self.lost.borrow_mut().take() else { return false };
        let Some(window) = self.window.clone() else { return false };
        let canvas = window.canvas();
        if self.start == Start::GpuFirst {
            error(&format!("The GPU device was lost ({why}). Reload the page to draw again."), canvas.as_ref(), self.embedded, self.document);
            self.end(event_loop);
            return true;
        }
        let mut running = self.running.borrow_mut();
        let Some(r) = &mut *running else { return false };
        let side = match std::mem::replace(&mut r.draw, Draw::Moving) {
            Draw::Gpu(side) => match canvas.as_ref().and_then(cpu_canvas::CpuCanvas::under) {
                Some(cpu) => { r.draw = Draw::Cpu { cpu, failed: true }; side }
                None => {
                    r.draw = Draw::Gpu(side);
                    drop(running);
                    error(&format!("The GPU device was lost ({why}). Reload the page to draw again."), canvas.as_ref(), self.embedded, self.document);
                    self.end(event_loop);
                    return true;
                }
            },
            // Mid hand-over: the CPU's canvas is still up, and stays.
            Draw::HandingOver { cpu, side, .. } => { r.draw = Draw::Cpu { cpu, failed: true }; side }
            other => { r.draw = other; return false; }
        };
        web_sys::console::warn_1(&format!("rune: the GPU device was lost ({why}); the page draws on the CPU").into());
        crate::marks::mark("web.gpu.lost");
        side.gpu.device.destroy();
        crate::render::cpu_boot::attach_text(&mut r.rt.text.atlas);
        r.rt.text.atlas.refill_gpu_masks();
        if let Some(c) = &canvas { let _ = c.style().set_property("opacity", "0"); }
        window.request_redraw();
        false
    }

    /// Makes the running app once its fonts are in, and for a page that starts on the GPU its
    /// GPU too: the runtime, the input bridge and what draws its first frame.
    fn start_app(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.borrow().is_some() || self.app.is_none() { return; }
        let Some(window) = self.window.clone() else { return };
        if self.fonts.borrow().is_none() { return; }
        let gpu_first = self.start == Start::GpuFirst;
        if gpu_first && self.pending.borrow().is_none() { return; }
        let fonts = self.fonts.borrow_mut().take().unwrap_or_default();
        let gpu = if gpu_first {
            match self.pending.borrow_mut().take() {
                Some(Ok(gpu)) => Some(gpu),
                Some(Err(e)) => {
                    if let Some(app) = &self.app { *app.shared.window_error.lock().unwrap() = Some(e.clone()); }
                    error(&e, window.canvas().as_ref(), self.embedded, self.document);
                    self.end(event_loop);
                    return;
                }
                None => return,
            }
        } else {
            None
        };
        let mut app = self.app.take().expect("one canvas initialization");
        for bytes in fonts { app.spec.fonts = crate::app::with_font(app.spec.fonts, bytes); }
        let compositor = app.compositor.take();
        let (on_focus, modifier_keys, srgb_blend) = (app.on_focus, app.modifier_keys, app.srgb_blend);
        let on_mods = app.on_mods;
        let (font_smoothing, fit_height) = (app.font_smoothing, app.fit_content_height);
        let tab_visits_controls = app.tab_visits_controls;
        let playground = app.playground.take();
        let on_ready = app.on_ready.take();
        let late_fonts = std::mem::take(&mut app.late_font_urls);
        // A page brings its own fonts for its mirror; an app's are the page's (`page_fonts`).
        let page_fonts = if self.document { &[][..] } else { app.spec.fonts };
        let (spec, state, shared) = app.into_parts(None);
        let scale = window.scale_factor() as f32;
        let document = self.document;
        let max_dim = gpu.as_ref().map_or(CPU_MAX_DIM, |(gpu, _)| gpu.device.limits().max_texture_dimension_2d.max(1));
        // A page's canvas is its band: the visible height plus a skirt of overscan, sized
        // before the box is measured so the first frame is that tall.
        let band = if document {
            let (view, band) = band_css(&window, scale, max_dim);
            set_band_height(&window, band);
            Some((view, band))
        } else {
            None
        };
        // Both sides from the canvas's own box: a browser that reports the box in CSS pixels
        // (Chromium's emulated device scale) would otherwise leave the height at the CSS
        // height, which its style stretches back over the screen: a 2× vertical stretch.
        let inner = window.inner_size();
        let css = window.canvas().map_or((0.0, 0.0), |c| (c.client_width().max(0) as f32, c.client_height().max(0) as f32));
        let fixed = web_math::fix_canvas_size(css, (inner.width, inner.height), spec.size, scale, fit_height);
        let mut size = winit::dpi::PhysicalSize::new(fixed.0, fixed.1);
        let dark = web_sys::window().and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten()).is_some_and(|q| q.matches());
        let proxy = self.proxy.clone();
        let wake: crate::Waker = Arc::new(move || { let _ = proxy.send_event(Wake::App); });
        let mut rt = crate::tooling::runtime_for(&spec, state, (size.width as f32 / scale, size.height as f32 / scale), Env::new(dark, scale), wake.clone());
        crate::playground::attach(&mut rt, playground, &spec, None, None, true);
        if let Some((view, band)) = band {
            rt.page_mode();
            rt.set_band(band, view);
            rt.set_viewport((size.width as f32 / scale, band), scale);
        } else if fit_height {
            rt.fit_height = true;
            let w = (size.width as f32 / scale).max(1.0);
            rt.set_viewport((w, spec.size.1), scale);
            let h = rt.content_height();
            rt.set_viewport((w, h), scale);
            size.height = (h * scale).round().max(1.0) as u32;
            if let Some(canvas) = window.canvas() {
                let _ = canvas.style().set_property("height", &format!("{h}px"));
            }
        }
        rt.on_focus = on_focus;
        rt.on_mods = on_mods;
        rt.set_modifier_keys(modifier_keys);
        rt.set_tab_visits_controls(tab_visits_controls);
        rt.set_srgb_blend(srgb_blend);
        rt.set_font_smoothing(font_smoothing);
        rt.set_os_drag(false);
        let now = Instant::now();
        rt.wall_start = Some(now);
        rt.open_urls_natively = true;
        rt.record_events = false;
        let menus = crate::native_menu::Guard::attach(&shared, false);
        let shortcuts = crate::shortcuts::Guard::attach(&shared, wake.clone(), false, false);
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.connect(wake);
        shared.visible.store(true, Ordering::Release);
        let physical = (size.width.clamp(1, max_dim), size.height.clamp(1, max_dim));
        let draw = match gpu {
            Some((gpu, surface)) => {
                let (lost, proxy) = (self.lost.clone(), self.proxy.clone());
                match gpu_side(gpu, surface, physical, &mut rt, move |why| { *lost.borrow_mut() = Some(why); let _ = proxy.send_event(Wake::App); }) {
                    Ok(side) => Draw::Gpu(side),
                    Err(e) => { error(&e, window.canvas().as_ref(), self.embedded, self.document); self.end(event_loop); return; }
                }
            }
            None => {
                let Some(canvas) = window.canvas() else { return };
                let Some(cpu) = cpu_canvas::CpuCanvas::under(&canvas) else {
                    error("This page couldn't make a canvas to draw in.", Some(&canvas), self.embedded, self.document);
                    self.end(event_loop);
                    return;
                };
                // The first frame's glyphs queue fills for the CPU painter.
                crate::render::cpu_boot::attach_text(&mut rt.text.atlas);
                Draw::Cpu { cpu, failed: self.start == Start::CpuOnly }
            }
        };
        let cursor = crate::cursor::os::OsCursor::new();
        let input = input::Bridge::new(&window.canvas().unwrap(), Rc::downgrade(&self.running), window.clone(),
            input::Mode { embedded: self.embedded, fit_height, document });
        *self.running.borrow_mut() = Some(Running { draw, rt, shared, input, clipboard,
            clock: crate::frame::clock::FrameClock::new(now), compositor, suspended: false, embedded: self.embedded,
            document, presented: false, band: None, size: physical, on_ready, late_fonts, fonts: page_fonts,
            cursor, _menus: menus, shortcuts });
        crate::marks::mark("web.started");
        window.request_redraw();
    }

    /// The GPU answered while the page draws on the CPU: after the first frame, its side is made
    /// (the browser compiles its pipelines) and the hand-over starts. A GPU that couldn't be made
    /// leaves the page on the CPU for good.
    fn gpu_came(&mut self) {
        if self.start != Start::CpuFirst { return; }
        let mut running = self.running.borrow_mut();
        let Some(r) = &mut *running else { return };
        if !r.presented || !matches!(r.draw, Draw::Cpu { failed: false, .. }) { return; }
        let Some(made) = self.pending.borrow_mut().take() else { return };
        let side = made.and_then(|(gpu, surface)| {
            let (lost, proxy) = (self.lost.clone(), self.proxy.clone());
            gpu_side(gpu, surface, r.size, &mut r.rt, move |why| { *lost.borrow_mut() = Some(why); let _ = proxy.send_event(Wake::App); })
        });
        let side = match side {
            Ok(side) => side,
            Err(why) => {
                web_sys::console::warn_1(&format!("rune: the page draws on the CPU ({why})").into());
                crate::marks::mark("web.gpu.failed");
                if let Draw::Cpu { failed, .. } = &mut r.draw { *failed = true; }
                return;
            }
        };
        // The browser compiles the renderer's pipelines in its GPU process, in order: a submit
        // made after them is answered once they're built.
        side.gpu.queue.submit([]);
        let built = when_done(&side.gpu.queue, &self.proxy);
        let Draw::Cpu { mut cpu, .. } = std::mem::replace(&mut r.draw, Draw::Moving) else { unreachable!("checked above") };
        cpu.half = crate::render::cpu_boot::HalfRounding::of_vendor(side.gpu.adapter.get_info().vendor);
        r.draw = Draw::HandingOver { cpu, side, built, done: None, since: Instant::now() };
        crate::marks::mark("web.gpu.ready");
    }

    /// While handing over: once the GPU's frame is done, its canvas shows and the CPU's goes, in
    /// one turn, so the next composition has the GPU's frame where the CPU's was. A frame that
    /// doesn't finish in `FINISH_LIMIT` leaves the page on the CPU.
    fn hand_over_if_done(&mut self) {
        let mut running = self.running.borrow_mut();
        let Some(r) = &mut *running else { return };
        let Draw::HandingOver { done: Some(done), since, .. } = &r.draw else { return };
        let (finished, late) = (done.get(), since.elapsed() > FINISH_LIMIT);
        if !finished && !late { return; }
        let Draw::HandingOver { cpu, side, .. } = std::mem::replace(&mut r.draw, Draw::Moving) else { unreachable!("checked above") };
        let canvas = self.window.as_ref().and_then(|w| w.canvas());
        if !finished {
            web_sys::console::warn_1(&"rune: the GPU didn't finish its first frame; the page draws on the CPU".into());
            crate::marks::mark("web.handover.failed");
            side.gpu.device.destroy();
            crate::render::cpu_boot::attach_text(&mut r.rt.text.atlas);
            r.rt.text.atlas.refill_gpu_masks();
            r.draw = Draw::Cpu { cpu, failed: true };
            return;
        }
        if let Some(c) = &canvas { let _ = c.style().remove_property("opacity"); }
        cpu.remove();
        // A harness checking the way back to the CPU (`globalThis.RUNE_LOSE_GPU_AFTER`, in ms)
        // has the device count as lost that long after the hand-over, as the browser reports it.
        if let Some(ms) = js_sys::Reflect::get(&js_sys::global(), &"RUNE_LOSE_GPU_AFTER".into()).ok().and_then(|v| v.as_f64()) {
            let (lost, proxy) = (self.lost.clone(), self.proxy.clone());
            after(Duration::from_millis(ms as u64), move || {
                *lost.borrow_mut() = Some("RUNE_LOSE_GPU_AFTER".into());
                let _ = proxy.send_event(Wake::App);
            });
        }
        r.draw = Draw::Gpu(side);
        crate::marks::mark("web.handover.done");
        if let Some(window) = &self.window { window.request_redraw(); }
    }

    /// Asks the browser for the GPU, once.
    fn ask_gpu(&mut self) {
        if self.gpu_asked || self.start == Start::CpuOnly { return; }
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        self.gpu_asked = true;
        crate::marks::mark("web.gpu.asked");
        let features = self.app.as_ref().and_then(|a| a.compositor.as_ref()).map_or(wgpu::Features::empty(), |c| c.features());
        let (pending, proxy) = (self.pending.clone(), self.proxy.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let result = gpu(canvas, features).await;
            // A loop that has closed was quit while the GPU started.
            if proxy.send_event(Wake::Ready).is_err() {
                if let Ok((gpu, _)) = result { gpu.device.destroy(); }
                return;
            }
            crate::marks::mark("web.gpu.answered");
            *pending.borrow_mut() = Some(result);
        });
    }

    /// After the first frame of a page that starts on the CPU: the GPU is asked for once the page
    /// has drawn nothing but ambient loops for `QUIET` (its first reads in and shown; a spinner on
    /// screen would otherwise hold the page on the CPU, painting each of its steps there), or `QUIET_LIMIT` after that
    /// frame at the latest; until then this looks again every `QUIET`.
    fn maybe_start_gpu(&mut self) {
        if self.gpu_asked || self.start != Start::CpuFirst { return; }
        let Some(first) = self.first_at else { return };
        let quiet = self.drawn_at.is_none_or(|at| at.elapsed() >= QUIET);
        if quiet || first.elapsed() >= QUIET_LIMIT { return self.ask_gpu(); }
        let proxy = self.proxy.clone();
        after(QUIET, move || { let _ = proxy.send_event(Wake::StartGpu); });
    }

    /// Draws a frame: the runtime steps, then the CPU paints it, the GPU draws it, or both while
    /// handing over.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        self.hand_over_if_done();
        let mut running = self.running.borrow_mut();
        let (Some(window), Some(r)) = (&self.window, &mut *running) else { return };
        if r.suspended || r.draw.waiting_for_gpu() { return; }
        let now = Instant::now();
        let (commits, input) = (r.rt.stats.commits, r.rt.has_pending_input());
        let (animating, dt) = r.clock.advance(&mut r.rt, now, r.compositor.as_ref().is_some_and(|c| c.animating()));
        r.apply_frame_requests(window);
        r.clipboard.flush(&mut r.rt);
        // A menu asked for opens as the page's own elements (bridge.js `showMenu`), once the frame
        // that asked has laid out what it anchors to.
        if !r.rt.popup_waits_for_layout() {
            match r.rt.take_popup() {
                Some(Some(show)) => r.input.show_menu(&show, r.rt.backdrop_dark().unwrap_or(r.rt.env.system_dark)),
                Some(None) => { let _ = r.rt.resolve_popup(None); }
                None => {}
            }
        }
        r.rt.refuse_file_dialog("native file dialogs are unsupported on web");
        r.sync_input(false);
        r.cursor.apply(window, r.rt.cursor(), r.rt.pointer().is_some());
        if !r.rt.surfaces.is_empty() {
            error("native video surfaces are unsupported on web", window.canvas().as_ref(), r.embedded, r.document);
            drop(running);
            self.end(event_loop);
            return;
        }
        let size = r.size;
        let drawn = match &mut r.draw {
            // A frame only the GPU draws (`Scene::needs_gpu`) leaves the last one up.
            Draw::Cpu { cpu, .. } => Ok(cpu.paint(&mut r.rt, size).is_ok()),
            Draw::HandingOver { cpu, side, built, done, since } => {
                let painted = cpu.paint(&mut r.rt, size).is_ok();
                // Both renderers draw this frame once the browser has compiled the GPU's
                // pipelines; the GPU's canvas stays see-through until its frame is done.
                if built.get() {
                    match gpu_frame(side, &mut r.rt, &mut r.compositor, now, dt, drift_wanted()) {
                        // That frame made pipelines its draw needed and drew a fallback for each
                        // while the browser checks it (the renderer hears the answers as it
                        // draws): frames go on drawing into the see-through canvas, and the
                        // first one drawn with every check answered is the hand-over frame.
                        Ok(_) if side.renderer.checking_gpu() => crate::marks::mark("web.handover.warming"),
                        Ok(copy) => {
                            if let Some(copy) = copy { copy.count(cpu.pixels().to_vec()); }
                            *done = Some(when_done(&side.gpu.queue, &self.proxy));
                            *since = Instant::now();
                            let proxy = self.proxy.clone();
                            after(FINISH_LIMIT + Duration::from_millis(50), move || { let _ = proxy.send_event(Wake::App); });
                            crate::marks::mark("web.handover.drawn");
                        }
                        Err(e) => web_sys::console::warn_1(&format!("rune: the GPU's hand-over frame wasn't drawn: {e}").into()),
                    }
                }
                Ok(painted)
            }
            Draw::Gpu(side) => {
                window.pre_present_notify();
                gpu_frame(side, &mut r.rt, &mut r.compositor, now, dt, false).map(|_| true)
            }
            Draw::Moving => Ok(false),
        };
        let drawn = match drawn {
            Ok(drawn) => drawn,
            Err(e) => {
                error(&e, window.canvas().as_ref(), r.embedded, r.document);
                drop(running);
                self.end(event_loop);
                return;
            }
        };
        // In the same commit as the frame: the canvas moves to where this frame drew it.
        r.sync_band(window);
        // Only an ambient loop's step (a frame its wake asked for, no commit, nothing else moving).
        let ambient = !input && !animating && r.rt.stats.commits == commits && r.rt.ambient.is_some();
        if drawn && !ambient { self.drawn_at = Some(now); }
        if drawn && !r.presented {
            r.presented = true;
            self.first_at = Some(now);
            let proxy = self.proxy.clone();
            after(QUIET, move || { let _ = proxy.send_event(Wake::StartGpu); });
            crate::marks::mark("web.first.presented");
            r.input.presented();
            if let Some(mut ready) = r.on_ready.take() { ready(&crate::WindowHandle::new("main")); }
            page_fonts::share_later(r.fonts);
            let proxy = self.proxy.clone();
            full_faces::fetch(std::mem::take(&mut r.late_fonts), !r.document, move || { let _ = proxy.send_event(Wake::App); });
        }
        if r.rt.shell_request.take().is_some() {
            drop(running);
            self.end(event_loop);
            return;
        }
        // A frame that drew a fallback while the browser checks a pipeline is drawn again once it
        // answers, and so is a hand-over waiting on that check.
        let checking = r.draw.gpu().is_some_and(|side| side.renderer.checking_gpu());
        let handing = matches!(r.draw, Draw::HandingOver { done: None, .. });
        if animating || r.rt.has_pending_input() || r.rt.live_pending() || r.compositor.as_ref().is_some_and(|c| c.animating()) || checking {
            window.request_redraw();
        }
        let presented = r.presented;
        drop(running);
        // A GPU that came before the first frame is taken up after it.
        if presented && !handing { self.gpu_came(); }
    }
}

/// How long a page that starts on the CPU draws nothing before it asks for the GPU, and the
/// longest after its first frame that it waits for that.
const QUIET: Duration = Duration::from_millis(250);
const QUIET_LIMIT: Duration = Duration::from_secs(3);

impl<S: 'static> ApplicationHandler<Wake> for Browser<S> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let app = self.app.as_ref().expect("app before canvas");
        self.document = app.web_document;
        self.embedded = app.web_inline() && !self.document;
        let existing = match &app.canvas_id {
            Some(id) => {
                let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id)) else {
                    error(&format!("canvas #{id} not found in document"), None, true, self.document);
                    self.end(event_loop);
                    return;
                };
                match el.dyn_into::<web_sys::HtmlCanvasElement>() {
                    Ok(canvas) => Some(canvas),
                    Err(_) => {
                        error(&format!("element #{id} is not a <canvas>"), None, true, self.document);
                        self.end(event_loop);
                        return;
                    }
                }
            }
            None => None,
        };
        self.owns_canvas = existing.is_none();
        if let Some(canvas) = &existing { self.borrowed.canvas(canvas); }
        // Not `active`: winit would focus the canvas before it's styled, and a browser scrolls a
        // focused element into view, so a page landed at its foot. The bridge focuses a
        // full-window canvas itself, without scrolling.
        let mut attrs = Window::default_attributes().with_title(app.spec.title).with_prevent_default(false).with_active(false);
        attrs = match &existing {
            Some(c) => attrs.with_canvas(Some(c.clone())).with_append(false),
            None => attrs.with_append(true),
        };
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(e) => { error(&e.to_string(), existing.as_ref(), self.embedded, self.document); self.end(event_loop); return; }
        };
        let canvas = window.canvas().expect("winit web canvas");
        if !canvas.has_attribute("tabindex") {
            let _ = canvas.set_attribute("tabindex", "0");
        }
        if self.document {
            // The browser scrolls the page over a canvas that is part of the document, not fixed
            // to the screen: the compositor moves the pixels Rune drew at the display's own rate,
            // and a frame is only drawn when the window nears the band's edge. Rune puts it in a
            // clipper as tall as the page (the bridge) and translates it to `page.top`.
            let _ = canvas.set_attribute("tabindex", "-1");
            canvas.style().set_css_text("position:absolute;left:0;top:0;width:100%;display:block;transform:translate3d(0,0,0);will-change:transform;pointer-events:auto;touch-action:pan-y pinch-zoom;outline:none;user-select:none;-webkit-user-select:none");
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                self.borrowed.viewport(&document);
                document.set_title(app.spec.title);
                if let Some(body) = document.body() { let _ = body.style().set_property("margin", "0"); }
            }
        } else if self.embedded {
            let touch = if app.fit_content_height { "pan-y" } else { "none" };
            canvas.style().set_css_text(&format!(
                "display:block;width:100%;touch-action:{touch};outline:none;user-select:none;-webkit-user-select:none"
            ));
        } else {
            // No border/padding: winit's physical input and resize coordinates must match pixels.
            canvas.style().set_css_text("position:fixed;inset:0;width:100%;height:100%;display:block;touch-action:none;outline:none;user-select:none;-webkit-user-select:none");
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                self.borrowed.viewport(&document);
                document.set_title(app.spec.title);
                if let Some(body) = document.body() {
                    let style = body.style();
                    for (name, value) in [("margin", "0"), ("overflow", "hidden"), ("overscroll-behavior", "none")] {
                        let _ = style.set_property(name, value);
                    }
                }
            }
        }
        // A full-window app starts on the CPU, under the GPU's canvas, which stays see-through
        // (and takes the input) until the GPU's first frame is done. A page that is the document
        // or sits inline, or an app with a compositor, starts on the GPU.
        let asked = cpu_first_asked();
        self.start = if self.document || self.embedded || app.compositor.is_some() || asked.as_deref() == Some("off") {
            Start::GpuFirst
        } else if asked.as_deref() == Some("only") {
            Start::CpuOnly
        } else {
            Start::CpuFirst
        };
        if self.start != Start::GpuFirst { let _ = canvas.style().set_property("opacity", "0"); }
        // The fonts download while the GPU starts; the CPU's first frame needs only them.
        let fonts: Vec<_> = app.font_urls.iter().map(|url| (url.clone(), platform::fetch_bytes(url))).collect();
        self.window = Some(window);
        let (slot, proxy) = (self.fonts.clone(), self.proxy.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let mut loaded = Vec::new();
            for (url, font) in fonts {
                match wasm_bindgen_futures::JsFuture::from(font).await {
                    Ok(bytes) => loaded.push(js_sys::Uint8Array::new(&bytes).to_vec()),
                    Err(e) => web_sys::console::warn_2(&format!("rune: font {url} did not load:").into(), &e),
                }
            }
            *slot.borrow_mut() = Some(loaded);
            let _ = proxy.send_event(Wake::Start);
        });
        // A page that starts on the CPU asks for the GPU once its first frames are up and it's
        // quiet (`maybe_start_gpu`): asked at once, the browser's GPU process makes the adapter
        // and compiles the renderer's pipelines while the first frame waits on it to show.
        if self.start == Start::GpuFirst { self.ask_gpu(); }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        if self.shared.quitting.load(Ordering::Acquire) { self.end(event_loop); return; }
        if self.lost_device(event_loop) { return; }
        if self.window.is_none() { return; }
        match event {
            Wake::Start | Wake::Ready => {
                self.start_app(event_loop);
                self.gpu_came();
            }
            Wake::StartGpu => self.maybe_start_gpu(),
            Wake::App => self.hand_over_if_done(),
        }
        let Some(window) = self.window.as_ref() else { return };
        if let Some(r) = &mut *self.running.borrow_mut() {
            r.shortcuts.drain();
            r.clipboard.receive(&mut r.rt);
            let requests = r.shared.take();
            if requests.quit.is_some() { self.end(event_loop); return; }
            if requests.window.as_ref().is_some_and(|r| !matches!(r, crate::app::WindowRequest::Open(_) | crate::app::WindowRequest::Show))
                || requests.position.is_some() || requests.place.is_some() || requests.activation == Some(crate::app::Activation::Minimize) {
                web_sys::console::warn_1(&"Native window management is ignored on web.".into());
            }
            let had_updates = !requests.updates.is_empty();
            for update in requests.updates { r.rt.update_if(update); }
            if !had_updates { r.rt.invalidate(); }
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::Focused(_)) { return; }
        if self.lost_device(event_loop) { return; }
        if matches!(event, WindowEvent::RedrawRequested) { return self.redraw(event_loop); }
        let mut running = self.running.borrow_mut();
        let (Some(window), Some(r)) = (&self.window, &mut *running) else { return };
        let _input = crate::input::time::arrived(&event);
        if r.window_input(&event, window) { window.request_redraw(); return; }
        match event {
            WindowEvent::Resized(size) => {
                r.resize_surface(window, size, window.scale_factor() as f32);
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                r.resize_surface(window, window.inner_size(), scale_factor as f32);
                window.request_redraw();
            }
            WindowEvent::ThemeChanged(theme) => { r.rt.set_system_dark(theme == winit::window::Theme::Dark); window.request_redraw(); }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        crate::cursor::os::make_pending(event_loop);
        // Channels the app serves with `task::serve` get their turn after the handlers that fed them.
        crate::task::pump();
        let Ok(running) = self.running.try_borrow() else { return };
        let Some(r) = &*running else { return };
        let (flow, waiting) = match r.rt.wake_deadline() {
            Some(deadline) if deadline <= r.rt.wall_start.unwrap().elapsed().as_secs_f64() => {
                if let Some(window) = &self.window { window.request_redraw(); }
                (ControlFlow::Wait, None)
            }
            Some(deadline) => (ControlFlow::WaitUntil(crate::time::for_winit(r.rt.wall_start.unwrap() + Duration::from_secs_f64(deadline.max(0.0)))), Some(deadline.to_bits())),
            None => (ControlFlow::Wait, None),
        };
        drop(running);
        if self.waiting != waiting || waiting.is_none() && !matches!(event_loop.control_flow(), ControlFlow::Wait) {
            self.waiting = waiting;
            event_loop.set_control_flow(flow);
        }
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: winit::event::StartCause) {
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. }) {
            if let Some(window) = &self.window { window.request_redraw(); }
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.shared.quitting.store(true, Ordering::Release);
        let running = self.running.borrow_mut().take();
        if let Some(r) = running { r.close(); }
        if let Some(Ok((gpu, _))) = self.pending.borrow_mut().take() { gpu.device.destroy(); }
        if let Some(window) = self.window.take() && self.owns_canvas && let Some(canvas) = window.canvas() { canvas.remove(); }
        std::mem::take(&mut self.borrowed).give_back();
        INSETS.set([0.0; 4]);
        LARGE_VIEWPORT.set(0.0);
        PRIMARY_POINTER.set(PrimaryPointer::Hover);
        page::vacate();
    }
}

/// Runs `f` once `delay` has passed, from the page's timer.
fn after(delay: Duration, f: impl FnOnce() + 'static) {
    let callback = wasm_bindgen::closure::Closure::once_into_js(f);
    if let Some(window) = web_sys::window() {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), delay.as_millis().min(i32::MAX as u128) as i32);
    }
}
