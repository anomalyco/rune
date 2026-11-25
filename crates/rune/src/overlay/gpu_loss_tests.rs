//! Owned-overlay tests for `GpuLossPolicy::Terminal` shared latch propagation, teardown,
//! `Availability::Unavailable` + `failure()`, and bidirectional failure tripping.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::app::SharedCore;
use crate::overlay::{Availability, Overlay, OverlayOptions, OverlaySpec, Presence};
use crate::render::loss_latch::{GpuFailureKind, GpuLossLatch, GpuLossPolicy};
use crate::{App, AppSpec, Compositor, Cx, El, Frame, OverlayHandle, Tx, el};

#[derive(Default)]
struct State {
    avail: Vec<(Availability, Option<String>)>,
    handle: Option<OverlayHandle<State>>,
}

fn view(_: &State, _: &mut Cx<State>) -> El { el().size(40.0, 20.0) }

fn on_avail(s: &mut State, a: Availability, _tx: &mut Tx) {
    let fail = s.handle.as_ref().and_then(OverlayHandle::failure);
    s.avail.push((a, fail));
}

struct CountingCompositor(Arc<AtomicUsize>);
impl Compositor<State> for CountingCompositor {
    fn render(&mut self, _f: &mut Frame<'_, State>) {}
    fn release_gpu(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn make_overlay(
    latch: Arc<GpuLossLatch>,
    releases: Arc<AtomicUsize>,
) -> (Overlay<State>, OverlayHandle<State>) {
    let app = AppSpec::new("ov-term", (80.0, 40.0), State::default, view);
    let configured = OverlaySpec::new("ov-term", app.size, view)
        .options(OverlayOptions::default())
        .compositor(CountingCompositor(releases))
        .on_availability(on_avail);
    let (mut overlay, handle) = Overlay::new(configured, State::default(), &app);
    overlay.core.gpu_loss_latch = Some(latch);
    overlay.rt.state.handle = Some(handle.clone());
    (overlay, handle)
}

#[test]
fn app_overlay_inherits_shared_latch_regardless_of_builder_call_order() {
    fn app_view(_: &(), _: &mut Cx<()>) -> El { el() }
    let ov_spec = || OverlaySpec::new("hud", (60.0, 30.0), view).options(OverlayOptions::default());

    // 1. .overlay(...) BEFORE .gpu_loss_policy(Terminal)
    let mut app_before = App::new(AppSpec::new("Before", (100.0, 60.0), || (), app_view));
    let _h1 = app_before.overlay(ov_spec(), State::default());
    let app_before = app_before.gpu_loss_policy(GpuLossPolicy::Terminal);
    let latch_before = app_before.gpu_loss_latch.as_ref().expect("app latch");
    let ov_latch_before = app_before.overlays[0].gpu_loss_latch().expect("overlay latch");
    assert!(Arc::ptr_eq(latch_before, &ov_latch_before), "overlay registered before policy shares app latch");

    // 2. .gpu_loss_policy(Terminal) BEFORE .overlay(...)
    let mut app_after = App::new(AppSpec::new("After", (100.0, 60.0), || (), app_view))
        .gpu_loss_policy(GpuLossPolicy::Terminal);
    let _h2 = app_after.overlay(ov_spec(), State::default());
    let latch_after = app_after.gpu_loss_latch.as_ref().expect("app latch");
    let ov_latch_after = app_after.overlays[0].gpu_loss_latch().expect("overlay latch");
    assert!(Arc::ptr_eq(latch_after, &ov_latch_after), "overlay registered after policy shares app latch");

    // 3. Resetting back to Recover clears the latch on both app and owned overlays.
    let app_recover = app_after.gpu_loss_policy(GpuLossPolicy::Recover);
    assert!(app_recover.gpu_loss_latch.is_none());
    assert!(app_recover.overlays[0].gpu_loss_latch().is_none());
}

#[test]
fn window_failure_tears_down_owned_overlay_and_marks_unavailable_with_reason() {
    let latch = GpuLossLatch::new();
    let releases = Arc::new(AtomicUsize::new(0));
    let (mut overlay, handle) = make_overlay(Arc::clone(&latch), Arc::clone(&releases));

    // Overlay starts available and shown.
    overlay.ready(true);
    overlay.presence = Presence::Shown { occluded: false };
    assert_eq!(handle.availability(), Availability::Available);
    assert_eq!(overlay.rt.state.avail.len(), 1);

    // Main window trips the shared latch.
    assert!(latch.record_device_lost(false, "main window GPU lost"));

    // Overlay polls the shared latch: hides, drops compositor GPU, reports Unavailable + failure.
    let latched = overlay.poll_terminal_gpu_loss();
    assert!(latched);
    assert!(!overlay.presence.shown(), "latched overlay must hide");
    assert!(overlay.core.gpu.is_none());
    assert!(overlay.core.renderer.is_none());
    assert_eq!(releases.load(Ordering::SeqCst), 1, "release_compositor_gpu called once");
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(handle.failure().as_deref(), Some("main window GPU lost"));
    assert_eq!(overlay.rt.state.avail.len(), 2, "on_availability fired once on Unavailable");
    assert!(overlay.core.idle);
    assert_eq!(overlay.core.wake_deadline(overlay.rt.core()), None);

    // Polling again is idempotent (no duplicate release or notification).
    let still_latched = overlay.poll_terminal_gpu_loss();
    assert!(still_latched);
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    assert_eq!(overlay.rt.state.avail.len(), 2);
}

#[test]
fn overlay_failure_trips_shared_app_latch_and_wakes_app_once() {
    let latch = GpuLossLatch::new();
    let shared_app = Arc::new(SharedCore::default());
    let shared_cb = Arc::clone(&shared_app);
    let app_wakes = Arc::new(AtomicUsize::new(0));
    let app_wakes_cb = Arc::clone(&app_wakes);
    latch.add_wake(Arc::new(move |reason| {
        shared_cb.set_window_error(Some(reason.to_owned()));
        app_wakes_cb.fetch_add(1, Ordering::SeqCst);
    }));

    let releases = Arc::new(AtomicUsize::new(0));
    let (mut overlay, handle) = make_overlay(Arc::clone(&latch), Arc::clone(&releases));
    overlay.ready(true);
    overlay.presence = Presence::Shown { occluded: false };

    overlay.latch_terminal_loss_latch(
        GpuFailureKind::SurfaceFailed,
        "overlay surface refused config",
    );

    // Both the app's shared latch + window_error AND the overlay's handle reflect the failure.
    assert!(latch.is_failed());
    assert_eq!(shared_app.window_error().as_deref(), Some("overlay surface refused config"));
    assert_eq!(app_wakes.load(Ordering::SeqCst), 1);
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(handle.failure().as_deref(), Some("overlay surface refused config"));
    assert!(!overlay.presence.shown());
    assert_eq!(releases.load(Ordering::SeqCst), 1);

    // A second failure never overwrites the first reason or emits a second app wake.
    overlay.latch_terminal_loss_latch(
        GpuFailureKind::DeviceLost,
        "cascading loss",
    );
    assert_eq!(shared_app.window_error().as_deref(), Some("overlay surface refused config"));
    assert_eq!(app_wakes.load(Ordering::SeqCst), 1);
}

#[test]
fn overlay_park_does_not_park_terminal_gpu_into_global_warm_slot_or_falsely_latch() {
    if !crate::render::Gpu::test_adapter_available() {
        return;
    }
    let latch = GpuLossLatch::new();
    let releases = Arc::new(AtomicUsize::new(0));
    let (mut overlay, _handle) = make_overlay(Arc::clone(&latch), Arc::clone(&releases));
    let gpu = Arc::new(
        crate::render::Gpu::try_for_display(crate::render::Gpu::wgpu_instance(), wgpu::Features::empty())
            .expect("display wgpu device"),
    );
    let renderer = crate::render::Renderer::new(&gpu.device, crate::render::OFFSCREEN_FORMAT);
    overlay.core.attach_terminal_gpu_latch(&gpu);
    overlay.core.gpu = Some(gpu);
    overlay.core.renderer = Some(renderer);

    // Clear any pre-existing parked GPU before exercising OverlayCore::park.
    let _ = crate::render::warmup::take_parked_gpu_for_test();

    overlay.core.park();
    assert!(
        !latch.is_failed(),
        "clean OverlayCore::park must not falsely trip the app's terminal latch",
    );
    assert!(
        crate::render::warmup::take_parked_gpu_for_test().is_none(),
        "OverlayCore::park must never park a Terminal-latch-bound GPU into the global warm slot",
    );
}

#[test]
fn overlay_warm_offscreen_latches_on_compositor_validation_error_and_marks_unavailable() {
    if !crate::render::Gpu::test_adapter_available() {
        return;
    }
    struct BadCompositor(Arc<AtomicUsize>);
    impl Compositor<State> for BadCompositor {
        fn render(&mut self, f: &mut Frame<'_, State>) {
            drop(f.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay-warm-offscreen-validation"),
                size: 4,
                usage: wgpu::BufferUsages::empty(),
                mapped_at_creation: false,
            }));
        }
        fn release_gpu(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let latch = GpuLossLatch::new();
    let releases = Arc::new(AtomicUsize::new(0));
    let app_spec = AppSpec::new("ov-warm", (64.0, 32.0), State::default, view);
    let configured = OverlaySpec::new("ov-warm", app_spec.size, view)
        .options(OverlayOptions::default())
        .compositor(BadCompositor(Arc::clone(&releases)))
        .on_availability(on_avail);
    let (mut overlay, handle) = Overlay::new(configured, State::default(), &app_spec);
    overlay.core.gpu_loss_latch = Some(Arc::clone(&latch));
    overlay.rt.state.handle = Some(handle.clone());
    let gpu = Arc::new(
        crate::render::Gpu::try_for_display(crate::render::Gpu::wgpu_instance(), wgpu::Features::empty())
            .expect("display wgpu device"),
    );
    let renderer = crate::render::Renderer::new(&gpu.device, crate::render::OFFSCREEN_FORMAT);
    overlay.core.attach_terminal_gpu_latch(&gpu);
    overlay.core.gpu = Some(gpu);
    overlay.core.renderer = Some(renderer);
    overlay.ready(false);

    overlay.warm_offscreen((64, 32));
    assert!(latch.is_failed(), "validation error in warm_offscreen must latch Terminal");
    assert!(overlay.core.gpu.is_none(), "warm_offscreen must tear down overlay GPU on latch");
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert!(handle.failure().is_some());
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn overlay_compositor_features_destroying_retained_device_latches_and_clears_native_materials() {
    if !crate::render::Gpu::test_adapter_available() {
        return;
    }
    struct DestroyingFeatures {
        gpu: Arc<crate::render::Gpu>,
        releases: Arc<AtomicUsize>,
    }
    impl Compositor<State> for DestroyingFeatures {
        fn features(&self) -> wgpu::Features {
            self.gpu.device.destroy();
            wgpu::Features::empty()
        }
        fn render(&mut self, _f: &mut Frame<'_, State>) {}
        fn release_gpu(&mut self) {
            self.releases.fetch_add(1, Ordering::SeqCst);
        }
    }
    let gpu = Arc::new(
        crate::render::Gpu::try_for_display(crate::render::Gpu::wgpu_instance(), wgpu::Features::empty())
            .expect("display wgpu device"),
    );
    let latch = GpuLossLatch::new();
    let releases = Arc::new(AtomicUsize::new(0));
    let app_spec = AppSpec::new("ov-feat", (64.0, 32.0), State::default, view);
    let configured = OverlaySpec::new("ov-feat", app_spec.size, view)
        .options(OverlayOptions::default())
        .compositor(DestroyingFeatures { gpu: Arc::clone(&gpu), releases: Arc::clone(&releases) })
        .on_availability(on_avail);
    let (mut overlay, handle) = Overlay::new(configured, State::default(), &app_spec);
    overlay.core.gpu_loss_latch = Some(Arc::clone(&latch));
    overlay.rt.state.handle = Some(handle.clone());
    overlay.core.attach_terminal_gpu_latch(&gpu);
    overlay.core.gpu = Some(gpu);
    overlay.rt.core_mut().paint.set_native_materials(true);
    overlay.ready(true);

    // Mirror OverlayCore::attach after Compositor::features(): features() destroys the retained
    // device, then poll_device_for_terminal_loss polls and latches before any fallback attach.
    let _ = overlay.compositor.as_ref().unwrap().features();
    assert!(overlay.poll_device_for_terminal_loss());
    assert!(latch.is_failed());
    assert!(overlay.core.gpu.is_none());
    assert!(!overlay.rt.core().paint.native_materials(), "latched overlay clears native_materials");
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn overlay_in_frame_loss_defers_teardown_until_surface_texture_is_released() {
    let latch = GpuLossLatch::new();
    let releases = Arc::new(AtomicUsize::new(0));
    let (mut overlay, handle) = make_overlay(Arc::clone(&latch), Arc::clone(&releases));
    overlay.ready(true);
    overlay.presence = Presence::Shown { occluded: false };

    // Simulate an overlay frame holding a live SurfaceTexture (post-acquire, pre-present).
    overlay.core.note_frame_texture_acquired();
    assert!(latch.record_device_lost(false, "submit-time loss on shared device"));

    // Polling during render/submit MUST notice the latch WITHOUT tearing down while the
    // SurfaceTexture is still alive.
    assert!(overlay.poll_terminal_gpu_loss());
    assert!(overlay.presence.shown(), "overlay must not tear down surface while SurfaceTexture is alive");
    assert_eq!(releases.load(Ordering::SeqCst), 0, "release_compositor_gpu must wait until texture drops");
    assert_eq!(handle.availability(), Availability::Available);

    // Dropping the SurfaceTexture and signaling TextureReleased runs teardown immediately.
    overlay.note_frame_texture_released();
    assert!(!overlay.presence.shown(), "overlay must tear down once SurfaceTexture is dropped");
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(handle.failure().as_deref(), Some("submit-time loss on shared device"));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "SurfaceTexture is still alive")]
fn overlay_teardown_debug_asserts_while_frame_texture_is_alive() {
    let latch = GpuLossLatch::new();
    let releases = Arc::new(AtomicUsize::new(0));
    let (mut overlay, _handle) = make_overlay(latch, releases);
    overlay.core.note_frame_texture_acquired();
    overlay.core.assert_no_live_frame_texture();
}
