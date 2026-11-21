//! GPU resource preparation, Windows partial damage planning, compositor/UI encoding,
//! presentation and `frame-profile` live-log stamping for [`super::Host`].

#[cfg(any(target_os = "macos", feature = "frame-profile"))]
use super::stepping::FrameMove;
use super::{CompositorFrame, Host};
use crate::frame::host_step::{RenderPrep, StepInfo, draw_ui, prepare_gpu_resources_with};
use crate::time::Instant;
#[cfg(target_os = "windows")]
use crate::window::win32::{damage, handover};

/// First frame encoded mark (`crate::marks`).
static FIRST_ENCODED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) struct PreparedWindowFrame {
    pub(super) frame: wgpu::SurfaceTexture,
    pub(super) view: wgpu::TextureView,
    pub(super) encoder: wgpu::CommandEncoder,
    pub(super) target_format: wgpu::TextureFormat,
    pub(super) size: (u32, u32),
    #[cfg(target_os = "windows")]
    pub(super) part: Option<Vec<[i32; 4]>>,
    #[cfg(target_os = "windows")]
    pub(super) present_rect: Option<Vec<[i32; 4]>>,
}

/// Optional `frame-profile` timestamps and diagnostics carried across one window frame.
#[derive(Default)]
pub(super) struct FrameProfile {
    #[cfg(feature = "frame-profile")]
    pub(super) loop_why: Option<&'static str>,
    #[cfg(feature = "frame-profile")]
    pub(super) whole_why: Option<&'static str>,
    #[cfg(feature = "frame-profile")]
    pub(super) loop_check: Option<(usize, f32)>,
    #[cfg(feature = "frame-profile")]
    pub(super) forced_miss_checked: bool,
    #[cfg(feature = "frame-profile")]
    pub(super) entry: Option<crate::frame::live_log::Stamp>,
    #[cfg(feature = "frame-profile")]
    pub(super) start: Option<crate::frame::live_log::Stamp>,
    #[cfg(feature = "frame-profile")]
    pub(super) acquired: Option<crate::frame::live_log::Stamp>,
    #[cfg(feature = "frame-profile")]
    pub(super) built: Option<crate::frame::live_log::Stamp>,
    #[cfg(feature = "frame-profile")]
    pub(super) uploaded: Option<crate::frame::live_log::Stamp>,
    #[cfg(feature = "frame-profile")]
    pub(super) encoded: Option<crate::frame::live_log::Stamp>,
    #[cfg(feature = "frame-profile")]
    pub(super) submitted: Option<crate::frame::live_log::Stamp>,
    #[cfg(feature = "frame-profile")]
    pub(super) frame: Option<crate::frame::live_log::Frame>,
}

