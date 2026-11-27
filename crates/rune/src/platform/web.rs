//! Single-threaded browser shell. GPU initialization yields to the browser; winit owns
//! demand-driven rAF and its device-pixel-content-box ResizeObserver (with DPR fallback).
use std::{cell::RefCell, rc::Rc, sync::{Arc, atomic::Ordering}, time::Duration};
use wasm_bindgen::JsCast;
use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys, WindowExtWebSys}, window::{Window, WindowId}};
use crate::{App, Env, app::SharedCore, time::Instant};
use super::web_math;
use app::{PendingApp, TypedWebApp, WebApp};
use gpu::Draw;

mod app;
mod cpu_canvas;
mod frame;
mod full_faces;
mod gpu;
mod handover;
pub(crate) mod health;
mod input;
mod page;
mod page_fonts;
mod pipelines;
pub(crate) mod platform;
pub(crate) use rune_text::segmenter;

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

use web_math::platform_events::PrimaryPointer;

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
/// browser popup policy applies. The new page receives no opener or referrer. An error when
/// no click or key press is active, as browsers block the tab then; a tab blocked otherwise
/// can't be seen, since the browser never hands back a tab opened without an opener.
pub fn open_url(url: &str) -> Result<(), String> {
    let url = crate::open_url::check(url).map_err(|e| e.to_string())?;
    match platform::open_link(&url) {
        Ok(true) => Ok(()),
        Ok(false) => Err("the browser blocks new tabs outside a click or key press".into()),
        Err(e) => Err(format!("{e:?}")),
    }
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
    /// Time to see whether the page is quiet enough to start the GPU (`Browser::gpu_startup_due`).
    StartGpu(crate::machine::Attempt),
    /// A CPU-first WebGPU probe failed; defer WebGL2 fallback until quiet or stay on CPU.
    WebGpuFailed,
    /// An async browser WebGPU error-scope check (`uber_check` or feature breaker) answered.
    GpuCheck,
    /// The empty-queue barrier after `Renderer::try_new` resolved (`HandoverEvent::Built`).
    HandoverBuilt,
    /// The hand-over GPU frame's `onSubmittedWorkDone` resolved (`HandoverEvent::GpuDone`).
    HandoverDone(crate::machine::Attempt),
    /// The hand-over `FINISH_LIMIT` timeout fired (`HandoverEvent::Tick`).
    HandoverTimeout(crate::machine::Attempt),
    App,
    /// A live value published: asks for a frame only while one is still pending.
    Live,
    /// A resize reconfigured the canvas, which clears it: draw now, before the browser paints.
    ResizeRedraw,
}

/// How a page's frames start (`crate::window::WebStart`): on the CPU until WebGPU/WebGL2 takes
/// over (a full-window app under `CpuFirst::Auto`, the default), on the CPU only
/// (`CpuFirst::Only`), or on the GPU from the first frame (`CpuFirst::Off`, or a document or
/// inline canvas). `globalThis.RUNE_CPU_FIRST` (`"0"`/`"off"`, `"only"`, `"auto"`) overrides
/// `App::cpu_first` without a rebuild.
type Start = crate::window::WebStart;

fn cpu_first_asked() -> Option<String> {
    js_sys::Reflect::get(&js_sys::global(), &"RUNE_CPU_FIRST".into()).ok().and_then(|v| v.as_string())
}

pub(crate) fn run<S: 'static>(app: App<S>) -> Result<(), String> {
    run_pending(TypedWebApp::pending(app)?)
}

fn run_pending(app: PendingApp) -> Result<(), String> {
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
    js_sys::Reflect::set(&global, &"__runeLines".into(), lines.as_ref()).ok();
    lines.forget();
}

fn quitting(shared: &Arc<SharedCore>) -> Box<dyn Fn() -> bool> {
    let shared = shared.clone();
    Box::new(move || shared.quitting.load(Ordering::Acquire))
}

