//! Host-side tests for `GpuLossPolicy::Terminal` latch wiring, guard points, same-device
//! healthy surface recreation, `Frame::restore_device_callbacks`, and Linux no-op GPU failure paths.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use winit::event::WindowEvent;
use winit::keyboard::ModifiersState;

use super::{
    LatchedEvent, LatchedWakeAction, LostSurfaceAction, TerminalLoss,
    classify_latched_wake_event, classify_latched_window_event, decide_lost_surface,
};
use crate::app::{SharedCore, WindowRequest};
use crate::render::loss_latch::{GpuFailureKind, GpuLossPolicy};
use crate::render::{Gpu, OFFSCREEN_FORMAT, Renderer};
use crate::window::WakeEvent;
use crate::window::host::lifecycle::{CloseDecision, close_request_core};
use crate::window::state::Life;
use crate::{App, AppSpec, CloseResponse, Cx, El, Tx, WindowSpec, el};

#[test]
fn decide_lost_surface_distinguishes_healthy_surface_recreation_from_device_loss() {
    // Recover policy keeps bounded recovery whenever GPU is lost or surface is missing.
    assert_eq!(decide_lost_surface(GpuLossPolicy::Recover, false, false, false), LostSurfaceAction::Healthy);
    assert_eq!(decide_lost_surface(GpuLossPolicy::Recover, false, false, true), LostSurfaceAction::Recover);
    assert_eq!(decide_lost_surface(GpuLossPolicy::Recover, false, true, false), LostSurfaceAction::Recover);

    // Terminal policy allows same-device surface recreation ONLY while the device and latch are healthy.
    assert_eq!(decide_lost_surface(GpuLossPolicy::Terminal, false, false, false), LostSurfaceAction::Healthy);
    assert_eq!(
        decide_lost_surface(GpuLossPolicy::Terminal, false, false, true),
        LostSurfaceAction::RecreateSurfaceSameDevice,
    );
    assert_eq!(decide_lost_surface(GpuLossPolicy::Terminal, false, true, false), LostSurfaceAction::LatchTerminal);
    assert_eq!(decide_lost_surface(GpuLossPolicy::Terminal, false, true, true), LostSurfaceAction::LatchTerminal);
    assert_eq!(decide_lost_surface(GpuLossPolicy::Terminal, true, false, false), LostSurfaceAction::LatchTerminal);
    assert_eq!(decide_lost_surface(GpuLossPolicy::Terminal, true, false, true), LostSurfaceAction::LatchTerminal);
}

#[test]
fn terminal_loss_publishes_first_reason_before_one_shot_wake_and_preserves_it() {
    let shared = Arc::new(SharedCore::default());
    let shared_in_wake = Arc::clone(&shared);
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    let tl = TerminalLoss::new(GpuLossPolicy::Terminal, None);
    tl.connect_wake(
        Arc::clone(&shared),
        Arc::new(move || {
            // window_error must already hold the first reason when AppSpec::poll wakes.
            assert_eq!(shared_in_wake.window_error().as_deref(), Some("first-gpu-error"));
            wakes_cb.fetch_add(1, Ordering::SeqCst);
        }),
    );

    let latch = tl.latch().expect("Terminal policy has a latch");
    assert!(latch.record_device_lost(false, "first-gpu-error"));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert_eq!(shared.window_error().as_deref(), Some("first-gpu-error"));

    // Cascading errors never overwrite window_error or emit a second wake.
    assert!(latch.record_failure(GpuFailureKind::UncapturedError, "second-error"));
    assert!(latch.record_failure(GpuFailureKind::SurfaceFailed, "third-error"));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert_eq!(shared.window_error().as_deref(), Some("first-gpu-error"));
}

