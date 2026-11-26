//! GPU resource preparation, Windows partial damage planning, compositor/UI encoding,
//! presentation and `frame-profile` live-log stamping for [`super::Host`].

#[cfg(any(target_os = "macos", feature = "frame-profile"))]
use super::stepping::FrameMove;
use super::{CompositorFrame, Host};
use crate::frame::host_step::{
    RenderPrep, StepInfo, create_frame_encoder, draw_ui, prepare_gpu_resources_with,
};
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
pub(in crate::window) struct FrameProfile {
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
    pub(in crate::window) fn mark_entry(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.entry = crate::frame::live_log::on().then(crate::frame::live_log::Stamp::now); }
    }

    #[inline]
    pub(in crate::window) fn mark_start(&mut self) {
        #[cfg(feature = "frame-profile")]
        if self.start.is_none() {
            self.start = crate::frame::live_log::on().then(crate::frame::live_log::Stamp::now);
        }
    }

    #[inline]
    pub(in crate::window) fn mark_acquired(&mut self) {
        #[cfg(feature = "frame-profile")]
        if self.acquired.is_none() {
            self.acquired = self.start.map(|_| crate::frame::live_log::Stamp::now());
        }
    }

    #[inline]
    pub(in crate::window) fn mark_built(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.built = self.start.map(|_| crate::frame::live_log::Stamp::now()); }
    }

    #[inline]
    pub(in crate::window) fn mark_uploaded(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.uploaded = self.start.map(|_| crate::frame::live_log::Stamp::now()); }
    }

    #[inline]
    pub(in crate::window) fn mark_encoded(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.encoded = self.start.map(|_| crate::frame::live_log::Stamp::now()); }
    }

    #[inline]
    pub(in crate::window) fn mark_submitted(&mut self) {
        #[cfg(feature = "frame-profile")]
        { self.submitted = self.start.map(|_| crate::frame::live_log::Stamp::now()); }
    }

    #[inline]
    pub(in crate::window) fn set_loop_why(&mut self, why: &'static str) {
        #[cfg(feature = "frame-profile")]
        { self.loop_why = Some(why); }
        #[cfg(not(feature = "frame-profile"))]
        let _ = why;
    }

    #[inline]
    pub(in crate::window) fn set_loop_why_if_none(&mut self, why: &'static str) {
        #[cfg(feature = "frame-profile")]
        if self.loop_why.is_none() { self.loop_why = Some(why); }
        #[cfg(not(feature = "frame-profile"))]
        let _ = why;
    }

    #[cfg(target_os = "windows")]
    #[inline]
    pub(in crate::window) fn set_whole_why(&mut self, why: &'static str) {
        #[cfg(feature = "frame-profile")]
        { self.whole_why = Some(why); }
        #[cfg(not(feature = "frame-profile"))]
        let _ = why;
    }

    #[inline]
    pub(in crate::window) fn take_loop_check(&mut self, rt: &mut crate::runtime::RuntimeCore) {
        #[cfg(feature = "frame-profile")]
        { self.loop_check = rt.loop_checked(); }
        #[cfg(not(feature = "frame-profile"))]
        { let _ = rt.loop_checked(); }
    }

    #[cfg(feature = "frame-profile")]
    pub(in crate::window) fn build_frame(
        &mut self,
        host: &mut Host,
        animating: bool,
        looped: Option<&Vec<u32>>,
        #[cfg(target_os = "windows")]
        damage: Option<Vec<[i32; 4]>>,
    ) {
        #[cfg(target_os = "windows")]
        let damage_seen = host.win.as_ref().and_then(|l| l.native.damage.seen);
        #[cfg(not(target_os = "windows"))]
        let damage_seen = None;
        #[cfg(not(target_os = "windows"))]
        let damage = None;
        self.build_frame_core(host.app.rt_mut(), damage_seen, animating, looped, damage);
    }

    #[cfg(feature = "frame-profile")]
    pub(in crate::window) fn build_frame_core(
        &mut self,
        rt: &mut crate::runtime::RuntimeCore,
        damage_seen: Option<[u32; 4]>,
        animating: bool,
        looped: Option<&Vec<u32>>,
        damage: Option<Vec<[i32; 4]>>,
    ) {
        self.frame = match (self.start, self.acquired, self.built, self.uploaded, self.encoded, self.submitted) {
            (Some(start), Some(acquired), Some(built), Some(uploaded), Some(encoded), Some(submitted)) => {
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
                    damage,
                    whole: self.whole_why,
                    change: damage_seen,
                    looped: looped.map(Vec::len),
                    loop_why: self.loop_why,
                    loop_check: self.loop_check.take(),
                })
            }
            _ => None,
        };
    }

    #[inline]
    pub(in crate::window) fn flush(&mut self, host: &Host, animating: bool, next_tick: Option<Instant>) {
        #[cfg(feature = "frame-profile")]
        self.flush_core(host.app.rt(), FrameMove::moving(&host.frame_move), animating, next_tick);
        #[cfg(not(feature = "frame-profile"))]
        let _ = (host, animating, next_tick);
    }

    #[cfg(any(target_os = "windows", feature = "frame-profile"))]
    #[inline]
    pub(in crate::window) fn flush_core(
        &mut self,
        rt: &crate::runtime::RuntimeCore,
        frame_moving: bool,
        animating: bool,
        next_tick: Option<Instant>,
    ) {
        #[cfg(feature = "frame-profile")]
        if let Some(mut f) = self.frame.take() {
            f.next = [
                animating,
                frame_moving,
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
        let _ = (rt, frame_moving, animating, next_tick);
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
    let encoder = create_frame_encoder(gpu);
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
        let atlas = damage::GlyphAtlas::of(&rt.text.atlas);
        let loop_ok = looped.is_none_or(|p| match l.native.damage.can_patch_loop(&rt.instances, p, size, rt.clear_color(), swap, atlas) {
            Ok(()) => true,
            Err(why) => { profile.set_loop_why(why); false }
        });
        let armed = looped.map(Vec::as_slice).filter(|_| whole.is_none() && loop_ok).and_then(|p| {
            match renderer.arm_loop(&gpu.device, &gpu.queue, rt.paint_count(), &rt.instances, p, rt.clear_color(), size, l.config.format) {
                Ok(rects) => Some((p, rects)),
                Err(why) => { profile.set_loop_why(why); None }
            }
        });
        if let Some((p, present)) = armed {
            l.native.damage.patched(&rt.instances, p);
            Some(present)
        } else {
            match l.native.damage.plan(&rt.instances, size, rt.clear_color(), swap, atlas, whole.is_none()) {
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
    if host.poll_gpu_loss() { return false; }
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
    // A catch-up frame (`KeptDraw::Whole`) drew whole to bring the kept frame current after a
    // frame that bypassed it (e.g. a native surface whose pixels `Damage::plan` does not track),
    // so it must present the whole window instead of stamping partial dirty rects over the old
    // front buffer.
    #[cfg(target_os = "windows")]
    match (renderer.damage_drawn(), prepared.part.take()) {
        (Ok(crate::render::KeptDraw::Partial), part) => prepared.present_rect = part,
        (Ok(crate::render::KeptDraw::Whole), Some(_)) => profile.set_whole_why("kept behind"),
        (Ok(crate::render::KeptDraw::Whole), None) => {}
        (Err(why), Some(_)) => profile.set_whole_why(why),
        (Err(_), None) => {}
    }
    // A client-side frame's rounded bottom corners: faded off the premultiplied composition,
    // before a surface that takes straight alpha converts it.
    crate::window::surface::corners::draw(l.corners.as_ref(), &mut prepared.encoder, target, corners);
    if let Some(alpha) = &l.alpha { alpha.draw(&mut prepared.encoder, &prepared.view); }
    profile.mark_encoded();
    if host.poll_device_for_terminal_loss() { return false; }
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
    // macOS: the first frame of a window already on screen is taken as finished when it is
    // presented. The GPU takes 9,000 to 18,000 µs over it (an M4's, by the command buffer's own
    // GPU times), and the UI thread has AppKit's launch work, the next frame and input to get
    // to meanwhile. A frame a hidden window prepares still waits for the GPU: a reveal shows it.
    #[cfg(target_os = "macos")]
    let waits = host.launcher() || !host.life.shown() || std::env::var_os("RUNE_FIRST_FRAME_WAITS").is_some();
    #[cfg(not(target_os = "macos"))]
    let waits = true;
    // Unpack so `frame` (the live `wgpu::SurfaceTexture`) can always be dropped BEFORE any
    // terminal surface/GPU teardown runs (`note_frame_texture_released`).
    let PreparedWindowFrame {
        frame,
        view,
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        mut encoder,
        #[cfg(target_os = "windows")]
        present_rect,
        ..
    } = prepared;
    if host.poll_device_for_terminal_loss() || host.gpu.as_ref().is_none_or(|(gpu, _)| gpu.is_lost()) {
        drop((encoder, view, frame));
        host.note_frame_texture_released();
        return crate::time::now();
    }
    #[cfg(target_os = "linux")]
    let readback = {
        let (Some(l), Some((gpu, _))) = (host.win.as_mut(), host.gpu.as_mut()) else {
            drop((encoder, view, frame));
            host.note_frame_texture_released();
            return crate::time::now();
        };
        crate::window::linux::handover::readback(l, gpu, &mut encoder, &frame)
    };
    let Some((gpu, renderer)) = host.gpu.as_mut() else {
        drop((encoder, view, frame));
        host.note_frame_texture_released();
        return crate::time::now();
    };
    renderer.submit(&gpu.queue, encoder);
    // Drop the view before checking/presenting; never call `pre_present_notify()` (which arms an
    // unfulfillable Wayland frame callback) or `queue.present()` on a lost/latched device under
    // either `Terminal` or `Recover`.
    drop(view);
    let terminal_lost = host.poll_device_for_terminal_loss();
    let gpu_lost = host.gpu.as_ref().is_none_or(|(gpu, _)| gpu.is_lost());
    if !crate::render::loss_latch::should_present_after_submit(gpu_lost, terminal_lost) {
        drop(frame);
        host.note_frame_texture_released();
        return crate::time::now();
    }
    let (Some(l), Some((gpu, _))) = (host.win.as_mut(), host.gpu.as_mut()) else {
        drop(frame);
        host.note_frame_texture_released();
        return crate::time::now();
    };
    profile.mark_submitted();
    crate::marks::mark_once(&FIRST_ENCODED, "frame.first.encoded");
    *encoded = host.frame_reports.stamp();
    l.window.pre_present_notify();
    #[cfg(target_os = "macos")]
    {
        let (draws, scale) = crate::system_material::present_draws(host.app.rt());
        l.native.materials.present(draws, scale, || gpu.queue.present(frame));
    }
    #[cfg(target_os = "windows")]
    if let (Some(rects), Some(surface)) = (&present_rect, l.surface.as_ref())
        // SAFETY: only names the rectangles the next present changed.
        && let Some(hal) = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() } {
        let rects: Vec<_> = rects.iter().map(|&[left, top, right, bottom]| windows::Win32::Foundation::RECT { left, top, right, bottom }).collect();
        hal.set_present_rects(&rects);
    }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    if host.wayland && let Some(blur) = l.native.wayland_backdrop.as_mut() {
        let rt = host.app.rt();
        blur.sync_frame_materials(
            host.window_backdrop,
            host.options.transparent,
            l.surface.is_some().then_some(l.config.alpha_mode),
            false,
            (l.config.width, l.config.height),
            rt.window_scale(),
            rt.paint.wayland_material_blurs(),
            false,
        );
    }
    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    let materials_ok = {
        let rt = host.app.rt();
        let draws = crate::system_material::windows_present_draws(rt);
        let dark = rt.env.system_dark;
        let native = &mut l.native;
        native.materials.present(draws, dark, native.mode.handover_mut(), || {
            gpu.queue.present(frame);
        })
    };
    #[cfg(not(any(target_os = "macos", all(target_os = "windows", feature = "windows-system-material"))))]
    gpu.queue.present(frame);
    // `frame` has been consumed by `queue.present`: mark released before any post-present polls.
    host.note_frame_texture_released();
    let (Some(l), Some((gpu, renderer))) = (host.win.as_mut(), host.gpu.as_mut()) else {
        return crate::time::now();
    };
    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    if !materials_ok {
        host.app.rt_mut().paint.set_windows_acrylic_materials(false);
        l.request_redraw();
    }
    // A window that opened on the CPU is the GPU's from this frame (`linux::handover`).
    #[cfg(target_os = "linux")]
    crate::window::linux::handover::handed(l, gpu, readback);
    #[cfg(target_os = "windows")]
    l.native.slot.presented();
    // A swap chain made again on a handed-over window's visual takes over now (`cpu_frame`).
    #[cfg(target_os = "windows")]
    if matches!(l.native.mode, handover::SurfaceMode::Rebinding { .. }) {
        let polled = rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely());
        if should_commit_rebind(&polled, gpu.is_lost())
            && let handover::SurfaceMode::Rebinding { handover } = std::mem::take(&mut l.native.mode)
        {
            if let Err(error) = handover.commit() { eprintln!("rune: the window's new swap chain couldn't take over: {error}"); }
            l.native.mode = handover::SurfaceMode::Handed { handover };
        }
    }
    // Synchronized drag resize exit (`RUNE_RESIZE_SYNC=1`): once this normal flip-model frame
    // finishes on the GPU, re-attach DirectComposition and free drag resources.
    #[cfg(target_os = "windows")]
    let drag_exit_completed = if l.native.resize_sync.is_leaving() {
        let polled = rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely());
        Some(should_commit_rebind(&polled, gpu.is_lost()))
    } else {
        None
    };
    // The frame landed with this present; later ones needn't wait for a transaction.
    #[cfg(target_os = "macos")]
    if !FrameMove::moving(&host.frame_move) { l.present_with_transaction(false); }
    if !l.first_frame.ready() && !waits && l.first_frame.deadline().is_none() {
        if !gpu.is_lost() {
            l.first_frame.gpu_done();
            gpu.queue.on_submitted_work_done(|| crate::marks::mark("frame.first.gpu_done"));
        }
    } else if !l.first_frame.ready()
        && rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).is_ok()
        && !gpu.is_lost() {
        l.first_frame.gpu_done();
        crate::marks::mark("frame.first.gpu_done");
        #[cfg(target_os = "windows")]
        crate::platform::win32::unidle_d3d_threads_soon();
    }
    // The window shows once the GPU has finished the frame it presents.
    #[cfg(target_os = "windows")]
    if l.native.cloak.held()
        && rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).is_ok()
        && !gpu.is_lost()
    {
        l.native.cloak.lift(&l.window);
        crate::marks::mark("window.shown");
    }
    // The frame reached the screen only if the device stayed alive through submit, present, and
    // any synchronous wait above; a lost frame must keep `surface_retries` moving toward GiveUp.
    if finalize_present_retries(gpu.is_lost(), &mut l.surface_retries) {
        #[cfg(target_os = "windows")]
        l.native.video_layers.drawn(|key| renderer.surface_drew_isolated(key));
        crate::window::frame_presented();
        crate::render::gpu_events::presented();
        #[cfg(target_os = "macos")]
        if let Some(shadow) = &l.native.shadow { shadow.presented(); }
        renderer.warm_effects(&gpu.device);
    }
    #[cfg(feature = "frame-profile")]
    profile.build_frame(
        host,
        info.animating,
        looped,
        #[cfg(target_os = "windows")]
        present_rect,
    );
    #[cfg(not(feature = "frame-profile"))]
    let _ = (looped, info);
    #[cfg(target_os = "windows")]
    if let Some(completed) = drag_exit_completed {
        host.finish_resize_sync_exit_frame(completed);
    }
    crate::time::now()
}