fn spawn(mut app: PendingApp) -> Result<(), String> {
    if app.shared.quitting.load(Ordering::Acquire) { return Ok(()); }
    let event_loop = EventLoop::<Wake>::with_user_event().build().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let shared = app.shared.clone();
    let wake = proxy.clone();
    shared.connect(Arc::new(move || { wake.send_event(Wake::App).ok(); }));
    page::occupy(quitting(&shared), app.same_tab_router.take());
    let resize_redraw = frame::ResizeRedraw::new(proxy.clone());
    event_loop.spawn_app(Browser { app: Some(app), shared, proxy, resize_redraw, window: None, embedded: false, document: false, owns_canvas: true,
        borrowed: page::Borrowed::default(), start: Start::GpuFirst, fonts: Rc::default(), pending: Rc::default(), warmed: Rc::default(), running: Rc::default(),
        gpu_loss_policy: crate::GpuLossPolicy::Recover, terminal_failed: false, terminal_poll_pending: false,
        lost: Rc::default(), webgl_watch: None, webgl_restored: Rc::default(), webgl_restoring: false, waiting: None,
        gpu_startup: (crate::machine::Seq::default(), web_math::GpuStartup::default()), webgpu_err: Rc::default() });
    Ok(())
}

fn warn_native_window(requests: &crate::app::WindowRequests) {
    if requests.window.as_ref().is_some_and(|r| !matches!(r, crate::app::WindowRequest::Open(_) | crate::app::WindowRequest::Show))
        || requests.position.is_some() || requests.place.is_some() || requests.activation == Some(crate::app::Activation::Minimize) {
        web_sys::console::warn_1(&"Native window management is ignored on web.".into());
    }
}

struct Browser {
    app: Option<PendingApp>,
    shared: Arc<SharedCore>,
    proxy: EventLoopProxy<Wake>,
    resize_redraw: frame::ResizeRedraw,
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
    pending: Rc<RefCell<Option<Result<gpu::ReadyGpu, String>>>>,
    /// The pipelines compiled ahead for that GPU (`pipelines::warm`), kept until its renderer is made.
    warmed: Rc<RefCell<Option<pipelines::Warmed>>>,
    running: Rc<RefCell<Option<Running>>>,
    /// Configured `App::gpu_loss_policy`.
    gpu_loss_policy: crate::GpuLossPolicy,
    /// Permanently latched once `GpuLossPolicy::Terminal` sees a GPU failure.
    terminal_failed: bool,
    /// One-shot `AppSpec::poll` wake owed after entering terminal GPU failure.
    terminal_poll_pending: bool,
    /// Why the browser lost the GPU device or WebGL2 context, until `lost_device` handles it.
    lost: Rc<RefCell<Option<gpu::Loss>>>,
    /// Active `webglcontextlost` / `webglcontextrestored` listeners once WebGL2 has run on the canvas.
    webgl_watch: Option<health::WebglWatch>,
    /// Set by `webglcontextrestored` until `maybe_restore_webgl` spawns a WebGL2 rebuild.
    webgl_restored: Rc<std::cell::Cell<bool>>,
    /// True while a WebGL2 rebuild after `webglcontextrestored` is in flight.
    webgl_restoring: bool,
    /// The deadline the event loop was last told to wait for (`None`: none). winit schedules a
    /// browser task for each new control flow and aborts the last one's, so the same deadline
    /// isn't set again.
    waiting: Option<u64>,
    /// For a page that starts on the CPU: schedules the early WebGPU probe and gates
    /// synchronous GPU / WebGL2 setup on a quiet window after non-ambient draws.
    gpu_startup: (crate::machine::Seq, web_math::GpuStartup),
    /// Error from a failed CPU-first WebGPU probe, kept for the combined error if WebGL2 fails too.
    webgpu_err: Rc<RefCell<Option<String>>>,
}

struct Running {
    draw: Draw,
    app: Box<dyn WebApp>,
    shared: Arc<SharedCore>,
    clock: crate::frame::clock::FrameClock,
    suspended: bool,
    /// An input or app event can share a redraw with the animation timer. In that case the
    /// timer must not consume the redraw as a stand-alone paint.
    full_redraw: bool,
    embedded: bool,
    document: bool,
    /// The first frame is on screen.
    presented: bool,
    /// What the GPU canvas currently shows, while nothing has invalidated it. A requested frame
    /// that would draw these same pixels skips acquiring a canvas texture and presenting.
    last_presented: Option<crate::window::Presented>,
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
    _menus: crate::native_menu::MenuGuard,
    shortcuts: crate::shortcuts::Guard,
    input: input::Bridge,
    clipboard: crate::clipboard::Client,
}

