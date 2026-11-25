use std::sync::Arc;
use std::time::Duration;

use super::presence::Presence;
use super::{AppSpec, Overlay, OverlaySpec};
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::window::Rest;
use crate::window::rest::{PRESSED_AFTER, REST_AFTER};
use crate::{Color, Cx, El, Spring, SpringPair, Transition, el, text};

struct SwapState {
    flips: usize,
}

fn swap_view(s: &SwapState, _: &mut Cx<SwapState>) -> El {
    let i = s.flips % 2;
    let fade = Spring::new(0.16, 0.0);
    el().size(440.0, 208.0)
        .child(
            el().absolute(20.0, 0.0)
                .size(400.0, 184.0)
                .radius(16.0)
                .fill(Color::BLACK.alpha(0.9)),
        )
        .child(
            el().key(format!("panel-{i}"))
                .absolute(20.0, 0.0)
                .size(400.0, 184.0)
                .transition(Transition::opacity().springs(SpringPair::new(fade, fade)))
                .child(
                    el().absolute(24.0, 36.0)
                        .size(352.0, 120.0)
                        .radius(12.0)
                        .fill(Color::WHITE.alpha(if i == 0 { 0.12 } else { 0.22 }))
                        .child(
                            text(if i == 0 { "Panel Zero" } else { "Panel One" })
                                .color(Color::WHITE),
                        ),
                ),
        )
}

#[test]
fn overlay_rest_deadline_fires_once_and_idle_overlay_has_zero_wakes_and_frames() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(120.0, 40.0) }
    let app = AppSpec::new("overlay-rest", (120.0, 40.0), || (), view);
    let (mut overlay, _) = Overlay::new(
        OverlaySpec::new("overlay-rest", app.size, view).options(Default::default()),
        (),
        &app,
    );
    overlay.presence = Presence::Shown { occluded: false };
    overlay.dirty = false;
    overlay.idle = true;

    assert_eq!(overlay.rest, Rest::Rested);
    assert_eq!(overlay.wake_deadline(), None);

    let t0 = crate::time::now();
    overlay.note_presented_with(t0, false);
    assert_eq!(overlay.wake_deadline(), Some(t0 + REST_AFTER));

    // Before REST_AFTER, waking does not give back and keeps the single deadline.
    overlay.wake_due(t0 + REST_AFTER - Duration::from_millis(1));
    assert_eq!(overlay.wake_deadline(), Some(t0 + REST_AFTER));

    // A new drawn frame before REST_AFTER pushes the deadline out (stays warm during active use).
    let t1 = t0 + Duration::from_secs(5);
    overlay.note_presented_with(t1, false);
    assert_eq!(overlay.wake_deadline(), Some(t1 + REST_AFTER));

    // Low-memory pressure shortens the deadline to PRESSED_AFTER.
    let t2 = t1 + Duration::from_secs(1);
    overlay.note_presented_with(t2, true);
    assert_eq!(overlay.wake_deadline(), Some(t2 + PRESSED_AFTER));

    // Passing the deadline gives back once and leaves no repeating timer or pending draw.
    let commits = overlay.rt.stats.commits;
    overlay.wake_due(t2 + PRESSED_AFTER);
    assert_eq!(overlay.rest, Rest::Idle);
    assert_eq!(overlay.wake_deadline(), None, "rest fires once and arms no repeating timer");
    assert!(!overlay.dirty, "giving back at rest does not dirty the overlay");
    assert!(overlay.idle, "giving back at rest keeps the overlay idle");
    assert_eq!(overlay.rt.stats.commits, commits, "rest triggers zero frames");

    // Further wakes while idle do nothing.
    overlay.wake_due(t2 + PRESSED_AFTER + Duration::from_secs(60));
    assert_eq!(overlay.rest, Rest::Idle);
    assert_eq!(overlay.wake_deadline(), None);
    assert!(!overlay.dirty);
    assert!(overlay.idle);
    assert_eq!(overlay.rt.stats.commits, commits);

    // Hiding steps Unseen -> Rested and leaves no wake deadline.
    overlay.hide();
    assert_eq!(overlay.rest, Rest::Rested);
    assert_eq!(overlay.wake_deadline(), None);

    // Hiding an overlay with release_gpu_when_hidden also clears the rest timer.
    let spec = OverlaySpec::new("overlay-rest-now", app.size, view)
        .options(Default::default())
        .release_gpu_when_hidden();
    let (mut releasing, _) = Overlay::new(spec, (), &app);
    releasing.presence = Presence::Shown { occluded: false };
    releasing.note_presented_with(t0, false);
    assert_eq!(releasing.wake_deadline(), Some(t0 + REST_AFTER));
    releasing.hide();
    assert_eq!(releasing.rest, Rest::Rested);
    assert_eq!(releasing.wake_deadline(), None);

    // On macOS, becoming occluded while visible steps Unseen -> Rested.
    #[cfg(target_os = "macos")]
    {
        overlay.presence = Presence::Shown { occluded: false };
        overlay.note_presented_with(t0, false);
        assert_eq!(overlay.wake_deadline(), Some(t0 + REST_AFTER));
        overlay.occluded(true);
        assert_eq!(overlay.rest, Rest::Rested);
        assert_eq!(overlay.wake_deadline(), None);
    }
}

