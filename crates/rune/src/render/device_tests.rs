use super::Gpu;

#[test]
fn default_instance_descriptor_hides_noncompliant_adapters() {
    let desc = Gpu::instance_descriptor(wgpu::Backends::all(), false);
    assert!(!desc.flags.contains(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER));
    assert!(!desc.flags.contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL));
}

#[test]
fn fallback_instance_descriptor_allows_noncompliant_adapters() {
    let desc = Gpu::instance_descriptor(wgpu::Backends::all(), true);
    assert!(desc.flags.contains(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER));
    assert!(!desc.flags.contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL));
}

#[test]
fn select_wgpu_instance_keeps_compliant_when_adapters_exist() {
    let mut built = Vec::new();
    let chosen = Gpu::select_wgpu_instance(
        |allow_noncompliant| {
            built.push(allow_noncompliant);
            allow_noncompliant
        },
        |&allow_noncompliant| !allow_noncompliant,
    );
    assert!(!chosen, "a machine with a conformant adapter keeps the compliant-only instance");
    assert_eq!(built, vec![false], "non-compliant instance is never built when compliant adapters exist");
}

#[test]
fn select_wgpu_instance_falls_back_when_no_compliant_adapters_exist() {
    let mut built = Vec::new();
    let chosen = Gpu::select_wgpu_instance(
        |allow_noncompliant| {
            built.push(allow_noncompliant);
            allow_noncompliant
        },
        |&allow_noncompliant| allow_noncompliant,
    );
    assert!(chosen, "a machine whose only Vulkan driver reports conformanceVersion 0 falls back to allowing it");
    assert_eq!(built, vec![false, true]);
}

#[test]
fn select_wgpu_instance_keeps_compliant_when_neither_has_adapters() {
    let mut built = Vec::new();
    let chosen = Gpu::select_wgpu_instance(
        |allow_noncompliant| {
            built.push(allow_noncompliant);
            allow_noncompliant
        },
        |_| false,
    );
    assert!(!chosen, "returns the compliant instance when no adapters exist either way");
    assert_eq!(built, vec![false, true]);
}

/// Real offscreen multi-adapter fallback test (no native window): refusing every hardware
/// adapter with `SurfaceRefused` walks remaining hardware adapters before WARP (`DeviceType::Cpu`)
/// and drops/recreates the surface between attempts.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn surface_refused_falls_back_across_real_adapters_hardware_before_software() {
    use crate::gpu::fallback::{Failure, Policy};
    use super::super::SurfaceSetupError;

    let instance = Gpu::instance();
    let available = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    if available.len() < 2 {
        return eprintln!("skipped: machine exposes fewer than two adapters ({})", available.len());
    }
    let has_cpu = available.iter().any(|a| a.get_info().device_type == wgpu::DeviceType::Cpu);

    let mut tried = Vec::new();
    let mut surfaces_made = 0usize;
    let res = Gpu::try_setup_on(
        instance,
        (),
        Some(Policy::Automatic),
        wgpu::Features::empty(),
        false,
        |_| {
            surfaces_made += 1;
            Ok(())
        },
        |_| None,
        |gpu, _| {
            let info = gpu.adapter.get_info();
            tried.push((info.name.clone(), info.device_type));
            if has_cpu && info.device_type != wgpu::DeviceType::Cpu {
                return Err(SurfaceSetupError {
                    kind: Failure::SurfaceRefused,
                    message: format!("refused on {}", info.name),
                });
            }
            if !has_cpu && tried.len() == 1 {
                return Err(SurfaceSetupError {
                    kind: Failure::SurfaceRefused,
                    message: format!("refused on {}", info.name),
                });
            }
            Ok(())
        },
    );
    let (gpu, (), ()) = res.expect("fallback chain must reach a working adapter");
    assert!(tried.len() >= 2, "expected at least one fallback attempt, tried: {tried:?}");
    assert!(surfaces_made >= 1, "a fresh surface must be created after a refused configuration");
    if has_cpu {
        assert_eq!(gpu.adapter.get_info().device_type, wgpu::DeviceType::Cpu);
        // Hardware adapters must all have been tried before software (WARP).
        let first_cpu = tried.iter().position(|(_, kind)| *kind == wgpu::DeviceType::Cpu).unwrap();
        assert_eq!(first_cpu, tried.len() - 1, "software adapter must be tried last: {tried:?}");
    }
}