impl FrameProfile {
    #[inline]
    pub(super) fn mark_entry(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.entry = crate::frame::live_log::on().then(crate::frame::live_log::Stamp::now); }
    }

    #[inline]
    pub(super) fn mark_start(&mut self) {
        #[cfg(feature = "frame-profile")]
        if self.start.is_none() {
            self.start = crate::frame::live_log::on().then(crate::frame::live_log::Stamp::now);
        }
    }

    #[inline]
    pub(super) fn mark_acquired(&mut self) {
        #[cfg(feature = "frame-profile")]
        if self.acquired.is_none() {
            self.acquired = self.start.map(|_| crate::frame::live_log::Stamp::now());
        }
    }

    #[inline]
    pub(super) fn mark_built(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.built = self.start.map(|_| crate::frame::live_log::Stamp::now()); }
    }

    #[inline]
    pub(super) fn mark_uploaded(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.uploaded = self.start.map(|_| crate::frame::live_log::Stamp::now()); }
    }

    #[inline]
    pub(super) fn mark_encoded(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.encoded = self.start.map(|_| crate::frame::live_log::Stamp::now()); }
    }

    #[inline]
    pub(super) fn mark_submitted(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.submitted = self.start.map(|_| crate::frame::live_log::Stamp::now()); }
    }

    #[inline]
    pub(super) fn set_loop_why(&mut self, why: &'static str) {
        #[cfg(feature = "frame-profile")]
        { self.loop_why = Some(why); }
        #[cfg(not(feature = "frame-profile"))]
        let _ = why;
    }

    #[inline]
    pub(super) fn set_loop_why_if_none(&mut self, why: &'static str) {
        #[cfg(feature = "frame-profile")]
        if self.loop_why.is_none() { self.loop_why = Some(why); }
        #[cfg(not(feature = "frame-profile"))]
        let _ = why;
    }

    #[inline]
    pub(super) fn set_whole_why(&mut self, why: &'static str) {
        #[cfg(feature = "frame-profile")]
        { self.whole_why = Some(why); }
        #[cfg(not(feature = "frame-profile"))]
        let _ = why;
    }

    #[inline]
    pub(super) fn take_loop_check(&mut self, rt: &mut crate::runtime::RuntimeCore) {
        #[cfg(feature = "frame-profile")]
        { self.loop_check = rt.loop_checked(); }
        #[cfg(not(feature = "frame-profile"))]
        { let _ = rt.loop_checked(); }
    }

    #[cfg(feature = "frame-profile")]
    pub(super) fn build_frame(
        &mut self,
        host: &mut Host,
        animating: bool,
        looped: Option<&Vec<u32>>,
        #[cfg(target_os = "windows")]
        damage: Option<Vec<[i32; 4]>>,
    ) {
        self.frame = match (self.start, self.acquired, self.built, self.uploaded, self.encoded, self.submitted) {
            (Some(start), Some(acquired), Some(built), Some(uploaded), Some(encoded), Some(submitted)) => {
                let rt = host.app.rt_mut();
                Some(crate::frame::live_log::Frame {
                    entry: self.entry.unwrap_or(start),
                    start, acquired, built, uploaded, encoded, submitted,
                    presented: crate::frame::live_log::Stamp::now(),
                    phases: crate::frame::profile::take(),
                    nodes: rt.tree.nodes.len(),
                    shaped: rt.stats.shaped,
                    instances: rt.instances.len(),
                    animating,
                    why: rt.commit.take_commit_why(),
                    next: 0,
                    poll_due: None,
                    loop_due: None,
                    loops: Vec::new(),
                    #[cfg(target_os = "windows")]
                    damage,
                    #[cfg(not(target_os = "windows"))]
                    damage: None,
                    whole: self.whole_why,
                    #[cfg(target_os = "windows")]
                    change: host.win.as_ref().and_then(|l| l.native.damage.seen),
                    #[cfg(not(target_os = "windows"))]
                    change: None,
                    looped: looped.map(Vec::len),
                    loop_why: self.loop_why,
                    loop_check: self.loop_check.take(),
                })
            }
            _ => None,
        };
    }

    #[inline]
    pub(super) fn flush(&mut self, host: &Host, animating: bool, next_tick: Option<Instant>) {
        #[cfg(feature = "frame-profile")]
        if let Some(mut f) = self.frame.take() {
            let rt = host.app.rt();
            f.next = [
                animating,
                FrameMove::moving(&host.frame_move),
                rt.needs_commit(),
                rt.stale() && !rt.needs_commit(),
                rt.has_pending_input(),
                rt.live_pending(),
                rt.host.window_frame.is_some(),
                crate::window::surface::timer_wants_frame_core(rt, next_tick),
            ]
            .iter()
            .enumerate()
            .fold(0u16, |bits, (i, on)| bits | (u16::from(*on) << i));
            f.poll_due = rt.poll_wake_deadline().map(|d| d - rt.polled_at());
            f.loop_due = rt.motion_wake_deadline().map(|d| d - rt.polled_at());
            if f.loop_due.is_some() { f.loops = rt.fp_loops(); }
            crate::frame::live_log::frame(f);
        }
        #[cfg(not(feature = "frame-profile"))]
        let _ = (host, animating, next_tick);
    }
}

