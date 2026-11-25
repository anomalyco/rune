//! Unit and Linux `test-gpu-noop` tests for app-attributed `AppHandle::warm_gpu_with`
//! ownership in `WarmSlot` (Windows & Linux; no-op on macOS and `wasm32`).

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use super::{
    FORMAT, Warmed, prepare_on_live_inline, take_from_slot, warm_on_slot,
};
use crate::render::{
    Gpu, Renderer,
    loss_latch::{GpuFailureKind, GpuLossLatch, GpuLossPolicy},
    warm_machine::{Clock, ShutdownOutcome, WarmSlot},
};
use crate::time::Instant;
use crate::{App, AppSpec, Cx, El, el};

struct FakeClock {
    now: Mutex<Instant>,
}

impl FakeClock {
    fn new() -> Self {
        Self { now: Mutex::new(crate::time::now()) }
    }

    fn advance(&self, delta: Duration) {
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) += delta;
    }
}

impl Clock for Arc<FakeClock> {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn test_gpu() -> Option<Gpu> {
    #[cfg(all(target_os = "linux", feature = "test-gpu-noop"))]
    {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::NOOP;
        descriptor.backend_options.noop.enable = true;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        return Some(Gpu::from_device(instance, adapter, device, queue));
    }
    #[cfg(not(all(target_os = "linux", feature = "test-gpu-noop")))]
    {
        Gpu::test_headless_or_noop()
    }
}

fn test_warmed(owner: Option<Arc<GpuLossLatch>>) -> Option<(Warmed, wgpu::Device)> {
    let gpu = test_gpu()?;
    if let Some(latch) = &owner {
        latch.rearm();
        gpu.attach_terminal_latch(Arc::clone(latch));
    }
    let device = gpu.device.clone();
    let renderer = Renderer::try_new_warm(&gpu.device, FORMAT).ok()?;
    Some((Warmed::new(gpu, renderer, owner), device))
}

fn wait_until(pred: impl Fn() -> bool) {
    for _ in 0..500 {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(pred(), "timed out waiting for warm slot state");
}

#[test]
fn warm_make_failure_latches_terminal_owner_and_wakes_once() {
    let slot = Arc::new(WarmSlot::<Warmed, Arc<FakeClock>>::new(Arc::new(FakeClock::new())));
    let latch = GpuLossLatch::new();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    latch.add_wake(Arc::new(move |_| {
        wakes_cb.fetch_add(1, Ordering::SeqCst);
    }));
    let prepare_ran = Arc::new(AtomicBool::new(false));
    let prep_flag = Arc::clone(&prepare_ran);

    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_secs(30),
        Some(Arc::clone(&latch)),
        |owner| {
            if let Some(owner) = owner {
                let _ = owner.record_failure(GpuFailureKind::InitFailed, "no compatible display adapter");
            }
            None
        },
        Some(Box::new(move |_| {
            prep_flag.store(true, Ordering::SeqCst);
        })),
    );

    wait_until(|| latch.is_failed() && slot.shutdown(Duration::from_secs(2)) == ShutdownOutcome::Drained);
    assert_eq!(latch.kind(), Some(GpuFailureKind::InitFailed));
    assert_eq!(latch.reason().as_deref(), Some("no compatible display adapter"));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert!(!prepare_ran.load(Ordering::SeqCst), "prepare must not run when make fails");
    assert!(take_from_slot(&slot).is_none());
}

#[test]
fn warm_prepare_destroy_latches_and_does_not_park() {
    let Some(_) = test_gpu() else { return };
    let slot = Arc::new(WarmSlot::<Warmed, Arc<FakeClock>>::new(Arc::new(FakeClock::new())));
    let latch = GpuLossLatch::new();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    latch.add_wake(Arc::new(move |_| {
        wakes_cb.fetch_add(1, Ordering::SeqCst);
    }));

    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_secs(30),
        Some(Arc::clone(&latch)),
        |owner| test_warmed(owner).map(|(w, _)| w),
        Some(Box::new(|device| {
            device.destroy();
        })),
    );

    wait_until(|| latch.is_failed());
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert!(take_from_slot(&slot).is_none(), "destroyed device must not be parked");
    assert_eq!(slot.shutdown(Duration::from_secs(2)), ShutdownOutcome::Drained);

    // Surface setup with this failed latch must refuse immediately without calling make_surface.
    let surface_called = AtomicBool::new(false);
    let res = Gpu::for_surface_setup_with_latch(
        Some(&latch),
        wgpu::Features::empty(),
        false,
        |_| {
            surface_called.store(true, Ordering::SeqCst);
            Err("should never be called".into())
        },
        |_, _, _| Ok(()),
    );
    assert!(res.is_err());
    assert!(!surface_called.load(Ordering::SeqCst), "failed latch must refuse fallback device creation");
}

