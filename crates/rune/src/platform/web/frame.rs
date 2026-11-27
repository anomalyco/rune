//! Per-frame redraw loop, document band sizing, canvas surface resize, and frame requests.

use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};
use wasm_bindgen::JsCast;
use winit::{event_loop::{ActiveEventLoop, EventLoopProxy}, platform::web::WindowExtWebSys, window::Window};
use crate::frame::clock::FrameClock;
use crate::frame::host_step::{self, Acquire, FrameHost, Gate, PostStepCheck, ReacquireReason, RenderPrep, StepInfo};
use crate::frame::stage::{FrameStage, StageGuard};
use crate::runtime::RuntimeCore;
use crate::time::Instant;
use crate::window::Missed;
use super::{Browser, Running, Start, Wake, after, error, full_faces, gpu::{self, Draw}, large_viewport, page_fonts, web_math};

/// The overscan a page's canvas carries above and below the window, in CSS points: the distance
/// the browser's compositor can scroll before Rune has to draw a new band.
const SKIRT: f32 = 256.0;

/// The largest texture side a canvas may have before a GPU says: WebGPU's default limit.
pub(super) const CPU_MAX_DIM: u32 = 8192;

/// The frame a resize owes, drawn in a microtask (`web_math::step_resize_redraw`).
pub(super) struct ResizeRedraw {
    proxy: EventLoopProxy<Wake>,
    state: Rc<Cell<web_math::ResizeRedraw>>,
}

impl ResizeRedraw {
    pub(super) fn new(proxy: EventLoopProxy<Wake>) -> Self {
        Self { proxy, state: Rc::default() }
    }

    fn step(&self, event: web_math::ResizeRedrawEvent) -> Option<web_math::ResizeRedrawEffect> {
        let mut state = self.state.get();
        let effect = web_math::step_resize_redraw(&mut state, event).into_iter().next();
        self.state.set(state);
        effect
    }

    pub(super) fn resized(&self) {
        if self.step(web_math::ResizeRedrawEvent::Resized) != Some(web_math::ResizeRedrawEffect::QueueMicrotask) { return; }
        let proxy = self.proxy.clone();
        let callback = wasm_bindgen::closure::Closure::once_into_js(move || { proxy.send_event(Wake::ResizeRedraw).ok(); });
        match web_sys::window() {
            Some(window) => window.queue_microtask(callback.unchecked_ref()),
            None => { self.step(web_math::ResizeRedrawEvent::Drawing); }
        }
    }

    /// The microtask ran: whether the frame is still owed.
    pub(super) fn fired(&self) -> bool {
        self.step(web_math::ResizeRedrawEvent::Fired) == Some(web_math::ResizeRedrawEffect::Draw)
    }
}

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
        if self.suspended {
            self.last_presented = None;
            if let Some(side) = self.draw.gpu_mut() { side.damage.reset(); }
            return;
        }
        let next = (size.width.min(max_dim), size.height.min(max_dim));
        let document = self.document;
        if let Some(side) = self.draw.gpu_mut() {
            let wake_blocked = gpu::on_resized(side);
            let (kept, reconfigure) = web_math::should_reconfigure_surface((side.config.width, side.config.height), next, document);
            if reconfigure || wake_blocked {
                side.config.width = kept.0;
                side.config.height = kept.1;
                gpu::configure_surface(side);
                self.last_presented = None;
            }
            self.size = (side.config.width, side.config.height);
        } else {
            if self.size != next { self.last_presented = None; }
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
            canvas.style().set_property("transform", &format!("translate3d(0,{top}px,0)")).ok();
        }
    }

    pub(super) fn sync_web_images(&mut self) {
        self.web_images.sync(&self.input.handle, self.app.rt().paint.web_images());
    }

    pub(super) fn apply_frame_requests(&mut self, window: &Window) {
        let rt = self.app.rt_mut();
        super::file_dialog::flush(rt, &self.input.handle);
        crate::haptics::flush_live(rt, false);
        let _ = rt.host.window_color_scheme.take();
        let _ = rt.host.window_backdrop_request.take();
        let _ = rt.host.represented_file.take_change();
        let _ = rt.host.document_edited.take_change();
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
            canvas.style().set_property("height", &format!("{h}px")).ok();
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
            canvas.style().set_property("height", &height).ok();
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

enum WebDrawable {
    Cpu,
    Gpu(wgpu::SurfaceTexture),
}

enum PreparedWeb {
    Cpu,
    HandingOver { gpu: Option<gpu::PreparedGpu>, flipping: bool },
    Gpu(gpu::PreparedGpu),
}

struct WebFrameHost<'a> {
    r: &'a mut Running,
    window: &'a Arc<Window>,
    proxy: &'a EventLoopProxy<Wake>,
    first_presented_at: Option<Instant>,
    non_ambient_drawn_at: Option<Instant>,
    now: Option<Instant>,
    commits: u64,
    had_input: bool,
    tick: bool,
    reblend_tried: Option<bool>,
    drawn: bool,
    handover_completed: bool,
    retry_handover: bool,
    cpu_needs_gpu_at: Option<Instant>,
    fatal_error: Option<String>,
    should_end: bool,
    try_gpu_came: bool,
}