pub(super) fn prepare_window_render(
    host: &mut Host,
    drawable: wgpu::SurfaceTexture,
    corners: f32,
    looped: Option<&Vec<u32>>,
    profile: &mut FrameProfile,
    info: &mut StepInfo,
) -> RenderPrep<PreparedWindowFrame> {
    let (Some(l), Some((gpu, renderer))) = (host.win.as_mut(), host.gpu.as_mut()) else {
        unreachable!("window and gpu live across frame");
    };
    let target_format = if host.app.rt().srgb_blend() { l.config.format.remove_srgb_suffix() } else { l.config.format };
    let view = drawable.texture.create_view(&wgpu::TextureViewDescriptor {
        format: (target_format != l.config.format).then_some(target_format),
        ..Default::default()
    });
    let size = (l.config.width, l.config.height);
    // A client-side frame's rounded bottom corners: the content outside them is cleared off
    // this drawable, right after everything else that draws there.
    if corners > 0. {
        crate::window::surface::corners::prepare(&mut l.corners, &gpu.device, &gpu.queue, size, target_format, corners);
    }
    // An image's pixels changed where its instances didn't: the whole frame draws (`damage`).
    #[cfg(target_os = "windows")]
    let images_changed = host.app.rt().images.pages.iter().any(|p| p.dirty.is_some());
    {
        let rt = host.app.rt_mut();
        #[cfg(target_os = "windows")]
        let mut layers_again = false;
        prepare_gpu_resources_with(renderer, gpu, rt, size, true, |renderer, rt| {
            // Playing clips on their own layer (`video_layer`): the surfaces this frame punches out.
            #[cfg(target_os = "windows")]
            {
                let premultiplied = l.config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied;
                let punch_ready = renderer.surface_punch_ready();
                let native = &mut l.native;
                let layers = native.video_layers.frame(&rt.surfaces, native.mode.handover(), premultiplied, punch_ready);
                renderer.set_punched_surfaces(layers.punched);
                layers_again = layers.again;
            }
            #[cfg(not(target_os = "windows"))]
            let _ = (renderer, rt);
        });
        #[cfg(target_os = "windows")]
        { info.animating |= layers_again; }
        #[cfg(not(target_os = "windows"))]
        let _ = info;
        // The paint the instances come from, and on a loop-only frame the only ones it changed.
        renderer.next_instances(rt.paint_count(), looped.cloned());
    }
    profile.mark_uploaded();
    let encoder = gpu.device.create_command_encoder(&Default::default());
    // Only what changed is redrawn and presented (`damage`): the renderer's `draw` takes the
    // armed rectangles when nothing but the instances draws there, a compositor's
    // `Frame::draw_ui` included. A loop-only frame knows what changed.
    #[cfg(target_os = "windows")]
    let part = {
        let rt = host.app.rt();
        let whole = if l.alpha.is_some() { Some("alpha") } else if images_changed { Some("image") }
            else if !l.config.usage.contains(wgpu::TextureUsages::COPY_DST) { Some("no copies") } else { None };
        // A new swap chain (`rebind_alpha`, a replaced surface, any configure: a resize, a blend
        // switch, `acquire` retrying an outdated surface) draws its first frame whole, through
        // `plan`, even when only a loop asked for it (the step has run; the frame draws it whole).
        let swap = l.surface.as_ref().map_or(0, crate::window::surface::configure::generation);
        let fresh = !l.native.damage.current(size, rt.clear_color(), swap);
        if fresh && looped.is_some() { profile.set_loop_why("a new swap chain"); }
        let armed = looped.map(Vec::as_slice).filter(|_| whole.is_none() && !fresh).and_then(|p| {
            match renderer.arm_loop(&gpu.device, &gpu.queue, rt.paint_count(), &rt.instances, p, rt.clear_color(), size, l.config.format) {
                Ok(rects) => Some((p, rects)),
                Err(why) => { profile.set_loop_why(why); None }
            }
        });
        if let Some((p, present)) = armed {
            l.native.damage.patched(&rt.instances, p);
            Some(present)
        } else {
            match l.native.damage.plan(&rt.instances, size, rt.clear_color(), swap, whole.is_none()) {
                damage::Plan::All(why) => { profile.set_whole_why(whole.unwrap_or(why)); None }
                damage::Plan::Part { draw, present } => {
                    renderer.arm_damage(&gpu.device, &gpu.queue, &draw, rt.clear_color(), size, l.config.format);
                    Some(present)
                }
            }
        }
    };
    RenderPrep::Ready(PreparedWindowFrame {
        frame: drawable,
        view,
        encoder,
        target_format,
        size,
        #[cfg(target_os = "windows")]
        part,
        #[cfg(target_os = "windows")]
        present_rect: None,
    })
}

