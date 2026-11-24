//! [`crate::frame::host_step::FrameHost`] adapter for the offscreen headless driver, plus the
//! shared `Tx::resize_window` frame-animation helpers used by both single-window headless runs
//! and the multi-window virtual desktop.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::app::SharedCore;
use crate::frame::clock::FrameClock;
use crate::frame::host_step::{
    Acquire, FrameHost, Gate, PostStepCheck, ReacquireReason, RenderPrep, StepInfo,
    draw_ui, prepare_gpu_resources,
};
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::runtime::RuntimeCore;
use crate::time::Instant;
use crate::tooling::{
    Args, Chrome, FrameTiming, Shot, Trace, crop, die, loop_check, tree_json_core,
};

use super::app::{HeadlessApp, HeadlessRenderArgs};
use super::report_traffic_light_overlaps;

/// Bound on extra zero-dt layout-callback passes `--settle` runs at the capture frame.
pub(in crate::tooling) const SETTLE_PASSES: usize = 3;

/// A frame readback's destination and, when `--chrome` is active, the resolved macOS chrome on
/// that frame so asynchronous readbacks paint the traffic lights that frame drew with.
#[derive(Clone, Copy)]
pub(super) struct FrameShot {
    pub(super) shot: Shot,
    pub(super) chrome: Option<Chrome>,
}

pub(super) struct PreparedHeadlessFrame {
    encoder: wgpu::CommandEncoder,
    shot: FrameShot,
}

/// Persistent offscreen rendering resources and frame-to-frame state owned by `run_headless`.
pub(super) struct HeadlessLoop<'a> {
    pub(super) launched: Instant,
    pub(super) state_ms: f64,
    pub(super) runtime_ms: f64,
    pub(super) init_ms: f64,
    pub(super) args: &'a Args,
    pub(super) dark: bool,
    pub(super) scale: f32,
    pub(super) size: (f32, f32),
    pub(super) wp: u32,
    pub(super) hp: u32,
    pub(super) limits: ((f32, f32), Option<(f32, f32)>),
    pub(super) has_comp: bool,
    pub(super) has_video: bool,
    pub(super) start_frame: usize,
    pub(super) frames: usize,
    pub(super) snap_frame: usize,
    pub(super) dt: f64,
    pub(super) shared_core: &'a Arc<SharedCore>,
    pub(super) desktop: &'a crate::desktop::displays::VirtualDesktop,
    pub(super) gpu: &'a Gpu,
    pub(super) target_format: wgpu::TextureFormat,
    pub(super) renderer: Renderer,
    pub(super) target: Offscreen,
    pub(super) readback: crate::tooling::record::Readback<FrameShot>,
    pub(super) in_flight: crate::tooling::record::InFlight,
    pub(super) alpha: Option<crate::render::surface_alpha::AlphaSurface>,
    pub(super) overlay: Option<(crate::input::keycaps::Overlay, Renderer)>,
    pub(super) reports: crate::frame::encoded::FrameReports,
    pub(super) trace: Option<Trace>,
    pub(super) gate: Option<crate::motion_gate::Sampler>,
    pub(super) frame_anim: Option<crate::window::frame::FrameAnim>,
    pub(super) idle: bool,
    pub(super) early_place: Option<crate::windows::PlaceRequest>,
    pub(super) placed: Option<Value>,
    pub(super) snap_crop: &'a Cell<Option<[u32; 4]>>,
    pub(super) cpu_boot: &'a RefCell<Option<crate::render::cpu_boot::tool::Tool>>,
    pub(super) loop_check: loop_check::Tally,
    pub(super) cpu_ms: Vec<f64>,
    pub(super) anim_ms: Vec<f64>,
    pub(super) rest_ms: Vec<f64>,
    #[cfg(feature = "frame-profile")]
    pub(super) profile: crate::frame::profile::Log,
}