#[test]
fn visible_overlay_gives_back_spare_layer_targets_at_rest_and_reuses_them_across_swaps() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let gpu = Arc::new(gpu);
    let app = AppSpec::new("overlay-swap-rest", (440.0, 208.0), || SwapState { flips: 0 }, swap_view);
    let (mut overlay, _) = Overlay::new(
        OverlaySpec::new("overlay-swap-rest", app.size, swap_view).options(Default::default()),
        SwapState { flips: 0 },
        &app,
    );
    overlay.rt.set_system_reduced_motion(false);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.core.gpu = Some(gpu.clone());
    overlay.core.renderer = Some(Renderer::new(&gpu.device, OFFSCREEN_FORMAT));
    let target = Offscreen::new(&gpu.device, 440, 208);

    let render_once = |overlay: &mut Overlay<SwapState>| {
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        overlay.render_offscreen(&gpu, &mut enc, &target.view, OFFSCREEN_FORMAT, (440, 208), 1.0 / 60.0);
        overlay.core.renderer.as_mut().unwrap().submit(&gpu.queue, enc);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    };

    // Initial frame + settle.
    overlay.rt.frame(0.0);
    render_once(&mut overlay);
    for _ in 0..60 {
        let animating = overlay.rt.frame(1.0 / 60.0);
        render_once(&mut overlay);
        if !animating { break; }
    }

    #[cfg(feature = "test-gpu-counters")]
    let mut base_hal = None;

    // 110 alternating keyed opacity-transition flips: both spare layer targets are reused
    // without allocating again and HAL counters stay flat from flip 10 to flip 110.
    for flip in 1..=110 {
        overlay.rt.state.flips = flip;
        overlay.rt.invalidate();
        for _ in 0..60 {
            let animating = overlay.rt.frame(1.0 / 60.0);
            render_once(&mut overlay);
            if !animating { break; }
        }
        #[cfg(feature = "test-gpu-counters")]
        if flip == 10 {
            let c = gpu.device.get_internal_counters().hal;
            base_hal = Some((
                c.textures.read(),
                c.texture_memory.read(),
                c.buffers.read(),
                c.buffer_memory.read(),
                c.texture_views.read(),
                c.bind_groups.read(),
            ));
        }
    }

    let renderer = overlay.core.renderer.as_ref().unwrap();
    assert_eq!(renderer.layer_stats().allocations, 2, "alternating swaps reuse 2 layer targets");
    assert_eq!(renderer.spare_layers(), 2, "settled swap keeps 2 spare layer targets warm before rest");

    #[cfg(feature = "test-gpu-counters")]
    {
        let c = gpu.device.get_internal_counters().hal;
        let now_hal = (
            c.textures.read(),
            c.texture_memory.read(),
            c.buffers.read(),
            c.buffer_memory.read(),
            c.texture_views.read(),
            c.bind_groups.read(),
        );
        assert_eq!(now_hal, base_hal.unwrap(), "HAL counters stay flat across 100 alternating swaps");
    }

    // Record the last visible frame at t0, mark settled/idle.
    let t0 = crate::time::now();
    overlay.dirty = false;
    overlay.idle = true;
    overlay.note_presented_with(t0, false);
    assert_eq!(overlay.wake_deadline(), Some(t0 + REST_AFTER));

    // Still warm just before REST_AFTER.
    overlay.wake_due(t0 + REST_AFTER - Duration::from_millis(1));
    assert_eq!(overlay.core.renderer.as_ref().unwrap().spare_layers(), 2);
    assert_eq!(overlay.wake_deadline(), Some(t0 + REST_AFTER));

    #[cfg(feature = "test-gpu-counters")]
    let textures_before_rest = gpu.device.get_internal_counters().hal.textures.read();
    let commits_before_rest = overlay.rt.stats.commits;

    // Once REST_AFTER elapses, spare layer targets are given back and HAL textures drop by 2.
    overlay.wake_due(t0 + REST_AFTER);
    assert_eq!(overlay.rest, Rest::Idle);
    assert_eq!(overlay.core.renderer.as_ref().unwrap().spare_layers(), 0, "rest trims all spare layer targets");
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(
        gpu.device.get_internal_counters().hal.textures.read(),
        textures_before_rest - 2,
        "giving back spare layer targets frees both HAL textures"
    );

    // At rest, no timer remains armed and no frames run.
    assert_eq!(overlay.wake_deadline(), None, "no repeating timer after rest");
    assert!(!overlay.dirty);
    assert!(overlay.idle);
    assert_eq!(overlay.rt.stats.commits, commits_before_rest);
    overlay.wake_due(t0 + REST_AFTER + Duration::from_secs(60));
    assert_eq!(overlay.wake_deadline(), None);
    assert!(!overlay.dirty);
    assert!(overlay.idle);
    assert_eq!(overlay.rt.stats.commits, commits_before_rest);
}