#[test]
fn latched_window_and_wake_events_drop_view_input_and_redraws_while_keeping_close_and_shell() {
    assert_eq!(classify_latched_window_event(&WindowEvent::CloseRequested), LatchedEvent::Close);
    assert_eq!(
        classify_latched_window_event(&WindowEvent::ModifiersChanged(ModifiersState::CONTROL.into())),
        LatchedEvent::ModifiersChanged(ModifiersState::CONTROL),
    );
    assert_eq!(classify_latched_window_event(&WindowEvent::RedrawRequested), LatchedEvent::Drop);
    assert_eq!(
        classify_latched_window_event(&WindowEvent::Resized(winit::dpi::PhysicalSize::new(640, 480))),
        LatchedEvent::Drop,
    );
    assert_eq!(classify_latched_window_event(&WindowEvent::Focused(true)), LatchedEvent::Drop);
    assert_eq!(
        classify_latched_window_event(&WindowEvent::HoveredFileCancelled),
        LatchedEvent::Drop,
    );

    // Wake events: App wake, Close/Hide, and non-GPU shell actions pass; Show/Open/DisplayTick/Live drop.
    assert_eq!(classify_latched_wake_event(&WakeEvent::App(0)), LatchedWakeAction::DrainApp);
    assert_eq!(
        classify_latched_wake_event(&WakeEvent::Window(0, WindowRequest::Close)),
        LatchedWakeAction::CloseOrHide,
    );
    assert_eq!(
        classify_latched_wake_event(&WakeEvent::Window(0, WindowRequest::Hide)),
        LatchedWakeAction::CloseOrHide,
    );
    assert_eq!(
        classify_latched_wake_event(&WakeEvent::Window(0, WindowRequest::Show)),
        LatchedWakeAction::Drop,
    );
    assert_eq!(
        classify_latched_wake_event(&WakeEvent::Window(0, WindowRequest::Open(None))),
        LatchedWakeAction::Drop,
    );
    assert_eq!(
        classify_latched_wake_event(&WakeEvent::Window(0, WindowRequest::Open(Some("tok".into())))),
        LatchedWakeAction::Drop,
    );
    assert_eq!(classify_latched_wake_event(&WakeEvent::Live(0)), LatchedWakeAction::Drop);
    assert_eq!(classify_latched_wake_event(&WakeEvent::Focus(0)), LatchedWakeAction::NonGpuShell);
    assert_eq!(
        classify_latched_wake_event(&WakeEvent::ColorScheme(0, crate::ColorScheme::Dark)),
        LatchedWakeAction::NonGpuShell,
    );
}

#[test]
fn initial_create_failure_finishes_without_keep_running_and_stays_alive_with_keep_running() {
    let mut life_exit = Life::default();
    assert!(
        life_exit.fail_create(false, false),
        "initial try_create failure with !keep_running must finish with startup_error",
    );
    assert!(life_exit.finished());

    let mut life_keep = Life::default();
    assert!(
        !life_keep.fail_create(true, false),
        "initial try_create failure with keep_running must stay alive without finishing",
    );
    assert!(!life_keep.finished());
}

#[test]
fn close_request_defer_and_poll_hidden_title_work_while_latched_without_redraw() {
    use crate::app::Shared;
    use crate::runtime::{Env, Runtime};
    use crate::window::host::app::TypedWindowApp;

    struct State {
        polls: u32,
        defer_close: bool,
        shared: Arc<SharedCore>,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El { el().size(100.0, 60.0) }
    fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
        s.polls += 1;
        if let Some(err) = s.shared.window_error() {
            tx.set_title(format!("GPU Lost: {err}"));
        }
        false
    }

    let shared = Arc::new(Shared::<State>::default());
    let shared_core = shared.core.clone();
    let mut rt = Runtime::new(
        State { polls: 0, defer_close: true, shared: shared_core.clone() },
        view,
        (100.0, 60.0),
        Env::new(true, 1.0),
    );
    rt.poll = Some(poll);
    let mut app = TypedWindowApp {
        rt,
        shared,
        comp: None,
        on_frame_drawn: None,
        on_close_request: Some(|s: &mut State, tx: &mut Tx| {
            if s.defer_close {
                tx.set_title("Close deferred while frozen");
                CloseResponse::Defer
            } else {
                CloseResponse::Close
            }
        }),
        on_quit_request: None,
        on_close: None,
        on_rest: None,
        on_quit: None,
        on_reopen: None,
        on_geometry: None,
        on_placement: None,
        turn_scope: None,
        keep_layout: None,
        kept_layout: None,
    };

    // Simulate latch wake: window_error is set before poll_hidden runs, and Tx::set_title lands.
    shared_core.set_window_error(Some("device lost".into()));
    let _ = app.rt.poll_hidden();
    assert_eq!(app.rt.state.polls, 1);
    assert_eq!(
        app.rt.core_mut().host.window_title.take_change(),
        Some("GPU Lost: device lost"),
    );

    // CloseRequested with Defer keeps the frozen window open and applies Tx::set_title.
    let mut clipboard = crate::clipboard::Client::default();
    let mut ends_app = false;
    let decision = close_request_core(&mut app, &mut clipboard, true, false, &mut ends_app);
    assert_eq!(decision, CloseDecision::Defer);
    assert_eq!(
        app.rt.core_mut().host.window_title.take_change(),
        Some("Close deferred while frozen"),
    );

    // CloseRequested returning Close permits closing the latched window.
    app.rt.state.defer_close = false;
    let decision = close_request_core(&mut app, &mut clipboard, true, false, &mut ends_app);
    assert_eq!(decision, CloseDecision::Close);
}