impl HeadlessLoop<'_> {
    #[inline]
    fn reblend_offscreen(&mut self, rt: &mut RuntimeCore) {
        let rt_srgb = rt.srgb_blend();
        self.target_format = if rt_srgb { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
        crate::window::rebuild_renderer_core(&self.gpu.device, &mut self.renderer, self.target_format, rt);
        self.target = Offscreen::with_format(&self.gpu.device, self.wp, self.hp, self.target_format);
        if self.alpha.is_some() {
            self.alpha = Some(crate::render::surface_alpha::AlphaSurface::new(&self.gpu.device, self.target_format, (self.wp, self.hp)));
        }
        if let Some((ov, ui)) = self.overlay.as_mut() {
            ov.set_srgb_blend(rt_srgb);
            ui.set_format(&self.gpu.device, self.target_format);
        }
    }
}

/// Per-frame borrow view driving one headless iteration through [`crate::frame::host_step::run`].
pub(super) struct HeadlessFrame<'a, 'b, F> {
    pub(super) state: &'a mut HeadlessLoop<'b>,
    pub(super) app: &'a mut dyn HeadlessApp,
    pub(super) deliver: &'a mut F,
    pub(super) k: usize,
    pub(super) real_t: f64,
    pub(super) frame_start: Instant,
    pub(super) script_had_events: bool,
    pub(super) requested_place: Option<crate::windows::PlaceRequest>,
    pub(super) want_pixels: bool,
    pub(super) first_pixels: bool,
    pub(super) encoded_stamp: Option<Instant>,
    #[cfg(feature = "frame-profile")]
    pub(super) fp_alloc0: u64,
    #[cfg(feature = "frame-profile")]
    pub(super) fp_commits0: u64,
    #[cfg(feature = "frame-profile")]
    pub(super) fp_cpu0: Option<crate::frame::profile::ThreadCpuMark>,
    #[cfg(feature = "frame-profile")]
    pub(super) fp_events: usize,
}

impl<F: FnMut(FrameShot, Vec<u8>)> FrameHost for HeadlessFrame<'_, '_, F> {
    type Drawable = ();
    type Prepared = PreparedHeadlessFrame;

    #[inline]
    fn gate(&mut self) -> Gate { Gate::Draw }

    #[inline]
    fn before_acquire(&mut self) {
        let s = &mut *self.state;
        take_window_frame(self.app.rt_mut(), &mut s.frame_anim, self.real_t, s.limits);
        if !s.idle {
            step_window_frame(self.app.rt_mut(), &mut s.frame_anim, self.real_t, s.scale);
        }
        {
            let rt = self.app.rt_mut();
            rt.host.window_resting = s.frame_anim.is_none() && rt.host.window_frame.is_none();
        }
        if let Some(g) = s.gate.as_mut() { self.app.gate_before(g); }
    }

    #[inline]
    fn needs_reblend(&self) -> bool {
        self.state.renderer.srgb_blend() != self.app.rt().srgb_blend()
    }

    #[inline]
    fn reblend(&mut self) -> bool {
        self.state.reblend_offscreen(self.app.rt_mut());
        true
    }

    #[inline]
    fn acquire(&mut self) -> Acquire<()> { Acquire::Ready(()) }

    #[inline]
    fn step_runtime(&mut self) -> StepInfo {
        let s = &mut *self.state;
        let step = if self.k == 0 { 0.0 } else { s.dt };
        // `RUNE_LOOP_CHECK=1`: every frame asks for a loop-only step first, and the step is
        // compared with the ordinary frame that runs instead, or says why it wouldn't run. A
        // window never asks after a script's kind of event (`LoopFrame::touched`); here the
        // step's own refusal shows for those too, counted apart.
        let asked = (self.app.rt().loop_check_on() && self.k > 0 && s.frame_anim.is_none())
            .then(|| self.app.rt_mut().loop_step(step));
        let mut animating = self.app.frame(step);
        if let Some(asked) = asked {
            s.loop_check.add(self.real_t, self.script_had_events, asked, self.app.rt_mut().loop_checked());
        }
        if s.args.settle && self.k == s.snap_frame {
            if let (_, Some(settled)) = self.app.settle_layout_passes(SETTLE_PASSES) {
                animating = settled;
            }
        }
        StepInfo {
            runtime_animating: animating,
            animating,
            dt: step,
            update_ms: 0.0,
            presents_at: None,
        }
    }

