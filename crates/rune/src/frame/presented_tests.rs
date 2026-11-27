//! `App::on_frame_presented`: drawable-to-frame correlation, ordering, skipped drawables,
//! bounded queue eviction, idle watch clearing, display-link integration, and macOS Metal
//! off-screen `LayerHook` plumbing.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use super::*;
use crate::frame::encoded::{FrameReports, OnFrameEncoded};
use crate::platform::display_link_machine::{DisplayLinkCore, LinkEffect, LinkState, Opportunity};
use crate::{App, AppSpec, FrameEncoded, FrameOutcome, WindowSpec};

#[derive(Default)]
struct FakeSource {
    ready: Mutex<Vec<(u64, Option<Instant>)>>,
    watch: AtomicBool,
    poll_calls: AtomicUsize,
    watch_transitions: Mutex<Vec<bool>>,
}

impl FakeSource {
    fn complete(&self, drawable_seq: u64, at: Option<Instant>) {
        self.ready.lock().unwrap().push((drawable_seq, at));
    }

    fn watching(&self) -> bool { self.watch.load(Ordering::Acquire) }

    fn polls(&self) -> usize { self.poll_calls.load(Ordering::Acquire) }

    fn transitions(&self) -> Vec<bool> { self.watch_transitions.lock().unwrap().clone() }
}

impl PresentSource for FakeSource {
    fn poll_and_watch(&self, out: &mut Vec<(u64, Option<Instant>)>, watch: bool) {
        self.poll_calls.fetch_add(1, Ordering::AcqRel);
        let prev = self.watch.swap(watch, Ordering::AcqRel);
        if prev != watch {
            self.watch_transitions.lock().unwrap().push(watch);
        }
        out.append(&mut self.ready.lock().unwrap());
    }

    fn set_watch(&self, watch: bool) {
        let prev = self.watch.swap(watch, Ordering::AcqRel);
        if prev != watch {
            self.watch_transitions.lock().unwrap().push(watch);
        }
    }
}

type Seen = Rc<RefCell<Vec<FramePresented>>>;

fn logging() -> (Seen, OnFramePresented) {
    let seen = Seen::default();
    let sink = seen.clone();
    (seen, Box::new(move |p: &FramePresented| sink.borrow_mut().push(p.clone())))
}

#[test]
fn unset_hook_does_zero_work_and_never_watches() {
    let source = Arc::new(FakeSource::default());
    let mut reports = FramePresentReports::new(None, Some("main"));
    assert!(!reports.wanted());
    reports.set_source(source.clone());
    reports.push_and_drain(1, 0);
    source.complete(1, Some(crate::time::now()));
    reports.drain();
    reports.reset();
    assert_eq!(source.polls(), 0, "unset hook must never lock or poll the source");
    assert!(!source.watching(), "unset hook must never enable watch");
    assert!(source.transitions().is_empty());
    assert_eq!(reports.pending_len(), 0);
}