#[test]
fn warm_prepare_validation_error_latches_without_panicking_and_does_not_park() {
    let Some(_) = test_gpu() else { return };
    let slot = Arc::new(WarmSlot::<Warmed, Arc<FakeClock>>::new(Arc::new(FakeClock::new())));
    let latch = GpuLossLatch::new();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    latch.add_wake(Arc::new(move |_| {
        wakes_cb.fetch_add(1, Ordering::SeqCst);
    }));

    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_secs(30),
        Some(Arc::clone(&latch)),
        |owner| test_warmed(owner).map(|(w, _)| w),
        Some(Box::new(|device| {
            drop(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("warm-prepare-validation"),
                size: 4,
                usage: wgpu::BufferUsages::empty(),
                mapped_at_creation: false,
            }));
        })),
    );

    wait_until(|| latch.is_failed());
    assert_eq!(latch.kind(), Some(GpuFailureKind::UncapturedError));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert!(take_from_slot(&slot).is_none(), "validation-failed device must not be parked");
    assert_eq!(slot.shutdown(Duration::from_secs(2)), ShutdownOutcome::Drained);
}

#[test]
fn parked_device_lost_while_waiting_trips_owner_on_take_and_refuses_fallback() {
    let Some(_) = test_gpu() else { return };
    let slot = Arc::new(WarmSlot::<Warmed, Arc<FakeClock>>::new(Arc::new(FakeClock::new())));
    let latch = GpuLossLatch::new();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    latch.add_wake(Arc::new(move |_| {
        wakes_cb.fetch_add(1, Ordering::SeqCst);
    }));
    let parked_dev: Arc<Mutex<Option<wgpu::Device>>> = Arc::default();
    let parked_dev_cb = Arc::clone(&parked_dev);

    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_secs(30),
        Some(Arc::clone(&latch)),
        move |owner| {
            let (warmed, dev) = test_warmed(owner)?;
            *parked_dev_cb.lock().unwrap_or_else(|e| e.into_inner()) = Some(dev);
            Some(warmed)
        },
        None,
    );

    wait_until(|| slot.has_ready());
    assert!(!latch.is_failed(), "healthy parked device must not be latched");

    // Device dies while waiting in Parked (Ready) BEFORE a surface calls take().
    let dev = parked_dev.lock().unwrap_or_else(|e| e.into_inner()).take().expect("parked device");
    dev.destroy();
    drop(dev);

    // take_from_slot polls, trips the parked owner's latch, and returns None.
    assert!(take_from_slot(&slot).is_none());
    assert!(latch.is_failed(), "dead parked device must trip its Terminal owner's latch on take");
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert_eq!(slot.shutdown(Duration::from_secs(2)), ShutdownOutcome::Drained);

    // Subsequent for_surface_setup_with_latch sees latch.is_failed() and refuses fallback creation.
    let surface_called = AtomicBool::new(false);
    let res = Gpu::for_surface_setup_with_latch(
        Some(&latch),
        wgpu::Features::empty(),
        false,
        |_| {
            surface_called.store(true, Ordering::SeqCst);
            Err("must not run".into())
        },
        |_, _, _| Ok(()),
    );
    assert!(res.is_err());
    assert!(!surface_called.load(Ordering::SeqCst));
}

#[test]
fn failed_terminal_refuses_warm_and_second_request_extends_linger_without_replacing_owner() {
    let Some(_) = test_gpu() else { return };
    let clock = Arc::new(FakeClock::new());
    let slot = Arc::new(WarmSlot::<Warmed, Arc<FakeClock>>::new(Arc::clone(&clock)));
    let owner_a = GpuLossLatch::new();
    let owner_b = GpuLossLatch::new();
    let failed_owner = GpuLossLatch::new();
    assert!(failed_owner.record_device_lost(false, "already dead"));

    // 1. Failed Terminal caller refuses warm_on_slot immediately without running make/prepare.
    let ran_failed = Arc::new(AtomicBool::new(false));
    let rf = Arc::clone(&ran_failed);
    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_secs(30),
        Some(Arc::clone(&failed_owner)),
        move |_| {
            rf.store(true, Ordering::SeqCst);
            None
        },
        None,
    );
    assert!(!ran_failed.load(Ordering::SeqCst));
    assert!(!slot.has_ready());

    // 2. Owner A starts warm-up and parks cleanly.
    let a_prep = Arc::new(AtomicUsize::new(0));
    let a_prep_cb = Arc::clone(&a_prep);
    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_secs(30),
        Some(Arc::clone(&owner_a)),
        |owner| test_warmed(owner).map(|(w, _)| w),
        Some(Box::new(move |_| {
            a_prep_cb.fetch_add(1, Ordering::SeqCst);
        })),
    );
    wait_until(|| slot.has_ready());
    assert_eq!(a_prep.load(Ordering::SeqCst), 1);

    // 3. Owner B calls warm_on_slot while Parked: extends LINGER without replacing owner A or
    //    running owner B's callback.
    let b_prep = Arc::new(AtomicUsize::new(0));
    let b_prep_cb = Arc::clone(&b_prep);
    clock.advance(Duration::from_secs(20));
    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_secs(30),
        Some(Arc::clone(&owner_b)),
        |_| panic!("second warm while Parked must not call make"),
        Some(Box::new(move |_| {
            b_prep_cb.fetch_add(1, Ordering::SeqCst);
        })),
    );
    assert_eq!(b_prep.load(Ordering::SeqCst), 0, "second warm must not replace initiating callback");

    // Failed caller while Parked also must not run or alter owner A.
    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_secs(60),
        Some(Arc::clone(&failed_owner)),
        |_| panic!("failed caller must not call make"),
        None,
    );

    // Past the original 30 s deadline (at t = 35 s), extended LINGER (t = 50 s) keeps it parked.
    clock.advance(Duration::from_secs(15));
    assert!(slot.has_ready(), "second request must extend LINGER");

    // Clean discard drops owner A's parked device without falsely latching either owner.
    slot.discard();
    assert_eq!(slot.shutdown(Duration::from_secs(2)), ShutdownOutcome::Drained);
    assert!(!owner_a.is_failed());
    assert!(!owner_b.is_failed());
}