#[test]
fn app_and_window_spec_builders_wire_gpu_loss_policy() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el() }
    let app_default = App::new(AppSpec::new("Default", (100.0, 60.0), || (), view));
    assert_eq!(app_default.gpu_loss_policy, GpuLossPolicy::Recover);
    assert!(app_default.gpu_loss_latch.is_none());

    let app_term = App::new(AppSpec::new("Term", (100.0, 60.0), || (), view))
        .gpu_loss_policy(GpuLossPolicy::Terminal);
    assert_eq!(app_term.gpu_loss_policy, GpuLossPolicy::Terminal);
    assert!(app_term.gpu_loss_latch.is_some());

    let spec = WindowSpec::new("sub", AppSpec::new("Sub", (100.0, 60.0), || (), view))
        .gpu_loss_policy(GpuLossPolicy::Terminal);
    assert_eq!(spec.app.gpu_loss_policy, GpuLossPolicy::Terminal);
    assert!(spec.app.gpu_loss_latch.is_some());
}

#[test]
fn terminal_callbacks_survive_restore_device_callbacks_and_latch_validation_and_destroy() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let gpu = Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
        .expect("display wgpu device");
    let shared = Arc::new(SharedCore::default());
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    let tl = TerminalLoss::new(GpuLossPolicy::Terminal, None);
    tl.connect_wake(
        Arc::clone(&shared),
        Arc::new(move || {
            wakes_cb.fetch_add(1, Ordering::SeqCst);
        }),
    );
    tl.attach_gpu(&gpu);

    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.attach_gpu(&gpu);

    // Overwrite both single-slot callbacks with foreign handlers, then restore Rune's callbacks.
    let foreign_lost = Arc::new(AtomicUsize::new(0));
    let foreign_err = Arc::new(AtomicUsize::new(0));
    let fl = Arc::clone(&foreign_lost);
    let fe = Arc::clone(&foreign_err);
    gpu.device.set_device_lost_callback(move |_, _| {
        fl.fetch_add(1, Ordering::SeqCst);
    });
    gpu.device.on_uncaptured_error(Arc::new(move |_| {
        fe.fetch_add(1, Ordering::SeqCst);
    }));
    renderer.restore_device_callbacks(&gpu.device);

    // Trigger an uncaptured validation error: under Terminal (even under cfg(test)) it must latch
    // without panicking, record the first reason, and wake once.
    drop(gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terminal-validation-probe"),
        size: 4,
        usage: wgpu::BufferUsages::empty(),
        mapped_at_creation: false,
    }));
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();

    assert!(tl.is_failed(), "uncaptured validation error must latch Terminal");
    assert!(!gpu.is_lost(), "uncaptured error under Terminal must not mark a healthy shared GPU lost for Recover siblings");
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert_eq!(foreign_err.load(Ordering::SeqCst), 0);
    let first = shared.window_error().expect("first error recorded");
    assert!(first.contains("uncaptured GPU error"), "got: {first}");

    // Subsequent device.destroy() must NOT invoke the foreign callback, overwrite reason, or re-wake.
    gpu.device.destroy();
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
    assert_eq!(foreign_lost.load(Ordering::SeqCst), 0);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert_eq!(shared.window_error().as_deref(), Some(first.as_str()));
}

