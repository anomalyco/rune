//! CPU-first GPU acquisition, hand-over from the CPU canvas to WebGPU/WebGL2, and device/context
//! loss and restore recovery.

use std::{sync::atomic::Ordering, time::Duration};
use winit::{event_loop::ActiveEventLoop, platform::web::WindowExtWebSys};
use crate::time::Instant;
use super::{
    Browser, Start, Wake, after, cpu_canvas, error,
    gpu::{self, Draw, FINISH_LIMIT, Loss},
    health, pipelines, web_math, yield_turn,
};

impl Browser {
    /// Installs `webglcontextlost` / `webglcontextrestored` listeners on the page's canvas once
    /// WebGL2 has attached to it. Lives on `Browser` so restore events still arrive after a lost
    /// `GpuSide` has been dropped.
    pub(super) fn ensure_webgl_watch(&mut self) {
        if self.webgl_watch.is_some() { return; }
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        let (lost, restored_lost, restored_ok, proxy_lost, proxy_restored) = (
            self.lost.clone(),
            self.webgl_restored.clone(),
            self.webgl_restored.clone(),
            self.proxy.clone(),
            self.proxy.clone(),
        );
        self.webgl_watch = Some(health::WebglWatch::new(
            &canvas,
            move |why| {
                restored_lost.set(false);
                *lost.borrow_mut() = Some(Loss::WebGl(why));
                proxy_lost.send_event(Wake::App).ok();
            },
            move || {
                restored_ok.set(true);
                proxy_restored.send_event(Wake::App).ok();
            },
        ));
    }