pub(super) fn render_window_frame(
    host: &mut Host,
    prepared: &mut PreparedWindowFrame,
    corners: f32,
    profile: &mut FrameProfile,
    info: &StepInfo,
) -> bool {
    let (Some(l), Some((gpu, renderer))) = (host.win.as_mut(), host.gpu.as_mut()) else { return false };
    let target = l.alpha.as_ref().map_or(&prepared.view, |a| &a.view);
    let comp_animating = if host.app.has_compositor() {
        host.app.render_compositor(CompositorFrame {
            device: &gpu.device,
            queue: &gpu.queue,
            encoder: &mut prepared.encoder,
            target,
            format: prepared.target_format,
            size: prepared.size,
            dt: info.dt,
            update_ms: info.update_ms,
            ui: &mut *renderer,
        })
    } else {
        let rt = host.app.rt_mut();
        let clear = rt.clear_color();
        draw_ui(renderer, gpu, &mut prepared.encoder, target, rt, prepared.size, clear);
        false
    };
    // Every frame, so a frame drawn whole leaves the renderer's kept frame behind.
    #[cfg(target_os = "windows")]
    match (renderer.damage_drawn(), prepared.part.take()) {
        (Ok(()), part) => prepared.present_rect = part,
        (Err(why), Some(_)) => profile.set_whole_why(why),
        (Err(_), None) => {}
    }
    // A client-side frame's rounded bottom corners: faded off the premultiplied composition,
    // before a surface that takes straight alpha converts it.
    crate::window::surface::corners::draw(l.corners.as_ref(), &mut prepared.encoder, target, corners);
    if let Some(alpha) = &l.alpha { alpha.draw(&mut prepared.encoder, &prepared.view); }
    profile.mark_encoded();
    comp_animating
}