#[test]
fn correlates_drawable_seq_to_frame_encoded_seq_in_submission_order() {
    let base = crate::time::now();
    let t = |ms: u64| base + Duration::from_millis(ms);
    let source = Arc::new(FakeSource::default());
    let encoded_log = Rc::new(RefCell::new(Vec::<FrameEncoded>::new()));
    let enc_sink = encoded_log.clone();
    let enc_hook: OnFrameEncoded = Box::new(move |f| enc_sink.borrow_mut().push(f.clone()));
    let mut encoded = FrameReports::with_presented_seq(Some(enc_hook), true, Some("editor"));
    let (seen, pres_hook) = logging();
    let mut presented = FramePresentReports::new(Some(pres_hook), Some("editor"));
    presented.set_source(source.clone());

    // Frame 0: Drawn (drawable 1)
    let s0 = encoded.drawn(Some(t(1)), Some(t(8)), t(0), 0.5).unwrap();
    presented.push_and_drain(1, s0);
    // Frame 1: Unchanged (no drawable)
    encoded.unchanged(Some(t(16)));
    // Frame 2: Skipped (no drawable)
    encoded.skipped();
    // Frame 3: Drawn (drawable 2)
    let s3 = encoded.drawn(Some(t(25)), Some(t(33)), t(24), 0.5).unwrap();
    presented.push_and_drain(2, s3);

    assert_eq!((s0, s3), (0, 3));
    assert!(seen.borrow().is_empty(), "neither drawable has landed yet");
    assert!(source.watching() && presented.is_watching());

    // Out-of-order: drawable 2 lands before drawable 1; submission order holds it back.
    source.complete(2, Some(t(34)));
    presented.drain();
    assert!(seen.borrow().is_empty(), "drawable 2 waits for drawable 1");
    assert_eq!(presented.pending_len(), 2);
    assert!(source.watching());

    // Drawable 1 lands -> both fire in submission order (seq 0, then seq 3), watch clears.
    source.complete(1, Some(t(9)));
    presented.drain();
    assert_eq!(presented.pending_len(), 0);
    assert!(!source.watching() && !presented.is_watching());

    let got: Vec<_> = seen.borrow().iter().map(|p| (p.seq(), p.window().map(str::to_owned), p.presented_at())).collect();
    assert_eq!(got, vec![
        (0, Some("editor".to_owned()), Some(t(9))),
        (3, Some("editor".to_owned()), Some(t(34))),
    ]);
    let drawn_seqs: Vec<u64> = encoded_log.borrow().iter().filter(|f| f.outcome() == FrameOutcome::Drawn).map(FrameEncoded::seq).collect();
    assert_eq!(drawn_seqs, vec![0, 3]);
}

#[test]
fn skipped_drawable_reports_none_and_turns_watch_off() {
    let source = Arc::new(FakeSource::default());
    let (seen, hook) = logging();
    let mut reports = FramePresentReports::new(Some(hook), Some("main"));
    reports.set_source(source.clone());

    reports.push_and_drain(7, 42);
    assert!(source.watching());

    source.complete(7, None);
    reports.drain();
    assert!(!source.watching(), "skipped last drawable must clear watch so idle window never wakes");
    assert_eq!(reports.pending_len(), 0);
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!((seen[0].seq(), seen[0].window(), seen[0].presented_at()), (42, Some("main"), None));
}

#[test]
fn queue_evicts_oldest_at_cap_64_and_unblocks_newer_frames() {
    let base = crate::time::now();
    let source = Arc::new(FakeSource::default());
    let (seen, hook) = logging();
    let mut reports = FramePresentReports::new(Some(hook), Some("main"));
    reports.set_source(source.clone());

    // Push MAX_PENDING (64) frames: drawable 1 never reports, drawables 2..=64 land.
    reports.push_and_drain(1, 0);
    for i in 1..MAX_PENDING as u64 {
        reports.push_and_drain(i + 1, i);
        source.complete(i + 1, Some(base + Duration::from_millis(i)));
    }
    reports.drain();
    assert_eq!(reports.pending_len(), MAX_PENDING);
    assert!(seen.borrow().is_empty(), "drawable 1 still holds frames 1..63 back");

    // Frame 64 (drawable 65) evicts drawable 1 without faking a callback for frame 0,
    // immediately flushing ready frames 1..=63 in order.
    reports.push_and_drain(MAX_PENDING as u64 + 1, MAX_PENDING as u64);
    assert_eq!(reports.pending_len(), 1, "only the newly pushed frame 64 remains pending");
    let flushed: Vec<u64> = seen.borrow().iter().map(FramePresented::seq).collect();
    let expected: Vec<u64> = (1..MAX_PENDING as u64).collect();
    assert_eq!(flushed, expected, "evicted frame 0 is omitted rather than faked; 1..=63 flush in order");

    // Once frame 64 lands, queue empties and watch turns off.
    source.complete(MAX_PENDING as u64 + 1, Some(base + Duration::from_millis(100)));
    reports.drain();
    assert_eq!(reports.pending_len(), 0);
    assert!(!source.watching());
    assert_eq!(seen.borrow().last().unwrap().seq(), MAX_PENDING as u64);
}

