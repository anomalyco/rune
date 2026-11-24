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