    #[inline]
    fn post_step_check(&mut self, _info: &mut StepInfo) -> PostStepCheck {
        if self.state.renderer.srgb_blend() != self.app.rt().srgb_blend() {
            PostStepCheck::Reacquire(ReacquireReason::Reblend)
        } else {
            PostStepCheck::Keep
        }
    }

    #[inline]
    fn prepare_reacquire(&mut self, reason: ReacquireReason, _info: &mut StepInfo) -> bool {
        match reason {
            ReacquireReason::Reblend => {
                // `Tx::set_srgb_blend`: the same bytes (sRGB-encoded) reach the readback either way.
                self.state.reblend_offscreen(self.app.rt_mut());
                true
            }
            #[cfg(any(test, target_os = "macos"))]
            ReacquireReason::NativeMaterials => true,
        }
    }

    #[inline]
    fn sync_after_step(&mut self, info: &mut StepInfo) {
        let s = &mut *self.state;
        self.app.open_popup_headless();
        // As on macOS, a frame change this commit asked for starts with the springs it
        // started, which have shown a step unless they held for the window.
        if self.app.rt().host.window_frame.is_some() {
            let shown = if self.app.rt_mut().host.fit.take_held() { 0.0 } else { info.dt };
            take_window_frame(self.app.rt_mut(), &mut s.frame_anim, self.real_t - shown, s.limits);
        }
        s.idle = !info.runtime_animating && s.frame_anim.is_none();
        // Updates, poll and input handlers place too; the latest request this frame wins.
        if let Some(place) = s.shared_core.take_place().or(self.requested_place.take()).or(s.early_place.take()) {
            let (display, frame) = s.desktop.place(&place, s.size);
            self.app.rt_mut().set_window_origin((frame[0], frame[1]));
            let record = json!({ "t": self.real_t, "display": display, "frame": frame });
            println!("rune-placed {record}");
            s.placed = Some(record);
        }
        info.update_ms = self.frame_start.elapsed().as_secs_f64() * 1000.0;
        if let Some(tool) = s.cpu_boot.borrow_mut().as_mut() {
            tool.frame(self.app.rt_mut(), (s.wp, s.hp), self.k, (info.update_ms * 1e6) as u64, self.k == s.snap_frame);
        }
    }

    #[inline]
    fn prepare_render(&mut self, _drawable: (), _info: &mut StepInfo) -> RenderPrep<PreparedHeadlessFrame> {
        let s = &mut *self.state;
        prepare_gpu_resources(&mut s.renderer, s.gpu, self.app.rt_mut(), (s.wp, s.hp), false);
        let encoder = s.gpu.device.create_command_encoder(&Default::default());
        let in_range = self.k >= s.start_frame && self.k < s.start_frame + s.frames;
        let shot = Shot {
            k: self.k,
            dump: in_range && (s.args.out.is_some() || s.args.strip.is_some()) && (self.k - s.start_frame).is_multiple_of(s.args.every),
            video: in_range && s.has_video,
            snap: self.k == s.snap_frame && s.args.snap.is_some(),
        };
        let first_pixels = s.args.first_pixels && self.k == 0;
        let want_pixels = shot.dump || shot.video || shot.snap || first_pixels;
        let chrome = (want_pixels && s.args.chrome_overlay).then(|| self.app.rt().headless_chrome_current());
        self.want_pixels = want_pixels;
        self.first_pixels = first_pixels;
        RenderPrep::Ready(PreparedHeadlessFrame { encoder, shot: FrameShot { shot, chrome } })
    }

