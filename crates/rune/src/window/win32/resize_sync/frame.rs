//! Drag-frame rendering, shared-texture presentation, and presented-frame telemetry.
//!
//! Keeps the drag-only resize path in parity with `WindowFrame` + `host_step::run` for every
//! presented-frame consumer (`RUNE_FRAME_LOG` / `FrameProfile`, `SurfaceMeter`, `FrameReports`,
//! `FrameClock::presented_core`, `RuntimeCore::interaction_presented`, `Trace`,
//! `App::on_frame_drawn`, `PaintKeep` / `loop_checked`, `Renderer::damage_drawn` + `Damage`,
//! `Rest`, `frame_presented`, `gpu_events::presented`, `haptics::flush_live`, and
//! `Renderer::warm_effects`) behind [`DragSeam`] so every consumer can be tested off screen.

use super::DragResizeSync;
use super::gpu::DragSeam;
use super::present::Win32DragSeam;
use crate::frame::clock::FrameClock;
use crate::frame::encoded::FrameReports;
use crate::frame::host_step::{draw_ui, prepare_gpu_resources_with};
use crate::frame::stage::{FrameStage, HostObservability, StageGuard};
use crate::frame::surface_stats::{Pacing, SurfaceMeter};
use crate::render::{Gpu, Renderer};
use crate::time::Instant;
use crate::tooling::Trace;
use crate::window::chrome::sync_platform_core;
use crate::window::host::draw::display_hz;
use crate::window::host::render::{FrameProfile, open_trace_if_needed};
use crate::window::host::stepping::FrameMove;
use crate::window::host::{CompositorFrame, Host, WindowApp};
use crate::window::state::{Rest, RestEvent, rest};
use crate::window::surface::BackoffEvent;
use crate::window::surface::damage::Damage;

/// Window-neutral presented-frame consumers driven by [`run_drag_frame`].
///
/// Borrows the exact fields `Host` owns (or test-constructed equivalents in unit tests) without
/// requiring a Win32 `HWND` or `winit::window::Window`.
pub(super) struct DragFrameConsumers<'a> {
    pub app: &'a mut dyn WindowApp,
    pub clock: &'a mut FrameClock,
    pub meter: &'a mut SurfaceMeter,
    pub frame_reports: &'a mut FrameReports,
    pub observability: Option<&'a HostObservability>,
    pub rest: &'a mut Rest,
    pub damage: &'a mut Damage,
    pub trace: &'a mut Option<Trace>,
    pub trace_start: Option<Instant>,
    pub trace_frames: &'a mut usize,
    pub trace_limit: Option<usize>,
    pub frame_moving: bool,
    pub idle: &'a mut bool,
    pub refresh_hz: Option<f64>,
    pub paced_for_stats: bool,
    pub next_tick: Option<Instant>,
    pub base_format: wgpu::TextureFormat,
}

/// Result of one [`run_drag_frame`] pass.
pub(super) struct DragFrameOutcome {
    pub animating: bool,
    pub trace_finished: bool,
}

