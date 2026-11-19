//! CPU-first GPU acquisition, hand-over from the CPU canvas to WebGPU, and device-loss recovery.

use std::time::Duration;
use winit::{event_loop::ActiveEventLoop, platform::web::WindowExtWebSys};
use crate::time::Instant;
use super::{Browser, Start, Wake, after, cpu_canvas, error, gpu::{self, Draw, FINISH_LIMIT}, pipelines};

/// How long a page that starts on the CPU draws nothing before it asks for the GPU, and the
/// longest after its first frame that it waits for that.
pub(super) const QUIET: Duration = Duration::from_millis(250);
const QUIET_LIMIT: Duration = Duration::from_secs(3);

impl<S: 'static> Browser<S> {
    /// The browser lost the GPU device. A page with a CPU renderer goes back to it: the next
    /// frame paints whole, and the glyph masks the device held fill again on the CPU. A page that
    /// started on the GPU ends, as nothing more can draw. True when the app ended.
    pub(super) fn lost_device(&mut self, event_loop: &ActiveEventLoop) -> bool {
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

    /// The GPU answered while the page draws on the CPU: after the first frame, its side is made
    /// (the browser compiles its pipelines) and the hand-over starts. A GPU that couldn't be made
    /// leaves the page on the CPU for good.
    pub(super) fn gpu_came(&mut self) {
        if self.start != Start::CpuFirst { return; }
        let mut running = self.running.borrow_mut();
        let Some(r) = &mut *running else { return };
        if !r.presented || !matches!(r.draw, Draw::Cpu { failed: false, .. }) { return; }
        let Some(made) = self.pending.borrow_mut().take() else { return };
        let side = made.and_then(|(gpu, surface)| {
            let (lost, proxy) = (self.lost.clone(), self.proxy.clone());
            gpu::gpu_side(gpu, surface, r.size, &mut r.rt, move |why| { *lost.borrow_mut() = Some(why); let _ = proxy.send_event(Wake::App); })
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
        // The browser compiles the renderer's pipelines in its GPU process, in order: a submit
        // made after them is answered once they're built.
        side.gpu.queue.submit([]);
        let built = gpu::when_done(&side.gpu.queue, &self.proxy);
        let Draw::Cpu { mut cpu, .. } = std::mem::replace(&mut r.draw, Draw::Moving) else { unreachable!("checked above") };
        cpu.half = crate::render::cpu_boot::HalfRounding::of_vendor(side.gpu.adapter.get_info().vendor);
        r.draw = Draw::HandingOver { cpu, side, built, done: None, since: Instant::now() };
        crate::marks::mark("web.gpu.ready");
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
    pub(super) fn ask_gpu(&mut self) {
        if self.gpu_asked || self.start == Start::CpuOnly { return; }
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        self.gpu_asked = true;
        crate::marks::mark("web.gpu.asked");
        // A page that started on the CPU asks after its first frame, when the running app holds the compositor.
        let features = match self.app.as_ref().and_then(|a| a.compositor.as_ref()) {
            Some(c) => c.features(),
            None => self.running.borrow().as_ref().and_then(|r| r.compositor.as_ref()).map_or(wgpu::Features::empty(), |c| c.features()),
        };
        // The renderer's pipelines are compiled ahead for the running app's canvas and blend, so
        // making its renderer at the hand-over finds them built (`pipelines::warm`).
        let canvas_now = self.running.borrow().as_ref().map(|r| (r.size, r.rt.srgb_blend()));
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
        after(QUIET, move || { let _ = proxy.send_event(Wake::StartGpu); });
    }
}