/// Whether a rebinding DirectComposition swap chain's first frame finished on a live device and
/// may be committed over the window's previous content.
#[cfg(any(test, target_os = "windows"))]
pub(super) fn should_commit_rebind(
    polled: &Result<wgpu::PollStatus, rune_gpu::health::PollFailed>,
    lost: bool,
) -> bool {
    polled.is_ok() && !lost
}

/// Finalizes post-present status after submission, presentation, and any synchronous GPU wait.
/// Returns `true` when the device stayed alive; returns `false` when the device was lost.
/// Following Chromium's `GpuProcessHost::IncrementCrashCount`, a single healthy frame never resets
/// `surface_retries` — earlier losses decay only through `FORGIVE_LOSS` (1 per 5 minutes).
pub(super) fn finalize_present_retries(
    lost: bool,
    _surface_retries: &mut crate::window::surface::SurfaceRetries,
) -> bool {
    !lost
}

pub(in crate::window) fn open_trace_if_needed(host: &mut Host, now: Instant) {
    let Some(path) = &host.args.trace else { return };
    if host.trace.is_some() { return; }
    let Some(l) = host.win.as_ref() else { return };
    let hz = super::draw::display_hz(l).unwrap_or(60.0);
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
pub(in crate::window) fn record_trace_frame(
    host: &mut Host,
    now: Instant,
    submitted: Instant,
    target: Option<Instant>,
    drawable: Option<u64>,
) -> bool {
    let Some(trace) = host.trace.as_mut() else { return false };
    let hz = host.win.as_ref().and_then(super::draw::display_hz);
    let target_ms = target.zip(host.trace_start).map(|(target, start)| crate::frame::present_time::ms_since(start, target));
    // A frame count keeps the trace redrawing while idle, as below.
    let continuous = !host.idle || host.args.frames.is_some();
    let timing = crate::tooling::FrameTiming { target_ms, drawable, continuous, hz };
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