#[allow(unsafe_code)]
pub(super) fn submit_and_present_window(
    host: &mut Host,
    prepared: PreparedWindowFrame,
    looped: Option<&Vec<u32>>,
    profile: &mut FrameProfile,
    encoded: &mut Option<Instant>,
    info: &StepInfo,
) -> Instant {
    let (Some(l), Some((gpu, renderer))) = (host.win.as_mut(), host.gpu.as_mut()) else {
        return crate::time::now();
    };
    renderer.submit(&gpu.queue, prepared.encoder);
    profile.mark_submitted();
    crate::marks::mark_once(&FIRST_ENCODED, "frame.first.encoded");
    *encoded = host.frame_reports.stamp();
    l.window.pre_present_notify();
    #[cfg(target_os = "macos")]
    {
        let rt = host.app.rt();
        l.native.materials.present(rt.paint.materials(), rt.env.scale, || gpu.queue.present(prepared.frame));
    }
    #[cfg(target_os = "windows")]
    if let (Some(rects), Some(surface)) = (&prepared.present_rect, l.surface.as_ref())
        // SAFETY: only names the rectangles the next present changed.
        && let Some(hal) = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() } {
        let rects: Vec<_> = rects.iter().map(|&[left, top, right, bottom]| windows::Win32::Foundation::RECT { left, top, right, bottom }).collect();
        hal.set_present_rects(&rects);
    }
    #[cfg(not(target_os = "macos"))]
    gpu.queue.present(prepared.frame);
    #[cfg(target_os = "windows")]
    if let Some(surface) = l.surface.as_ref() {
        l.native.slot.presented_surface(surface);
    }
    // A swap chain made again on a handed-over window's visual takes over now (`cpu_frame`).
    #[cfg(target_os = "windows")]
    if matches!(l.native.mode, handover::SurfaceMode::Rebinding { .. }) {
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
        if let handover::SurfaceMode::Rebinding { handover } = std::mem::take(&mut l.native.mode) {
            if let Err(error) = handover.commit() { eprintln!("rune: the window's new swap chain couldn't take over: {error}"); }
            l.native.mode = handover::SurfaceMode::Handed { handover };
        }
    }
    #[cfg(target_os = "windows")]
    l.native.video_layers.drawn(|key| renderer.surface_drew_isolated(key));
    crate::window::frame_presented();
    crate::render::gpu_events::presented();
    // The frame reached the screen: a surface replaced by `replace_lost_surface` works, and
    // the next loss starts its retries over.
    l.surface_retries.worked();
    #[cfg(target_os = "macos")]
    if let Some(shadow) = &l.native.shadow { shadow.presented(); }
    // The frame landed with this present; later ones needn't wait for a transaction.
    #[cfg(target_os = "macos")]
    if !FrameMove::moving(&host.frame_move) { l.present_with_transaction(false); }
    if !l.first_frame.ready()
        && rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).is_ok() {
        l.first_frame.gpu_done();
        crate::marks::mark("frame.first.gpu_done");
        #[cfg(target_os = "windows")]
        crate::platform::win32::unidle_d3d_threads_soon();
    }
    // The window shows once the GPU has finished the frame it presents.
    #[cfg(target_os = "windows")]
    if l.native.cloak.held() {
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
        l.native.cloak.lift(&l.window);
        crate::marks::mark("window.shown");
    }
    #[cfg(feature = "frame-profile")]
    profile.build_frame(
        host,
        info.animating,
        looped,
        #[cfg(target_os = "windows")]
        prepared.present_rect,
    );
    #[cfg(not(feature = "frame-profile"))]
    let _ = (looped, info);
    crate::time::now()
}

pub(super) fn open_trace_if_needed(host: &mut Host, now: Instant) {
    let Some(path) = &host.args.trace else { return };
    if host.trace.is_some() { return; }
    let Some(l) = host.win.as_ref() else { return };
    let hz = l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map_or(60.0, |hz| hz as f64 / 1000.0);
    #[cfg(target_os = "macos")]
    let hz = l.native.pacing.as_ref().map_or(hz, |pacing| pacing.refresh_hz());
    let rt = host.app.rt_mut();
    let mut trace = crate::tooling::Trace::open_clock(path, rt.viewport, hz, rt.env.scale, "wall");
    trace.all = host.args.trace_all;
    #[cfg(target_os = "macos")]
    if let Some(log) = host.present_log.clone() && l.native.present.is_some() { trace.record_presents(log, now); }
    trace.record_surface(host.meter.shared(), rt.host.app_stats);
    host.trace = Some(trace);
    host.trace_start = Some(now);
    rt.scene.store_mut().audit_retargets(true);
}

/// Records one traced frame; returns `true` when `--frames` finished the run.
pub(super) fn record_trace_frame(
    host: &mut Host,
    now: Instant,
    submitted: Instant,
    target: Option<Instant>,
    drawable: Option<u64>,
) -> bool {
    let Some(trace) = host.trace.as_mut() else { return false };
    let target_ms = target.zip(host.trace_start).map(|(target, start)| crate::frame::present_time::ms_since(start, target));
    // A frame count keeps the trace redrawing while idle, as below.
    let continuous = !host.idle || host.args.frames.is_some();
    let timing = crate::tooling::FrameTiming { target_ms, drawable, continuous };
    trace.frame_timed_core(
        host.app.rt_mut(),
        (submitted - host.trace_start.unwrap()).as_secs_f64() * 1000.0,
        now.elapsed().as_secs_f64() * 1000.0,
        timing,
    );
    host.trace_frames += 1;
    if host.args.frames.is_some_and(|n| host.trace_frames >= n) {
        // While the window still shows its last frames.
        trace.settle_presents(crate::frame::present_time::SETTLE);
        host.life.finish();
        return true;
    }
    false
}