#[test]
fn watch_enables_while_pending_and_clears_when_drained_so_idle_never_wakes() {
    let base = crate::time::now();
    let source = Arc::new(FakeSource::default());
    let (seen, hook) = logging();
    let mut reports = FramePresentReports::new(Some(hook), Some("main"));
    reports.set_source(source.clone());

    // Idle before any frame: drain does not even poll the source.
    for _ in 0..5 {
        reports.drain();
    }
    assert_eq!(source.polls(), 0);
    assert!(!source.watching());
    assert!(seen.borrow().is_empty());

    // Push frame 0 -> watch turns on; frame lands -> watch turns off.
    reports.push_and_drain(1, 0);
    assert_eq!(source.transitions(), vec![true]);
    source.complete(1, Some(base));
    reports.drain();
    assert_eq!(source.transitions(), vec![true, false]);

    // Idle again: repeated turns do zero source polls and zero callbacks.
    let polls_after_land = source.polls();
    for _ in 0..10 {
        reports.drain();
    }
    assert_eq!(source.polls(), polls_after_land, "idle window must never poll source");
    assert!(!source.watching());
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn synchronous_landing_before_push_clears_watch_immediately() {
    let base = crate::time::now();
    let source = Arc::new(FakeSource::default());
    let (seen, hook) = logging();
    let mut reports = FramePresentReports::new(Some(hook), Some("main"));
    reports.set_source(source.clone());

    source.complete(1, Some(base));
    reports.push_and_drain(1, 0);
    assert_eq!(reports.pending_len(), 0);
    assert!(!source.watching(), "watch must be cleared immediately when drained during push");
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(seen.borrow()[0].presented_at(), Some(base));
}

#[test]
fn reset_delivers_already_landed_clears_orphans_and_stops_watching() {
    let base = crate::time::now();
    let source = Arc::new(FakeSource::default());
    let (seen, hook) = logging();
    let mut reports = FramePresentReports::new(Some(hook), Some("main"));
    reports.set_source(source.clone());

    reports.push_and_drain(1, 0);
    reports.push_and_drain(2, 1);
    source.complete(1, Some(base));

    reports.reset();
    assert_eq!(reports.pending_len(), 0);
    assert!(!source.watching());
    assert_eq!(seen.borrow().len(), 1, "already-landed frame 0 still delivered; orphaned frame 1 dropped");
    assert_eq!(seen.borrow()[0].seq(), 0);
}

#[test]
fn frame_encoded_always_fires_before_frame_presented_for_same_seq() {
    let base = crate::time::now();
    let order = Rc::new(RefCell::new(Vec::<(&'static str, u64)>::new()));
    let (o_enc, o_pres) = (order.clone(), order.clone());
    let mut encoded = FrameReports::with_presented_seq(
        Some(Box::new(move |f| o_enc.borrow_mut().push(("encoded", f.seq())))),
        true,
        Some("main"),
    );
    let mut presented = FramePresentReports::new(
        Some(Box::new(move |p| o_pres.borrow_mut().push(("presented", p.seq())))),
        Some("main"),
    );
    let source = Arc::new(FakeSource::default());
    presented.set_source(source.clone());

    // Even when Metal completes drawable 1 before settle runs, settle calls encoded.drawn()
    // before presented.push_and_drain(), so encoded(0) always precedes presented(0).
    source.complete(1, Some(base + Duration::from_millis(2)));
    let seq = encoded.drawn(Some(base + Duration::from_millis(1)), None, base, 0.5).unwrap();
    presented.push_and_drain(1, seq);

    assert_eq!(*order.borrow(), vec![("encoded", 0), ("presented", 0)]);
}

#[test]
fn only_on_frame_presented_without_on_frame_encoded_numbers_seq_without_stamping() {
    let base = crate::time::now();
    let mut reports = FrameReports::with_presented_seq(None, true, Some("main"));
    assert_eq!(reports.stamp(), None, "no encode timestamp when on_frame_encoded is unset");
    assert!(!reports.wanted(), "no refresh_hz query when on_frame_encoded is unset");
    assert_eq!(reports.drawn(None, None, base, 0.0), Some(0));
    reports.unchanged(None);
    reports.skipped();
    assert_eq!(reports.drawn(None, None, base, 0.0), Some(3));
}

#[test]
fn window_spec_keeps_app_on_frame_presented_until_overridden() {
    fn spec() -> AppSpec<()> { AppSpec::new("test", (100.0, 100.0), || (), |_, _| crate::el()) }
    let calls = Rc::new(RefCell::new(Vec::new()));
    let (app_calls, win_calls) = (calls.clone(), calls.clone());
    let source = Arc::new(FakeSource::default());

    let w1 = WindowSpec::from_app("main", App::new(spec()).on_frame_presented(move |_| app_calls.borrow_mut().push("app")));
    let mut r1 = FramePresentReports::new(w1.app.on_frame_presented, Some("main"));
    r1.set_source(source.clone());
    source.complete(1, Some(crate::time::now()));
    r1.push_and_drain(1, 0);

    let w2 = WindowSpec::from_app("main", App::new(spec()).on_frame_presented(|_| unreachable!()))
        .on_frame_presented(move |_| win_calls.borrow_mut().push("window"));
    let mut r2 = FramePresentReports::new(w2.app.on_frame_presented, Some("main"));
    r2.set_source(source.clone());
    source.complete(2, Some(crate::time::now()));
    r2.push_and_drain(2, 0);

    assert!(WindowSpec::new("plain", spec()).app.on_frame_presented.is_none());
    assert_eq!(*calls.borrow(), vec!["app", "window"]);
}

/// Drives the macOS `DisplayLinkCore` state machine alongside `FrameReports` and
/// `FramePresentReports` on an injected clock to prove the full active -> coast -> idle rest
/// lifecycle without a real 120 Hz panel: frames correlate against display-link targets, in-flight
/// drawables finish after the window settles, watch turns off, and the resting window draws and
/// calls nothing.
#[test]
fn display_link_state_machine_and_present_reports_settle_cleanly_at_rest() {
    let period = Duration::from_nanos(1_000_000_000 / 120);
    let t0 = crate::time::now();
    let mut link = DisplayLinkCore::new(period);
    let source = Arc::new(FakeSource::default());

    let targets = Rc::new(RefCell::new(Vec::<(u64, Option<Instant>)>::new()));
    let t_sink = targets.clone();
    let mut encoded = FrameReports::with_presented_seq(
        Some(Box::new(move |f: &FrameEncoded| {
            if f.outcome() == FrameOutcome::Drawn {
                t_sink.borrow_mut().push((f.seq(), f.presents_at()));
            }
        })),
        true,
        Some("main"),
    );
    let (seen, hook) = logging();
    let mut presented = FramePresentReports::new(Some(hook), Some("main"));
    presented.set_source(source.clone());

    // Start pacing on 120 Hz display link.
    assert_eq!(link.start().into_iter().collect::<Vec<_>>(), vec![LinkEffect::StartLink]);
    assert_eq!(link.state(), LinkState::Active);

    // Draw 3 display-paced frames (drawables 1..=3).
    let mut tick_targets = Vec::new();
    for i in 0u64..3 {
        let now = t0 + period * i as u32;
        let target = now + period;
        let opp = link.claim(Some(target), now);
        assert_eq!(opp, Opportunity::Tick(target));
        tick_targets.push(target);
        let frame_seq = encoded.drawn(Some(now + Duration::from_millis(1)), Some(target), now, 0.4).unwrap();
        link.presented(now + Duration::from_millis(1));
        presented.push_and_drain(i + 1, frame_seq);
    }
    assert!(source.watching() && presented.pending_len() == 3);

    // Motion finishes -> display link coasts and stops with zero redraws.
    let coast_effects: Vec<_> = link.coast().into_iter().collect();
    assert!(matches!(coast_effects.as_slice(), [LinkEffect::ArmCoast { .. }]));
    assert!(matches!(link.state(), LinkState::Coasting { .. }));

    // Drawables 1 (on time), 2 (late by 1 refresh — missed vsync detectable), 3 (skipped) land
    // while coasting/resting.
    source.complete(1, Some(tick_targets[0]));
    source.complete(2, Some(tick_targets[1] + period));
    source.complete(3, None);
    presented.drain();
    assert_eq!(presented.pending_len(), 0);
    assert!(!source.watching(), "watch cleared once all 3 in-flight drawables landed");

    // Coast expires -> display link stops completely.
    while matches!(link.state(), LinkState::Coasting { ticks_left } if ticks_left > 0) {
        let _ = link.coast_tick();
    }
    assert_eq!(link.stop_if_expired(false).into_iter().collect::<Vec<_>>(), vec![LinkEffect::StopLink]);
    assert_eq!(link.state(), LinkState::Stopped);
    assert!(!link.hardware_running());

    // Resting window: further turns poll nothing, draw nothing, and invoke no callbacks.
    let polls_before_rest = source.polls();
    for _ in 0..20 {
        presented.drain();
    }
    assert_eq!(source.polls(), polls_before_rest);
    assert!(!source.watching());

    let seen = seen.borrow();
    let targets = targets.borrow();
    assert_eq!(seen.len(), 3);
    assert_eq!(targets.len(), 3);
    // Frame 0 landed on target; frame 1 missed by one 120 Hz refresh; frame 2 was skipped.
    assert_eq!(seen[0].seq(), targets[0].0);
    assert_eq!(seen[0].presented_at().unwrap().saturating_duration_since(targets[0].1.unwrap()), Duration::ZERO);
    assert_eq!(seen[1].seq(), targets[1].0);
    assert_eq!(seen[1].presented_at().unwrap().saturating_duration_since(targets[1].1.unwrap()), period);
    assert_eq!(seen[2].seq(), targets[2].0);
    assert_eq!(seen[2].presented_at(), None);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_log_separates_trace_callbacks_and_shadow_watches_and_bounds_buffer() {
    use crate::frame::present_time::{Source, metal};

    let log = Arc::new(metal::Log::new(true, true));
    let mut out_cb = Vec::new();
    let mut out_trace = Vec::new();

    // Enable callback watch; shadow watch stays off.
    log.poll_and_watch(&mut out_cb, true);
    assert!(log.callback_watching());

    // Skipped drawable (presentedTime == 0.0) MUST wake callback watch, and lands in both
    // trace and callback buffers without advancing shadow's on_screen.
    assert!(log.record_presented(1, 0.0), "skipped drawable must wake watched callback consumer");
    assert_eq!(log.on_screen(), 0, "skipped drawable is not on screen");

    // Shadow turning its watch off does not disturb callback_watching; clearing callback watch
    // does not disturb shadow watch.
    log.watch(false);
    assert!(log.callback_watching());
    log.set_watch(false);
    assert!(!log.callback_watching());
    log.watch(true);
    assert!(!log.record_presented(2, 0.0), "shadow watch ignores skipped drawables");
    assert!(log.record_presented(3, 123.456), "shadow watch wakes on real present (at > 0.0)");
    assert_eq!(log.on_screen(), 3);

    // Draining callbacks leaves trace intact, and draining trace leaves callbacks intact.
    log.poll_and_watch(&mut out_cb, false);
    assert_eq!(out_cb.len(), 3);
    assert_eq!(out_cb[0].0, 1);
    assert_eq!(out_cb[0].1, None);
    assert_eq!(out_cb[1].0, 2);
    assert_eq!(out_cb[1].1, None);
    assert_eq!(out_cb[2].0, 3);
    assert!(out_cb[2].1.is_some());

    log.take(&mut out_trace);
    assert_eq!(out_trace.len(), 3, "trace consumer sees all 3 completions independently");

    // Callback buffer is bounded at MAX_PENDING (64): pushing 100 retains only the newest 64.
    for seq in 1..=100u64 {
        let _ = log.record_presented(seq, 200.0 + seq as f64 * 0.008);
    }
    out_cb.clear();
    log.poll_and_watch(&mut out_cb, false);
    assert_eq!(out_cb.len(), MAX_PENDING);
    assert_eq!(out_cb.first().unwrap().0, 100 - MAX_PENDING as u64 + 1);
    assert_eq!(out_cb.last().unwrap().0, 100);
}

#[cfg(target_os = "macos")]
#[test]
#[allow(unsafe_code)]
fn metal_layer_hook_attaches_to_offscreen_metal_surface_and_tracks_acquired_drawables() {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;
    use crate::frame::present_time::metal;

    let Some(gpu) = crate::render::Gpu::test_headless() else { return };
    let layer_cls = AnyClass::get(c"CAMetalLayer").expect("CAMetalLayer class");
    // SAFETY: allocates a standalone off-screen CAMetalLayer (no NSWindow, nothing visible).
    let layer: Retained<AnyObject> = unsafe { msg_send![layer_cls, new] };
    let original_cls = layer.class();

    // SAFETY: layer is a live CAMetalLayer retained for the surface's lifetime.
    let surface = unsafe {
        gpu.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(
            Retained::as_ptr(&layer) as *mut std::ffi::c_void,
        ))
    }
    .expect("create off-screen CAMetalLayer surface");

    let caps = surface.get_capabilities(&gpu.adapter);
    let format = caps.formats.first().copied().unwrap_or(wgpu::TextureFormat::Bgra8Unorm);
    surface.configure(&gpu.device, &wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: 16,
        height: 16,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Opaque),
        view_formats: vec![],
        color_space: Default::default(),
    });

    let log = Arc::new(metal::Log::new(false, true));
    let hook = metal::LayerHook::attach(&surface, log.clone())
        .expect("LayerHook must attach to off-screen Metal surface");
    assert!(
        layer.class().name().to_bytes().starts_with(b"RunePresentTimed_"),
        "LayerHook swaps CAMetalLayer isa to RunePresentTimed subclass"
    );

    // Acquire one drawable through the Metal backend without opening any window.
    let acquired = surface.get_current_texture();
    if let wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) = acquired {
        assert_eq!(log.acquired(), Some(1), "nextDrawable hook must record drawable seq 1");
        assert_eq!(log.last_acquired(), 1);
        drop(frame);
    } else {
        panic!("off-screen CAMetalLayer get_current_texture failed: {acquired:?}");
    }

    drop(hook);
    assert!(
        std::ptr::eq(layer.class(), original_cls),
        "dropping LayerHook must restore the original CAMetalLayer class"
    );
    drop(surface);
}

#[test]
fn presented_delivery_records_first_on_display_launch_mark_with_real_timestamp() {
    let source = Arc::new(FakeSource::default());
    let mut reports = FramePresentReports::new(Some(Box::new(|_| {})), Some("main"));
    reports.set_source(source.clone());

    let landed_at = crate::time::now() - Duration::from_millis(17);
    // Skipped drawable (None) must not stamp frame.first.on_display; the first landed one stamps it.
    source.complete(1, None);
    source.complete(2, Some(landed_at));
    reports.push_and_drain(1, 1);
    reports.push_and_drain(2, 2);

    let mark = crate::marks::marks()
        .into_iter()
        .find(|(name, _)| *name == "frame.first.on_display")
        .map(|(_, at)| at);
    assert!(mark.is_some(), "frame.first.on_display must be recorded when a drawable lands");
    // In isolation (or if this test runs first in the process), the timestamp matches landed_at.
    if let Some(at) = mark {
        assert!(at <= crate::time::now(), "frame.first.on_display timestamp must not be in the future");
    }
}