/// Runs one drag-sync frame against `seam` and feeds every presented-frame consumer.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_drag_frame<S: DragSeam>(
    sync: &mut DragResizeSync,
    gpu: &Gpu,
    renderer: &mut Renderer,
    consumers: &mut DragFrameConsumers<'_>,
    seam: &mut S,
    size: (u32, u32),
    detach_dcomp: bool,
    pre_present: impl FnOnce(),
) -> Result<DragFrameOutcome, String> {
    if gpu.is_lost() {
        return Err("GPU device lost during drag".into());
    }
    let _stage_scope = consumers.observability.map(|o| o.scope_stages(consumers.frame_reports.next_seq()));
    let mut redraw_stage = StageGuard::enter(FrameStage::Redraw);
    let mut profile = FrameProfile::default();
    profile.mark_entry();
    profile.set_loop_why("drag resize sync");
    profile.set_whole_why("drag resize sync");
    consumers.meter.redraw(consumers.paced_for_stats);
    if consumers.frame_reports.wanted() {
        consumers.frame_reports.set_refresh(consumers.refresh_hz);
    }

    let now = crate::time::now();
    let srgb_blend = consumers.app.rt().srgb_blend();
    let target_format = if srgb_blend {
        consumers.base_format.remove_srgb_suffix()
    } else {
        consumers.base_format
    };
    let exact = consumers.app.has_compositor();
    profile.mark_start();
    let (slot_idx, slot_release) = {
        let _acq = StageGuard::enter(FrameStage::Acquire);
        let (_drag, slot_idx, slot) =
            sync.ensure_gpu_target(&gpu.device, size, target_format, exact)?;
        (slot_idx, slot.last_release)
    };
    profile.mark_acquired();

    let (mut animating, dt) = {
        let _rt = StageGuard::enter(FrameStage::Runtime);
        let compositing = consumers.app.compositor_animating();
        consumers.app.rt_mut().host.window_resting = false;
        consumers.app.clock_advance(consumers.clock, now, compositing)
    };
    profile.take_loop_check(consumers.app.rt_mut());
    let update_ms = now.elapsed().as_secs_f64() * 1000.0;
    profile.mark_built();

    let srgb_after = consumers.app.rt().srgb_blend();
    let target_format = if srgb_after {
        consumers.base_format.remove_srgb_suffix()
    } else {
        consumers.base_format
    };
    if renderer.format() != target_format {
        renderer
            .try_set_format(&gpu.device, target_format)
            .map_err(|e| format!("drag target format switch: {e}"))?;
        renderer.set_srgb_blend(srgb_after);
    }
    let (slot_idx, slot_release) = if srgb_after != srgb_blend {
        let (_drag, idx, slot) =
            sync.ensure_gpu_target(&gpu.device, size, target_format, exact)?;
        (idx, slot.last_release)
    } else {
        (slot_idx, slot_release)
    };

    {
        let _up = StageGuard::enter(FrameStage::Upload);
        let rt = consumers.app.rt_mut();
        prepare_gpu_resources_with(renderer, gpu, rt, size, true, |renderer, _rt| {
            renderer.set_punched_surfaces(Vec::new());
        });
        renderer.next_instances(rt.paint_count(), None);
    }
    profile.mark_uploaded();

    let (drag, target) = sync.borrow_slot(slot_idx)?;
    drag.wait_before_d3d12(&gpu.queue, slot_release)?;
    let mut encode_stage = StageGuard::enter(FrameStage::Encode);
    let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("rune drag resize frame"),
    });
    let comp_animating = if consumers.app.has_compositor() {
        consumers.app.render_compositor(CompositorFrame {
            device: &gpu.device,
            queue: &gpu.queue,
            encoder: &mut encoder,
            target: &target.view,
            format: target_format,
            size,
            dt,
            update_ms,
            ui: &mut *renderer,
        })
    } else {
        let rt = consumers.app.rt_mut();
        let clear = rt.clear_color();
        draw_ui(renderer, gpu, &mut encoder, &target.view, rt, size, clear);
        false
    };
    animating |= comp_animating;
    // Always settle renderer damage state so any retained partial-damage buffer is invalidated
    // and `pixel_check` stays in sync; drag frames always draw whole so `Err("not armed")` is expected.
    let _ = renderer.damage_drawn().ok();
    target.transition_for_d3d11(&mut encoder);
    encode_stage.end();
    profile.mark_encoded();

    let ready_value = {
        let _submit = StageGuard::enter(FrameStage::Submit);
        renderer.submit(&gpu.queue, encoder);
        drag.signal_after_d3d12(&gpu.queue)?
    };
    profile.mark_submitted();
    let encoded = consumers.frame_reports.stamp();
    let release_value = {
        let _present = StageGuard::enter(FrameStage::Present);
        pre_present();
        seam.present_shared(drag, target, size, ready_value)?
    };
    let present_returned = consumers.frame_reports.stamp();
    sync.note_slot_release(slot_idx, release_value);
    if detach_dcomp {
        seam.detach_dcomp()?;
        seam.set_valid_rects_active(true);
    }
    // Reclaim finished wgpu command allocators and staging buffers continuously during drags
    // without blocking the UI thread; any device loss surfaces via `gpu.is_lost()`.
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();

    let submitted = crate::time::now();
    crate::window::frame_presented();
    crate::render::gpu_events::presented();
    renderer.warm_effects(&gpu.device);
    #[cfg(feature = "frame-profile")]
    profile.build_frame_core(consumers.app.rt_mut(), consumers.damage.seen, animating, None, None);

    consumers.clock.presented_core(consumers.app.rt_mut(), submitted);
    consumers.app.rt_mut().interaction_presented(submitted, None);
    crate::haptics::flush_live(consumers.app.rt_mut(), true);
    consumers.meter.presented();
    redraw_stage.end();
    let _ = consumers.frame_reports.drawn_with_present(encoded, present_returned, None, now, update_ms, false);
    consumers.app.inspect_frame();

    let mut trace_finished = false;
    if let Some(trace) = consumers.trace.as_mut() {
        let start = consumers.trace_start.unwrap_or(now);
        let continuous = !*consumers.idle || consumers.trace_limit.is_some();
        let timing = crate::tooling::FrameTiming {
            target_ms: None,
            drawable: None,
            continuous,
            hz: consumers.refresh_hz,
        };
        trace.frame_timed_core(
            consumers.app.rt_mut(),
            submitted.saturating_duration_since(start).as_secs_f64() * 1000.0,
            now.elapsed().as_secs_f64() * 1000.0,
            timing,
        );
        *consumers.trace_frames += 1;
        if consumers.trace_limit.is_some_and(|n| *consumers.trace_frames >= n) {
            trace.settle_presents(crate::frame::present_time::SETTLE);
            trace_finished = true;
        }
    }

    consumers.damage.reset();
    let _ = rest::step(consumers.rest, RestEvent::Drew { now, low: rest::low_memory() });
    *consumers.idle = !animating && !consumers.frame_moving;
    consumers.meter.set_pacing(if *consumers.idle { Pacing::Idle } else { Pacing::Vsync });
    profile.flush_core(consumers.app.rt(), consumers.frame_moving, animating, consumers.next_tick);

    Ok(DragFrameOutcome { animating, trace_finished })
}