#[test]
fn clean_linger_expiry_discard_cancel_and_shutdown_never_falsely_latch_terminal_owner() {
    let Some(_) = test_gpu() else { return };

    // 1. Clean LINGER expiry via FakeClock.
    let clock = Arc::new(FakeClock::new());
    let slot = Arc::new(WarmSlot::<Warmed, Arc<FakeClock>>::new(Arc::clone(&clock)));
    let owner = GpuLossLatch::new();
    warm_on_slot(
        Arc::clone(&slot),
        Duration::from_millis(20),
        Some(Arc::clone(&owner)),
        |owner| test_warmed(owner).map(|(w, _)| w),
        None,
    );
    wait_until(|| slot.has_ready());
    clock.advance(Duration::from_millis(50));
    wait_until(|| !slot.has_ready());
    assert_eq!(slot.shutdown(Duration::from_secs(2)), ShutdownOutcome::Drained);
    assert!(!owner.is_failed(), "clean LINGER expiry must never falsely latch Terminal owner");

    // 2. Clean Cancel + Shutdown.
    let slot2 = Arc::new(WarmSlot::<Warmed, Arc<FakeClock>>::new(Arc::new(FakeClock::new())));
    let owner2 = GpuLossLatch::new();
    warm_on_slot(
        Arc::clone(&slot2),
        Duration::from_secs(30),
        Some(Arc::clone(&owner2)),
        |owner| test_warmed(owner).map(|(w, _)| w),
        None,
    );
    wait_until(|| slot2.has_ready());
    slot2.cancel();
    assert_eq!(slot2.shutdown(Duration::from_secs(2)), ShutdownOutcome::Drained);
    assert!(!owner2.is_failed(), "clean Cancel/Shutdown must never falsely latch Terminal owner");
}

#[test]
fn live_gpu_warm_prepare_latches_terminal_on_validation_without_marking_shared_gpu_lost() {
    let Some(gpu) = test_gpu() else { return };
    let latch = GpuLossLatch::new();
    latch.rearm();
    gpu.attach_terminal_latch(Arc::clone(&latch));

    prepare_on_live_inline(&gpu, &latch, |device| {
        drop(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("live-warm-prepare-validation"),
            size: 4,
            usage: wgpu::BufferUsages::empty(),
            mapped_at_creation: false,
        }));
    });

    assert!(latch.is_failed(), "validation error in live-GPU prepare must latch Terminal");
    assert_eq!(latch.kind(), Some(GpuFailureKind::UncapturedError));
    assert!(!gpu.is_lost(), "validation error in live-GPU prepare must not mark healthy shared GPU lost");
}

#[test]
fn app_handle_cloned_before_or_after_gpu_loss_policy_shares_latch_and_refuses_warm_after_latch() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el() }
    let app = App::new(AppSpec::new("WarmHandle", (100.0, 60.0), || (), view));
    let handle_before = app.handle();
    let app = app.gpu_loss_policy(GpuLossPolicy::Terminal);
    let handle_after = app.handle();

    let app_latch = app.gpu_loss_latch.as_ref().expect("app latch");
    let before_latch = handle_before.shared.core.gpu_loss_latch().expect("handle cloned before policy");
    let after_latch = handle_after.shared.core.gpu_loss_latch().expect("handle cloned after policy");
    assert!(Arc::ptr_eq(app_latch, &before_latch));
    assert!(Arc::ptr_eq(app_latch, &after_latch));

    // Tripping the latch before app.run() immediately populates handle.window_error().
    assert!(app_latch.record_failure(GpuFailureKind::InitFailed, "warm init failed"));
    assert_eq!(handle_before.window_error().as_deref(), Some("warm init failed"));
    assert_eq!(handle_after.window_error().as_deref(), Some("warm init failed"));

    // Calling handle.warm_gpu_with once latched must refuse immediately without running prepare.
    let ran = Arc::new(AtomicBool::new(false));
    let ran_cb = Arc::clone(&ran);
    handle_before.warm_gpu_with(wgpu::Features::empty(), move |_| {
        ran_cb.store(true, Ordering::SeqCst);
    });
    assert!(!ran.load(Ordering::SeqCst));
}