#[test]
fn clean_gpu_drop_detaches_terminal_latch_while_unexpected_destroy_trips_it() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let tl = TerminalLoss::new(GpuLossPolicy::Terminal, None);
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    tl.connect_wake(
        Arc::new(SharedCore::default()),
        Arc::new(move || {
            wakes_cb.fetch_add(1, Ordering::SeqCst);
        }),
    );

    // 1. Clean release_gpu_when_closed: prepare_clean_gpu_drop detaches before drop so
    //    wgpu's drop-triggered Destroyed does not falsely latch.
    {
        let gpu = Arc::new(
            Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
                .expect("display wgpu device"),
        );
        tl.attach_gpu(&gpu);
        tl.prepare_clean_gpu_drop(Some(&gpu));
        drop(gpu);
    }
    assert!(!tl.is_failed(), "clean GPU drop must not trip Terminal latch");
    assert_eq!(wakes.load(Ordering::SeqCst), 0);

    // 2. Reopening attaches a fresh GPU and re-arms the latch; unexpected destroy trips it.
    let gpu2 = Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
        .expect("reopened wgpu device");
    tl.attach_gpu(&gpu2);
    gpu2.device.destroy();
    rune_gpu::health::poll(&gpu2.device, wgpu::PollType::Poll).ok();
    assert!(tl.is_failed(), "unexpected device.destroy() after reopen must latch");
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
}

#[test]
fn mixed_terminal_and_recover_shared_gpu_isolates_uncaptured_error_and_propagates_real_loss() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let gpu = Arc::new(
        Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
            .expect("display wgpu device"),
    );
    let shared = Arc::new(SharedCore::default());
    let tl = TerminalLoss::new(GpuLossPolicy::Terminal, None);
    tl.connect_wake(Arc::clone(&shared), Arc::new(|| {}));
    tl.attach_gpu(&gpu);

    // Uncaptured validation error latches Terminal, but must NOT mark the shared healthy Gpu
    // lost so a sibling Recover window on the same Gpu continues uninterrupted.
    drop(gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mixed-pool-validation-probe"),
        size: 4,
        usage: wgpu::BufferUsages::empty(),
        mapped_at_creation: false,
    }));
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();

    assert!(tl.is_failed(), "Terminal must latch on uncaptured validation error");
    assert!(
        !gpu.is_lost(),
        "uncaptured error must not force a sibling Recover window off a healthy shared Gpu",
    );

    // Real device loss still marks gpu.is_lost() so Recover sees it and recreates cleanly.
    gpu.device.destroy();
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
    assert!(gpu.is_lost(), "real device.destroy() must mark gpu.is_lost() for Recover");
}

#[test]
fn clean_drop_of_one_terminal_window_preserves_destroyed_detection_on_sibling_terminal() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let gpu_b = Arc::new(
        Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
            .expect("display wgpu device"),
    );
    let gpu_a = Arc::clone(&gpu_b);

    let tl_a = TerminalLoss::new(GpuLossPolicy::Terminal, None);
    let tl_b = TerminalLoss::new(GpuLossPolicy::Terminal, None);
    tl_a.attach_gpu(&gpu_a);
    tl_b.attach_gpu(&gpu_b);

    // Window A closes cleanly (release_gpu_when_closed) while Window B still shares the Gpu.
    tl_a.prepare_clean_gpu_drop(Some(&gpu_a));
    drop(gpu_a);
    assert!(!tl_a.is_failed());
    assert!(!tl_b.is_failed());

    // Later unexpected destroy on the shared Gpu MUST still trip Window B's Terminal latch.
    gpu_b.device.destroy();
    rune_gpu::health::poll(&gpu_b.device, wgpu::PollType::Poll).ok();
    assert!(!tl_a.is_failed(), "detached Window A must stay unlatched");
    assert!(
        tl_b.is_failed(),
        "clean close of Window A must NOT suppress Destroyed detection on sibling Terminal Window B",
    );
}

#[test]
fn handover_failure_policy_latches_terminal_without_retry_or_cpu_fallback() {
    use super::{HandoverFailureAction, decide_handover_failure};

    assert_eq!(decide_handover_failure(GpuLossPolicy::Terminal, 1, 3), HandoverFailureAction::LatchTerminal);
    assert_eq!(decide_handover_failure(GpuLossPolicy::Terminal, 3, 3), HandoverFailureAction::LatchTerminal);
    assert_eq!(
        decide_handover_failure(GpuLossPolicy::Recover, 1, 3),
        HandoverFailureAction::RetryOrStayOnCpu { retry: true },
    );
    assert_eq!(
        decide_handover_failure(GpuLossPolicy::Recover, 3, 3),
        HandoverFailureAction::RetryOrStayOnCpu { retry: false },
    );
}

