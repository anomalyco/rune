//! An overlay's frame on its surface: whether one is due, the acquire, the step and the draw,
//! and the occlusion and pacing that decide when the next comes.

use super::*;
use crate::frame::host_step::{
    self, Acquire, FrameHost, Gate, PostStepCheck, ReacquireReason, RenderPrep, StepInfo,
    create_frame_encoder, draw_ui, prepare_gpu_resources,
};

struct PreparedOverlayFrame {
    frame: wgpu::SurfaceTexture,
    view: wgpu::TextureView,
    encoder: wgpu::CommandEncoder,
    target_format: wgpu::TextureFormat,
    size: (u32, u32),
}

struct OverlayFrame<'a> {
    core: &'a mut OverlayCore,
    app: &'a mut dyn OverlayApp,
    event_loop: Option<&'a ActiveEventLoop>,
    prewarm: bool,
    read_occlusion: bool,
    paced: bool,
    content_resized: bool,
    pending_content_size: Option<(f32, f32)>,
    #[cfg(target_os = "macos")]
    target: Option<Instant>,
    #[cfg(any(target_os = "macos", all(target_os = "windows", feature = "windows-system-material")))]
    materials_checked: bool,
    /// This frame draws presuming the panel on screen (`OverlayCore::prime`).
    #[cfg(target_os = "macos")]
    presume: bool,
    now: Option<Instant>,
    more: bool,
}

impl OverlayFrame<'_> {
    fn reblend_surface(&mut self) -> bool {
        let (Some(live), Some(renderer), Some(gpu)) = (
            self.core.surface.live_mut(),
            self.core.renderer.as_mut(),
            self.core.gpu.as_ref(),
        ) else {
            return false;
        };
        let ok = crate::window::reblend_core(&live.surface, gpu, &mut live.config, renderer, &mut live.alpha, self.app.rt_mut());
        #[cfg(target_os = "windows")]
        live.slot.reconfigured(ok);
        ok
    }
}

