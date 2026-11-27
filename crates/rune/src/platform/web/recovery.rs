//! Bounded WebGPU device-loss recovery and GPU fallback notifications (`App::on_gpu_fallback`
//! and the bubbling `runegpufallback` DOM `CustomEvent`).

use std::sync::atomic::Ordering;
use winit::{event_loop::ActiveEventLoop, platform::web::WindowExtWebSys};
use crate::time::Instant;
use super::{
    Browser, Start, Wake, after, cpu_canvas, error,
    gpu::{self, Draw},
    health, pipelines, web_math,
};

impl Browser {
    /// Delivers a permanent GPU fallback notification to both the DOM (bubbling `runegpufallback`
    /// `CustomEvent` on `canvas[data-rune-canvas]`) and any registered `App::on_gpu_fallback`
    /// hook. Must be called without holding a `RefCell` borrow on `self.running`.
    pub(super) fn notify_gpu_fallback(&mut self, stage: crate::GpuFallbackStage, reason: &str) {
        if let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) {
            health::dispatch_gpu_fallback(&canvas, stage.as_str(), reason);
        }
        if let Some(hook) = &mut self.on_gpu_fallback {
            let fallback = crate::GpuFallback::new(stage, reason);
            hook(&fallback);
        }
    }

    /// Cancels any in-flight or scheduled WebGPU device-loss recovery and discards any pending
    /// replacement device.
    pub(super) fn cancel_webgpu_recovery(&mut self) {
        let (seq, recovery) = &mut self.webgpu_recovery;
        let _ = recovery.step(seq, web_math::gpu_recovery::WebGpuRecoveryEvent::Cancel);
        if let Some((_, Ok(ready))) = self.pending_webgpu_recovery.borrow_mut().take() {
            drop(ready.renderer);
            ready.gpu.device.destroy();
        }
    }

    /// Handles a WebGPU `device.lost` event under `GpuLossPolicy::Recover`: either schedules a
    /// bounded re-acquisition (falling back to CPU on full-window `CpuFirst` or pausing in
    /// `Draw::ContextLost` on `GpuFirst` / inline / document) or handles budget exhaustion.
    /// Returns `true` when the app ended.
    pub(super) fn handle_webgpu_loss(&mut self, event_loop: &ActiveEventLoop, why: &str) -> bool {
        use web_math::gpu_recovery::{WebGpuRecoveryEffect, WebGpuRecoveryEvent};
        let effect = {
            let (seq, recovery) = &mut self.webgpu_recovery;
            recovery
                .step(seq, WebGpuRecoveryEvent::DeviceLost { now: Instant::now(), reason: why.to_owned() })
                .into_iter()
                .next()
        };
        match effect {
            None => false,
            Some(WebGpuRecoveryEffect::GiveUp { reason }) => self.finish_webgpu_exhausted(event_loop, &reason),
            Some(WebGpuRecoveryEffect::ScheduleRetry { attempt, delay }) => {
                let Some(window) = self.window.clone() else { return false };
                let canvas = window.canvas();
                let action = web_math::loss_action_for_policy(
                    self.gpu_loss_policy,
                    false,
                    self.start == Start::GpuFirst,
                    self.embedded,
                    self.document,
                );
                let mut running = self.running.borrow_mut();
                let Some(r) = &mut *running else { return false };
                match action {
                    web_math::LossAction::PauseForRestore => {
                        let side = match std::mem::replace(&mut r.draw, Draw::Moving) {
                            Draw::Gpu(side) => { r.draw = Draw::ContextLost; r.last_presented = None; side }
                            other => { r.draw = other; return false; }
                        };
                        web_sys::console::warn_1(&format!("rune: the WebGPU device was lost ({why}); waiting to re-acquire").into());
                        crate::marks::mark("web.gpu.lost");
                        side.gpu.device.destroy();
                        drop(side);
                        r.app.release_compositor_gpu();
                    }
                    web_math::LossAction::FallbackToCpu { .. } => {
                        let side = match std::mem::replace(&mut r.draw, Draw::Moving) {
                            Draw::Gpu(side) => match canvas.as_ref().and_then(cpu_canvas::CpuCanvas::under) {
                                Some(cpu) => { r.draw = Draw::Cpu { cpu, failed: true }; r.last_presented = None; side }
                                None => {
                                    r.draw = Draw::Gpu(side);
                                    drop(running);
                                    self.cancel_webgpu_recovery();
                                    return self.finish_webgpu_exhausted(event_loop, why);
                                }
                            },
                            Draw::HandingOver { cpu, side, .. } => { r.draw = Draw::Cpu { cpu, failed: true }; side }
                            other => { r.draw = other; return false; }
                        };
                        web_sys::console::warn_1(&format!("rune: the WebGPU device was lost ({why}); drawing on CPU while recovering").into());
                        crate::marks::mark("web.gpu.lost");
                        side.gpu.device.destroy();
                        drop(side);
                        r.app.release_compositor_gpu();
                        let atlas = &mut r.app.rt_mut().text.atlas;
                        crate::render::cpu_boot::attach_text(atlas);
                        atlas.refill_gpu_masks();
                        if let Some(c) = &canvas { c.style().set_property("opacity", "0").ok(); }
                        window.request_redraw();
                    }
                    web_math::LossAction::TerminalLatch | web_math::LossAction::Fatal => {
                        drop(running);
                        self.cancel_webgpu_recovery();
                        return self.finish_webgpu_exhausted(event_loop, why);
                    }
                }
                drop(running);
                self.schedule_webgpu_retry(attempt, delay);
                false
            }
            Some(WebGpuRecoveryEffect::Acquire { .. }) => false,
        }
    }

    fn schedule_webgpu_retry(&self, attempt: crate::machine::Attempt, delay: std::time::Duration) {
        let proxy = self.proxy.clone();
        after(delay, move || {
            proxy.send_event(Wake::WebGpuRecover(attempt)).ok();
        });
    }

    /// Fires when the scheduled WebGPU device-loss recovery backoff timer expires.
    pub(super) fn on_webgpu_recover_due(&mut self, attempt: crate::machine::Attempt) {
        if self.gpu_loss_policy == crate::GpuLossPolicy::Terminal || self.terminal_failed {
            return;
        }
        use web_math::gpu_recovery::{WebGpuRecoveryEffect, WebGpuRecoveryEvent};
        let probe = {
            let (seq, recovery) = &mut self.webgpu_recovery;
            match recovery.step(seq, WebGpuRecoveryEvent::RetryDue { attempt }).into_iter().next() {
                Some(WebGpuRecoveryEffect::Acquire { attempt }) => attempt,
                _ => return,
            }
        };
        let Some(canvas) = self.window.as_ref().and_then(|w| w.canvas()) else { return };
        let (features, canvas_now) = {
            let running = self.running.borrow();
            let Some(r) = &*running else { return };
            if !matches!(r.draw, Draw::Cpu { .. } | Draw::ContextLost) {
                return;
            }
            (r.app.compositor_features(), Some((r.size, r.app.rt().srgb_blend())))
        };
        let reject_software = web_math::gpu_recovery::reject_software_adapters(self.start);
        let (pending, warmed, proxy, shared) = (
            self.pending_webgpu_recovery.clone(),
            self.warmed.clone(),
            self.proxy.clone(),
            self.shared.clone(),
        );
        wasm_bindgen_futures::spawn_local(async move {
            let result = gpu::try_webgpu(&canvas, features, reject_software).await;
            if shared.quitting.load(Ordering::Acquire) {
                if let Ok((gpu, _)) = result { gpu.device.destroy(); }
                return;
            }
            let ready = match result {
                Err(err) => Err(err),
                Ok((gpu, surface)) => {
                    if let Some((size, srgb)) = canvas_now
                        && let Some((_, format)) = gpu::canvas_config(&gpu, &surface, size, srgb)
                        && let Some(w) = pipelines::warm(&gpu.device, format).await
                    {
                        crate::marks::mark("web.gpu.warmed");
                        *warmed.borrow_mut() = Some(w);
                    }
                    Ok(gpu::ReadyGpu { gpu, surface, renderer: None })
                }
            };
            if shared.quitting.load(Ordering::Acquire) || proxy.send_event(Wake::WebGpuRecovered(probe)).is_err() {
                warmed.borrow_mut().take();
                if let Ok(ready_gpu) = ready {
                    drop(ready_gpu.renderer);
                    ready_gpu.gpu.device.destroy();
                }
                return;
            }
            if let Some((_, Ok(old))) = pending.borrow_mut().replace((probe, ready)) {
                drop(old.renderer);
                old.gpu.device.destroy();
            }
        });
    }

    /// Completes an async WebGPU recovery probe for `attempt`. Returns `true` when exhausted
    /// recovery terminated a `GpuFirst` / inline / document app.
    pub(super) fn on_webgpu_recovered(
        &mut self,
        event_loop: &ActiveEventLoop,
        attempt: crate::machine::Attempt,
    ) -> bool {
        let Some((got_attempt, made)) = self.pending_webgpu_recovery.borrow_mut().take() else { return false };
        let discard = |res: Result<gpu::ReadyGpu, String>| {
            if let Ok(ready) = res {
                drop(ready.renderer);
                ready.gpu.device.destroy();
            }
        };
        if got_attempt != attempt
            || self.terminal_failed
            || self.gpu_loss_policy == crate::GpuLossPolicy::Terminal
            || !self.webgpu_recovery.1.is_acquiring(attempt)
        {
            discard(made);
            return false;
        }
        let outcome = {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else {
                discard(made);
                return false;
            };
            if !matches!(r.draw, Draw::Cpu { .. } | Draw::ContextLost) {
                discard(made);
                return false;
            }
            let side = made.and_then(|ready| {
                gpu::gpu_side(ready, r.size, r.app.rt_mut(), self.gpu_loss_policy, self.lost.clone(), self.proxy.clone())
            });
            self.warmed.borrow_mut().take();
            match side {
                Ok(side) => {
                    r.last_presented = None;
                    if matches!(r.draw, Draw::ContextLost) {
                        r.draw = Draw::Gpu(side);
                        r.app.rt_mut().invalidate();
                        crate::marks::mark("web.gpu.restored");
                        if let Some(window) = &self.window { window.request_redraw(); }
                    } else {
                        side.gpu.queue.submit([]);
                        let barrier = gpu::when_done(&side, &self.proxy, Wake::HandoverBuilt);
                        let Draw::Cpu { mut cpu, .. } = std::mem::replace(&mut r.draw, Draw::Moving) else {
                            unreachable!("checked above")
                        };
                        cpu.half = crate::render::cpu_boot::HalfRounding::of_vendor(side.gpu.adapter.get_info().vendor);
                        r.draw = Draw::HandingOver {
                            cpu,
                            side,
                            seq: crate::machine::Seq::default(),
                            state: web_math::handover_machine::HandoverState::default(),
                            barrier: Some(barrier),
                        };
                        crate::marks::mark("web.gpu.restored");
                        crate::marks::mark("web.gpu.ready");
                    }
                    Ok(())
                }
                Err(why) => Err(why),
            }
        };
        use web_math::gpu_recovery::{WebGpuRecoveryEffect, WebGpuRecoveryEvent};
        match outcome {
            Ok(()) => {
                let (seq, recovery) = &mut self.webgpu_recovery;
                let _ = recovery.step(seq, WebGpuRecoveryEvent::AcquireSucceeded { attempt });
                false
            }
            Err(why) => {
                let effect = {
                    let (seq, recovery) = &mut self.webgpu_recovery;
                    recovery
                        .step(seq, WebGpuRecoveryEvent::AcquireFailed { attempt, now: Instant::now(), reason: why })
                        .into_iter()
                        .next()
                };
                match effect {
                    Some(WebGpuRecoveryEffect::ScheduleRetry { attempt, delay }) => {
                        self.schedule_webgpu_retry(attempt, delay);
                        false
                    }
                    Some(WebGpuRecoveryEffect::GiveUp { reason }) => {
                        self.finish_webgpu_exhausted(event_loop, &reason)
                    }
                    _ => false,
                }
            }
        }
    }

    /// Finalizes an exhausted WebGPU device-loss recovery budget: terminates `GpuFirst` / inline /
    /// document pages, or pins full-window `CpuFirst` pages to `CpuCanvas`, and emits
    /// `GpuFallbackStage::DeviceLost`. Returns `true` when the app ended.
    fn finish_webgpu_exhausted(&mut self, event_loop: &ActiveEventLoop, reason: &str) -> bool {
        let Some(window) = self.window.clone() else { return false };
        let canvas = window.canvas();
        if web_math::gpu_recovery::webgpu_exhausted_is_fatal(
            self.start == Start::GpuFirst,
            self.embedded,
            self.document,
        ) {
            if let Some(r) = &mut *self.running.borrow_mut() {
                r.last_presented = None;
                match std::mem::replace(&mut r.draw, Draw::ContextLost) {
                    Draw::Gpu(side) | Draw::HandingOver { side, .. } => {
                        side.gpu.device.destroy();
                        drop(side);
                        r.app.release_compositor_gpu();
                    }
                    other => { r.draw = other; }
                }
            }
            let message = format!("The GPU device was lost ({reason}). Reload the page to draw again.");
            self.shared.set_window_error(Some(message.clone()));
            self.notify_gpu_fallback(crate::GpuFallbackStage::DeviceLost, reason);
            error(&message, canvas.as_ref(), self.embedded, self.document);
            self.end(event_loop);
            return true;
        }
        let fell_back_to_cpu = {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else { return false };
            match std::mem::replace(&mut r.draw, Draw::Moving) {
                Draw::Gpu(side) => match canvas.as_ref().and_then(cpu_canvas::CpuCanvas::under) {
                    Some(cpu) => {
                        r.draw = Draw::Cpu { cpu, failed: true };
                        r.last_presented = None;
                        side.gpu.device.destroy();
                        drop(side);
                        r.app.release_compositor_gpu();
                        let atlas = &mut r.app.rt_mut().text.atlas;
                        crate::render::cpu_boot::attach_text(atlas);
                        atlas.refill_gpu_masks();
                        if let Some(c) = &canvas { c.style().set_property("opacity", "0").ok(); }
                        window.request_redraw();
                        true
                    }
                    None => {
                        r.draw = Draw::Gpu(side);
                        false
                    }
                },
                Draw::HandingOver { cpu, side, .. } => {
                    r.draw = Draw::Cpu { cpu, failed: true };
                    r.last_presented = None;
                    side.gpu.device.destroy();
                    drop(side);
                    r.app.release_compositor_gpu();
                    let atlas = &mut r.app.rt_mut().text.atlas;
                    crate::render::cpu_boot::attach_text(atlas);
                    atlas.refill_gpu_masks();
                    if let Some(c) = &canvas { c.style().set_property("opacity", "0").ok(); }
                    window.request_redraw();
                    true
                }
                Draw::Cpu { cpu, .. } => {
                    r.draw = Draw::Cpu { cpu, failed: true };
                    true
                }
                other => {
                    r.draw = other;
                    true
                }
            }
        };
        if !fell_back_to_cpu {
            let message = format!("The GPU device was lost ({reason}). Reload the page to draw again.");
            self.shared.set_window_error(Some(message.clone()));
            self.notify_gpu_fallback(crate::GpuFallbackStage::DeviceLost, reason);
            error(&message, canvas.as_ref(), self.embedded, self.document);
            self.end(event_loop);
            return true;
        }
        web_sys::console::warn_1(&format!("rune: WebGPU recovery exhausted ({reason}); the page draws on the CPU").into());
        crate::marks::mark("web.gpu.failed");
        self.notify_gpu_fallback(crate::GpuFallbackStage::DeviceLost, reason);
        false
    }
}
