//! CPU-first GPU acquisition, hand-over from the CPU canvas to WebGPU/WebGL2, and device/context
//! loss and restore recovery.

use std::time::Duration;
use winit::{event_loop::ActiveEventLoop, platform::web::WindowExtWebSys};
use crate::time::Instant;
use super::{
    Browser, Start, Wake, after, cpu_canvas, error,
    gpu::{self, Draw, FINISH_LIMIT, Loss},
    health, pipelines, web_math,
};

/// How long a page that starts on the CPU draws nothing before it asks for the GPU, and the
/// longest after its first frame that it waits for that.
pub(super) const QUIET: Duration = Duration::from_millis(250);
const QUIET_LIMIT: Duration = Duration::from_secs(3);

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
                    Draw::Gpu(side) => { r.draw = Draw::ContextLost; side }
                    other => { r.draw = other; return false; }
                };
                self.webgl_restoring = false;
                web_sys::console::warn_1(&format!("rune: the WebGL2 context was lost ({why}); waiting for restore").into());
                crate::marks::mark("web.gpu.lost");
                side.gpu.device.destroy();
                drop(side);
                return false;
            }
            web_math::LossAction::FallbackToCpu { .. } => {}
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
        self.webgl_restoring = false;
        web_sys::console::warn_1(&format!("rune: the GPU device was lost ({why}); the page draws on the CPU").into());
        crate::marks::mark("web.gpu.lost");
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
        let features = {
            let running = self.running.borrow();
            let Some(r) = &*running else { return };
            if !matches!(r.draw, Draw::Cpu { failed: true, .. } | Draw::ContextLost) {
                self.webgl_restored.set(false);
                return;
            }
            r.app.compositor_features()
        };
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        self.webgl_restored.set(false);
        #[cfg(feature = "webgl")]
        {
            self.webgl_restoring = true;
            let (pending, proxy) = (self.pending.clone(), self.proxy.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let result = gpu::try_webgl2(&canvas, features).await;
                if proxy.send_event(Wake::Ready).is_err() {
                    if let Ok((gpu, _)) = result { gpu.device.destroy(); }
                    return;
                }
                *pending.borrow_mut() = Some(result);
            });
        }
        #[cfg(not(feature = "webgl"))]
        let _ = (canvas, features);
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
            let side = made.and_then(|(gpu, surface)| {
                gpu::gpu_side(gpu, surface, r.size, r.app.rt_mut(), self.lost.clone(), self.proxy.clone())
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
        crate::marks::mark("web.handover.done");
        if let Some(window) = &self.window { window.request_redraw(); }
    }

    /// Asks the browser for the GPU, once.
    pub(super) fn ask_gpu(&mut self) {
        if self.gpu_asked || self.start == Start::CpuOnly { return; }
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        self.gpu_asked = true;
        crate::marks::mark("web.gpu.asked");
        // A page that started on the CPU asks after its first frame, when the running app holds the compositor.
        let features = match &self.app {
            Some(app) => app.compositor_features,
            None => self.running.borrow().as_ref().map_or(wgpu::Features::empty(), |r| r.app.compositor_features()),
        };
        // The renderer's pipelines are compiled ahead for the running app's canvas and blend, so
        // making its renderer at the hand-over finds them built (`pipelines::warm`).
        let canvas_now = self.running.borrow().as_ref().map(|r| (r.size, r.app.rt().srgb_blend()));
        let (pending, warmed, proxy) = (self.pending.clone(), self.warmed.clone(), self.proxy.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let result = gpu::gpu(canvas, features).await;
            crate::marks::mark("web.gpu.answered");
            let format = result.as_ref().ok().zip(canvas_now).and_then(|((gpu, surface), (size, srgb))| gpu::canvas_config(gpu, surface, size, srgb)).map(|(_, format)| format);
            if let (Ok((gpu, _)), Some(format)) = (&result, format) && let Some(w) = pipelines::warm(&gpu.device, format).await {
                crate::marks::mark("web.gpu.warmed");
                *warmed.borrow_mut() = Some(w);
            }
            // A loop that has closed was quit while the GPU started.
            if proxy.send_event(Wake::Ready).is_err() {
                warmed.borrow_mut().take();
                if let Ok((gpu, _)) = result { gpu.device.destroy(); }
                return;
            }
            *pending.borrow_mut() = Some(result);
        });
    }

    /// After the first frame of a page that starts on the CPU: the GPU is asked for once the page
    /// has drawn nothing but ambient loops for `QUIET` (its first reads in and shown; a spinner on
    /// screen would otherwise hold the page on the CPU, painting each of its steps there), or `QUIET_LIMIT` after that
    /// frame at the latest; until then this looks again every `QUIET`. A browser that compiles
    /// pipelines asynchronously is asked at the first look: its pipelines build off the thread
    /// that presents the CPU's frames (`pipelines::warm`), so nothing waits for quiet.
    pub(super) fn maybe_start_gpu(&mut self) {
        if self.gpu_asked || self.start != Start::CpuFirst { return; }
        let Some(first) = self.first_at else { return };
        let quiet = self.drawn_at.is_none_or(|at| at.elapsed() >= QUIET);
        if quiet || first.elapsed() >= QUIET_LIMIT || pipelines::browser_compiles_async() { return self.ask_gpu(); }
        let proxy = self.proxy.clone();
        after(QUIET, move || { proxy.send_event(Wake::StartGpu).ok(); });
    }
}
