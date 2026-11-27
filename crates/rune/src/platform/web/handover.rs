//! CPU-first GPU acquisition, hand-over from the CPU canvas to WebGPU/WebGL2, and device/context
//! loss and restore recovery.

use std::{sync::atomic::Ordering, time::Duration};
use winit::{event_loop::ActiveEventLoop, platform::web::WindowExtWebSys};
use crate::time::Instant;
use super::{
    Browser, Start, Wake, after, cpu_canvas, error,
    gpu::{self, Draw, Loss},
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
            web_math::webgl_prevent_default(self.gpu_loss_policy),
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

    /// The browser lost the GPU device or WebGL2 context. Under `GpuLossPolicy::Recover`, WebGPU
    /// loss triggers bounded re-acquisition (`WebGpuRecovery`), mirroring WebGL2 restore (falling
    /// back to CPU on full-window `CpuFirst` pages or pausing in `Draw::ContextLost` on `GpuFirst`
    /// / inline / document pages). WebGL2 context loss falls back to the CPU canvas on full-window
    /// pages or pauses inline/document pages without exiting, so a later `webglcontextrestored`
    /// brings WebGL2 back. True when the app ended.
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
        if self.gpu_loss_policy == crate::GpuLossPolicy::Terminal {
            self.enter_terminal_gpu_loss(why.to_owned());
            self.notify_gpu_fallback(crate::GpuFallbackStage::DeviceLost, why);
            return false;
        }
        if !loss.is_webgl() {
            return self.handle_webgpu_loss(event_loop, why);
        }
        match web_math::loss_action_for_policy(self.gpu_loss_policy, true, self.start == Start::GpuFirst, self.embedded, self.document) {
            web_math::LossAction::TerminalLatch => {
                self.enter_terminal_gpu_loss(why.to_owned());
                self.notify_gpu_fallback(crate::GpuFallbackStage::DeviceLost, why);
                return false;
            }
            web_math::LossAction::Fatal => {
                self.notify_gpu_fallback(crate::GpuFallbackStage::DeviceLost, why);
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
        self.cancel_webgpu_recovery();
        if self.gpu_loss_policy == crate::GpuLossPolicy::Terminal {
            self.enter_terminal_gpu_loss(format!("The GPU rejected Rune's shader ({why})."));
            self.notify_gpu_fallback(crate::GpuFallbackStage::Shader, &why);
            return false;
        }
        let Some(window) = self.window.clone() else { return false };
        let canvas = window.canvas();
        if self.start == Start::GpuFirst || self.embedded || self.document {
            let message = format!("The GPU rejected Rune's shader ({why}).");
            self.shared.set_window_error(Some(message.clone()));
            self.notify_gpu_fallback(crate::GpuFallbackStage::Shader, &why);
            error(&message, canvas.as_ref(), self.embedded, self.document);
            self.end(event_loop);
            return true;
        }
        let fell_back = {
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
                        self.notify_gpu_fallback(crate::GpuFallbackStage::Shader, &why);
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
            true
        };
        if fell_back {
            self.notify_gpu_fallback(crate::GpuFallbackStage::Shader, &why);
        }
        false
    }

    /// Rebuilds the WebGL2 `GpuSide` after `webglcontextrestored` fires on the canvas.
    /// Enters `GpuLossPolicy::Terminal` latched state on web: keeps the page and canvas alive in
    /// the DOM (never calling `self.end(event_loop)`), drops `GpuSide` and calls
    /// `release_compositor_gpu()`, refuses CPU fallback and WebGL2 restore, records the first
    /// reason in `SharedCore::window_error`, and wakes `AppSpec::poll` once.
    pub(super) fn enter_terminal_gpu_loss(&mut self, reason: String) {
        if self.terminal_failed {
            return;
        }
        self.cancel_webgpu_recovery();
        let bounded = crate::render::loss_latch::truncate_reason(&reason, crate::render::loss_latch::GpuFailureKind::DeviceLost);
        self.terminal_failed = true;
        self.terminal_poll_pending = true;
        self.webgl_restored.set(false);
        self.webgl_restoring = false;
        self.shared.set_window_error(Some(bounded));
        crate::marks::mark("web.gpu.lost");
        if let Some(r) = &mut *self.running.borrow_mut() {
            r.last_presented = None;
            let old = std::mem::replace(&mut r.draw, Draw::TerminalFailed);
            match old {
                Draw::Gpu(side) | Draw::HandingOver { side, .. } => {
                    side.gpu.device.destroy();
                    drop(side);
                }
                _ => {}
            }
            r.app.release_compositor_gpu();
        }
        self.proxy.send_event(Wake::App).ok();
    }

    /// Handles `Wake::App` while `terminal_failed` is set: runs `AppSpec::poll` once without
    /// redrawing, applies non-GPU requests (page title, clipboard, quit), and stays idle.
    pub(super) fn drain_terminal_wake(&mut self, event_loop: &ActiveEventLoop) {
        let requests = self.shared.take_window();
        if requests.quit.is_some() {
            self.end(event_loop);
            return;
        }
        let Some(window) = self.window.as_ref() else { return };
        let should_end = if let Some(r) = &mut *self.running.borrow_mut() {
            let _ = r.app.drain_updates();
            if std::mem::take(&mut self.terminal_poll_pending) {
                let _ = r.app.poll_hidden();
            }
            r.apply_frame_requests(window);
            r.clipboard.flush_core(r.app.rt_mut());
            r.app.rt_mut().shell_request.take().is_some()
        } else {
            false
        };
        if should_end {
            self.end(event_loop);
        }
    }

    pub(super) fn maybe_restore_webgl(&mut self) {
        if self.gpu_loss_policy == crate::GpuLossPolicy::Terminal || self.terminal_failed {
            self.webgl_restored.set(false);
            return;
        }
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
            let reject_software = web_math::gpu_recovery::reject_software_adapters(self.start);
            let (pending, proxy, shared) = (self.pending.clone(), self.proxy.clone(), self.shared.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let result = gpu::try_webgl2(&canvas, features, &shared.quitting, reject_software).await;
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
        let mut fallback = None;
        let watch_webgl = {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else { return };
            let can_take = web_math::cpu_canvas::can_take_pending_gpu(
                r.presented,
                r.cpu_blocked_on_gpu,
                self.start == Start::CpuFirst,
                matches!(r.draw, Draw::Cpu { failed: false, .. }),
                self.webgl_restoring,
                matches!(r.draw, Draw::Cpu { .. } | Draw::ContextLost),
            );
            if !can_take { return; }
            let Some(made) = self.pending.borrow_mut().take() else { return };
            let restoring = std::mem::take(&mut self.webgl_restoring);
            if !restoring && self.start == Start::CpuFirst {
                let (seq, startup) = &mut self.gpu_startup;
                let _ = startup.step(seq, web_math::GpuStartupEvent::WebGpuSucceeded);
            }
            let side = made.and_then(|ready| {
                gpu::gpu_side(ready, r.size, r.app.rt_mut(), self.gpu_loss_policy, self.lost.clone(), self.proxy.clone())
            });
            // The renderer is made: the pipelines compiled ahead for it can go.
            self.warmed.borrow_mut().take();
            match side {
                Err(why) => {
                    web_sys::console::warn_1(&format!("rune: the page draws on the CPU ({why})").into());
                    crate::marks::mark("web.gpu.failed");
                    if let Draw::Cpu { failed, .. } = &mut r.draw { *failed = true; }
                    let stage = if restoring {
                        crate::GpuFallbackStage::DeviceLost
                    } else {
                        crate::GpuFallbackStage::Init
                    };
                    fallback = Some((stage, why));
                    false
                }
                Ok(side) => {
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
                        let barrier = gpu::when_done(&side, &self.proxy, Wake::HandoverBuilt);
                        let Draw::Cpu { mut cpu, .. } = std::mem::replace(&mut r.draw, Draw::Moving) else { unreachable!("checked above") };
                        cpu.half = crate::render::cpu_boot::HalfRounding::of_vendor(side.gpu.adapter.get_info().vendor);
                        r.draw = Draw::HandingOver {
                            cpu,
                            side,
                            seq: crate::machine::Seq::default(),
                            state: web_math::handover_machine::HandoverState::default(),
                            barrier: Some(barrier),
                        };
                        if restoring { crate::marks::mark("web.gpu.restored"); }
                        crate::marks::mark("web.gpu.ready");
                    }
                    is_gl
                }
            }
        };
        if let Some((stage, why)) = fallback {
            self.notify_gpu_fallback(stage, &why);
        } else if watch_webgl {
            self.ensure_webgl_watch();
        }
    }

    /// An async browser error-scope check (`uber_check` or a lazy feature breaker) resolved.
    /// Advances any answered feature breakers without encoding a GPU frame, then either advances
    /// `Draw::HandingOver` or requests one redraw for `Draw::Gpu` once all checks have cleared.
    pub(super) fn on_gpu_check(&mut self) {
        use web_math::handover_machine::{HandoverEvent, should_redraw_after_gpu_check};
        let (handover_effects, gpu_redraw) = {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else { return };
            match &mut r.draw {
                Draw::HandingOver { side, seq, state, .. } => {
                    let _ = side.renderer.poll_breaker_checks(&side.gpu.device);
                    let effects = state.step(seq, HandoverEvent::ChecksSettled {
                        still_checking: side.renderer.checking_gpu(),
                    });
                    (effects, false)
                }
                Draw::Gpu(side) => {
                    let settled = side.renderer.poll_breaker_checks(&side.gpu.device);
                    let redraw = should_redraw_after_gpu_check(
                        side.renderer.checking_gpu(),
                        settled,
                        r.last_presented.is_some(),
                    );
                    (crate::machine::Effects::none(), redraw)
                }
                _ => (crate::machine::Effects::none(), false),
            }
        };
        self.apply_handover_effects(handover_effects);
        if gpu_redraw && let Some(window) = &self.window { window.request_redraw(); }
    }

    /// The empty-queue barrier submitted after `Renderer::try_new` resolved.
    pub(super) fn on_handover_built(&mut self) {
        use web_math::handover_machine::HandoverEvent;
        let effects = {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else { return };
            let Draw::HandingOver { side, seq, state, barrier, .. } = &mut r.draw else { return };
            *barrier = None;
            let _ = side.renderer.poll_breaker_checks(&side.gpu.device);
            state.step(seq, HandoverEvent::Built { checking_gpu: side.renderer.checking_gpu() })
        };
        self.apply_handover_effects(effects);
    }

    /// `onSubmittedWorkDone` resolved for the submitted hand-over GPU frame `attempt`.
    pub(super) fn on_handover_done(&mut self, attempt: crate::machine::Attempt) {
        self.step_handover(web_math::handover_machine::HandoverEvent::GpuDone { attempt });
    }

    /// The `FINISH_LIMIT` timeout timer fired for `attempt`. On WebGL2, gives the device one
    /// final non-blocking poll first so a tab that was hidden (pausing rAF) still completes
    /// cleanly if the GPU already finished.
    pub(super) fn on_handover_timeout(&mut self, attempt: crate::machine::Attempt) {
        let polled_done = {
            let running = self.running.borrow();
            match running.as_ref().map(|r| &r.draw) {
                Some(Draw::HandingOver { side, barrier: Some(barrier), state, .. })
                    if state.waiting_for_gpu() && web_math::needs_device_poll(side.backend) =>
                {
                    let _ = side.gpu.device.poll(wgpu::PollType::Poll);
                    barrier.is_done()
                }
                _ => false,
            }
        };
        if polled_done {
            self.on_handover_done(attempt);
        } else {
            self.poll_handover_timeout();
        }
    }

    /// Checks whether a hand-over frame in `WaitingDone` has passed `FINISH_LIMIT`.
    pub(super) fn poll_handover_timeout(&mut self) {
        self.step_handover(web_math::handover_machine::HandoverEvent::Tick { now: Instant::now() });
    }

    fn step_handover(&mut self, event: web_math::handover_machine::HandoverEvent) {
        let effects = {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else { return };
            let Draw::HandingOver { seq, state, .. } = &mut r.draw else { return };
            state.step(seq, event)
        };
        self.apply_handover_effects(effects);
    }

    fn apply_handover_effects(
        &mut self,
        effects: crate::machine::Effects<web_math::handover_machine::HandoverEffect>,
    ) {
        use web_math::handover_machine::HandoverEffect;
        for effect in effects {
            match effect {
                HandoverEffect::RequestRedraw => {
                    if let Some(window) = &self.window { window.request_redraw(); }
                }
                HandoverEffect::FailTimeout => self.fail_handover_timeout(),
                HandoverEffect::PrecheckFeatures
                | HandoverEffect::DrawGpuFrame
                | HandoverEffect::DrawFlipFrame
                | HandoverEffect::ArmFinishWait { .. }
                | HandoverEffect::Complete => {}
            }
        }
    }

    /// Arms the optional `globalThis.RUNE_LOSE_GPU_AFTER` harness timer after hand-over completes.
    pub(super) fn arm_lose_gpu_harness(&self) {
        if let Some(ms) = js_sys::Reflect::get(&js_sys::global(), &"RUNE_LOSE_GPU_AFTER".into()).ok().and_then(|v| v.as_f64()) {
            let (lost, proxy) = (self.lost.clone(), self.proxy.clone());
            after(Duration::from_millis(ms as u64), move || {
                *lost.borrow_mut() = Some(Loss::WebGpu("RUNE_LOSE_GPU_AFTER".into()));
                proxy.send_event(Wake::App).ok();
            });
        }
    }

    /// Destroys the timed-out GPU device and falls back to the CPU canvas after `FINISH_LIMIT`.
    fn fail_handover_timeout(&mut self) {
        {
            let mut running = self.running.borrow_mut();
            let Some(r) = &mut *running else { return };
            let Draw::HandingOver { cpu, side, .. } = std::mem::replace(&mut r.draw, Draw::Moving) else { return };
            web_sys::console::warn_1(&"rune: the GPU didn't finish its first frame; the page draws on the CPU".into());
            crate::marks::mark("web.handover.failed");
            side.gpu.device.destroy();
            drop(side);
            r.app.release_compositor_gpu();
            let atlas = &mut r.app.rt_mut().text.atlas;
            crate::render::cpu_boot::attach_text(atlas);
            atlas.refill_gpu_masks();
            r.draw = Draw::Cpu { cpu, failed: true };
        }
        self.cancel_webgpu_recovery();
        self.notify_gpu_fallback(
            crate::GpuFallbackStage::HandoverTimeout,
            "GPU hand-over frame timed out",
        );
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
        let reject_software = web_math::gpu_recovery::reject_software_adapters(self.start);
        let (pending, warmed, proxy, shared) = (self.pending.clone(), self.warmed.clone(), self.proxy.clone(), self.shared.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let result = gpu::gpu(canvas, features, &shared.quitting, reject_software).await;
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

    /// A frame hit a non-degradable `NeedsGpu` error on the CPU canvas: start or escalate GPU
    /// acquisition immediately on the next macrotask instead of waiting for frame 1 or quiet.
    pub(super) fn on_cpu_needs_gpu(&mut self, at: Instant) {
        let effects = {
            let (seq, startup) = &mut self.gpu_startup;
            startup.step(seq, web_math::GpuStartupEvent::NeedsGpu {
                at,
                has_navigator_gpu: health::has_navigator_gpu(),
                webgl_enabled: cfg!(feature = "webgl"),
            })
        };
        self.run_gpu_startup_effects(effects);
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
        let reject_software = web_math::gpu_recovery::reject_software_adapters(self.start);
        let (pending, warmed, webgpu_err, proxy, shared) = (
            self.pending.clone(),
            self.warmed.clone(),
            self.webgpu_err.clone(),
            self.proxy.clone(),
            self.shared.clone(),
        );
        wasm_bindgen_futures::spawn_local(async move {
            match gpu::try_webgpu(&canvas, features, reject_software).await {
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
        let reject_software = web_math::gpu_recovery::reject_software_adapters(self.start);
        let (pending, webgpu_err, proxy, shared) = (
            self.pending.clone(),
            self.webgpu_err.clone(),
            self.proxy.clone(),
            self.shared.clone(),
        );
        wasm_bindgen_futures::spawn_local(async move {
            let result = match gpu::try_webgl2(&canvas, features, &shared.quitting, reject_software).await {
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