    /// The browser lost the GPU device or WebGL2 context. WebGPU loss keeps the existing policy
    /// (CPU fallback on CPU-first pages, fatal on GPU-first pages). WebGL2 context loss falls back
    /// to the CPU canvas on full-window pages or pauses inline/document pages without exiting, so
    /// a later `webglcontextrestored` brings WebGL2 back. True when the app ended.
    pub(super) fn lost_device(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let mut loss = self.lost.borrow_mut().take();
        if loss.is_none()
            && self.webgl_watch.as_ref().is_some_and(health::WebglWatch::is_lost)
            && self.running.borrow().as_ref().is_some_and(|r| r.draw.gpu().is_some_and(|s| s.backend == wgpu::Backend::Gl))
        {
            loss = Some(Loss::WebGl("WebGL2 context lost".into()));
        }
        let Some(loss) = loss else { return false };
        let Some(window) = self.window.clone() else { return false };
        let canvas = window.canvas();
        let why = loss.reason();
        match web_math::loss_action(loss.is_webgl(), self.start == Start::GpuFirst, self.embedded, self.document) {
            web_math::LossAction::Fatal => {
                error(&format!("The GPU device was lost ({why}). Reload the page to draw again."), canvas.as_ref(), self.embedded, self.document);
                self.end(event_loop);
                return true;
            }
            web_math::LossAction::PauseForRestore => {
                let mut running = self.running.borrow_mut();
                let Some(r) = &mut *running else { return false };
                let side = match std::mem::replace(&mut r.draw, Draw::Moving) {
                    Draw::Gpu(side) => { r.draw = Draw::ContextLost; r.last_presented = None; side }
                    other => { r.draw = other; return false; }
                };
                self.webgl_restoring = false;
                web_sys::console::warn_1(&format!("rune: the WebGL2 context was lost ({why}); waiting for restore").into());
                crate::marks::mark("web.gpu.lost");
                side.gpu.device.destroy();
                drop(side);
                r.app.release_compositor_gpu();
                return false;
            }
            web_math::LossAction::FallbackToCpu { .. } => {}
        }
        let mut running = self.running.borrow_mut();
        let Some(r) = &mut *running else { return false };
        let side = match std::mem::replace(&mut r.draw, Draw::Moving) {
            Draw::Gpu(side) => match canvas.as_ref().and_then(cpu_canvas::CpuCanvas::under) {
                Some(cpu) => { r.draw = Draw::Cpu { cpu, failed: true }; r.last_presented = None; side }
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
        self.webgl_restoring = false;
        web_sys::console::warn_1(&format!("rune: the GPU device was lost ({why}); the page draws on the CPU").into());
        crate::marks::mark("web.gpu.lost");
        side.gpu.device.destroy();
        drop(side);
        r.app.release_compositor_gpu();
        let atlas = &mut r.app.rt_mut().text.atlas;
        crate::render::cpu_boot::attach_text(atlas);
        atlas.refill_gpu_masks();
        if let Some(c) = &canvas { c.style().set_property("opacity", "0").ok(); }
        window.request_redraw();
        false
    }

    /// Handles an async browser WebGPU error-scope rejection of the primary uber shader or
    /// pipeline (`Renderer::poll_uber_check`). On a GPU-first page, records `window_error`, shows
    /// the error on the page and ends the app so it never stays blank; on a CPU-first page, stays
    /// on or falls back to the CPU 2D canvas. Returns true when the app ended.
    pub(super) fn failed_shader(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let why = {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else { return false };
            r.draw.gpu_mut().and_then(|side| side.renderer.poll_uber_check())
        };
        let Some(why) = why else { return false };
        let Some(window) = self.window.clone() else { return false };
        let canvas = window.canvas();
        if self.start == Start::GpuFirst || self.embedded || self.document {
            let message = format!("The GPU rejected Rune's shader ({why}).");
            self.shared.set_window_error(Some(message.clone()));
            error(&message, canvas.as_ref(), self.embedded, self.document);
            self.end(event_loop);
            return true;
        }
        let mut running = self.running.borrow_mut();
        let Some(r) = &mut *running else { return false };
        let side = match std::mem::replace(&mut r.draw, Draw::Moving) {
            Draw::HandingOver { cpu, side, .. } => {
                r.draw = Draw::Cpu { cpu, failed: true };
                r.last_presented = None;
                side
            }
            Draw::Gpu(side) => match canvas.as_ref().and_then(cpu_canvas::CpuCanvas::under) {
                Some(cpu) => {
                    r.draw = Draw::Cpu { cpu, failed: true };
                    r.last_presented = None;
                    side
                }
                None => {
                    r.draw = Draw::Gpu(side);
                    drop(running);
                    let message = format!("The GPU rejected Rune's shader ({why}).");
                    self.shared.set_window_error(Some(message.clone()));
                    error(&message, canvas.as_ref(), self.embedded, self.document);
                    self.end(event_loop);
                    return true;
                }
            },
            other => {
                r.draw = other;
                return false;
            }
        };
        web_sys::console::warn_1(&format!("rune: the GPU rejected the renderer shader ({why}); the page draws on the CPU").into());
        crate::marks::mark("web.gpu.failed");
        side.gpu.device.destroy();
        drop(side);
        let atlas = &mut r.app.rt_mut().text.atlas;
        crate::render::cpu_boot::attach_text(atlas);
        atlas.refill_gpu_masks();
        if let Some(c) = &canvas { c.style().set_property("opacity", "0").ok(); }
        window.request_redraw();
        false
    }

    /// Rebuilds the WebGL2 `GpuSide` after `webglcontextrestored` fires on the canvas.
    pub(super) fn maybe_restore_webgl(&mut self) {
        if !self.webgl_restored.get() || self.webgl_restoring { return; }
        if self.webgl_watch.as_ref().is_some_and(health::WebglWatch::is_lost) { return; }
        let (features, canvas_now, srgb_blend) = {
            let running = self.running.borrow();
            let Some(r) = &*running else { return };
            if !matches!(r.draw, Draw::Cpu { failed: true, .. } | Draw::ContextLost) {
                self.webgl_restored.set(false);
                return;
            }
            let srgb = r.app.rt().srgb_blend();
            (r.app.compositor_features(), Some((r.size, srgb)), srgb)
        };
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        self.webgl_restored.set(false);
        #[cfg(feature = "webgl")]
        {
            self.webgl_restoring = true;
            let (pending, proxy, shared) = (self.pending.clone(), self.proxy.clone(), self.shared.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let result = gpu::try_webgl2(&canvas, features, &shared.quitting).await;
                deliver_gpu(result, canvas_now, srgb_blend, pending, None, proxy, shared, false).await;
            });
        }
        #[cfg(not(feature = "webgl"))]
        let _ = (canvas, features, canvas_now, srgb_blend);
    }

    /// The GPU answered while the page draws on the CPU (initial CPU-first hand-over) or after
    /// `webglcontextrestored`: its side is made and either handed over from the CPU canvas or
    /// installed directly for inline/document pages waiting in `Draw::ContextLost`.
    pub(super) fn gpu_came(&mut self) {
        let watch_webgl = {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else { return };
            let can_take = r.presented && (
                (self.start == Start::CpuFirst && matches!(r.draw, Draw::Cpu { failed: false, .. }))
                || (self.webgl_restoring && matches!(r.draw, Draw::Cpu { .. } | Draw::ContextLost))
            );
            if !can_take { return; }
            let Some(made) = self.pending.borrow_mut().take() else { return };
            let restoring = std::mem::take(&mut self.webgl_restoring);
            if !restoring && self.start == Start::CpuFirst {
                let (seq, startup) = &mut self.gpu_startup;
                let _ = startup.step(seq, web_math::GpuStartupEvent::WebGpuSucceeded);
            }
            let side = made.and_then(|ready| {
                gpu::gpu_side(ready, r.size, r.app.rt_mut(), self.lost.clone(), self.proxy.clone())
            });
            // The renderer is made: the pipelines compiled ahead for it can go.
            self.warmed.borrow_mut().take();
            let side = match side {
                Ok(side) => side,
                Err(why) => {
                    web_sys::console::warn_1(&format!("rune: the page draws on the CPU ({why})").into());
                    crate::marks::mark("web.gpu.failed");
                    if let Draw::Cpu { failed, .. } = &mut r.draw { *failed = true; }
                    return;
                }
            };
            let is_gl = side.backend == wgpu::Backend::Gl;
            r.last_presented = None;
            if matches!(r.draw, Draw::ContextLost) {
                r.draw = Draw::Gpu(side);
                r.app.rt_mut().invalidate();
                crate::marks::mark("web.gpu.restored");
                if let Some(window) = &self.window { window.request_redraw(); }
            } else {
                // The browser compiles the renderer's pipelines in its GPU process, in order: a submit
                // made after them is answered once they're built.
                side.gpu.queue.submit([]);
                let built = gpu::when_done(&side, &self.proxy);
                let Draw::Cpu { mut cpu, .. } = std::mem::replace(&mut r.draw, Draw::Moving) else { unreachable!("checked above") };
                cpu.half = crate::render::cpu_boot::HalfRounding::of_vendor(side.gpu.adapter.get_info().vendor);
                r.draw = Draw::HandingOver { cpu, side, built, done: None, since: Instant::now() };
                if restoring { crate::marks::mark("web.gpu.restored"); }
                crate::marks::mark("web.gpu.ready");
            }
            is_gl
        };
        if watch_webgl { self.ensure_webgl_watch(); }
    }

    /// While handing over: once the GPU's frame is done, its canvas shows and the CPU's goes, in
    /// one turn, so the next composition has the GPU's frame where the CPU's was. A frame that
    /// doesn't finish in `FINISH_LIMIT` leaves the page on the CPU.
    pub(super) fn hand_over_if_done(&mut self) {
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
            drop(side);
            r.app.release_compositor_gpu();
            let atlas = &mut r.app.rt_mut().text.atlas;
            crate::render::cpu_boot::attach_text(atlas);
            atlas.refill_gpu_masks();
            r.draw = Draw::Cpu { cpu, failed: true };
            return;
        }
        if let Some(c) = &canvas { c.style().remove_property("opacity").ok(); }
        cpu.remove();
        // A harness checking the way back to the CPU (`globalThis.RUNE_LOSE_GPU_AFTER`, in ms)
        // has the device count as lost that long after the hand-over, as the browser reports it.
        if let Some(ms) = js_sys::Reflect::get(&js_sys::global(), &"RUNE_LOSE_GPU_AFTER".into()).ok().and_then(|v| v.as_f64()) {
            let (lost, proxy) = (self.lost.clone(), self.proxy.clone());
            after(Duration::from_millis(ms as u64), move || {
                *lost.borrow_mut() = Some(Loss::WebGpu("RUNE_LOSE_GPU_AFTER".into()));
                proxy.send_event(Wake::App).ok();
            });
        }
        r.draw = Draw::Gpu(side);
        r.last_presented = None;
        crate::marks::mark("web.handover.done");
        if let Some(window) = &self.window { window.request_redraw(); }
    }