/// Launch-speed guarantee: when the first adapter succeeds, no fallback adapters are tried and
/// `make_surface` is never called a second time.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn happy_path_first_adapter_does_no_fallback_work() {
    use crate::gpu::fallback::Policy;

    let instance = Gpu::instance();
    let mut tried = 0usize;
    let mut extra_surfaces = 0usize;
    let res = Gpu::try_setup_on(
        instance,
        (),
        Some(Policy::Automatic),
        wgpu::Features::empty(),
        false,
        |_| {
            extra_surfaces += 1;
            Ok(())
        },
        |_| None,
        |_, _| {
            tried += 1;
            Ok(42u32)
        },
    );
    let (_, (), value) = res.expect("first adapter succeeds");
    assert_eq!(value, 42);
    assert_eq!(tried, 1);
    assert_eq!(extra_surfaces, 0);
}

/// Transient OOM, device loss, and pinned (named) adapters never fall back to another adapter.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn transient_and_pinned_failures_do_not_switch_real_adapters() {
    use crate::gpu::fallback::{Failure, Policy};
    use super::super::SurfaceSetupError;

    for (policy, kind) in [
        (Policy::Automatic, Failure::OutOfMemory),
        (Policy::Automatic, Failure::DeviceLost),
        (Policy::Pinned, Failure::SurfaceRefused),
    ] {
        let mut tried = 0usize;
        let mut extra_surfaces = 0usize;
        let err = Gpu::try_setup_on(
            Gpu::instance(),
            (),
            Some(policy),
            wgpu::Features::empty(),
            false,
            |_| {
                extra_surfaces += 1;
                Ok(())
            },
            |_| None,
            |_, _| {
                tried += 1;
                Err::<(), _>(SurfaceSetupError { kind, message: "stop here".into() })
            },
        )
        .err()
        .expect("must stop with error");
        assert_eq!(err.kind, kind, "policy={policy:?}, kind={kind:?}");
        assert_eq!(err.message, "stop here");
        assert_eq!(tried, 1, "must not try a second adapter for policy={policy:?}, kind={kind:?}");
        assert_eq!(extra_surfaces, 0);
    }
}

/// When every adapter refuses configuration, the chain returns the first error so `Host::create`
/// hands it to `Life::fail_create` / `run()` (PR #1762).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn exhausting_every_real_adapter_returns_first_error() {
    use crate::gpu::fallback::{Failure, Policy};
    use super::super::SurfaceSetupError;

    let mut tried = 0usize;
    let err = Gpu::try_setup_on(
        Gpu::instance(),
        (),
        Some(Policy::Automatic),
        wgpu::Features::empty(),
        false,
        |_| Ok(()),
        |_| None,
        |gpu, _| {
            tried += 1;
            Err::<(), _>(SurfaceSetupError {
                kind: Failure::SurfaceRefused,
                message: format!("refused #{tried} on {}", gpu.adapter.get_info().name),
            })
        },
    )
    .err()
    .expect("all adapters refused");
    assert!(tried >= 1);
    assert!(err.message.starts_with("refused #1 on "), "got: {}", err.message);
}

#[test]
fn configure_error_classification_separates_oom_from_refusal() {
    use crate::window::{ConfigureErrorKind, classify_scope_error};

    let oom = wgpu::Error::OutOfMemory { source: Box::new(std::io::Error::other("oom")) };
    let internal = wgpu::Error::Internal {
        source: Box::new(std::io::Error::other("swapchain")),
        description: "swapchain creation".into(),
    };
    let validation = wgpu::Error::Validation {
        source: Box::new(std::io::Error::other("invalid")),
        description: "invalid".into(),
    };

    assert_eq!(classify_scope_error(None, None, None).map(|(k, _)| k), None);
    assert_eq!(
        classify_scope_error(Some(oom), Some(internal), None).map(|(k, _)| k),
        Some(ConfigureErrorKind::OutOfMemory),
    );
    let internal2 = wgpu::Error::Internal {
        source: Box::new(std::io::Error::other("swapchain")),
        description: "swapchain creation".into(),
    };
    assert_eq!(
        classify_scope_error(None, Some(internal2), Some(validation)).map(|(k, _)| k),
        Some(ConfigureErrorKind::Refused),
    );
    let validation2 = wgpu::Error::Validation {
        source: Box::new(std::io::Error::other("invalid")),
        description: "invalid".into(),
    };
    assert_eq!(
        classify_scope_error(None, None, Some(validation2)).map(|(k, _)| k),
        Some(ConfigureErrorKind::Refused),
    );
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::DeviceLost), rune_gpu::fallback::Failure::DeviceLost);
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::OutOfMemory), rune_gpu::fallback::Failure::OutOfMemory);
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::Refused), rune_gpu::fallback::Failure::SurfaceRefused);
}
