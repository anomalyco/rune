//! Backend-switch fallback tests for `Gpu::try_setup_on_with_fallback` and `Gpu::try_picked_with_fallback`.

use std::cell::Cell;

use crate::gpu::fallback::{Failure, Policy};

use super::{InstanceFallback, SurfaceSetupError};
use crate::render::Gpu;

fn empty_instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::empty();
    wgpu::Instance::new(desc)
}

/// Switching backends (as Apple native Metal does when a driver lacks `MTLSharedEvent`) clears
/// `tried` so the same physical GPU adapter can be selected on the wgpu backend, calls the
/// fallback once, and recreates the surface on the new instance.
#[test]
fn switch_backend_recovers_same_adapter_after_device_create_failure() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let switches = Cell::new(0usize);
    let mut surfaces = 0usize;
    let mut first_adapter_name = String::new();
    let res = Gpu::try_setup_on_with_fallback(
        Gpu::instance(),
        (),
        Some(Policy::Automatic),
        wgpu::Features::empty(),
        false,
        |_| {
            surfaces += 1;
            Ok(())
        },
        |kind, _allow_widen, reason| {
            assert_eq!(kind, Failure::DeviceCreate);
            assert!(reason.contains("MTLSharedEvent"), "got reason={reason}");
            switches.set(switches.get() + 1);
            Some(InstanceFallback::SwitchBackend(Gpu::instance()))
        },
        |_| None,
        |gpu, _| {
            let name = gpu.adapter.get_info().name;
            if switches.get() == 0 {
                if first_adapter_name.is_empty() {
                    first_adapter_name = name;
                }
                Err(SurfaceSetupError {
                    kind: Failure::DeviceCreate,
                    message: "no GPU device: Metal device does not support MTLSharedEvent".into(),
                })
            } else {
                Ok(name)
            }
        },
    );
    let (_gpu, (), recovered_name) = res.expect("SwitchBackend must recover on the same GPU");
    assert_eq!(switches.get(), 1, "backend switch runs once");
    assert!(surfaces >= 2, "surface is recreated for the switched backend");
    assert_eq!(recovered_name, first_adapter_name, "same physical GPU must not be excluded by tried");
}

/// `SwitchBackend` recovers even under `Policy::Pinned` (e.g. `WGPU_ADAPTER_NAME` or
/// `GpuLossPolicy::Terminal` before any device loss), while transient failures (`OutOfMemory`,
/// `DeviceLost`) never switch backends.
#[test]
fn switch_backend_recovers_when_pinned_and_never_switches_on_transient_errors() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let switches = Cell::new(0usize);
    let mut surfaces = 0usize;
    let ok = Gpu::try_setup_on_with_fallback(
        Gpu::instance(),
        (),
        Some(Policy::Pinned),
        wgpu::Features::empty(),
        false,
        |_| {
            surfaces += 1;
            Ok(())
        },
        |kind, allow_widen, _| {
            assert!(!allow_widen, "Pinned policy must disallow Linux driver widening");
            if matches!(kind, Failure::DeviceCreate) {
                switches.set(switches.get() + 1);
                Some(InstanceFallback::SwitchBackend(Gpu::instance()))
            } else {
                None
            }
        },
        |_| None,
        |_, _| {
            if switches.get() == 0 {
                Err(SurfaceSetupError {
                    kind: Failure::DeviceCreate,
                    message: "Metal device does not support MTLSharedEvent".into(),
                })
            } else {
                Ok(42u32)
            }
        },
    );
    let (_gpu, (), value) = ok.expect("SwitchBackend must recover under Policy::Pinned");
    assert_eq!(value, 42);
    assert_eq!(switches.get(), 1);
    assert_eq!(surfaces, 1);

    for transient in [Failure::OutOfMemory, Failure::DeviceLost] {
        assert!(
            Gpu::next_instance_fallback(true, false, transient, "transient").is_none(),
            "transient failure {transient:?} must not trigger backend switch",
        );
    }
}

/// When the initial instance exposes no adapters (e.g. empty Metal device list), `SwitchBackend`
/// recovers even under `Policy::Pinned`.
#[test]
fn switch_backend_recovers_from_missing_adapter_even_when_pinned() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let mut switches = 0usize;
    let mut surfaces = 0usize;
    let res = Gpu::try_setup_on_with_fallback(
        empty_instance(),
        (),
        Some(Policy::Pinned),
        wgpu::Features::empty(),
        false,
        |_| {
            surfaces += 1;
            Ok(())
        },
        |kind, allow_widen, _| {
            assert_eq!(kind, Failure::DeviceCreate);
            assert!(!allow_widen);
            switches += 1;
            Some(InstanceFallback::SwitchBackend(Gpu::instance()))
        },
        |_| None,
        |_, _| Ok(9u32),
    );
    let (_gpu, (), value) = res.expect("SwitchBackend must recover from empty initial instance");
    assert_eq!(value, 9);
    assert_eq!(switches, 1);
    assert_eq!(surfaces, 1);
}

/// `Gpu::try_picked_with_fallback` (used by `prewarm_gpu`, `Gpu::try_for_display`, and
/// `Gpu::try_new`) switches cleanly to the fallback instance once when the initial instance
/// fails, and preserves the error when backend switching is disabled.
#[test]
fn try_picked_switches_backend_once_when_initial_instance_fails() {
    if !Gpu::test_adapter_available() {
        return;
    }
    let mut switches = 0usize;
    let gpu = Gpu::try_picked_with_fallback(
        empty_instance(),
        None,
        wgpu::Features::empty(),
        false,
        true,
        true,
        |reason| {
            assert!(reason.contains("no GPU adapter"), "got reason={reason}");
            switches += 1;
            Gpu::instance()
        },
    )
    .expect("try_picked_with_fallback must recover on fallback instance");
    assert_eq!(switches, 1);
    drop(gpu);

    let mut disabled_switches = 0usize;
    let err = Gpu::try_picked_with_fallback(
        empty_instance(),
        None,
        wgpu::Features::empty(),
        false,
        true,
        false,
        |_| {
            disabled_switches += 1;
            Gpu::instance()
        },
    )
    .err()
    .expect("disabled switch must return the initial error");
    assert!(err.contains("no GPU adapter"), "got err={err}");
    assert_eq!(disabled_switches, 0);
}