#[test]
fn effect_warm_does_not_spawn_once_terminal_latch_tripped() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let gpu = Arc::new(
        Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
            .expect("display wgpu device"),
    );
    let latch = crate::render::loss_latch::GpuLossLatch::new();
    gpu.attach_terminal_latch(Arc::clone(&latch));
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.attach_gpu(&gpu);
    renderer.set_prewarm_effects(crate::PrewarmEffects::ALL);

    assert!(latch.record_failure(GpuFailureKind::SurfaceFailed, "local surface failed"));
    renderer.warm_effects(&gpu.device);
    assert!(
        !renderer.has_active_effect_warm_worker(),
        "Renderer::warm_effects must not spawn once a Terminal latch on the GPU has tripped",
    );
}


#[test]
fn in_render_destroy_validation_and_submit_loss_defer_teardown_until_texture_released() {
    use crate::render::loss_latch::{
        FrameTeardownEffect, FrameTeardownEvent, FrameTeardownState, should_present_after_submit,
    };

    // Covers cases (a) in-render device.destroy(), (b) window submit-time loss, and (c) in-render
    // uncaptured validation error: while a SurfaceTexture is alive, TerminalLoss records the
    // failure and defers teardown until the texture drops without presenting.
    if !Gpu::test_adapter_available() {
        return;
    }

    // Case (a): device.destroy() inside Compositor::render while SurfaceTexture is alive.
    {
        let gpu = Arc::new(
            Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
                .expect("display wgpu device"),
        );
        let shared = Arc::new(SharedCore::default());
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = Arc::clone(&wakes);
        let tl = TerminalLoss::new(GpuLossPolicy::Terminal, None);
        tl.connect_wake(Arc::clone(&shared), Arc::new(move || { wakes_cb.fetch_add(1, Ordering::SeqCst); }));
        tl.attach_gpu(&gpu);

        let mut frame_state = FrameTeardownState::Idle;
        assert!(frame_state.step(FrameTeardownEvent::TextureAcquired).is_empty());
        assert!(frame_state.has_live_texture());

        // Compositor::render destroys device while frame texture is alive.
        gpu.device.destroy();
        let polled = rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll);
        assert!(polled.is_ok() || matches!(polled, Err(rune_gpu::health::PollFailed::Lost)));
        assert!(tl.is_failed());
        assert!(frame_state.step(FrameTeardownEvent::LossNoticed).is_empty(), "teardown must wait for texture drop");
        assert!(!should_present_after_submit(gpu.is_lost(), tl.is_failed()));

        // Only after dropping SurfaceTexture does TextureReleased emit TeardownNow.
        let fx = frame_state.step(FrameTeardownEvent::TextureReleased);
        assert_eq!(fx.into_iter().collect::<Vec<_>>(), vec![FrameTeardownEffect::TeardownNow]);
        assert!(!frame_state.has_live_texture());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert!(shared.window_error().unwrap_or_default().contains("Destroyed"));
    }

    // Case (c): uncaptured validation error inside Compositor::render while SurfaceTexture is alive.
    {
        let gpu = Arc::new(
            Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
                .expect("display wgpu device"),
        );
        let shared = Arc::new(SharedCore::default());
        let tl = TerminalLoss::new(GpuLossPolicy::Terminal, None);
        tl.connect_wake(Arc::clone(&shared), Arc::new(|| {}));
        tl.attach_gpu(&gpu);

        let mut frame_state = FrameTeardownState::Idle;
        assert!(frame_state.step(FrameTeardownEvent::TextureAcquired).is_empty());

        drop(gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("in-render-validation-teardown-order"),
            size: 4,
            usage: wgpu::BufferUsages::empty(),
            mapped_at_creation: false,
        }));
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
        assert!(tl.is_failed());
        assert!(frame_state.step(FrameTeardownEvent::LossNoticed).is_empty());
        assert!(!should_present_after_submit(gpu.is_lost(), tl.is_failed()));

        let fx = frame_state.step(FrameTeardownEvent::TextureReleased);
        assert_eq!(fx.into_iter().collect::<Vec<_>>(), vec![FrameTeardownEffect::TeardownNow]);
        assert!(!frame_state.has_live_texture());
        assert!(shared.window_error().unwrap_or_default().contains("uncaptured GPU error"));
    }
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "SurfaceTexture is still alive")]
fn teardown_debug_asserts_when_frame_texture_is_still_alive() {
    use crate::render::loss_latch::{FrameTeardownEvent, FrameTeardownState};
    let mut state = FrameTeardownState::Idle;
    let _ = state.step(FrameTeardownEvent::TextureAcquired);
    debug_assert!(
        !state.has_live_texture(),
        "surface teardown or reconfiguration while a SurfaceTexture is still alive",
    );
}