impl FrameHost for OverlayFrame<'_> {
    type Drawable = wgpu::SurfaceTexture;
    type Prepared = PreparedOverlayFrame;

    fn gate(&mut self) -> Gate {
        if self.core.poll_terminal_gpu_loss(self.app) { return Gate::Skip; }
        let timer = !self.core.quiet.step(frame::QuietEvent::FrameStarts).is_empty();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = self.core.surface.live_mut() {
            let changes = layer.dispatch();
            if changes.closed {
                let reason = layer.close_reason().map(str::to_owned);
                self.core.layer_closed(self.app, reason.as_deref());
                return Gate::Skip;
            }
            for input in changes.input { self.core.surface_input(self.app, input); }
            if let Some((w, h)) = changes.size {
                for effect in self.core.wayland_resize.step(super::sizing::WaylandResizeEvent::Configured((w, h))) {
                    let super::sizing::WaylandResizeEffect::ApplyConfigured((cw, ch)) = effect else { continue };
                    let size = (if cw == 0 { self.core.at.size.0 } else { cw as f32 }, if ch == 0 { self.core.at.size.1 } else { ch as f32 });
                    if size != self.core.at.size {
                        self.core.at.size = size;
                        if let Some(live) = self.core.surface.live_mut() { live.scale = 0.0; }
                    }
                    self.core.dirty = true;
                }
            }
            if let Some(scale) = changes.scale {
                if let Some(Live { backing: Backing::Layer(layer), .. }) = self.core.surface.live_mut() { layer.set_buffer_scale(scale); }
                if let Some(gpu) = self.core.gpu.clone() {
                    self.core.rescale(self.app, &gpu, scale as f32);
                }
            }
            if self.core.surface.live().is_some_and(|l| l.scale == 0.0) && let Some(gpu) = self.core.gpu.clone() {
                // Read before `app` is reborrowed for `rescale`.
                let scale = layer_resize_scale(self.app.rt());
                self.core.rescale(self.app, &gpu, scale);
            }
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        self.core.poll_wayland_backdrop(self.app.rt_mut());
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Xwayland(surface), .. }) = self.core.surface.live_mut() {
            let input = surface.dispatch();
            for input in input { self.core.surface_input(self.app, input); }
        }
        self.core.apply_input(self.app.rt());
        if self.core.surface.live().is_none() || self.core.renderer.is_none() || self.core.gpu.is_none() {
            return Gate::Skip;
        }
        // `sync_occlusion` below reads, once a turn, the panel hosting the surface: the window
        // the acquire would check again.
        #[cfg(target_os = "macos")]
        { self.read_occlusion = !self.prewarm && self.core.panel.is_some(); }
        #[cfg(not(target_os = "macos"))]
        { self.read_occlusion = false; }
        // Even an idle overlay looks again on its retry while occluded: the occlusion
        // notification that should uncover it may never come.
        #[cfg(target_os = "macos")]
        if !self.prewarm { self.core.sync_occlusion(self.app); }
        let gate = frame::Gate {
            prewarm: self.prewarm,
            occluded: cfg!(target_os = "macos") && self.core.presence.occluded(),
            awaiting_frame: self.core.awaiting_layer_frame() || self.core.wayland_resize.is_pending(),
            dirty: self.core.dirty || self.app.rt().has_input_waiting(),
            idle: self.core.idle,
            retry: self.core.backoff.deadline(),
            missed_this_turn: self.core.turn_frame.missed(),
        };
        match frame::turn(gate, crate::time::now) {
            frame::Turn::Occluded { recheck } => {
                if recheck {
                    self.core.meter.skipped();
                    self.core.missed(self.app, crate::window::Missed::Occluded, false, self.core.turn_frame.stepped());
                }
                return Gate::Skip;
            }
            // `wake_due` runs due timers while it backs off.
            frame::Turn::Rest | frame::Turn::BackedOff => return Gate::Skip,
            frame::Turn::AwaitingFrame => {
                if self.app.rt().has_pending_input() {
                    self.core.commit_at(self.app, crate::time::now());
                }
                self.core.meter.skipped();
                self.core.meter.set_pacing(self.core.pacing_state(true));
                return Gate::Skip;
            }
            frame::Turn::Draw => if !self.prewarm { self.core.backoff_step(crate::window::BackoffEvent::Lapsed); },
        }
        // The swap chain still holds a frame the compositor hasn't shown: this one draws when it
        // has room, built from what changed by then (`slot`).
        #[cfg(target_os = "windows")]
        if let Some(wake) = self.core.slot_wake.clone() && let Some(live) = self.core.surface.live_mut() {
            if live.slot.take_surface(&live.surface, move || wake()) == Take::Later {
                // The registered wait asks for this frame (`WakeEvent::FrameSlot`): not a miss,
                // and no timer of its own to turn the loop with (`waits_for_slot`).
                return Gate::Skip;
            }
        }
        #[cfg(target_os = "macos")]
        { self.presume = !self.prewarm && self.core.prime.presumes(); }
        let (Some(live), Some(renderer)) = (self.core.surface.live_mut(), self.core.renderer.as_ref()) else {
            return Gate::Skip;
        };
        // Only a timer woke the resting overlay: build the frame first, and take neither a
        // drawable nor a display tick when it draws what's on screen. The link, stopped, would
        // draw it at once anyway (`defers_start`).
        let quiet = self.core.quiet.presented().is_some()
            && timer && !self.prewarm && !self.app.has_compositor()
            && !self.app.rt().has_pending_input()
            && renderer.srgb_blend() == self.app.rt().srgb_blend();
        #[cfg(target_os = "macos")]
        let quiet = quiet && live.pacing.as_ref().is_none_or(|p| p.defers_start(crate::time::now()));
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        let quiet = quiet && !live.materials.native();
        if quiet {
            return Gate::QuietTimer;
        }
        // Claim display pacing before any step when not deferring a quiet timer frame, matching
        // `window/host/draw.rs` so a Wait never discards a frame `quiet_step` already stepped.
        #[cfg(target_os = "macos")]
        {
            let opportunity = if self.prewarm { None } else { live.pacing.as_mut().map(|p| p.next_frame()) };
            let Some(target) = frame::claim(opportunity, || live.settle()) else { return Gate::Skip };
            self.target = target;
            self.paced = live.pacing.as_ref().is_some_and(|p| p.paced());
        }
        #[cfg(not(target_os = "macos"))]
        {
            live.settle();
            self.paced = false;
        }
        // The present queue's vsync paces redraws where no display link does.
        if !self.prewarm { self.core.meter.redraw(self.paced || !cfg!(target_os = "macos")); }
        #[cfg(target_os = "macos")]
        if self.presume { self.core.prime.spend(); }
        Gate::Draw
    }

    fn needs_reblend(&self) -> bool {
        self.core.renderer.as_ref().is_some_and(|r| r.srgb_blend() != self.app.rt().srgb_blend())
    }

    fn reblend(&mut self) -> bool { self.reblend_surface() }

    fn step_quiet(&mut self) -> StepInfo {
        let now = crate::time::now();
        self.now = Some(now);
        #[cfg(target_os = "macos")]
        if let Some(live) = self.core.surface.live() {
            self.app.rt_mut().paint.set_native_materials(live.materials.native());
        }
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        if let Some(live) = self.core.surface.live() {
            self.app.rt_mut().paint.set_windows_acrylic_materials(live.materials.native());
        }
        let (animating, dt) = self.app.clock_advance(&mut self.core.clock, now, false);
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: None }
    }

    fn is_unchanged(&mut self, info: &StepInfo) -> bool {
        let (Some(live), Some(presented)) = (self.core.surface.live(), self.core.quiet.presented()) else {
            return false;
        };
        #[cfg(target_os = "macos")]
        let next_tick = {
            let now = self.now.unwrap_or_else(crate::time::now);
            live.pacing.as_ref().map(|p| p.tick_after(now))
        };
        #[cfg(not(target_os = "macos"))]
        let next_tick = None;
        presented.unchanged_core(self.app.rt(), &live.config, info.animating, next_tick)
    }

    fn on_unchanged(&mut self, _info: &StepInfo) {
        self.more = self.core.unchanged_frame(self.app);
    }

    fn on_quiet_will_draw(&mut self) {
        if let Some(live) = self.core.surface.live_mut() {
            #[cfg(target_os = "macos")]
            {
                if let Some(pacing) = live.pacing.as_mut() { let _ = pacing.next_frame(); }
                live.settle();
                self.paced = live.pacing.as_ref().is_some_and(|p| p.paced());
            }
            #[cfg(not(target_os = "macos"))]
            {
                live.settle();
                self.paced = false;
            }
        }
        if !self.prewarm { self.core.meter.redraw(self.paced || !cfg!(target_os = "macos")); }
        #[cfg(target_os = "macos")]
        if self.presume { self.core.prime.spend(); }
    }

    fn acquire(&mut self) -> Acquire<wgpu::SurfaceTexture> {
        if self.core.poll_terminal_gpu_loss(self.app) {
            return Acquire::Missed(crate::window::Missed::Unconfigured);
        }
        let (Some(live), Some(gpu)) = (self.core.surface.live(), self.core.gpu.as_ref()) else {
            return Acquire::Missed(crate::window::Missed::Unconfigured);
        };
        // wgpu checks the panel's occlusion itself before it hands out a drawable.
        #[cfg(target_os = "macos")]
        let _shown = if self.presume { crate::platform::macos::PresumeShown::new(&live.surface) } else { None };
        match crate::window::acquire_checked(
            &live.surface,
            gpu,
            &live.config,
            true,
            if self.prewarm { 2 } else { 1 },
            &mut self.core.meter,
            !self.read_occlusion,
        ) {
            Ok(frame) => {
                self.core.note_frame_texture_acquired();
                Acquire::Ready(frame)
            }
            Err(missed) => Acquire::Missed(missed),
        }
    }

    fn step_runtime(&mut self) -> StepInfo {
        let now = crate::time::now();
        self.now = Some(now);
        #[cfg(not(target_os = "macos"))]
        let frame_time = now;
        #[cfg(target_os = "macos")]
        let frame_time = self.target.unwrap_or(now);
        #[cfg(target_os = "macos")]
        if let Some(live) = self.core.surface.live() {
            self.app.rt_mut().paint.set_native_materials(live.materials.native());
        }
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        if let Some(live) = self.core.surface.live() {
            self.app.rt_mut().paint.set_windows_acrylic_materials(live.materials.native());
        }
        let (animating, dt) = if !self.prewarm && frame::rests(self.app, self.core.runtime_idle, self.core.dirty) {
            frame::rest(self.app, &mut self.core.clock, frame_time)
        } else {
            step(self.app, &mut self.core.clock, frame_time, self.prewarm)
        };
        #[cfg(target_os = "macos")]
        let presents_at = self.target;
        #[cfg(not(target_os = "macos"))]
        let presents_at = None;
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at }
    }

    fn post_step_check(&mut self, _info: &mut StepInfo) -> PostStepCheck {
        if self.core.poll_terminal_gpu_loss(self.app) {
            return PostStepCheck::Keep;
        }
        if !self.content_resized && let Some(next) = self.core.take_live_fit_resize(self.app.rt_mut()) {
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if self.core.wayland_request_content_resize(next) {
                return PostStepCheck::Hold;
            }
            self.content_resized = true;
            self.pending_content_size = Some(next);
            return PostStepCheck::Reacquire(ReacquireReason::ContentResize);
        }
        #[cfg(target_os = "macos")]
        if !self.materials_checked {
            self.materials_checked = true;
            if let Some(live) = self.core.surface.live_mut() {
                let Backing::Window(window) = &live.backing;
                if live.materials.update(self.app.rt().transparent, !self.app.rt().paint.materials().is_empty(), window, &live.surface) {
                    return PostStepCheck::Reacquire(ReacquireReason::NativeMaterials);
                }
            }
        }
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        if !self.materials_checked {
            self.materials_checked = true;
            let device_type = self.core.gpu.as_ref().map(|gpu| gpu.adapter.get_info().device_type);
            let window_backdrop = self.app.rt().window_backdrop();
            let found = !self.app.rt().paint.windows_material_draws().is_empty();
            let high_contrast = self.app.rt().system_contrast().is_some();
            if let Some(live) = self.core.surface.live_mut()
                && (found || live.materials.native())
            {
                let mut effects = crate::system_material::windows::current_policy();
                effects.high_contrast |= high_contrast;
                let hwnd = live.backing.window().and_then(|w| crate::platform::win32::hwnd(w)).map(|h| h as isize);
                let inputs = crate::system_material::windows::MaterialInputs {
                    transparent: true,
                    build: crate::platform::win32::dark_mode::windows_build(),
                    swapchain: crate::render::Gpu::dx12_swapchain_kind(),
                    alpha_mode: Some(live.config.alpha_mode),
                    cpu_painter: false,
                    renderer_kind: self.app.rt().renderer_kind(),
                    device_type,
                    window_backdrop,
                    mode: live.materials.mode(),
                    has_handover: live.handover.is_some(),
                    effects,
                };
                if live.materials.update(hwnd, inputs, found, live.handover.as_mut()) {
                    return PostStepCheck::Reacquire(ReacquireReason::NativeMaterials);
                }
            }
        }
        if self.needs_reblend() {
            return PostStepCheck::Reacquire(ReacquireReason::Reblend);
        }
        PostStepCheck::Keep
    }

    fn prepare_reacquire(&mut self, reason: ReacquireReason, info: &mut StepInfo) -> bool {
        if self.core.poll_terminal_gpu_loss(self.app) {
            return false;
        }
        match reason {
            #[cfg(any(test, target_os = "macos", all(target_os = "windows", feature = "windows-system-material")))]
            ReacquireReason::NativeMaterials => {
                // Rebuild with the updated native-materials state into a replacement drawable.
                #[cfg(target_os = "macos")]
                {
                    let native = self.core.surface.live().is_some_and(|l| l.materials.native());
                    self.app.rt_mut().paint.set_native_materials(native);
                }
                #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                {
                    let native = self.core.surface.live().is_some_and(|l| l.materials.native());
                    self.app.rt_mut().paint.set_windows_acrylic_materials(native);
                }
                #[cfg(not(any(target_os = "macos", all(target_os = "windows", feature = "windows-system-material"))))]
                self.app.rt_mut().paint.set_native_materials(true);
                let more = self.app.frame(0.0);
                info.runtime_animating |= more;
                info.animating |= more;
                true
            }
            ReacquireReason::Reblend => {
                let _ = info;
                let ok = self.reblend_surface();
                if !ok && self.core.is_terminal_gpu_policy() {
                    self.core.latch_terminal_loss_latch(
                        self.app,
                        crate::render::loss_latch::GpuFailureKind::SurfaceFailed,
                        "its GPU surface refused blend reconfiguration",
                    );
                }
                ok
            }
            ReacquireReason::ContentResize => {
                let _ = info;
                self.pending_content_size
                    .take()
                    .is_none_or(|next| self.core.apply_content_resize_gpu(self.app, self.event_loop, next))
            }
        }
    }

    fn on_drawable_dropped(&mut self) {
        self.core.note_frame_texture_released(self.app);
    }

    fn on_missed(&mut self, missed: crate::window::Missed, stepped: bool) {
        self.core.acquire_missed(self.app, missed, self.paced, self.prewarm, stepped);
        self.more = false;
    }

    fn sync_after_step(&mut self, info: &mut StepInfo) {
        let now = self.now.unwrap_or_else(crate::time::now);
        info.update_ms = if self.app.has_compositor() { now.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
        if let Some(backdrop) = self.app.rt_mut().host.window_backdrop_request.take() {
            self.core.window_backdrop = backdrop;
            self.core.sync_window_backdrop(self.app.rt_mut(), false);
        }
        self.core.take_window_requests(self.app.rt_mut());
        self.core.clipboard.flush_core(self.app.rt_mut());
        self.core.sync_ime(self.app);
        // Publish the geometry just rendered, never a layout cached from the preceding frame.
        self.core.publish_regions(self.app.rt());
    }

    fn prepare_render(&mut self, drawable: wgpu::SurfaceTexture, _info: &mut StepInfo) -> RenderPrep<PreparedOverlayFrame> {
        let (Some(live), Some(renderer), Some(gpu)) = (
            self.core.surface.live(),
            self.core.renderer.as_mut(),
            self.core.gpu.as_ref(),
        ) else {
            unreachable!("overlay surface, renderer and gpu live across frame");
        };
        let target_format = if self.app.rt().srgb_blend() { live.config.format.remove_srgb_suffix() } else { live.config.format };
        let view = drawable.texture.create_view(&wgpu::TextureViewDescriptor {
            format: (target_format != live.config.format).then_some(target_format),
            ..Default::default()
        });
        let size = (live.config.width, live.config.height);
        prepare_gpu_resources(renderer, gpu, self.app.rt_mut(), size, true);
        let encoder = create_frame_encoder(gpu);
        RenderPrep::Ready(PreparedOverlayFrame { frame: drawable, view, encoder, target_format, size })
    }

    fn render(&mut self, prepared: &mut PreparedOverlayFrame, info: &StepInfo) -> bool {
        let (Some(live), Some(renderer), Some(gpu)) = (
            self.core.surface.live(),
            self.core.renderer.as_mut(),
            self.core.gpu.as_ref(),
        ) else {
            return false;
        };
        let target = live.alpha.as_ref().map_or(&prepared.view, |a| &a.view);
        let comp_animating = if self.app.has_compositor() {
            self.app.render_compositor(OverlayRenderArgs {
                device: &gpu.device,
                queue: &gpu.queue,
                encoder: &mut prepared.encoder,
                target,
                format: prepared.target_format,
                size: prepared.size,
                dt: info.dt,
                update_ms: info.update_ms,
                ui: renderer,
            })
        } else {
            draw_ui(renderer, gpu, &mut prepared.encoder, target, self.app.rt_mut(), prepared.size, [0.0; 4]);
            false
        };
        if let Some(alpha) = &live.alpha {
            alpha.draw(&mut prepared.encoder, &prepared.view);
        }
        if self.core.poll_device_for_terminal_loss(self.app) {
            return false;
        }
        comp_animating
    }

    fn submit_and_present(&mut self, prepared: PreparedOverlayFrame, _info: &StepInfo) -> Instant {
        let PreparedOverlayFrame { frame, view, encoder, .. } = prepared;
        if self.core.poll_device_for_terminal_loss(self.app)
            || self.core.gpu.as_ref().is_none_or(|gpu| gpu.is_lost())
        {
            drop((encoder, view, frame));
            self.core.note_frame_texture_released(self.app);
            return crate::time::now();
        }
        let terminal = self.core.is_terminal_gpu_policy();
        drop(view);
        // Submit synchronously first so any submit-time device loss is detected BEFORE
        // `pre_present_notify()`, `layer.request_frame()`, or `queue.present()`, and so `frame`
        // is dropped strictly before any surface teardown.
        let submitted_on_worker = {
            let (Some(live), Some(renderer), Some(gpu)) = (
                self.core.surface.live_mut(),
                self.core.renderer.as_mut(),
                self.core.gpu.as_ref(),
            ) else {
                drop((encoder, frame));
                self.core.note_frame_texture_released(self.app);
                return crate::time::now();
            };
            frame::try_submit_on_macos_worker(live, renderer, gpu, self.app.rt(), encoder, frame, self.prewarm, terminal, &mut self.core.on_materials)
        };
        let (encoder, frame) = match submitted_on_worker {
            Ok(()) => {
                self.core.note_frame_texture_released(self.app);
                frame::record_presented(self.prewarm);
                crate::render::gpu_events::presented();
                return crate::time::now();
            }
            Err(back) => back,
        };
        {
            let (Some(renderer), Some(gpu)) = (self.core.renderer.as_mut(), self.core.gpu.as_ref()) else {
                drop((encoder, frame));
                self.core.note_frame_texture_released(self.app);
                return crate::time::now();
            };
            renderer.submit(&gpu.queue, encoder);
        }
        let terminal_lost = self.core.poll_device_for_terminal_loss(self.app);
        let gpu_lost = self.core.gpu.as_ref().is_none_or(|gpu| gpu.is_lost());
        if !crate::render::loss_latch::should_present_after_submit(gpu_lost, terminal_lost) {
            drop(frame);
            self.core.note_frame_texture_released(self.app);
            return crate::time::now();
        }
        let (Some(live), Some(renderer), Some(gpu)) = (
            self.core.surface.live_mut(),
            self.core.renderer.as_mut(),
            self.core.gpu.as_ref(),
        ) else {
            drop(frame);
            self.core.note_frame_texture_released(self.app);
            return crate::time::now();
        };
        let materials_ok = frame::present_submitted(live, gpu, self.app.rt(), self.core.window_backdrop, frame, self.prewarm, &mut self.core.on_materials);
        #[cfg(target_os = "windows")]
        live.slot.presented();
        frame::record_presented(self.prewarm);
        crate::render::gpu_events::presented();
        if !self.prewarm && !gpu.is_lost() {
            renderer.warm_effects(&gpu.device);
        }
        // `frame` was consumed by `queue.present`: release texture tracking before post-present poll.
        self.core.note_frame_texture_released(self.app);
        if terminal {
            let _ = self.core.poll_device_for_terminal_loss(self.app);
        }
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        if !materials_ok {
            self.app.rt_mut().paint.set_windows_acrylic_materials(false);
            self.core.dirty = true;
        }
        #[cfg(not(all(target_os = "windows", feature = "windows-system-material")))]
        let _ = materials_ok;
        crate::time::now()
    }

    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        (Some(&mut self.core.clock), self.app.rt_mut())
    }

    fn settle(&mut self, submitted: Instant, info: &StepInfo) {
        if self.core.poll_terminal_gpu_loss(self.app) { self.more = false; return; }
        // If the device was lost during a layer surface's own present, Mesa never committed the
        // buffer or the `wl_surface.frame` callback we just requested, so take the surface down
        // instead of waiting on a callback that will never arrive.
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if matches!(self.core.surface.live().map(|l| &l.backing), Some(Backing::Layer(_)))
            && self.core.gpu.as_ref().is_some_and(|gpu| gpu.is_lost())
        {
            self.core.surface_refused(self.app, "its GPU device was lost".into());
            self.more = false;
            return;
        }
        crate::haptics::flush_live(self.app.rt_mut(), !self.prewarm);
        if !self.prewarm { self.core.meter.presented(); self.core.note_presented(submitted); }
        self.core.dirty = false;

        let Some(live) = self.core.surface.live_mut() else {
            self.more = false;
            return;
        };
        #[cfg(target_os = "macos")]
        let next_tick = live.pacing.as_ref().filter(|p| !self.prewarm && p.paced()).map(|p| submitted + Duration::from_secs_f64(1.0 / p.refresh_hz()));
        #[cfg(not(target_os = "macos"))]
        let next_tick = None;
        self.core.idle = idle_after(self.app.rt(), info.animating, next_tick);
        self.core.runtime_idle = !self.prewarm && idle_after(self.app.rt(), info.runtime_animating, next_tick);
        let surfaces_pending = self.core.renderer.as_ref().is_some_and(crate::render::Renderer::surfaces_pending);
        let rt = self.app.rt();
        self.core.quiet.presented_frame(!self.prewarm && self.core.idle && !self.app.has_compositor() && !surfaces_pending, |slot| {
            crate::window::Presented::record_core(slot, rt, &live.config, false);
        });
        if !self.prewarm { self.core.backoff.step(crate::window::BackoffEvent::Presented); }
        // The display link wakes this overlay, never ControlFlow::Poll. Without one (it can't
        // be made while the displays sleep, or didn't start), a timer paces its motion.
        #[cfg(target_os = "macos")]
        {
            let now = self.now.unwrap_or(submitted);
            let idle = self.core.idle;
            let paced = live.pacing.as_mut().is_some_and(|pacing| {
                pacing.presented(submitted);
                if self.prewarm {
                    pacing.stop();
                } else if idle {
                    pacing.coast();
                }
                pacing.paced()
            });
            if !paced && !self.prewarm && !idle { self.core.backoff.step(crate::window::BackoffEvent::At(now + crate::window::TIMER_FRAME)); }
            // A display link that couldn't be made while the displays slept: they're back.
            if !self.prewarm && live.pacing.is_none() && let Some(wake) = &self.core.frame_wake {
                let Backing::Window(window) = &live.backing;
                live.pacing = crate::platform::display_link::DisplayLink::retry(window, wake.clone());
            }
            if !self.prewarm { self.core.meter.set_pacing(crate::platform::display_link::pacing(live.pacing.as_ref(), !idle)); }
            self.more = false;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = submitted;
            if !self.prewarm { self.core.meter.set_pacing(self.core.pacing_state(!self.core.idle)); }
            self.more = frame::wants_more(self.core.idle, self.core.awaiting_layer_frame());
        }
    }
}