#[test]
fn hidden_overlay_keeping_renderer_gives_back_scratch_layers_on_unseen() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let gpu = Arc::new(gpu);
    let app = AppSpec::new("overlay-unseen-rest", (440.0, 208.0), || SwapState { flips: 0 }, swap_view);
    let (mut overlay, _) = Overlay::new(
        OverlaySpec::new("overlay-unseen-rest", app.size, swap_view).options(Default::default()),
        SwapState { flips: 0 },
        &app,
    );
    overlay.rt.set_system_reduced_motion(false);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.core.gpu = Some(gpu.clone());
    overlay.core.renderer = Some(Renderer::new(&gpu.device, OFFSCREEN_FORMAT));
    let target = Offscreen::new(&gpu.device, 440, 208);

    // Initial settled frame, then one flip mid-transition so both layer targets are live scratch targets.
    overlay.rt.frame(0.0);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    overlay.render_offscreen(&gpu, &mut enc, &target.view, OFFSCREEN_FORMAT, (440, 208), 0.0);
    overlay.core.renderer.as_mut().unwrap().submit(&gpu.queue, enc);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");

    #[cfg(feature = "test-gpu-counters")]
    let base_textures = gpu.device.get_internal_counters().hal.textures.read();

    overlay.rt.state.flips = 1;
    overlay.rt.invalidate();
    assert!(overlay.rt.frame(1.0 / 60.0), "mid-fade transition is animating");
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    overlay.render_offscreen(&gpu, &mut enc, &target.view, OFFSCREEN_FORMAT, (440, 208), 1.0 / 60.0);
    overlay.core.renderer.as_mut().unwrap().submit(&gpu.queue, enc);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");

    assert_eq!(overlay.core.renderer.as_ref().unwrap().layer_stats().targets, 2, "mid-fade holds 2 live scratch targets");
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(gpu.device.get_internal_counters().hal.textures.read(), base_textures + 2);

    overlay.note_presented_with(crate::time::now(), false);
    assert!(overlay.wake_deadline().is_some());

    // Hiding while keeping the renderer steps Unseen -> GiveBackAll without blocking.
    overlay.hide();
    assert_eq!(overlay.rest, Rest::Rested);
    assert_eq!(overlay.core.renderer.as_ref().unwrap().layer_stats().targets, 0, "Unseen drops live scratch targets");
    assert_eq!(overlay.core.renderer.as_ref().unwrap().spare_layers(), 0);
    assert_eq!(overlay.wake_deadline(), None, "idle GPU retires immediately with no leftover deadline");
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(
        gpu.device.get_internal_counters().hal.textures.read(),
        base_textures,
        "Unseen frees both scratch layer HAL textures"
    );
}

