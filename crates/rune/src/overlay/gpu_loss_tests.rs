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