impl FrameHost for WebFrameHost<'_> {
    type Drawable = WebDrawable;
    type Prepared = PreparedWeb;

    fn gate(&mut self) -> Gate {
        let quiet_candidate = match &self.r.draw {
            Draw::Gpu(side) => {
                self.r.last_presented.is_some()
                    && !side.renderer.checking_gpu()
                    && !self.r.app.compositor_draws_own()
                    && side.renderer.srgb_blend() == self.r.app.rt().srgb_blend()
            }
            _ => false,
        };
        web_math::web_frame_gate(self.r.suspended, self.r.draw.kind(), quiet_candidate)
    }

    fn needs_reblend(&self) -> bool {
        matches!(&self.r.draw, Draw::Gpu(side) if side.renderer.srgb_blend() != self.r.app.rt().srgb_blend())
    }

    fn reblend(&mut self) -> bool {
        let target = self.r.app.rt().srgb_blend();
        self.reblend_tried = Some(target);
        if let Draw::Gpu(side) = &mut self.r.draw {
            gpu::reblend(side, self.r.app.rt_mut());
            self.r.last_presented = None;
        }
        true
    }

    fn step_quiet(&mut self) -> StepInfo {
        self.step_runtime()
    }

    fn is_unchanged(&mut self, info: &StepInfo) -> bool {
        let Draw::Gpu(side) = &self.r.draw else { return false };
        !self.tick
            && !side.renderer.checking_gpu()
            && !self.r.app.compositor_draws_own()
            && !self.r.app.compositor_animating()
            && side.renderer.srgb_blend() == self.r.app.rt().srgb_blend()
            && self.r.last_presented.as_ref().is_some_and(|p| {
                p.unchanged_core(self.r.app.rt(), &side.config, info.animating, None)
            })
    }

    fn on_unchanged(&mut self, info: &StepInfo) {
        let mut info = *info;
        self.sync_after_step(&mut info);
        self.r.sync_band(self.window);
        self.r.sync_web_images();
        if self.r.app.rt_mut().shell_request.take().is_some() {
            self.should_end = true;
            return;
        }
        self.try_gpu_came = self.r.presented;
    }

    fn acquire(&mut self) -> Acquire<WebDrawable> {
        let _acquire = StageGuard::enter(FrameStage::Acquire);
        match &mut self.r.draw {
            Draw::Gpu(side) => match gpu::acquire_texture(side) {
                Ok(frame) => Acquire::Ready(WebDrawable::Gpu(frame)),
                Err(missed) => Acquire::Missed(missed),
            },
            Draw::Cpu { .. } | Draw::HandingOver { .. } => Acquire::Ready(WebDrawable::Cpu),
            Draw::ContextLost | Draw::TerminalFailed | Draw::Moving => Acquire::Missed(Missed::Unconfigured),
        }
    }

    fn step_runtime(&mut self) -> StepInfo {
        let _runtime = StageGuard::enter(FrameStage::Runtime);
        let now = Instant::now();
        self.now = Some(now);
        let (commits, had_input) = {
            let rt = self.r.app.rt();
            (rt.stats.commits, rt.has_pending_input())
        };
        self.commits = commits;
        self.had_input = had_input;
        let tick = !std::mem::take(&mut self.r.full_redraw)
            && matches!(&self.r.draw, Draw::Gpu(side) if !side.renderer.checking_gpu())
            && !self.r.app.compositor_animating()
            && !self.r.app.compositor_draws_own()
            && self.r.app.rt_mut().paint_cycle_tick();
        self.tick = tick;
        let (animating, dt) = if tick {
            let dt = now.saturating_duration_since(self.r.clock.last()).as_secs_f64();
            self.r.clock.hold(now);
            (false, dt)
        } else {
            let compositing = self.r.app.compositor_animating();
            self.r.app.clock_advance(&mut self.r.clock, now, compositing)
        };
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: None }
    }

    fn post_step_check(&mut self, _info: &mut StepInfo) -> PostStepCheck {
        if let Draw::Gpu(side) = &self.r.draw
            && web_math::should_post_step_reblend(
                side.renderer.srgb_blend(),
                self.r.app.rt().srgb_blend(),
                self.reblend_tried,
            )
        {
            PostStepCheck::Reacquire(ReacquireReason::Reblend)
        } else {
            PostStepCheck::Keep
        }
    }

    fn prepare_reacquire(&mut self, reason: ReacquireReason, _info: &mut StepInfo) -> bool {
        match reason {
            ReacquireReason::Reblend => self.reblend(),
        }
    }

    fn on_missed(&mut self, missed: Missed, stepped: bool) {
        self.r.last_presented = None;
        match web_math::web_miss_action(missed) {
            web_math::WebMissAction::Fatal => {
                self.fatal_error = Some("WebGPU canvas acquisition failed".into());
            }
            web_math::WebMissAction::CoveredOnly => {
                if !stepped {
                    self.r.app.covered_wake();
                    self.r.clipboard.flush_core(self.r.app.rt_mut());
                }
                crate::haptics::flush_live(self.r.app.rt_mut(), false);
            }
            web_math::WebMissAction::RetryNextFrame => {
                if !stepped {
                    self.r.app.covered_wake();
                    self.r.clipboard.flush_core(self.r.app.rt_mut());
                }
                crate::haptics::flush_live(self.r.app.rt_mut(), false);
                self.window.request_redraw();
            }
        }
    }

    fn sync_after_step(&mut self, info: &mut StepInfo) {
        let now = self.now.unwrap_or_else(Instant::now);
        info.update_ms = now.elapsed().as_secs_f64() * 1000.0;
        self.r.apply_frame_requests(self.window);
        self.r.clipboard.flush_core(self.r.app.rt_mut());
        // A menu asked for opens as the page's own elements (bridge.js `showMenu`), once the frame
        // that asked has laid out what it anchors to.
        if !self.r.app.rt().popup_waits_for_layout() {
            match self.r.app.rt_mut().take_popup() {
                Some(Some(show)) => {
                    let rt = self.r.app.rt();
                    self.r.input.show_menu(&show, rt.backdrop_dark().unwrap_or(rt.env.system_dark));
                }
                Some(None) => { self.r.app.resolve_popup(None).ok(); }
                None => {}
            }
        }
        if !self.tick { self.r.sync_input(false); }
        let rt = self.r.app.rt();
        self.r.cursor.apply(self.window, rt.cursor(), rt.pointer().is_some());
        if !rt.surfaces.is_empty() {
            self.fatal_error = Some("native video surfaces are unsupported on web".into());
        }
    }

    fn prepare_render(&mut self, drawable: WebDrawable, _info: &mut StepInfo) -> RenderPrep<PreparedWeb> {
        let _upload = StageGuard::enter(FrameStage::Upload);
        if self.fatal_error.is_some() {
            return RenderPrep::Abort;
        }
        match (&mut self.r.draw, drawable) {
            (Draw::Cpu { .. }, WebDrawable::Cpu) => RenderPrep::Ready(PreparedWeb::Cpu),
            (Draw::HandingOver { side, seq, state, .. }, WebDrawable::Cpu) => {
                use super::web_math::handover_machine::{HandoverEffect, HandoverEvent};
                let _ = side.renderer.poll_breaker_checks(&side.gpu.device);
                let rt = self.r.app.rt_mut();
                gpu::reblend(side, rt);
                let mut draw_gpu = false;
                let mut flipping = false;
                let prep_effects = state.step(seq, HandoverEvent::PrepareFrame {
                    checking_gpu: side.renderer.checking_gpu(),
                });
                for effect in prep_effects {
                    if effect == HandoverEffect::PrecheckFeatures {
                        side.renderer.precheck_layer_features(&side.gpu.device, &rt.instances);
                        for checked in state.step(seq, HandoverEvent::Prechecked {
                            checking_gpu: side.renderer.checking_gpu(),
                        }) {
                            match checked {
                                HandoverEffect::DrawGpuFrame => { draw_gpu = true; }
                                HandoverEffect::DrawFlipFrame => { draw_gpu = true; flipping = true; }
                                _ => {}
                            }
                        }
                    }
                }
                let compositor_draws_own = self.r.app.compositor_draws_own();
                let rt = self.r.app.rt_mut();
                let gpu = if draw_gpu {
                    match gpu::acquire_texture(side) {
                        Ok(frame) => {
                            let readback = !flipping && gpu::drift_wanted();
                            Some(gpu::prepare_gpu_frame(side, rt, frame, readback, false, compositor_draws_own))
                        }
                        Err(missed) => {
                            web_sys::console::warn_1(
                                &format!("rune: the GPU's hand-over frame wasn't drawn ({missed:?})").into(),
                            );
                            if web_math::web_miss_action(missed) == web_math::WebMissAction::RetryNextFrame {
                                self.retry_handover = true;
                            }
                            None
                        }
                    }
                } else {
                    None
                };
                RenderPrep::Ready(PreparedWeb::HandingOver { gpu, flipping })
            }
            (Draw::Gpu(side), WebDrawable::Gpu(frame)) => {
                let compositor_draws_own = self.r.app.compositor_draws_own();
                let prepared = gpu::prepare_gpu_frame(side, self.r.app.rt_mut(), frame, false, self.tick, compositor_draws_own);
                RenderPrep::Ready(PreparedWeb::Gpu(prepared))
            }
            _ => RenderPrep::Abort,
        }
    }

    fn render(&mut self, prepared: &mut PreparedWeb, info: &StepInfo) -> bool {
        let _encode = StageGuard::enter(FrameStage::Encode);
        let size = self.r.size;
        match (&mut self.r.draw, prepared) {
            // Degradable GPU-only features paint live via `Scene::degraded(true)` inside
            // `CpuCanvas::paint`; non-degradable `NeedsGpu` leaves the last frame up and escalates
            // GPU startup so frame 1 cannot deadlock blank.
            (Draw::Cpu { cpu, .. }, PreparedWeb::Cpu) => {
                match cpu.paint(self.r.app.rt_mut(), size) {
                    Ok(_) => {
                        self.drawn = true;
                        self.r.cpu_blocked_on_gpu = false;
                    }
                    Err(_) => {
                        self.drawn = false;
                        self.r.cpu_blocked_on_gpu = true;
                        self.cpu_needs_gpu_at = Some(self.now.unwrap_or_else(Instant::now));
                    }
                }
                false
            }
            (Draw::HandingOver { cpu, side, .. }, PreparedWeb::HandingOver { gpu, flipping }) => {
                // Keep painting the visible CPU canvas throughout Building, WaitingChecks, and
                // WaitingDone so animations never freeze. On a clean ReadyToFlip frame, skip the
                // redundant CPU paint unless GPU acquisition missed or encoding triggers a new check.
                if !*flipping || gpu.is_none() {
                    self.drawn = cpu.paint(self.r.app.rt_mut(), size).is_ok();
                    if self.drawn {
                        self.r.cpu_blocked_on_gpu = false;
                    }
                }
                let comp = match gpu.as_mut() {
                    Some(prepared_gpu) => {
                        gpu::encode_gpu_frame(side, &mut *self.r.app, prepared_gpu, info.dt, info.update_ms)
                    }
                    None => false,
                };
                if *flipping && gpu.is_some() && side.renderer.checking_gpu() {
                    self.drawn = cpu.paint(self.r.app.rt_mut(), size).is_ok();
                    if self.drawn {
                        self.r.cpu_blocked_on_gpu = false;
                    }
                }
                comp
            }
            (Draw::Gpu(side), PreparedWeb::Gpu(prepared_gpu)) => {
                gpu::encode_gpu_frame(side, &mut *self.r.app, prepared_gpu, info.dt, info.update_ms)
            }
            _ => false,
        }
    }

    fn submit_and_present(&mut self, prepared: PreparedWeb, _info: &StepInfo) -> Instant {
        let _submit = StageGuard::enter(FrameStage::Submit);
        let mut complete_handover = false;
        match (&mut self.r.draw, prepared) {
            (Draw::Cpu { .. }, PreparedWeb::Cpu) => {}
            (Draw::HandingOver { cpu, side, seq, state, barrier }, PreparedWeb::HandingOver { gpu, flipping }) => {
                use super::web_math::handover_machine::{HandoverEffect, HandoverEvent};
                if let Some(prepared_gpu) = gpu {
                    let window = self.window;
                    let mut copy = if flipping {
                        gpu::submit_gpu_frame(side, prepared_gpu, || window.pre_present_notify())
                    } else {
                        gpu::submit_gpu_frame(side, prepared_gpu, || {})
                    };
                    let checking_gpu = side.renderer.checking_gpu();
                    let effects = state.step(seq, HandoverEvent::GpuSubmitted {
                        checking_gpu,
                        now: Instant::now(),
                    });
                    if checking_gpu {
                        crate::marks::mark("web.handover.warming");
                    }
                    for effect in effects {
                        match effect {
                            HandoverEffect::ArmFinishWait { attempt, delay } => {
                                if web_math::cpu_canvas::should_count_handover_drift(self.drawn, cpu.last_degraded())
                                    && let Some(copy) = copy.take()
                                {
                                    copy.count(cpu.pixels().to_vec());
                                }
                                *barrier = Some(gpu::when_done(side, self.proxy, Wake::HandoverDone(attempt)));
                                let proxy = self.proxy.clone();
                                after(delay, move || { proxy.send_event(Wake::HandoverTimeout(attempt)).ok(); });
                                crate::marks::mark("web.handover.drawn");
                            }
                            HandoverEffect::Complete => { complete_handover = true; }
                            _ => {}
                        }
                    }
                }
            }
            (Draw::Gpu(side), PreparedWeb::Gpu(prepared_gpu)) => {
                let window = self.window;
                let _ = gpu::submit_gpu_frame(side, prepared_gpu, || window.pre_present_notify());
                self.drawn = true;
            }
            _ => {}
        }
        if complete_handover
            && let Draw::HandingOver { cpu, side, .. } = std::mem::replace(&mut self.r.draw, Draw::Moving)
        {
            // Flip canvas visibility atomically in the same rAF turn that presented the
            // fresh GPU frame so active animations continue with zero freeze or jump.
            if let Some(c) = self.window.canvas() { c.style().remove_property("opacity").ok(); }
            cpu.remove();
            self.r.draw = Draw::Gpu(side);
            self.r.cpu_blocked_on_gpu = false;
            self.r.last_presented = None;
            self.drawn = true;
            self.handover_completed = true;
            crate::marks::mark("web.handover.done");
        }
        Instant::now()
    }

    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        (Some(&mut self.r.clock), self.r.app.rt_mut())
    }

    fn settle(&mut self, submitted: Instant, info: &StepInfo) {
        let now = self.now.unwrap_or(submitted);
        // In the same commit as the frame: the canvas moves to where this frame drew it.
        self.r.sync_band(self.window);
        self.r.sync_web_images();
        // Only an ambient loop's step (a frame its wake asked for, no commit, nothing else moving).
        let ambient = {
            let rt = self.r.app.rt();
            !self.had_input && !info.runtime_animating && rt.stats.commits == self.commits && rt.ambient.is_some()
        };
        if self.drawn && !ambient { self.non_ambient_drawn_at = Some(now); }
        if self.drawn && !self.r.presented {
            self.r.presented = true;
            self.first_presented_at = Some(now);
            crate::marks::mark("web.first.presented");
            self.r.input.presented();
            // In a task of its own: a click or key that came with the first frame is handled
            // first, not after what the app starts once it shows (about 9 ms in OpenCode's).
            if let Some(mut ready) = self.r.on_ready.take() { after(Duration::ZERO, move || ready(&crate::WindowHandle::new("main"))); }
            page_fonts::share_later(self.r.fonts);
            let proxy = self.proxy.clone();
            full_faces::fetch(std::mem::take(&mut self.r.late_fonts), !self.r.document, move || { proxy.send_event(Wake::App).ok(); });
        }
        if self.drawn {
            self.r.app.inspect_frame();
        }
        if self.r.app.rt_mut().shell_request.take().is_some() {
            self.should_end = true;
            return;
        }
        // Async pipeline checks wake the loop directly via `Wake::GpuCheck` when `popErrorScope()`
        // resolves, so `checking` suppresses `last_presented` caching without spinning rAF.
        let checking = self.r.draw.gpu().is_some_and(|side| side.renderer.checking_gpu());
        let handing = matches!(self.r.draw, Draw::HandingOver { .. });
        let comp_animating = self.r.app.compositor_animating();
        if let Draw::Gpu(side) = &self.r.draw {
            let rt = self.r.app.rt();
            let moving = info.animating || comp_animating || checking;
            if web_math::should_record_presented(self.drawn, moving, rt.stale(), rt.host.window_frame.is_some()) {
                crate::window::Presented::record_core(
                    &mut self.r.last_presented,
                    rt,
                    &side.config,
                    self.r.app.compositor_draws_own(),
                );
            } else {
                self.r.last_presented = None;
            }
        } else {
            self.r.last_presented = None;
        }
        let wants_frame = {
            let rt = self.r.app.rt();
            web_math::handover_machine::web_wants_frame(
                info.animating,
                rt.has_pending_input(),
                rt.live_pending(),
                comp_animating,
            )
        };
        if wants_frame || self.retry_handover {
            self.window.request_redraw();
        }
        self.try_gpu_came = (self.r.presented || self.r.cpu_blocked_on_gpu) && !handing;
    }
}