#[test]
fn overlay_give_back_drops_freed_image_textures_at_rest_and_all_on_hide_then_wakes() {
    use crate::pixels::Pixels;
    struct ImageState {
        small: Pixels,
        large: Pixels,
        show_small: bool,
        show_large: bool,
    }
    fn img_view(s: &ImageState, _: &mut Cx<ImageState>) -> El {
        let mut root = el().size(16.0, 16.0);
        if s.show_small {
            root = root.child(el().size(8.0, 8.0).pixels(&s.small));
        }
        if s.show_large {
            root = root.child(el().size(8.0, 8.0).pixels(&s.large));
        }
        root
    }

    let Some(gpu) = Gpu::test_headless() else { return };
    let gpu = Arc::new(gpu);
    let small = Pixels::new();
    small.set(4, 4, [255, 0, 0, 255].repeat(16));
    let large = Pixels::new();
    large.set(1025, 64, [0, 200, 40, 255].repeat(1025 * 64));

    fn init_state() -> ImageState {
        ImageState { small: Pixels::new(), large: Pixels::new(), show_small: true, show_large: true }
    }
    let app = AppSpec::new("overlay-image-rest", (16.0, 16.0), init_state, img_view);
    let (mut overlay, _) = Overlay::new(
        OverlaySpec::new("overlay-image-rest", app.size, img_view).options(Default::default()),
        ImageState { small, large, show_small: true, show_large: true },
        &app,
    );
    overlay.presence = Presence::Shown { occluded: false };
    overlay.core.gpu = Some(gpu.clone());
    overlay.core.renderer = Some(Renderer::new(&gpu.device, OFFSCREEN_FORMAT));
    let target = Offscreen::new(&gpu.device, 16, 16);

    #[cfg(feature = "test-gpu-counters")]
    let (base_textures, base_mem) = {
        let c = gpu.device.get_internal_counters().hal;
        (c.textures.read(), c.texture_memory.read())
    };

    let render_read = |overlay: &mut Overlay<ImageState>| -> Vec<u8> {
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        overlay.render_offscreen(&gpu, &mut enc, &target.view, OFFSCREEN_FORMAT, (16, 16), 0.0);
        target.copy(&mut enc);
        overlay.core.renderer.as_mut().unwrap().submit(&gpu.queue, enc);
        target.read(&gpu.device)
    };

    // Frame 1: both small (shelf page 0: 1024x128) and large (standalone page 1: 1025x64) shown.
    let page0_bytes = 1024u64 * 128 * 4;
    let page1_bytes = 1025u64 * 64 * 4;
    overlay.rt.frame(0.0);
    let _ = render_read(&mut overlay);
    assert_eq!(overlay.rt.images.pages.len(), 2);
    assert_eq!(overlay.core.renderer.as_ref().unwrap().image_texture_count(), 2);
    assert_eq!(overlay.core.renderer.as_ref().unwrap().image_texture_bytes(), page0_bytes + page1_bytes);
    #[cfg(feature = "test-gpu-counters")]
    let tracks_mem = {
        let c = gpu.device.get_internal_counters().hal;
        assert_eq!(c.textures.read(), base_textures + 2, "two image page textures uploaded");
        c.texture_memory.read() > base_mem
    };

    // Frame 2: small removed, large stays. Shelf page 0 stays warm until rest.
    overlay.rt.state.show_small = false;
    overlay.rt.invalidate();
    overlay.rt.frame(0.0);
    let before_hide = render_read(&mut overlay);
    #[cfg(feature = "test-gpu-counters")]
    let (tex_before_rest, mem_before_rest) = {
        let c = gpu.device.get_internal_counters().hal;
        (c.textures.read(), c.texture_memory.read())
    };
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(tex_before_rest, base_textures + 2);

    // Idle rest (GiveBackUnused): page 0's CPU buffer is emptied and its GPU texture drops;
    // page 1 is still on screen and keeps its GPU texture.
    let t0 = crate::time::now();
    overlay.note_presented_with(t0, false);
    overlay.wake_due(t0 + REST_AFTER);
    assert_eq!(overlay.rest, Rest::Idle);
    assert!(overlay.rt.images.pages[0].data.is_empty());
    assert!(!overlay.rt.images.pages[1].data.is_empty());
    assert_eq!(overlay.core.renderer.as_ref().unwrap().image_texture_count(), 1, "visible page stays resident at rest");
    assert_eq!(overlay.core.renderer.as_ref().unwrap().image_texture_bytes(), page1_bytes);
    #[cfg(feature = "test-gpu-counters")]
    {
        let c = gpu.device.get_internal_counters().hal;
        assert_eq!(c.textures.read(), base_textures + 1, "rest frees emptied page 0 GPU texture");
        if tracks_mem {
            assert!(c.texture_memory.read() < mem_before_rest, "rest reclaims page 0 HAL texture memory");
        }
    }

    // Hidden (GiveBackAll): every image GPU texture drops.
    overlay.hide();
    assert_eq!(overlay.rest, Rest::Rested);
    assert_eq!(overlay.core.renderer.as_ref().unwrap().image_texture_count(), 0, "hide drops all GPU image textures");
    assert_eq!(overlay.core.renderer.as_ref().unwrap().image_texture_bytes(), 0);
    #[cfg(feature = "test-gpu-counters")]
    {
        let c = gpu.device.get_internal_counters().hal;
        assert_eq!(c.textures.read(), base_textures, "hide frees all image HAL textures");
        assert_eq!(c.texture_memory.read(), base_mem, "hide reclaims all image HAL texture memory");
    }

    // Wake: first drawn frame re-uploads and renders identically with no blank or stale frame.
    overlay.presence = Presence::Shown { occluded: false };
    let after_wake = render_read(&mut overlay);
    assert_eq!(after_wake, before_hide, "waking from hide draws the image identically on its first frame");
    assert!(overlay.core.renderer.as_ref().unwrap().has_images());
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(gpu.device.get_internal_counters().hal.textures.read(), base_textures + 1);
}
#[test]
fn overlay_idle_rest_after_24_resizes_retires_metal_submissions_and_releases_spare_textures() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let Some(gpu) = Gpu::test_headless() else { return };
    let gpu = Arc::new(gpu);
    fn resize_view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().w_full().h_full().fill(crate::Color::BLACK)
            .child(el().key("card").w_full().h(80.0).radius(8.0).opacity(0.95).fill(crate::Color::WHITE.alpha(0.2)).child(crate::text("Resize")))
            .child(el().key("blur").size(96.0, 32.0).radius(8.0).backdrop_blur(8.0).fill(crate::Color::WHITE.alpha(0.12)))
            .child(el().key("cached").w_full().h(64.0).cache().fill(crate::Color::WHITE.alpha(0.15)).child(crate::text("Cached")))
    }
    let app = AppSpec::new("overlay-resize-rest", (440.0, 208.0), || (), resize_view);
    let (mut overlay, _) = Overlay::new(
        OverlaySpec::new("overlay-resize-rest", app.size, resize_view).options(Default::default()),
        (),
        &app,
    );
    overlay.presence = Presence::Shown { occluded: false };
    overlay.core.gpu = Some(gpu.clone());
    overlay.core.renderer = Some(Renderer::new(&gpu.device, OFFSCREEN_FORMAT));

    let draw_at = |overlay: &mut Overlay<()>, w: u32, h: u32| {
        overlay.rt.set_viewport((w as f32, h as f32), 1.0);
        overlay.rt.frame(1.0 / 60.0);
        let target = Offscreen::new(&gpu.device, w, h);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        overlay.render_offscreen(&gpu, &mut enc, &target.view, OFFSCREEN_FORMAT, (w, h), 1.0 / 60.0);
        overlay.core.renderer.as_mut().unwrap().submit(&gpu.queue, enc);
    };

    // Warm baseline at (440, 208), settled and retired.
    draw_at(&mut overlay, 440, 208);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("baseline poll");
    #[cfg(all(target_os = "macos", feature = "test-gpu-counters"))]
    let base_textures = gpu.device.get_internal_counters().hal.textures.read();

    // 24 rapid resizes without intermediate polls, ending back at (440, 208).
    for i in 1..=23u32 {
        let w = 320 + (i * 37) % 360;
        let h = 180 + (i * 29) % 240;
        draw_at(&mut overlay, w, h);
    }
    draw_at(&mut overlay, 440, 208);

    let retired = Arc::new(AtomicBool::new(false));
    let done = retired.clone();
    gpu.queue.on_submitted_work_done(move || done.store(true, Ordering::Release));
    // Give the GPU plenty of time to finish the tiny offscreen pass without polling.
    std::thread::sleep(Duration::from_millis(80));

    let t0 = crate::time::now();
    overlay.dirty = false;
    overlay.idle = true;
    overlay.note_presented_with(t0, false);
    overlay.wake_due(t0 + REST_AFTER);
    assert_eq!(overlay.rest, Rest::Idle);
    assert_eq!(overlay.core.renderer.as_ref().unwrap().spare_layers(), 0, "rest trims all spare layer targets");
    #[cfg(target_os = "macos")]
    assert!(retired.load(Ordering::Acquire), "macOS idle rest must poll and retire completed Metal command buffers");
    #[cfg(all(target_os = "macos", feature = "test-gpu-counters"))]
    assert_eq!(
        gpu.device.get_internal_counters().hal.textures.read(),
        base_textures,
        "24 resizes followed by idle rest must leave no extra live HAL textures",
    );
}
#[test]
fn should_poll_on_idle_rest_polls_on_macos_and_only_for_oversized_uploads_elsewhere() {
    use crate::window::rest::should_poll_on_idle_rest;
    assert!(should_poll_on_idle_rest(true, false));
    assert!(should_poll_on_idle_rest(true, true));
    assert!(!should_poll_on_idle_rest(false, false));
    assert!(should_poll_on_idle_rest(false, true));
}
