//! Per-frame redraw loop, document band sizing, canvas surface resize, and frame requests.

use std::time::Duration;
use winit::{event_loop::ActiveEventLoop, platform::web::WindowExtWebSys, window::Window};
use crate::time::Instant;
use super::{Browser, Running, Wake, after, error, full_faces, gpu::{self, Draw, FINISH_LIMIT}, handover::QUIET, large_viewport, page_fonts, pipelines, web_math};

/// The overscan a page's canvas carries above and below the window, in CSS points: the distance
/// the browser's compositor can scroll before Rune has to draw a new band.
const SKIRT: f32 = 256.0;

/// The largest texture side a canvas may have before a GPU says: WebGPU's default limit.
pub(super) const CPU_MAX_DIM: u32 = 8192;

impl Running {
    fn max_dim(&self) -> u32 {
        self.draw.gpu().map_or(CPU_MAX_DIM, |side| side.gpu.device.limits().max_texture_dimension_2d.max(1))
    }

    pub(super) fn resize_surface(&mut self, window: &Window, size: winit::dpi::PhysicalSize<u32>, scale: f32) {
        let max_dim = self.max_dim();
        if self.document {
            // The canvas covers a band of the page: size it before the box is measured.
            let (view, band) = band_css(window, scale, max_dim);
            set_band_height(window, band);
            self.app.rt_mut().set_band(band, view);
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
        self.app.rt_mut().set_viewport((next.0 as f32 / scale, next.1 as f32 / scale), scale);
    }

    /// Puts a page's canvas where the band it just drew starts in the document. The browser's
    /// compositor scrolls those pixels itself, at its own rate, until the window nears the band's
    /// edge and the next frame draws a new one.
    fn sync_band(&mut self, window: &Window) {
        if !self.document { return; }
        let top = self.app.rt().page_offset();
        if self.band == Some(top) { return; }
        self.band = Some(top);
        if let Some(canvas) = window.canvas() {
            let _ = canvas.style().set_property("transform", &format!("translate3d(0,{top}px,0)"));
        }
    }

    pub(super) fn apply_frame_requests(&mut self, window: &Window) {
        let rt = self.app.rt_mut();
        if !self.embedded
            && let Some(title) = rt.host.window_title.take_change()
            && let Some(doc) = web_sys::window().and_then(|w| w.document())
        {
            doc.set_title(title);
        }
        // A page's height is the mirror's, which `sync_input` keeps; the canvas stays the band.
        if self.document {
            rt.host.window_frame = None;
        } else if let Some(req) = rt.host.window_frame.take()
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
pub(super) fn band_css(window: &Window, scale: f32, max_dim: u32) -> (f32, f32) {
    let scale = scale.max(1e-3);
    let reported = large_viewport();
    let view = if reported > 0.0 { reported } else { window.inner_size().height as f32 / scale };
    let max = max_dim as f32 / scale;
    let band = (view + 2.0 * SKIRT).min(max.max(view));
    (view, band)
}

/// The canvas's CSS height is its band, in points.
pub(super) fn set_band_height(window: &Window, band: f32) {
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

impl Browser {
    /// Draws a frame: the runtime steps, then the CPU paints it, the GPU draws it, or both while
    /// handing over.
    pub(super) fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.lost_device(event_loop) { return; }
        self.maybe_restore_webgl();
        self.hand_over_if_done();
        let mut running = self.running.borrow_mut();
        let (Some(window), Some(r)) = (&self.window, &mut *running) else { return };
        if r.suspended || r.draw.waiting_for_gpu() || matches!(r.draw, Draw::ContextLost) { return; }
        let now = Instant::now();
        let (commits, input) = {
            let rt = r.app.rt();
            (rt.stats.commits, rt.has_pending_input())
        };
        let tick = !std::mem::take(&mut r.full_redraw) && matches!(&r.draw, Draw::Gpu(side) if !side.renderer.checking_gpu())
            && !r.app.compositor_animating() && !r.app.compositor_draws_own() && r.app.rt_mut().paint_cycle_tick();
        let (animating, dt) = if tick {
            let dt = now.saturating_duration_since(r.clock.last()).as_secs_f64();
            r.clock.hold(now);
            (false, dt)
        } else {
            let compositing = r.app.compositor_animating();
            r.app.clock_advance(&mut r.clock, now, compositing)
        };
        r.apply_frame_requests(window);
        r.clipboard.flush_core(r.app.rt_mut());
        // A menu asked for opens as the page's own elements (bridge.js `showMenu`), once the frame
        // that asked has laid out what it anchors to.
        if !r.app.rt().popup_waits_for_layout() {
            match r.app.rt_mut().take_popup() {
                Some(Some(show)) => {
                    let rt = r.app.rt();
                    r.input.show_menu(&show, rt.backdrop_dark().unwrap_or(rt.env.system_dark));
                }
                Some(None) => { let _ = r.app.resolve_popup(None); }
                None => {}
            }
        }
        r.app.rt_mut().refuse_file_dialog("native file dialogs are unsupported on web");
        if !tick { r.sync_input(false); }
        {
            let rt = r.app.rt();
            r.cursor.apply(window, rt.cursor(), rt.pointer().is_some());
            if !rt.surfaces.is_empty() {
                error("native video surfaces are unsupported on web", window.canvas().as_ref(), r.embedded, r.document);
                drop(running);
                self.end(event_loop);
                return;
            }
        }
        let size = r.size;
        let drawn = match &mut r.draw {
            // A frame only the GPU draws (`Scene::needs_gpu`) leaves the last one up.
            Draw::Cpu { cpu, .. } => Ok(cpu.paint(r.app.rt_mut(), size).is_ok()),
            Draw::HandingOver { cpu, side, built, done, since } => {
                let painted = cpu.paint(r.app.rt_mut(), size).is_ok();
                // Both renderers draw this frame once the browser has compiled the GPU's
                // pipelines; the GPU's canvas stays see-through until its frame is done.
                if built.get() {
                    match gpu::gpu_frame(side, &mut *r.app, now, dt, gpu::drift_wanted(), false) {
                        // That frame made pipelines its draw needed and drew a fallback for each
                        // while the browser checks it (the renderer hears the answers as it
                        // draws): frames go on drawing into the see-through canvas, and the
                        // first one drawn with every check answered is the hand-over frame.
                        Ok(_) if side.renderer.checking_gpu() => crate::marks::mark("web.handover.warming"),
                        Ok(copy) => {
                            if let Some(copy) = copy { copy.count(cpu.pixels().to_vec()); }
                            *done = Some(gpu::when_done(side, &self.proxy));
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
                gpu::gpu_frame(side, &mut *r.app, now, dt, false, tick).map(|_| true)
            }
            Draw::ContextLost | Draw::Moving => Ok(false),
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
        let ambient = {
            let rt = r.app.rt();
            !input && !animating && rt.stats.commits == commits && rt.ambient.is_some()
        };
        if drawn && !ambient { self.drawn_at = Some(now); }
        if drawn && !r.presented {
            r.presented = true;
            self.first_at = Some(now);
            let proxy = self.proxy.clone();
            // A browser that compiles pipelines asynchronously is asked in the task after this
            // frame: nothing waits for quiet there (`maybe_start_gpu`).
            let first_look = if pipelines::browser_compiles_async() { Duration::ZERO } else { QUIET };
            after(first_look, move || { let _ = proxy.send_event(Wake::StartGpu); });
            crate::marks::mark("web.first.presented");
            r.input.presented();
            // In a task of its own: a click or key that came with the first frame is handled
            // first, not after what the app starts once it shows (about 9 ms in OpenCode's).
            if let Some(mut ready) = r.on_ready.take() { after(Duration::ZERO, move || ready(&crate::WindowHandle::new("main"))); }
            page_fonts::share_later(r.fonts);
            let proxy = self.proxy.clone();
            full_faces::fetch(std::mem::take(&mut r.late_fonts), !r.document, move || { let _ = proxy.send_event(Wake::App); });
        }
        if r.app.rt_mut().shell_request.take().is_some() {
            drop(running);
            self.end(event_loop);
            return;
        }
        // A frame that drew a fallback while the browser checks a pipeline is drawn again once it
        // answers, and so is a hand-over waiting on that check.
        let checking = r.draw.gpu().is_some_and(|side| side.renderer.checking_gpu());
        let handing = matches!(r.draw, Draw::HandingOver { done: None, .. });
        let wants_frame = {
            let rt = r.app.rt();
            animating || rt.has_pending_input() || rt.live_pending() || r.app.compositor_animating() || checking
        };
        if wants_frame {
            window.request_redraw();
        }
        let presented = r.presented;
        drop(running);
        // A GPU that came before the first frame is taken up after it.
        if presented && !handing { self.gpu_came(); }
    }
}
