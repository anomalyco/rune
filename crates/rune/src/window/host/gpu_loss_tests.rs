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
    assert!(gpu.is_lost(), "uncaptured error under Terminal marks GPU lost");
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
        let gpu = Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
            .expect("display wgpu device");
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
            gpu.mark_clean_drop();
            latch.detach();
            drop(gpu);
        }
        assert!(!latch.is_failed());
    }
}