#[cfg(all(target_os = "linux", feature = "test-gpu-noop"))]
mod noop_tests {
    use super::*;
    use crate::render::loss_latch::GpuLossLatch;
    use crate::window::surface::configure_checked;
    use std::num::NonZeroU32;
    use std::ptr::NonNull;
    use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};

    #[allow(unsafe_code)]
    fn noop_gpu() -> (Gpu, wgpu::Surface<'static>) {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::NOOP;
        descriptor.backend_options.noop.enable = true;
        let instance = wgpu::Instance::new(descriptor);
        // SAFETY: NOOP backend never dereferences raw handles.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(NonNull::dangling()), 0))),
                raw_window_handle: RawWindowHandle::Xcb(XcbWindowHandle::new(NonZeroU32::new(1).unwrap())),
            })
        }
        .unwrap();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        (Gpu::from_device(instance, adapter, device, queue), surface)
    }

    fn noop_config() -> wgpu::SurfaceConfiguration {
        wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width: 200,
            height: 80,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
            view_formats: vec![],
            color_space: Default::default(),
        }
    }

    #[test]
    fn noop_device_destroy_restore_callbacks_and_shared_latch() {
        let (gpu, surface) = noop_gpu();
        let shared = Arc::new(SharedCore::default());
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = Arc::clone(&wakes);
        let latch_main = GpuLossLatch::new();
        let latch_sibling = GpuLossLatch::new();
        let shared_cb = Arc::clone(&shared);
        latch_main.add_wake(Arc::new(move |reason| {
            shared_cb.set_window_error(Some(reason.to_owned()));
            wakes_cb.fetch_add(1, Ordering::SeqCst);
        }));

        gpu.attach_terminal_latch(Arc::clone(&latch_main));
        gpu.attach_terminal_latch(Arc::clone(&latch_sibling));

        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.attach_gpu(&gpu);

        // Overwrite callbacks and restore via Renderer (same path as Frame::restore_device_callbacks).
        gpu.device.set_device_lost_callback(|_, _| {});
        gpu.device.on_uncaptured_error(Arc::new(|_| {}));
        renderer.restore_device_callbacks(&gpu.device);

        // Destroy the device and run configure_checked: poll triggers lost callback and latches both.
        gpu.device.destroy();
        let err = configure_checked(&surface, &gpu, &noop_config()).unwrap_err();
        assert!(gpu.is_lost(), "got: {err:?}");
        assert!(latch_main.is_failed());
        assert!(latch_sibling.is_failed());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert!(shared.window_error().is_some());
    }

    #[test]
    fn noop_validation_error_latches_without_panicking_under_cfg_test() {
        let (gpu, _surface) = noop_gpu();
        let latch = GpuLossLatch::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = Arc::clone(&wakes);
        latch.add_wake(Arc::new(move |_| {
            wakes_cb.fetch_add(1, Ordering::SeqCst);
        }));
        gpu.attach_terminal_latch(Arc::clone(&latch));

        drop(gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noop-validation"),
            size: 4,
            usage: wgpu::BufferUsages::empty(),
            mapped_at_creation: false,
        }));
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();

        assert!(latch.is_failed());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert!(latch.reason().unwrap_or_default().contains("uncaptured GPU error"));
    }

    #[test]
    fn noop_clean_drop_does_not_latch() {
        let latch = GpuLossLatch::new();
        {
            let (gpu, _surface) = noop_gpu();
            gpu.attach_terminal_latch(Arc::clone(&latch));
            gpu.prepare_clean_drop_for(&latch, true);
            drop(gpu);
        }
        assert!(!latch.is_failed());
    }

    #[test]
    fn noop_mixed_terminal_and_recover_isolation_and_sibling_clean_drop() {
        // 1. Uncaptured validation error latches Terminal without marking shared healthy Gpu lost.
        let (gpu, _surface) = noop_gpu();
        let latch = GpuLossLatch::new();
        gpu.attach_terminal_latch(Arc::clone(&latch));
        drop(gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noop-mixed-validation"),
            size: 4,
            usage: wgpu::BufferUsages::empty(),
            mapped_at_creation: false,
        }));
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
        assert!(latch.is_failed());
        assert!(!gpu.is_lost(), "healthy shared Gpu stays usable by Recover siblings");

        // Real destroy marks gpu.is_lost() so Recover sees it.
        gpu.device.destroy();
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
        assert!(gpu.is_lost());

        // 2. Sibling clean drop with strong_count > 1 preserves Destroyed detection on sibling.
        let (gpu_raw, _s2) = noop_gpu();
        let gpu_b = Arc::new(gpu_raw);
        let gpu_a = Arc::clone(&gpu_b);
        let latch_a = GpuLossLatch::new();
        let latch_b = GpuLossLatch::new();
        gpu_a.attach_terminal_latch(Arc::clone(&latch_a));
        gpu_b.attach_terminal_latch(Arc::clone(&latch_b));
        gpu_a.prepare_clean_drop_for(&latch_a, Arc::strong_count(&gpu_a) == 1);
        drop(gpu_a);
        assert!(!latch_a.is_failed());
        assert!(!latch_b.is_failed());
        gpu_b.device.destroy();
        rune_gpu::health::poll(&gpu_b.device, wgpu::PollType::Poll).ok();
        assert!(!latch_a.is_failed());
        assert!(latch_b.is_failed(), "sibling Terminal must still catch Destroyed");
    }

    #[test]
    fn noop_in_render_destroy_validation_submit_overlay_and_recover_teardown_order() {
        use crate::render::loss_latch::{
            FrameTeardownEffect, FrameTeardownEvent, FrameTeardownState, should_present_after_submit,
        };

        // Case (a) & (b): in-render device.destroy() or submit-time loss under Terminal on
        // wgpu-core noop backend defers teardown until SurfaceTexture drops and skips present.
        let (gpu, _surface) = noop_gpu();
        let latch = GpuLossLatch::new();
        gpu.attach_terminal_latch(Arc::clone(&latch));
        let mut frame_state = FrameTeardownState::Idle;
        assert!(frame_state.step(FrameTeardownEvent::TextureAcquired).is_empty());
        gpu.device.destroy();
        let poll_res = rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll);
        assert!(poll_res.is_ok() || matches!(poll_res, Err(rune_gpu::health::PollFailed::Lost)));
        assert!(latch.is_failed());
        assert!(frame_state.step(FrameTeardownEvent::LossNoticed).is_empty());
        assert!(!should_present_after_submit(gpu.is_lost(), latch.is_failed()));
        assert_eq!(
            frame_state.step(FrameTeardownEvent::TextureReleased).into_iter().collect::<Vec<_>>(),
            vec![FrameTeardownEffect::TeardownNow],
        );

        // Case (c): in-render uncaptured validation error under Terminal on noop backend.
        let (gpu2, _surface2) = noop_gpu();
        let latch2 = GpuLossLatch::new();
        gpu2.attach_terminal_latch(Arc::clone(&latch2));
        let mut frame_state2 = FrameTeardownState::Idle;
        assert!(frame_state2.step(FrameTeardownEvent::TextureAcquired).is_empty());
        drop(gpu2.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noop-in-render-validation"),
            size: 4,
            usage: wgpu::BufferUsages::empty(),
            mapped_at_creation: false,
        }));
        let _ = rune_gpu::health::poll(&gpu2.device, wgpu::PollType::Poll);
        assert!(latch2.is_failed());
        assert!(frame_state2.step(FrameTeardownEvent::LossNoticed).is_empty());
        assert!(!should_present_after_submit(gpu2.is_lost(), latch2.is_failed()));
        assert_eq!(
            frame_state2.step(FrameTeardownEvent::TextureReleased).into_iter().collect::<Vec<_>>(),
            vec![FrameTeardownEffect::TeardownNow],
        );

        // Case (d) & Recover: submit-time loss on shared device (Recover policy has no latch)
        // still forbids pre_present_notify()/queue.present() once gpu.is_lost() is set.
        let (gpu3, _surface3) = noop_gpu();
        gpu3.mark_lost();
        assert!(gpu3.is_lost());
        assert!(!should_present_after_submit(gpu3.is_lost(), false));
    }
}