    #[inline]
    fn render(&mut self, prepared: &mut PreparedHeadlessFrame, info: &StepInfo) -> bool {
        let s = &mut *self.state;
        let view = s.alpha.as_ref().map_or(&s.target.view, |a| &a.view);
        let comp_animating = if s.has_comp {
            self.app.render_compositor(HeadlessRenderArgs {
                device: &s.gpu.device,
                queue: &s.gpu.queue,
                encoder: &mut prepared.encoder,
                target: view,
                format: s.target_format,
                size: (s.wp, s.hp),
                dt: info.dt,
                update_ms: info.update_ms,
                ui: &mut s.renderer,
            })
        } else {
            let rt = self.app.rt_mut();
            let clear = rt.clear_color();
            draw_ui(&mut s.renderer, s.gpu, &mut prepared.encoder, view, rt, (s.wp, s.hp), clear);
            false
        };
        if let Some((ov, ui)) = s.overlay.as_mut() {
            let ort = ov.frame(self.real_t, info.dt, self.app.rt().backdrop_dark().unwrap_or(s.dark));
            if !ort.instances.is_empty() {
                ui.set_fade_stops(ort.fade_stops());
                ui.prepare_ui(&s.gpu.device, &s.gpu.queue, &mut ort.text.atlas, &ort.instances, (s.wp, s.hp));
                ui.draw_over(&mut prepared.encoder, view);
            }
        }
        if let Some(alpha) = &s.alpha {
            alpha.draw(&mut prepared.encoder, &s.target.view);
        }
        if prepared.shot.shot.snap {
            s.snap_crop.set(crop::snap_rect(self.app.rt(), s.args, s.scale, (s.wp, s.hp)));
        }
        if self.want_pixels {
            for (shot, px) in s.readback.make_room(&s.gpu.device) { (self.deliver)(shot, px); }
            s.readback.copy(&mut prepared.encoder, &s.target, prepared.shot);
        }
        comp_animating
    }

    #[inline]
    fn submit_and_present(&mut self, prepared: PreparedHeadlessFrame, _info: &StepInfo) -> Instant {
        let s = &mut *self.state;
        let submission = s.renderer.submit(&s.gpu.queue, prepared.encoder);
        s.in_flight.push(submission);
        self.encoded_stamp = s.reports.stamp();
        if self.want_pixels { s.readback.submitted(); }
        crate::time::now()
    }