    fn requested_features(&self) -> wgpu::Features {
        match &self.app {
            Some(app) => app.compositor_features,
            None => self.running.borrow().as_ref().map_or(wgpu::Features::empty(), |r| r.app.compositor_features()),
        }
    }

    /// Asks the browser for the GPU immediately (`Start::GpuFirst`), once.
    pub(super) fn ask_gpu(&mut self) {
        if self.gpu_startup.1 != web_math::GpuStartup::Idle || self.start != Start::GpuFirst { return; }
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        self.gpu_startup.1 = web_math::GpuStartup::Done;
        crate::marks::mark("web.gpu.asked");
        let features = self.requested_features();
        let canvas_now = self.running.borrow().as_ref().map(|r| (r.size, r.app.rt().srgb_blend()));
        let srgb_blend = canvas_now
            .map(|(_, s)| s)
            .or_else(|| self.app.as_ref().map(|a| a.srgb_blend))
            .unwrap_or(true);
        let (pending, warmed, proxy, shared) = (self.pending.clone(), self.warmed.clone(), self.proxy.clone(), self.shared.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let result = gpu::gpu(canvas, features, &shared.quitting).await;
            deliver_gpu(result, canvas_now, srgb_blend, pending, Some(warmed), proxy, shared, true).await;
        });
    }

    /// First CPU frame presented on a `CpuFirst` page: arms the GPU startup state machine.
    pub(super) fn on_first_cpu_frame(&mut self, at: Instant) {
        let effects = {
            let (seq, startup) = &mut self.gpu_startup;
            startup.step(seq, web_math::GpuStartupEvent::FirstPresented {
                at,
                has_navigator_gpu: health::has_navigator_gpu(),
                compiles_async: pipelines::browser_compiles_async(),
                webgl_enabled: cfg!(feature = "webgl"),
            })
        };
        self.run_gpu_startup_effects(effects);
    }

    /// A non-ambient CPU frame drew: resets the quiet window if synchronous GPU setup is waiting.
    pub(super) fn on_cpu_frame_drawn(&mut self, at: Instant) {
        let (seq, startup) = &mut self.gpu_startup;
        let _ = startup.step(seq, web_math::GpuStartupEvent::FrameDrawn { at });
    }

    /// A scheduled CPU-first GPU startup timer fired.
    pub(super) fn gpu_startup_due(&mut self, attempt: crate::machine::Attempt) {
        if self.start != Start::CpuFirst { return; }
        let effects = {
            let (seq, startup) = &mut self.gpu_startup;
            startup.step(seq, web_math::GpuStartupEvent::CheckDue { attempt, now: Instant::now() })
        };
        self.run_gpu_startup_effects(effects);
    }

    /// The CPU-first WebGPU probe failed; defer WebGL2 fallback until quiet, or stay on CPU.
    pub(super) fn webgpu_probe_failed(&mut self) {
        if self.start != Start::CpuFirst { return; }
        let effects = {
            let (seq, startup) = &mut self.gpu_startup;
            startup.step(seq, web_math::GpuStartupEvent::WebGpuFailed { now: Instant::now() })
        };
        self.run_gpu_startup_effects(effects);
    }

    fn run_gpu_startup_effects(&mut self, effects: crate::machine::Effects<web_math::GpuStartupEffect>) {
        for effect in effects {
            match effect {
                web_math::GpuStartupEffect::ScheduleCheck { attempt, delay } => {
                    let proxy = self.proxy.clone();
                    after(delay, move || { proxy.send_event(Wake::StartGpu(attempt)).ok(); });
                }
                web_math::GpuStartupEffect::AskWebGpu => self.ask_webgpu(),
                web_math::GpuStartupEffect::AskWebGl2 => {
                    #[cfg(feature = "webgl")]
                    self.ask_webgl2();
                    #[cfg(not(feature = "webgl"))]
                    self.fail_no_backend();
                }
                web_math::GpuStartupEffect::FailNoBackend => self.fail_no_backend(),
            }
        }
    }

    /// Probes WebGPU only on a CPU-first page. On failure, signals `Wake::WebGpuFailed` so any
    /// synchronous WebGL2 fallback still waits for a quiet window instead of running immediately.
    fn ask_webgpu(&mut self) {
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        crate::marks::mark("web.gpu.asked");
        let features = self.requested_features();
        let canvas_now = self.running.borrow().as_ref().map(|r| (r.size, r.app.rt().srgb_blend()));
        let srgb_blend = canvas_now.map_or(true, |(_, s)| s);
        let (pending, warmed, webgpu_err, proxy, shared) = (
            self.pending.clone(),
            self.warmed.clone(),
            self.webgpu_err.clone(),
            self.proxy.clone(),
            self.shared.clone(),
        );
        wasm_bindgen_futures::spawn_local(async move {
            match gpu::try_webgpu(&canvas, features).await {
                Ok(pair) => deliver_gpu(Ok(pair), canvas_now, srgb_blend, pending, Some(warmed), proxy, shared, true).await,
                Err(err) => {
                    if shared.quitting.load(Ordering::Acquire) { return; }
                    *webgpu_err.borrow_mut() = Some(err);
                    proxy.send_event(Wake::WebGpuFailed).ok();
                }
            }
        });
    }

    /// Starts WebGL2 setup on a CPU-first page after the quiet window, yielding between setup
    /// stages (`try_webgl2`) and before `Wake::Ready` so `gpu_side` (`Renderer::new`) runs in its
    /// own browser task.
    #[cfg(feature = "webgl")]
    fn ask_webgl2(&mut self) {
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        if self.webgpu_err.borrow().is_none() {
            crate::marks::mark("web.gpu.asked");
        }
        let features = self.requested_features();
        let canvas_now = self.running.borrow().as_ref().map(|r| (r.size, r.app.rt().srgb_blend()));
        let srgb_blend = canvas_now.map_or(true, |(_, s)| s);
        let (pending, webgpu_err, proxy, shared) = (
            self.pending.clone(),
            self.webgpu_err.clone(),
            self.proxy.clone(),
            self.shared.clone(),
        );
        wasm_bindgen_futures::spawn_local(async move {
            let result = match gpu::try_webgl2(&canvas, features, &shared.quitting).await {
                Ok(pair) => Ok(pair),
                Err(gl_err) => {
                    let prev = webgpu_err.borrow_mut().take();
                    let webgpu = prev.as_deref().unwrap_or("navigator.gpu unavailable");
                    Err(format!("WebGPU ({webgpu}) and WebGL2 ({gl_err}) unavailable"))
                }
            };
            deliver_gpu(result, canvas_now, srgb_blend, pending, None, proxy, shared, true).await;
        });
    }

    /// Reports that no GPU backend is available on a CPU-first page so it stays on the CPU.
    fn fail_no_backend(&mut self) {
        let prev = self.webgpu_err.borrow_mut().take();
        if prev.is_none() {
            crate::marks::mark("web.gpu.asked");
        }
        crate::marks::mark("web.gpu.answered");
        let why = prev.unwrap_or_else(|| "navigator.gpu unavailable".into());
        *self.pending.borrow_mut() = Some(Err(format!(
            "WebGPU adapter unavailable (HTTPS and WebGPU are required): {why}"
        )));
        self.gpu_came();
    }
}