impl Running {
    /// Ends the app: listeners off the page, then the GPU device, which the WebGPU backend
    /// doesn't destroy on drop.
    fn close(self) {
        self.shared.set_visible(false);
        let device = self.draw.gpu().map(|side| side.gpu.device.clone());
        drop(self);
        if let Some(device) = device { device.destroy(); }
    }
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
            label.set_attribute("style", "margin:0;padding:16px;white-space:pre-wrap;background:Canvas;color:CanvasText;border:1px solid #444;border-radius:8px").ok();
            parent.insert_before(&label, Some(canvas)).ok();
            return;
        }
        if let Some(body) = doc.body() {
            label.set_attribute("style", "position:fixed;inset:0;z-index:1;padding:24px;white-space:pre-wrap;background:Canvas;color:CanvasText").ok();
            body.append_child(&label).ok();
        }
    }
}

impl Browser {
    fn end(&self, event_loop: &ActiveEventLoop) {
        self.shared.quitting.store(true, Ordering::Release);
        event_loop.exit();
    }

    /// Runs `covered_wake` on a hidden tab or suspended canvas (where browsers pause rAF or
    /// drawing skips), flushes requests and clipboard, and exits if a handler asked to quit.
    fn covered_wake(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.as_ref() else { return };
        let should_end = if let Some(r) = &mut *self.running.borrow_mut() {
            r.app.covered_wake();
            r.apply_frame_requests(window);
            r.clipboard.flush_core(r.app.rt_mut());
            r.app.rt_mut().shell_request.take().is_some()
        } else {
            false
        };
        if should_end { self.end(event_loop); }
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
        let mut terminal_init_error = None;
        let gpu = if gpu_first {
            match self.pending.borrow_mut().take() {
                Some(Ok(gpu)) => Some(gpu),
                Some(Err(e)) => {
                    if self.gpu_loss_policy == crate::GpuLossPolicy::Terminal {
                        terminal_init_error = Some(e);
                        None
                    } else {
                        self.shared.set_window_error(Some(e.clone()));
                        error(&e, window.canvas().as_ref(), self.embedded, self.document);
                        self.end(event_loop);
                        return;
                    }
                }
                None => return,
            }
        } else {
            None
        };
        let mut pending = self.app.take().expect("one canvas initialization");
        let fit_height = pending.fit_height;
        let spec_size = pending.size;
        let on_ready = pending.on_ready.take();
        let late_fonts = std::mem::take(&mut pending.late_font_urls);
        let scale = window.scale_factor() as f32;
        let document = self.document;
        let max_dim = gpu.as_ref().map_or(frame::CPU_MAX_DIM, |ready| ready.gpu.device.limits().max_texture_dimension_2d.max(1));
        // A page's canvas is its band: the visible height plus a skirt of overscan, sized
        // before the box is measured so the first frame is that tall.
        let band = if document {
            let (view, band) = frame::band_css(&window, scale, max_dim);
            frame::set_band_height(&window, band);
            Some((view, band))
        } else {
            None
        };
        // Both sides from the canvas's own box: a browser that reports the box in CSS pixels
        // (Chromium's emulated device scale) would otherwise leave the height at the CSS
        // height, which its style stretches back over the screen: a 2× vertical stretch.
        let inner = window.inner_size();
        let css = window.canvas().map_or((0.0, 0.0), |c| (c.client_width().max(0) as f32, c.client_height().max(0) as f32));
        let fixed = web_math::fix_canvas_size(css, (inner.width, inner.height), spec_size, scale, fit_height);
        let mut size = winit::dpi::PhysicalSize::new(fixed.0, fixed.1);
        let dark = web_sys::window().and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten()).is_some_and(|q| q.matches());
        let proxy = self.proxy.clone();
        let wake: crate::Waker = Arc::new(move || { proxy.send_event(Wake::App).ok(); });
        let live_proxy = self.proxy.clone();
        let live_wake: crate::Waker = Arc::new(move || { live_proxy.send_event(Wake::Live).ok(); });
        let app::StartedWebApp { mut app, menus, shortcuts, fonts: spec_fonts } = pending.start(
            fonts,
            (size.width as f32 / scale, size.height as f32 / scale),
            Env::new(dark, scale),
            wake.clone(),
            live_wake,
        );
        // A page brings its own fonts for its mirror; an app's are the page's (`page_fonts`).
        let page_fonts = if document { &[][..] } else { spec_fonts };
        if let Some((view, band)) = band {
            let rt = app.rt_mut();
            rt.page_mode();
            rt.set_band(band, view);
            rt.set_viewport((size.width as f32 / scale, band), scale);
        } else if fit_height {
            let w = (size.width as f32 / scale).max(1.0);
            {
                let rt = app.rt_mut();
                rt.host.set_fit_height(true);
                rt.set_viewport((w, spec_size.1), scale);
            }
            let h = app.content_height();
            app.rt_mut().set_viewport((w, h), scale);
            size.height = (h * scale).round().max(1.0) as u32;
            if let Some(canvas) = window.canvas() {
                canvas.style().set_property("height", &format!("{h}px")).ok();
            }
        }
        let now = Instant::now();
        {
            let rt = app.rt_mut();
            rt.set_os_drag(false);
            rt.set_pointer_lock_policy(crate::runtime::LockPolicy::Async);
            rt.wall_start = Some(now);
            rt.host.open_urls_natively = true;
            rt.frame.set_record_events(false);
        }
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.connect(wake);
        self.shared.set_visible(true);
        let physical = (size.width.clamp(1, max_dim), size.height.clamp(1, max_dim));
        let mut started_webgl = false;
        let draw = match gpu {
            Some(ready) => {
                match gpu::gpu_side(ready, physical, app.rt_mut(), self.gpu_loss_policy, self.lost.clone(), self.proxy.clone()) {
                    Ok(side) => {
                        started_webgl = side.backend == wgpu::Backend::Gl;
                        Draw::Gpu(side)
                    }
                    Err(e) => {
                        if self.gpu_loss_policy == crate::GpuLossPolicy::Terminal {
                            terminal_init_error = Some(e);
                            Draw::TerminalFailed
                        } else {
                            self.shared.set_window_error(Some(e.clone()));
                            error(&e, window.canvas().as_ref(), self.embedded, self.document);
                            self.end(event_loop);
                            return;
                        }
                    }
                }
            }
            None if terminal_init_error.is_some() => Draw::TerminalFailed,
            None => {
                let Some(canvas) = window.canvas() else { return };
                let Some(cpu) = cpu_canvas::CpuCanvas::under(&canvas) else {
                    error("This page couldn't make a canvas to draw in.", Some(&canvas), self.embedded, self.document);
                    self.end(event_loop);
                    return;
                };
                // The first frame's glyphs queue fills for the CPU painter.
                crate::render::cpu_boot::attach_text(&mut app.rt_mut().text.atlas);
                Draw::Cpu { cpu, failed: self.start == Start::CpuOnly }
            }
        };
        if started_webgl { self.ensure_webgl_watch(); }
        let cursor = crate::cursor::os::OsCursor::new();
        app.rt_mut().set_touch_hold_policy(web_math::web_touch_hold_policy(input::live_touch_context_menu()));
        let input = input::Bridge::new(&window.canvas().unwrap(), Rc::downgrade(&self.running), window.clone(),
            input::Mode { embedded: self.embedded, fit_height, document });
        *self.running.borrow_mut() = Some(Running { draw, app, shared: self.shared.clone(), input, clipboard,
            clock: crate::frame::clock::FrameClock::new(now), suspended: false, full_redraw: true, embedded: self.embedded,
            document, presented: false, last_presented: None, band: None, size: physical, on_ready, late_fonts, fonts: page_fonts,
            cursor, _menus: menus, shortcuts });
        crate::marks::mark("web.started");
        if let Some(reason) = terminal_init_error {
            self.enter_terminal_gpu_loss(reason);
        } else {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler<Wake> for Browser {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let app = self.app.as_ref().expect("app before canvas");
        self.document = app.document;
        self.embedded = app.embedded;
        self.gpu_loss_policy = app.gpu_loss_policy;
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
        let mut attrs = Window::default_attributes().with_title(app.title).with_prevent_default(false).with_active(false);
        attrs = match &existing {
            Some(c) => attrs.with_canvas(Some(c.clone())).with_append(false),
            None => attrs.with_append(true),
        };
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(e) => { error(&e.to_string(), existing.as_ref(), self.embedded, self.document); self.end(event_loop); return; }
        };
        crate::window::displays::track(&window);
        let canvas = window.canvas().expect("winit web canvas");
        // The app's own canvas, for pages and tests to find beside the CPU-first one (`data-rune-cpu`).
        canvas.set_attribute("data-rune-canvas", "").ok();
        if !canvas.has_attribute("tabindex") {
            canvas.set_attribute("tabindex", "0").ok();
        }
        if self.document {
            // The browser scrolls the page over a canvas that is part of the document, not fixed
            // to the screen: the compositor moves the pixels Rune drew at the display's own rate,
            // and a frame is only drawn when the window nears the band's edge. Rune puts it in a
            // clipper as tall as the page (the bridge) and translates it to `page.top`.
            canvas.set_attribute("tabindex", "-1").ok();
            canvas.style().set_css_text("position:absolute;left:0;top:0;width:100%;display:block;transform:translate3d(0,0,0);will-change:transform;pointer-events:auto;touch-action:pan-y pinch-zoom;outline:none;user-select:none;-webkit-user-select:none");
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                self.borrowed.viewport(&document);
                document.set_title(app.title);
                if let Some(body) = document.body() { body.style().set_property("margin", "0").ok(); }
            }
        } else if self.embedded {
            platform::style_inline_canvas(&canvas, app.size.1, app.fit_height);
        } else {
            // No border/padding: winit's physical input and resize coordinates must match pixels.
            canvas.style().set_css_text("position:fixed;inset:0;width:100%;height:100%;display:block;touch-action:none;outline:none;user-select:none;-webkit-user-select:none");
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                self.borrowed.viewport(&document);
                document.set_title(app.title);
                if let Some(body) = document.body() {
                    let style = body.style();
                    for (name, value) in [("margin", "0"), ("overflow", "hidden"), ("overscroll-behavior", "none")] {
                        style.set_property(name, value).ok();
                    }
                }
            }
        }
        // A full-window app starts on the CPU, under the GPU's canvas, which stays see-through
        // (and takes the input) until the GPU's first frame is done; one with a compositor too:
        // its effect slots draw from the hand-over on, and never where the browser has no GPU
        // (starting on the GPU would leave such a page with nothing at all). A page that is the
        // document or sits inline starts on the GPU.
        let asked = cpu_first_asked();
        self.start = if self.gpu_loss_policy == crate::GpuLossPolicy::Terminal {
            Start::GpuFirst
        } else {
            app.cpu_first.resolve(asked.as_deref()).web_start(self.document, self.embedded)
        };
        if self.start != Start::GpuFirst { canvas.style().set_property("opacity", "0").ok(); }
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
            proxy.send_event(Wake::Start).ok();
        });
        // A page that starts on the CPU asks for the GPU once its first frame is presented
        // (`gpu_startup`): asked at once, the browser's GPU process makes the adapter and
        // compiles the renderer's pipelines while the first frame waits on it to show.
        if self.start == Start::GpuFirst { self.ask_gpu(); }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        if self.shared.quitting.load(Ordering::Acquire) { self.end(event_loop); return; }
        if self.lost_device(event_loop) || self.failed_shader(event_loop) { return; }
        if self.terminal_failed {
            if matches!(event, Wake::App) {
                self.drain_terminal_wake(event_loop);
            }
            return;
        }
        if self.window.is_none() { return; }
        self.maybe_restore_webgl();
        match event {
            Wake::Start | Wake::Ready => {
                self.start_app(event_loop);
                self.gpu_came();
            }
            Wake::StartGpu(attempt) => { self.gpu_startup_due(attempt); return; }
            Wake::WebGpuFailed => { self.webgpu_probe_failed(); return; }
            Wake::GpuCheck => { self.on_gpu_check(); return; }
            Wake::HandoverBuilt => { self.on_handover_built(); return; }
            Wake::HandoverDone(attempt) => { self.on_handover_done(attempt); return; }
            Wake::HandoverTimeout(attempt) => { self.on_handover_timeout(attempt); return; }
            Wake::App => self.poll_handover_timeout(),
            Wake::Live => {
                if let Some(r) = &mut *self.running.borrow_mut() && r.app.rt().live_pending() {
                    r.full_redraw = true;
                    if let Some(window) = self.window.as_ref() { window.request_redraw(); }
                }
                return;
            }
            Wake::ResizeRedraw => {
                if self.resize_redraw.fired() { self.redraw(event_loop); }
                return;
            }
        }
        let Some(window) = self.window.as_ref() else { return };
        let mut should_end = false;
        if let Some(r) = &mut *self.running.borrow_mut() {
            r.full_redraw = true;
            r.shortcuts.drain();
            r.clipboard.receive_core(r.app.rt_mut());
            let requests = self.shared.take_window();
            if requests.quit.is_some() { self.end(event_loop); return; }
            warn_native_window(&requests);
            let had_updates = {
                let _receiving = r.shortcuts.receiving();
                r.app.drain_updates()
            };
            if !had_updates { r.app.rt_mut().invalidate(); }
            if web_math::web_wake_covered(!self.shared.is_visible(), r.suspended) {
                r.app.covered_wake();
                r.apply_frame_requests(window);
                r.clipboard.flush_core(r.app.rt_mut());
                should_end = r.app.rt_mut().shell_request.take().is_some();
            } else {
                window.request_redraw();
            }
        }
        if should_end { self.end(event_loop); }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::Focused(_)) { return; }
        if self.lost_device(event_loop) || self.failed_shader(event_loop) { return; }
        if self.terminal_failed { return; }
        if matches!(event, WindowEvent::RedrawRequested) { return self.redraw(event_loop); }
        let mut running = self.running.borrow_mut();
        let (Some(window), Some(r)) = (&self.window, &mut *running) else { return };
        let _input = crate::input::time::arrived(&event);
        if r.window_input(&event, window) { r.full_redraw = true; window.request_redraw(); return; }
        match event {
            // Reconfiguring the surface clears the canvas. These arrive from winit's
            // ResizeObserver, after this frame's rAF and before it paints, so a requested redraw
            // would land a frame late over a blank canvas. winit holds its canvas borrowed while
            // it delivers them, so the frame is drawn in the microtask right after.
            WindowEvent::Resized(size) => {
                r.full_redraw = true;
                r.resize_surface(window, size, window.scale_factor() as f32);
                self.resize_redraw.resized();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                r.full_redraw = true;
                r.resize_surface(window, window.inner_size(), scale_factor as f32);
                self.resize_redraw.resized();
            }
            WindowEvent::ThemeChanged(theme) => { r.full_redraw = true; r.app.rt_mut().set_system_dark(theme == winit::window::Theme::Dark); window.request_redraw(); }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.terminal_failed {
            if self.waiting.is_some() || !matches!(event_loop.control_flow(), ControlFlow::Wait) {
                self.waiting = None;
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            return;
        }
        crate::cursor::os::make_pending(event_loop);
        // Channels the app serves with `task::serve` get their turn after the handlers that fed them.
        crate::task::pump();
        let Ok(running) = self.running.try_borrow() else { return };
        let Some(r) = &*running else { return };
        let rt = r.app.rt();
        let covered = web_math::web_wake_covered(!self.shared.is_visible(), r.suspended);
        let (flow, waiting) = match rt.wake_deadline() {
            Some(deadline) if !covered && deadline <= rt.wall_start.unwrap().elapsed().as_secs_f64() => {
                if let Some(window) = &self.window { window.request_redraw(); }
                (ControlFlow::Wait, None)
            }
            Some(deadline) => {
                let target = if covered { web_math::web_hidden_timer_target(deadline, rt.polled_at()) } else { deadline };
                (ControlFlow::WaitUntil(crate::time::for_winit(rt.wall_start.unwrap() + Duration::from_secs_f64(target.max(0.0)))), Some(target.to_bits()))
            }
            None => (ControlFlow::Wait, None),
        };
        drop(running);
        if self.waiting != waiting || waiting.is_none() && !matches!(event_loop.control_flow(), ControlFlow::Wait) {
            self.waiting = waiting;
            event_loop.set_control_flow(flow);
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        if self.terminal_failed { return; }
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. }) {
            let covered = self.running.borrow().as_ref().is_some_and(|r| {
                web_math::web_wake_covered(!self.shared.is_visible(), r.suspended)
            });
            if covered {
                self.covered_wake(event_loop);
            } else if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.shared.quitting.store(true, Ordering::Release);
        self.webgl_watch = None;
        let running = self.running.borrow_mut().take();
        if let Some(r) = running { r.close(); }
        self.warmed.borrow_mut().take();
        if let Some(Ok(ready)) = self.pending.borrow_mut().take() {
            drop(ready.renderer);
            ready.gpu.device.destroy();
        }
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
        window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), delay.as_millis().min(i32::MAX as u128) as i32).ok();
    }
}

/// Yields one browser macrotask (`setTimeout(0)`), letting the browser paint and handle input
/// before the next synchronous step runs.
async fn yield_turn() {
    sleep_turn(Duration::ZERO).await;
}

/// Waits `delay` on the browser's timer queue before resuming the async task.
async fn sleep_turn(delay: Duration) {
    let ms = delay.as_millis().min(i32::MAX as u128) as i32;
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let Some(window) = web_sys::window() else {
            resolve.call0(&wasm_bindgen::JsValue::UNDEFINED).ok();
            return;
        };
        let done = resolve.clone();
        let callback = wasm_bindgen::closure::Closure::once_into_js(move || {
            done.call0(&wasm_bindgen::JsValue::UNDEFINED).ok();
        });
        if window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), ms).is_err() {
            resolve.call0(&wasm_bindgen::JsValue::UNDEFINED).ok();
        }
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.ok();
}