    #[inline]
    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        (None, self.app.rt_mut())
    }

    #[inline]
    fn settle(&mut self, _submitted: Instant, info: &StepInfo) {
        let s = &mut *self.state;
        s.reports.drawn(self.encoded_stamp, None, self.frame_start, info.update_ms);
        let ms = self.frame_start.elapsed().as_secs_f64() * 1000.0;
        #[cfg(feature = "frame-profile")]
        {
            // Convert the frame's raw CPU mark before taking phase deltas so that once Windows'
            // cycle rate is known here, `take()` sees the cached rate for every phase too.
            let cpu_ms = frame_cpu_ms(self.fp_cpu0, crate::frame::profile::thread_cpu_since);
            let phases = crate::frame::profile::take();
            let rt = self.app.rt_mut();
            s.profile.push(crate::frame::profile::Row {
                phases,
                update_ms: info.update_ms,
                encode_ms: ms - info.update_ms,
                events: self.fp_events,
                allocations: procwatch::alloc::thread_allocations() - self.fp_alloc0,
                commits: rt.stats.commits - self.fp_commits0,
                why: rt.commit.take_commit_why(),
                shaped: rt.stats.shaped,
                animating: info.animating,
                cpu_ms,
            });
        }
        s.cpu_ms.push(ms);
        if self.k > 0 {
            if info.animating { s.anim_ms.push(ms) } else { s.rest_ms.push(ms) }
        }
        if s.has_comp {
            // Pace like a display link so a compositor's async readbacks (GPU timers) land between frames.
            rune_gpu::health::poll(&s.gpu.device, wgpu::PollType::wait_indefinitely()).ok();
            s.in_flight.clear();
        }
        if let Some(tr) = s.trace.as_mut() {
            tr.frame_timed_core(self.app.rt_mut(), self.real_t * 1000.0, ms, FrameTiming::default());
        }
        if let Some(g) = s.gate.as_mut() { self.app.gate_frame(g, info.animating); }
        if self.first_pixels {
            let wait = crate::time::now();
            let done = s.readback.finish(&s.gpu.device);
            s.in_flight.clear();
            let marker = json!({
                "headless_ms": s.launched.elapsed().as_secs_f64() * 1000.0,
                "state_ms": s.state_ms, "runtime_ms": s.runtime_ms, "gpu_init_ms": s.init_ms,
                "frame_cpu_ms": ms, "readback_ms": wait.elapsed().as_secs_f64() * 1000.0,
                "pixel_bytes": done.last().map_or(0, |(_, px)| px.len()),
            });
            println!("rune-first-pixels {marker}");
            std::io::stdout().flush().unwrap();
            for (shot, px) in done { (self.deliver)(shot, px); }
        } else if self.want_pixels {
            for (shot, px) in s.readback.ready(&s.gpu.device) { (self.deliver)(shot, px); }
        }
        s.in_flight.pace(&s.gpu.device);
        if self.k == s.snap_frame
            && s.args.chrome_overlay
            && let Some(lights) = self.app.rt().headless_chrome_current().traffic_light_rects_in(s.size.0, crate::labels::rtl())
        {
            report_traffic_light_overlaps(self.app.rt(), &lights);
        }
        if self.k == s.snap_frame
            && let Some(p) = &s.args.tree
        {
            let mut tree = tree_json_core(self.app.rt(), self.real_t);
            if let Some(placed) = &s.placed { tree["placed"] = placed.clone(); }
            let text = serde_json::to_string_pretty(&tree).unwrap();
            std::fs::write(p, text).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
        }
    }
}

/// Takes the size handlers asked for a headless window, inside the limits the app set; its
/// viewport moves there on the virtual clock from `start`. Centring has nothing to centre on.
pub(in crate::tooling) fn take_window_frame(
    rt: &mut RuntimeCore,
    anim: &mut Option<crate::window::frame::FrameAnim>,
    start: f64,
    (min, max): ((f32, f32), Option<(f32, f32)>),
) {
    use crate::window::frame::{FrameAnim, clamp_size};
    rt.host.fit.take_held();
    let Some(request) = rt.host.window_frame.take() else { return };
    let Some(size) = request.size else { return };
    let (w, h) = clamp_size(size, min, max);
    let from = [0.0, 0.0, rt.window_viewport().0, rt.window_viewport().1];
    *anim = Some(FrameAnim::retarget(anim.as_ref(), request.spring, start, from, [0.0, 0.0, w, h]));
}

#[cfg(feature = "frame-profile")]
fn frame_cpu_ms(
    start: Option<crate::frame::profile::ThreadCpuMark>,
    since: impl FnOnce(crate::frame::profile::ThreadCpuMark) -> Option<f64>,
) -> Option<f64> {
    start.and_then(since).map(|ns| ns / 1e6)
}

/// Moves a headless window's viewport one frame along, before the frame lays out.
pub(in crate::tooling) fn step_window_frame(
    rt: &mut RuntimeCore,
    anim: &mut Option<crate::window::frame::FrameAnim>,
    now: f64,
    scale: f32,
) {
    let Some(moving) = *anim else { return };
    let [_, _, w, h] = crate::window::frame::snap(moving.sample(now).0, scale);
    rt.step_viewport((w, h), scale);
    if moving.done(now) { *anim = None; }
}

#[cfg(all(test, feature = "frame-profile"))]
#[path = "frame_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "settle_tests.rs"]
mod settle_tests;