impl OverlayCore {
    /// Draws a frame if one is due; true while it needs more (animating).
    pub(super) fn draw(&mut self, app: &mut dyn OverlayApp, event_loop: Option<&ActiveEventLoop>) -> bool {
        self.presence_due(app, crate::time::now());
        if !self.presence.drawing() {
            self.drain_hidden(app);
            return false;
        }
        let more = self.draw_frame(app, event_loop, false);
        self.open_popup(app);
        more
    }

    /// A layer or XWayland surface kept while hidden (prewarmed, not shown yet) still drains its
    /// connection so nothing queues for the first show.
    fn drain_hidden(&mut self, app: &mut dyn OverlayApp) {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            let closed = match self.surface.live_mut().map(|l| &mut l.backing) {
                Some(Backing::Layer(layer)) => layer.drain().then(|| layer.close_reason().map(str::to_owned)),
                Some(Backing::Xwayland(surface)) => {
                    surface.drain();
                    None
                }
                _ => None,
            };
            if let Some(reason) = closed { self.layer_closed(app, reason.as_deref()); }
        }
        crate::haptics::flush_live(app.rt_mut(), false);
    }

    /// One input event a layer or XWayland surface read on its own connection.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn surface_input(&mut self, app: &mut dyn OverlayApp, input: crate::platform::layer_shell::Input) {
        use crate::platform::layer_shell::Input;
        let _input = (!matches!(input, Input::Focus(_))
            && !(matches!(input, Input::Ime(_)) && !self.options.keys.takes_keys()))
            .then(crate::input::time::arrived_now);
        match input {
            Input::Move(x, y) => {
                self.native_surface_move(app, x, y);
                return;
            }
            Input::Leave => {
                self.native_pointer_leave(app);
                return;
            }
            Input::Button(down) => {
                self.native_button(app, crate::PointerButton::Primary, down);
                return;
            }
            Input::Secondary(down) => {
                self.native_button(app, crate::PointerButton::Secondary, down);
                return;
            }
            Input::ExtraButton(button, down) => {
                self.native_button(app, button, down);
                return;
            }
            Input::Wheel(wheel) => {
                if !self.native_surface_wheel(app, wheel) { return; }
            }
            Input::Focus(focused) => {
                app.window_focus(focused);
                if !focused && self.options.keys.takes_keys() {
                    self.ime.clear();
                    if let Some(Live { backing: Backing::Layer(layer), .. }) = self.surface.live_mut()
                        && let Some(ime) = layer.sync_ime(crate::input::host_events::ImeStep {
                            cancel: false,
                            set_allowed: Some(false),
                            cursor_area: None,
                        })
                    {
                        crate::input::host_events::ime_core(app.rt_mut(), None, ime);
                    }
                }
                app.rt_mut().ignore_minimize("an overlay");
                if matches!(app.rt_mut().shell_request.take(), Some(crate::el::Shell::Close)) {
                    self.hide(app);
                    return;
                }
            }
            Input::Key(key) => {
                if matches!(key.key, crate::Key::Media(_)) && !app.rt().media_keys() { return; }
                app.rt_mut().key(key);
            }
            Input::KeyRepeat(key) => {
                if matches!(key.key, crate::Key::Media(_)) && !app.rt().media_keys() { return; }
                app.rt_mut().repeat_key(key);
            }
            Input::KeyUp(key) => {
                self.dirty |= app.key_up(key);
                return;
            }
            Input::Physical(event, fallback) => {
                let keep = matches!(event, crate::PhysicalInput::Key { pressed: true, .. }) || app.rt().wants_key_up();
                let media_keys = app.rt().media_keys();
                self.dirty |= app.physical_with_fallback(
                    event,
                    fallback.filter(|ev| keep && (media_keys || !matches!(ev.key, crate::Key::Media(_)))),
                );
                return;
            }
            Input::Ime(ime) if self.options.keys.takes_keys() => {
                deliver_layer_ime(app.rt_mut(), &mut self.ime, ime);
            }
            Input::Ime(_) => return,
        }
        self.dirty = true;
    }

    /// Opens the pop-up menu a handler asked for over the overlay's panel. A hidden or occluded
    /// overlay answers `None`, committed with its next frame. One anchored to an element waits
    /// for the frame that commits what changed since the last.
    pub(super) fn open_popup(&mut self, app: &mut dyn OverlayApp) {
        let window = (self.presence.shown() && !self.presence.occluded()).then(|| self.live_window().cloned()).flatten();
        if app.popup_open(&mut self.popup, window.as_ref(), true) { self.dirty = true; }
    }

    /// Syncs OS IME allowance and cursor area for overlays that accept on-demand keyboard focus.
    /// Returns true when the sync queued an immediate IME state event for the runtime.
    fn sync_ime(&mut self, app: &mut dyn OverlayApp) -> bool {
        if !self.options.keys.takes_keys() { return false; }
        let Some(live) = self.surface.live_mut() else { return false };
        match &mut live.backing {
            Backing::Window(window) => {
                #[cfg(target_os = "macos")]
                let focused = self.panel.as_ref().map_or_else(|| window.has_focus(), |p| p.is_key());
                #[cfg(not(target_os = "macos"))]
                let focused = window.has_focus();
                crate::input::host_events::sync_ime(window, app.rt_mut(), &mut self.ime, focused);
                false
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(layer) => {
                let focused = layer.has_keyboard_focus();
                sync_layer_ime(app.rt_mut(), &mut self.ime, focused, |step| layer.sync_ime(step))
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Xwayland(_) => false,
        }
    }

    pub(super) fn draw_frame(&mut self, app: &mut dyn OverlayApp, event_loop: Option<&ActiveEventLoop>, prewarm: bool) -> bool {
        let mut frame = OverlayFrame {
            core: self,
            app,
            event_loop,
            prewarm,
            read_occlusion: false,
            paced: false,
            content_resized: false,
            pending_content_size: None,
            #[cfg(target_os = "macos")]
            target: None,
            #[cfg(any(target_os = "macos", all(target_os = "windows", feature = "windows-system-material")))]
            materials_checked: false,
            #[cfg(target_os = "macos")]
            presume: false,
            now: None,
            more: false,
        };
        host_step::run(&mut frame);
        frame.more
    }

    /// Handles a failed drawable acquisition during `draw_frame`.
    pub(super) fn acquire_missed(&mut self, app: &mut dyn OverlayApp, missed: crate::window::Missed, paced: bool, prewarm: bool, built: bool) {
        if missed == crate::window::Missed::Unconfigured {
            if built {
                self.take_window_requests(app.rt_mut());
                self.clipboard.flush_core(app.rt_mut());
            }
            crate::haptics::flush_live(app.rt_mut(), false);
            self.surface_refused(app, "its GPU surface refused a new configuration".into());
            return;
        }
        if !prewarm {
            #[cfg(target_os = "macos")]
            if missed == crate::window::Missed::Occluded { self.set_occluded(app, true); }
            self.missed(app, missed, paced, built);
        }
    }

    /// A timer frame that would draw what's on screen: what a drawn frame does besides drawing.
    fn unchanged_frame(&mut self, app: &mut dyn OverlayApp) -> bool {
        self.take_window_requests(app.rt_mut());
        self.clipboard.flush_core(app.rt_mut());
        crate::haptics::flush_live(app.rt_mut(), true);
        let ime_queued = self.sync_ime(app);
        // Hit regions paint nothing, so a frame can change only them (a full-screen drawing
        // surface turning on); no presentation follows to commit them.
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.publish_regions(app.rt()) && let Some(Live { backing: Backing::Layer(layer), .. }) = self.surface.live_mut() { layer.commit(); }
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        self.publish_regions(app.rt());
        self.meter.redraw(true);
        self.dirty = ime_queued;
        self.idle = !ime_queued;
        self.runtime_idle = !ime_queued;
        self.meter.set_pacing(self.pacing_state(ime_queued));
        ime_queued
    }

    /// Records whether the shown panel is on screen. Occluded, it stops its display link,
    /// ambient loops and live values; uncovered, it redraws once and resumes them. A
    /// fading-out panel never becomes occluded (`Presence::set_occluded`).
    #[cfg(target_os = "macos")]
    pub(super) fn occluded(&mut self, app: &mut dyn OverlayApp, occluded: bool) {
        if !self.presence.set_occluded(occluded) { return; }
        self.idle = true;
        // What it last presented may not be what shows once uncovered.
        self.quiet.step(frame::QuietEvent::Forget);
        if occluded {
            if let Some(live) = self.surface.live_mut() && let Some(pacing) = &mut live.pacing { pacing.stop(); }
            app.set_live_visible(false);
            self.rest_step(app, crate::window::RestEvent::Unseen, crate::time::now());
        } else if self.presence.drawing() {
            self.dirty = true;
            self.backoff_step(crate::window::BackoffEvent::Revealed);
            app.set_live_visible(true);
            self.ensure_pacing();
        }
    }

    /// Reads the panel's occlusion directly: one shown onto covered or sleeping displays gets
    /// no event saying so. Once a loop turn: the tick's draw and `about_to_wait` share it, and
    /// `Occluded` events land in between.
    #[cfg(target_os = "macos")]
    pub(super) fn sync_occlusion(&mut self, app: &mut dyn OverlayApp) {
        if std::mem::replace(&mut self.occlusion_read, true) { return; }
        let occluded = self.panel.as_ref().is_some_and(|p| p.occluded());
        let occluded = self.occlusion_reading(occluded);
        self.set_occluded(app, occluded);
    }

    /// What a reading of the panel's occlusion means now. Right after a show AppKit may not yet
    /// report the panel on screen, so a primed overlay stays shown and its first frames draw
    /// (`prime`); a visible reading ends that.
    #[cfg(target_os = "macos")]
    pub(super) fn occlusion_reading(&mut self, occluded: bool) -> bool {
        if !occluded { self.prime.stop(); }
        occluded && !self.prime.presumes()
    }

    /// Applies an occlusion reading when it changes what the panel holds.
    #[cfg(target_os = "macos")]
    pub(super) fn set_occluded(&mut self, app: &mut dyn OverlayApp, occluded: bool) {
        if occluded != self.presence.occluded() { self.occluded(app, occluded); }
    }

    /// A display link made while the displays slept fails; try again once they may be back. One
    /// that exists follows the panel to the display it is on now.
    #[cfg(target_os = "macos")]
    pub(super) fn ensure_pacing(&mut self) {
        let (Some(live), Some(wake)) = (self.surface.live_mut(), &self.frame_wake) else { return };
        let Backing::Window(window) = &live.backing;
        match &mut live.pacing {
            Some(pacing) => pacing.update_display(window),
            None => live.pacing = crate::platform::display_link::DisplayLink::new(window, wake.clone()),
        }
    }

    /// Warm glyphs/pipelines and, on macOS, the retained alpha-zero host's actual surface.
    pub(super) fn warm(&mut self, app: &mut dyn OverlayApp) {
        if self.poll_terminal_gpu_loss(app) { return; }
        app.frame(0.0);
        app.rt_mut().settle_all();
        app.frame(0.0);
        self.clock.hold(crate::time::now());
        self.warm_frame(app);
    }

    pub(super) fn warm_frame(&mut self, app: &mut dyn OverlayApp) {
        if self.poll_terminal_gpu_loss(app) { return; }
        self.publish_regions(app.rt());
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            let _prewarm = panel.prewarm();
            self.dirty = true;
            self.draw_frame(app, None, true);
            if let Some(gpu) = &self.gpu { rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok(); }
            let _ = self.poll_terminal_gpu_loss(app);
            return;
        }
        let Some(live) = self.surface.live() else { return };
        self.warm_offscreen(app, (live.config.width, live.config.height));
    }

    pub(super) fn warm_offscreen(&mut self, app: &mut dyn OverlayApp, size: (u32, u32)) {
        if self.poll_device_for_terminal_loss(app) { return; }
        let (enc, _target) = {
            let (Some(renderer), Some(gpu)) = (&mut self.renderer, &self.gpu) else { return };
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("overlay prewarm"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: renderer.format(), usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            render_frame(app, OverlayRenderArgs {
                device: &gpu.device, queue: &gpu.queue, encoder: &mut enc,
                target: &view, format: renderer.format(), size,
                dt: 0.0, update_ms: 0.0, ui: renderer,
            });
            (enc, target)
        };
        // Pre-submit poll so a validation/destroy error inside `render_compositor` aborts before
        // `renderer.submit`, then post-submit wait + poll catches async submission failure.
        if self.poll_device_for_terminal_loss(app) { return; }
        if let (Some(renderer), Some(gpu)) = (&mut self.renderer, &self.gpu) {
            renderer.submit(&gpu.queue, enc);
            rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
        }
        let _ = self.poll_terminal_gpu_loss(app);
    }
}

/// Backing scale (device pixels per window point) to pass to `OverlayCore::rescale` when a
/// layer-shell surface resizes without a scale change. `rescale` sizes the surface buffer and
/// updates the runtime via `RuntimeCore::set_viewport`, both of which take `window_scale()`
/// rather than `env.scale` (`window_scale * zoom`).
#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
fn layer_resize_scale(rt: &RuntimeCore) -> f32 {
    rt.window_scale()
}

/// Routes a layer-shell IME event into the runtime, invalidating `ImeSync`'s cached cursor area
/// on `Enabled`/`Disabled` just like `host_events::ime` does for winit windows.
#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
fn deliver_layer_ime(
    rt: &mut RuntimeCore,
    ime_sync: &mut crate::input::host_events::ImeSync,
    ime: crate::Ime,
) {
    if matches!(ime, crate::Ime::Enabled | crate::Ime::Disabled) {
        ime_sync.invalidate_area();
    }
    crate::input::host_events::ime_core(rt, None, ime);
}

/// Computes the next `ImeStep` for a layer-shell overlay, hands it to `apply`, and forwards any
/// immediate `Ime::Enabled`/`Disabled` transition into `rt`.
#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
fn sync_layer_ime(
    rt: &mut RuntimeCore,
    ime_sync: &mut crate::input::host_events::ImeSync,
    focused: bool,
    apply: impl FnOnce(crate::input::host_events::ImeStep) -> Option<crate::Ime>,
) -> bool {
    let step = crate::input::host_events::ime_sync_step(rt, ime_sync, focused);
    if let Some(ime) = apply(step) {
        crate::input::host_events::ime_core(rt, None, ime);
        return true;
    }
    false
}

#[cfg(test)]
#[path = "draw_tests.rs"]
mod tests;