impl Browser {
    /// Draws a frame through the shared [`host_step::run`] frame sequencer: the runtime steps,
    /// then the CPU paints it, the GPU draws it, or both while handing over.
    pub(super) fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        self.resize_redraw.step(web_math::ResizeRedrawEvent::Drawing);
        if self.lost_device(event_loop) { return; }
        if self.terminal_failed { return; }
        self.maybe_restore_webgl();
        self.poll_handover_timeout();
        let (fatal_error, should_end, try_gpu_came, handover_completed, first_presented_at, non_ambient_drawn_at, cpu_needs_gpu_at, embedded, document) = {
            let mut running = self.running.borrow_mut();
            let (Some(window), Some(r)) = (&self.window, &mut *running) else { return };
            if r.draw.gpu().is_some_and(|side| gpu::drain_configure_retry(side, r.suspended)) {
                r.last_presented = None;
            }
            let seq = r.frame_seq;
            let _stage_scope = r.observability.scope_stages(Some(seq));
            let _redraw = StageGuard::enter(FrameStage::Redraw);
            let mut host = WebFrameHost {
                r,
                window,
                proxy: &self.proxy,
                first_presented_at: None,
                non_ambient_drawn_at: None,
                now: None,
                commits: 0,
                had_input: false,
                tick: false,
                reblend_tried: None,
                drawn: false,
                handover_completed: false,
                retry_handover: false,
                cpu_needs_gpu_at: None,
                fatal_error: None,
                should_end: false,
                try_gpu_came: false,
            };
            let outcome = host_step::run(&mut host);
            if !matches!(outcome, host_step::Outcome::Skipped) {
                host.r.frame_seq += 1;
            }
            (
                host.fatal_error,
                host.should_end,
                host.try_gpu_came,
                host.handover_completed,
                host.first_presented_at,
                host.non_ambient_drawn_at,
                host.cpu_needs_gpu_at,
                host.r.embedded,
                host.r.document,
            )
        };
        if let Some(msg) = fatal_error {
            if self.gpu_loss_policy == crate::GpuLossPolicy::Terminal {
                self.enter_terminal_gpu_loss(msg);
                return;
            }
            error(&msg, self.window.as_ref().and_then(|w| w.canvas()).as_ref(), embedded, document);
            self.end(event_loop);
            return;
        }
        if should_end {
            self.end(event_loop);
            return;
        }
        if handover_completed {
            self.arm_lose_gpu_harness();
        }
        if self.start == Start::CpuFirst {
            if let Some(at) = cpu_needs_gpu_at {
                self.on_cpu_needs_gpu(at);
            } else if let Some(at) = first_presented_at {
                self.on_first_cpu_frame(at);
            } else if let Some(at) = non_ambient_drawn_at {
                self.on_cpu_frame_drawn(at);
            }
        }
        // A GPU that came before the first frame is taken up after it.
        if try_gpu_came { self.gpu_came(); }
    }
}