/// Finishes an async GPU acquisition:
/// - WebGPU: warms pipelines via `createRenderPipelineAsync` when available.
/// - WebGL2: when `KHR_parallel_shader_compile` is supported, builds and links the real `Renderer`
///   once across browser turns via `pipelines::warm_webgl2` and hands it straight into `gpu_side`;
///   otherwise yields a browser macrotask so synchronous `Renderer::try_new` runs in its own task.
async fn deliver_gpu(
    result: Result<(crate::render::Gpu, wgpu::Surface<'static>), String>,
    canvas_now: Option<((u32, u32), bool)>,
    srgb_blend: bool,
    pending: std::rc::Rc<std::cell::RefCell<Option<Result<gpu::ReadyGpu, String>>>>,
    warmed: Option<std::rc::Rc<std::cell::RefCell<Option<pipelines::Warmed>>>>,
    proxy: winit::event_loop::EventLoopProxy<Wake>,
    shared: std::sync::Arc<crate::app::SharedCore>,
    mark_answered: bool,
) {
    if shared.quitting.load(Ordering::Acquire) {
        if let Ok((gpu, _)) = result { gpu.device.destroy(); }
        return;
    }
    if mark_answered { crate::marks::mark("web.gpu.answered"); }
    let ready = match result {
        Err(err) => Err(err),
        Ok((gpu, surface)) => {
            let backend = gpu.adapter.get_info().backend;
            if backend == wgpu::Backend::Gl {
                #[cfg(feature = "webgl")]
                {
                    let size = canvas_now.map_or((1, 1), |(s, _)| s);
                    if let Some((_, format)) = gpu::canvas_config(&gpu, &surface, size, srgb_blend) {
                        match pipelines::warm_webgl2(&gpu.device, format, &shared.quitting).await {
                            pipelines::WebGlWarm::Ready(renderer) => {
                                Ok(gpu::ReadyGpu { gpu, surface, renderer: Some(renderer) })
                            }
                            pipelines::WebGlWarm::Unsupported => {
                                if web_math::needs_ready_yield(backend) {
                                    yield_turn().await;
                                }
                                Ok(gpu::ReadyGpu { gpu, surface, renderer: None })
                            }
                            pipelines::WebGlWarm::Failed(why) => {
                                gpu.device.destroy();
                                Err(why)
                            }
                            pipelines::WebGlWarm::Aborted => {
                                gpu.device.destroy();
                                return;
                            }
                        }
                    } else {
                        if web_math::needs_ready_yield(backend) {
                            yield_turn().await;
                        }
                        Ok(gpu::ReadyGpu { gpu, surface, renderer: None })
                    }
                }
                #[cfg(not(feature = "webgl"))]
                {
                    let _ = srgb_blend;
                    if web_math::needs_ready_yield(backend) {
                        yield_turn().await;
                    }
                    Ok(gpu::ReadyGpu { gpu, surface, renderer: None })
                }
            } else {
                if let (Some((size, srgb)), Some(warmed_slot)) = (canvas_now, &warmed)
                    && let Some((_, format)) = gpu::canvas_config(&gpu, &surface, size, srgb)
                    && let Some(w) = pipelines::warm(&gpu.device, format).await
                {
                    crate::marks::mark("web.gpu.warmed");
                    *warmed_slot.borrow_mut() = Some(w);
                }
                Ok(gpu::ReadyGpu { gpu, surface, renderer: None })
            }
        }
    };
    // A loop that has closed was quit while the GPU started.
    if shared.quitting.load(Ordering::Acquire) || proxy.send_event(Wake::Ready).is_err() {
        if let Some(warmed_slot) = warmed { warmed_slot.borrow_mut().take(); }
        if let Ok(ready_gpu) = ready {
            drop(ready_gpu.renderer);
            ready_gpu.gpu.device.destroy();
        }
        return;
    }
    *pending.borrow_mut() = Some(ready);
}