impl Host {
    /// Renders and presents one drag-sync frame for the live window, feeding all presented-frame
    /// consumers identically to the normal `WindowFrame` path.
    pub(super) fn render_and_present_drag(
        &mut self,
        size: (u32, u32),
        detach_dcomp: bool,
    ) -> Result<(), String> {
        let now = crate::time::now();
        open_trace_if_needed(self, now);
        let (Some(l), Some((gpu, renderer))) = (self.win.as_mut(), self.gpu.as_mut()) else {
            return Err("window or GPU missing during drag".into());
        };
        let Some(hwnd) = crate::platform::win32::hwnd(&l.window) else {
            return Err("window has no Win32 HWND".into());
        };
        let refresh_hz = display_hz(l);
        let paced_for_stats = l.paced_for_stats();
        let next_tick = l.next_tick(now);
        let base_format = l.config.format;
        let frame_moving = FrameMove::moving(&self.frame_move);
        let trace_limit = self.args.frames;
        let trace_start = self.trace_start;

        l.timer_frame.drawn();
        l.backoff_step(BackoffEvent::Lapsed);
        l.backoff_step(BackoffEvent::Presented);

        let mut swap_chain = l.native.resize_sync.swap_chain.take();
        let mut dcomp_detached = l.native.resize_sync.dcomp_detached;
        let mut sync = std::mem::take(&mut l.native.resize_sync);
        let mut damage = std::mem::take(&mut l.damage);
        let mut seam = Win32DragSeam {
            hwnd,
            handover: l.native.mode.handover(),
            swap_chain: &mut swap_chain,
            dcomp_detached: &mut dcomp_detached,
        };
        let window = l.window.clone();
        let result = {
            let mut consumers = DragFrameConsumers {
                app: &mut *self.app,
                clock: &mut self.clock,
                meter: &mut self.meter,
                frame_reports: &mut self.frame_reports,
                observability: Some(&self.observability),
                rest: &mut self.rest,
                damage: &mut damage,
                trace: &mut self.trace,
                trace_start,
                trace_frames: &mut self.trace_frames,
                trace_limit,
                frame_moving,
                idle: &mut self.idle,
                refresh_hz,
                paced_for_stats,
                next_tick,
                base_format,
            };
            run_drag_frame(
                &mut sync,
                gpu,
                renderer,
                &mut consumers,
                &mut seam,
                size,
                detach_dcomp,
                || window.pre_present_notify(),
            )
        };
        sync.swap_chain = swap_chain;
        sync.dcomp_detached = dcomp_detached;
        l.native.resize_sync = sync;
        l.damage = damage;
        l.presented = None;
        sync_platform_core(
            l,
            self.app.rt_mut(),
            &mut self.secure_input,
            &mut self.ime,
            self.ime_waits,
            &self.clipboard,
        );

        let outcome = result?;
        if outcome.trace_finished {
            self.life.finish();
            return Ok(());
        }
        if outcome.animating || (self.trace.is_some() && self.args.frames.is_some()) {
            l.request_redraw();
        }
        Ok(())
    }
}
