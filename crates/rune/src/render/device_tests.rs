use super::Gpu;

#[test]
fn default_instance_descriptor_hides_noncompliant_adapters() {
    let desc = Gpu::instance_descriptor(
        wgpu::Backends::all(),
        false,
        wgpu::Dx12SwapchainKind::DxgiFromVisual,
    );
    assert!(!desc.flags.contains(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER));
    assert!(!desc.flags.contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL));
    assert_eq!(
        desc.backend_options.dx12.presentation_system,
        wgpu::Dx12SwapchainKind::DxgiFromVisual,
    );
    assert!(matches!(
        desc.backend_options.dx12.latency_waitable_object,
        wgpu::Dx12UseFrameLatencyWaitableObject::DontWait,
    ));
    assert!(matches!(desc.backend_options.dx12.shader_compiler, wgpu::Dx12Compiler::Fxc));
}

#[test]
fn fallback_instance_descriptor_allows_noncompliant_adapters() {
    let desc = Gpu::instance_descriptor(
        wgpu::Backends::all(),
        true,
        wgpu::Dx12SwapchainKind::DxgiFromVisual,
    );
    assert!(desc.flags.contains(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER));
    assert!(!desc.flags.contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL));
}

#[test]
fn instance_descriptor_sets_requested_dx12_swapchain_kind() {
    let visual = Gpu::instance_descriptor(
        wgpu::Backends::DX12,
        false,
        wgpu::Dx12SwapchainKind::DxgiFromVisual,
    );
    let hwnd = Gpu::instance_descriptor(
        wgpu::Backends::DX12,
        false,
        wgpu::Dx12SwapchainKind::DxgiFromHwnd,
    );
    assert_eq!(
        visual.backend_options.dx12.presentation_system,
        wgpu::Dx12SwapchainKind::DxgiFromVisual,
    );
    assert_eq!(
        hwnd.backend_options.dx12.presentation_system,
        wgpu::Dx12SwapchainKind::DxgiFromHwnd,
    );
    assert!(matches!(
        hwnd.backend_options.dx12.latency_waitable_object,
        wgpu::Dx12UseFrameLatencyWaitableObject::DontWait,
    ));
    assert!(matches!(hwnd.backend_options.dx12.shader_compiler, wgpu::Dx12Compiler::Fxc));
}

#[cfg(target_os = "windows")]
#[test]
fn dx12_instance_cache_builds_both_visual_and_hwnd_instances_offscreen() {
    use crate::gpu::fallback::Presentation;

    let visual_a = Gpu::wgpu_instance_for_presentation(Presentation::DirectComposition);
    let visual_b = Gpu::wgpu_instance_for_presentation(Presentation::DirectComposition);
    let hwnd_a = Gpu::wgpu_instance_for_presentation(Presentation::PlainHwnd);
    let hwnd_b = Gpu::wgpu_instance_for_presentation(Presentation::PlainHwnd);

    // Both presentation kinds enumerate DX12 adapters cleanly offscreen without opening a window.
    let visual_adapters = pollster::block_on(visual_a.enumerate_adapters(wgpu::Backends::DX12));
    let hwnd_adapters = pollster::block_on(hwnd_a.enumerate_adapters(wgpu::Backends::DX12));
    assert!(!visual_adapters.is_empty(), "DX12 DirectComposition instance must see adapters");
    assert!(!hwnd_adapters.is_empty(), "DX12 PlainHwnd instance must see adapters");
    let _ = (visual_b, hwnd_b);
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
/// adapter with `SurfaceRefused` walks remaining hardware adapters before WARP (`DeviceType::Cpu`),
/// drops/recreates the surface between attempts, and NEVER switches to PlainHwnd.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn surface_refused_falls_back_across_real_adapters_hardware_before_software() {
    use crate::gpu::fallback::{Failure, Policy, Presentation, PresentationPolicy};
    use super::super::SurfaceSetupError;

    let instance = Gpu::instance();
    let available = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    if available.len() < 2 {
        return eprintln!("skipped: machine exposes fewer than two adapters ({})", available.len());
    }
    let has_cpu = available.iter().any(|a| a.get_info().device_type == wgpu::DeviceType::Cpu);

    let mut tried = Vec::new();
    let mut surfaces_made = 0usize;
    let mut hwnd_switches = 0usize;
    let res = Gpu::try_setup_on(
        instance,
        (),
        Some(Policy::Automatic),
        Presentation::DirectComposition,
        PresentationPolicy::Automatic,
        wgpu::Features::empty(),
        false,
        |_, pres| {
            assert_eq!(pres, Presentation::DirectComposition);
            surfaces_made += 1;
            Ok(())
        },
        |_| {
            hwnd_switches += 1;
            Gpu::instance()
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
    assert_eq!(hwnd_switches, 0, "generic SurfaceRefused must never switch to PlainHwnd");
    assert_eq!(gpu.presentation(), Presentation::DirectComposition);
    assert!(tried.len() >= 2, "expected at least one fallback attempt, tried: {tried:?}");
    assert!(surfaces_made >= 1, "a fresh surface must be created after a refused configuration");
    if has_cpu {
        assert_eq!(gpu.adapter.get_info().device_type, wgpu::DeviceType::Cpu);
        // Hardware adapters must all have been tried before software (WARP).
        let first_cpu = tried.iter().position(|(_, kind)| *kind == wgpu::DeviceType::Cpu).unwrap();
        assert_eq!(first_cpu, tried.len() - 1, "software adapter must be tried last: {tried:?}");
    }
}

/// Real offscreen test (no native window): when DirectComposition is unavailable
/// (`Failure::DirectCompositionUnavailable`), the chain switches immediately to
/// `Presentation::PlainHwnd` and retries on the SAME first adapter without walking remaining
/// adapters or WARP first.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dcomp_unavailable_switches_to_hwnd_on_same_first_adapter_without_walking_adapters() {
    use crate::gpu::fallback::{Failure, Policy, Presentation, PresentationPolicy};
    use super::super::SurfaceSetupError;

    let visual_instance = Gpu::instance_for_presentation(
        wgpu::Features::empty(),
        Presentation::DirectComposition,
    );
    let hwnd_instance = Gpu::instance_for_presentation(
        wgpu::Features::empty(),
        Presentation::PlainHwnd,
    );

    let mut attempts = Vec::new();
    let mut surfaces_made = Vec::new();
    let mut hwnd_switches = 0usize;
    let res = Gpu::try_setup_on(
        visual_instance,
        (),
        Some(Policy::Automatic),
        Presentation::DirectComposition,
        PresentationPolicy::Automatic,
        wgpu::Features::empty(),
        false,
        |_, pres| {
            surfaces_made.push(pres);
            Ok(())
        },
        |reason| {
            assert!(reason.contains("E_NOTIMPL"), "reason={reason}");
            hwnd_switches += 1;
            hwnd_instance.clone()
        },
        |_| None,
        |gpu, _| {
            let info = gpu.adapter.get_info();
            attempts.push((gpu.presentation(), info.name.clone(), info.device_type));
            if gpu.presentation() == Presentation::DirectComposition {
                return Err(SurfaceSetupError {
                    kind: Failure::DirectCompositionUnavailable,
                    message: "CreateSwapChainForComposition: E_NOTIMPL".into(),
                });
            }
            Ok(99u32)
        },
    );
    let (gpu, (), value) = res.expect("PlainHwnd retry must succeed on the same first adapter");
    assert_eq!(value, 99);
    assert_eq!(hwnd_switches, 1);
    assert_eq!(surfaces_made, vec![Presentation::PlainHwnd]);
    assert_eq!(gpu.presentation(), Presentation::PlainHwnd);
    assert_eq!(
        attempts.len(),
        2,
        "must try first adapter on DComp then immediately retry on PlainHwnd without walking other adapters: {attempts:?}"
    );
    assert_eq!(attempts[0].0, Presentation::DirectComposition);
    assert_eq!(attempts[1].0, Presentation::PlainHwnd);
    assert_eq!(attempts[0].1, attempts[1].1, "must keep the same first hardware adapter");
    assert_eq!(attempts[0].2, attempts[1].2);
}

/// Launch-speed guarantee on real Windows: when the first adapter succeeds on DirectComposition,
/// no fallback adapters are tried, `make_surface` is never called a second time, `switch_to_hwnd`
/// is never called, and `DCOMP_UNAVAILABLE` stays false.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn happy_path_first_adapter_does_no_fallback_work() {
    use crate::gpu::fallback::{Policy, Presentation, PresentationPolicy};

    let instance = Gpu::instance();
    let mut tried = 0usize;
    let mut extra_surfaces = 0usize;
    let mut hwnd_switches = 0usize;
    let res = Gpu::try_setup_on(
        instance,
        (),
        Some(Policy::Automatic),
        Presentation::DirectComposition,
        PresentationPolicy::Automatic,
        wgpu::Features::empty(),
        false,
        |_, _| {
            extra_surfaces += 1;
            Ok(())
        },
        |_| {
            hwnd_switches += 1;
            Gpu::instance()
        },
        |_| None,
        |gpu, _| {
            assert_eq!(gpu.presentation(), Presentation::DirectComposition);
            tried += 1;
            Ok(42u32)
        },
    );
    let (gpu, (), value) = res.expect("first adapter succeeds");
    assert_eq!(value, 42);
    assert_eq!(gpu.presentation(), Presentation::DirectComposition);
    assert_eq!(tried, 1);
    assert_eq!(extra_surfaces, 0);
    assert_eq!(hwnd_switches, 0);
    assert!(!Gpu::dcomp_unavailable());
}

/// Transient OOM, device loss, pinned (named) adapters, and pinned presentation
/// (`WGPU_DX12_PRESENTATION_SYSTEM=visual` or overlays) never switch to another adapter or HWND.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn transient_and_pinned_failures_do_not_switch_real_adapters() {
    use crate::gpu::fallback::{Failure, Policy, Presentation, PresentationPolicy};
    use super::super::SurfaceSetupError;

    for (policy, pres_policy, kind) in [
        (Policy::Automatic, PresentationPolicy::Automatic, Failure::OutOfMemory),
        (Policy::Automatic, PresentationPolicy::Automatic, Failure::DeviceLost),
        (Policy::Pinned, PresentationPolicy::Automatic, Failure::SurfaceRefused),
        (Policy::Automatic, PresentationPolicy::Pinned, Failure::DirectCompositionUnavailable),
    ] {
        let mut tried = 0usize;
        let mut extra_surfaces = 0usize;
        let mut hwnd_switches = 0usize;
        let err = Gpu::try_setup_on(
            Gpu::instance(),
            (),
            Some(policy),
            Presentation::DirectComposition,
            pres_policy,
            wgpu::Features::empty(),
            false,
            |_, _| {
                extra_surfaces += 1;
                Ok(())
            },
            |_| {
                hwnd_switches += 1;
                Gpu::instance()
            },
            |_| None,
            |_, _| {
                tried += 1;
                Err::<(), _>(SurfaceSetupError { kind, message: "stop here".into() })
            },
        )
        .err()
        .expect("must stop with error");
        assert_eq!(err.kind, kind, "policy={policy:?}, pres_policy={pres_policy:?}, kind={kind:?}");
        assert_eq!(err.message, "stop here");
        assert_eq!(tried, 1, "must not try a second adapter for policy={policy:?}, kind={kind:?}");
        assert_eq!(extra_surfaces, 0);
        assert_eq!(hwnd_switches, 0);
    }
}

/// When every adapter refuses configuration, the chain returns the first error so `Host::create`
/// hands it to `Life::fail_create` / `run()` (PR #1762).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn exhausting_every_real_adapter_returns_first_error() {
    use crate::gpu::fallback::{Failure, Policy, Presentation, PresentationPolicy};
    use super::super::SurfaceSetupError;

    let mut tried = 0usize;
    let mut hwnd_switches = 0usize;
    let err = Gpu::try_setup_on(
        Gpu::instance(),
        (),
        Some(Policy::Automatic),
        Presentation::DirectComposition,
        PresentationPolicy::Automatic,
        wgpu::Features::empty(),
        false,
        |_, _| Ok(()),
        |_| {
            hwnd_switches += 1;
            Gpu::instance()
        },
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
    assert_eq!(hwnd_switches, 0);
    assert!(err.message.starts_with("refused #1 on "), "got: {}", err.message);
}

/// Offscreen software fallback picks `DeviceType::Cpu` (WARP on Windows) and builds a live
/// device without falling back to any hardware adapter.
#[cfg(target_os = "windows")]
#[test]
fn software_fallback_picks_cpu_adapter_offscreen() {
    let instance = Gpu::instance();
    let (gpu, (), value) = Gpu::try_software_setup_on(
        instance,
        (),
        wgpu::Features::empty(),
        |_| None,
        |gpu, _| {
            assert_eq!(gpu.adapter.get_info().device_type, wgpu::DeviceType::Cpu);
            assert!(!gpu.is_lost());
            Ok(7u32)
        },
    )
    .expect("Windows DX12 must expose WARP for software fallback");
    assert_eq!(value, 7);
    assert_eq!(gpu.adapter.get_info().device_type, wgpu::DeviceType::Cpu);
}

#[test]
fn configure_error_classification_separates_oom_from_refusal_and_dcomp_unavailable() {
    use crate::window::{ConfigureErrorKind, classify_scope_error};

    let mk_oom = || wgpu::Error::OutOfMemory { source: Box::new(std::io::Error::other("oom")) };
    let mk_internal = || wgpu::Error::Internal {
        source: Box::new(std::io::Error::other("swapchain")),
        description: "swapchain creation".into(),
    };
    let mk_validation = || wgpu::Error::Validation {
        source: Box::new(std::io::Error::other("Invalid surface")),
        description: "Invalid surface".into(),
    };

    assert_eq!(classify_scope_error(None, None, None, false).map(|(k, _)| k), None);
    assert_eq!(classify_scope_error(None, None, None, true).map(|(k, _)| k), None);
    // OutOfMemory always wins, even if a HAL flag were set.
    assert_eq!(
        classify_scope_error(Some(mk_oom()), Some(mk_internal()), None, true).map(|(k, _)| k),
        Some(ConfigureErrorKind::OutOfMemory),
    );
    // Without dcomp_unavailable, internal and validation errors stay Refused.
    assert_eq!(
        classify_scope_error(None, Some(mk_internal()), Some(mk_validation()), false).map(|(k, _)| k),
        Some(ConfigureErrorKind::Refused),
    );
    assert_eq!(
        classify_scope_error(None, None, Some(mk_validation()), false).map(|(k, _)| k),
        Some(ConfigureErrorKind::Refused),
    );
    // With dcomp_unavailable, wgpu-core's InvalidSurface validation error classifies as DirectCompositionUnavailable.
    assert_eq!(
        classify_scope_error(None, None, Some(mk_validation()), true).map(|(k, _)| k),
        Some(ConfigureErrorKind::DirectCompositionUnavailable),
    );
    assert_eq!(
        classify_scope_error(None, Some(mk_internal()), None, true).map(|(k, _)| k),
        Some(ConfigureErrorKind::DirectCompositionUnavailable),
    );
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::DeviceLost), rune_gpu::fallback::Failure::DeviceLost);
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::OutOfMemory), rune_gpu::fallback::Failure::OutOfMemory);
    assert_eq!(
        rune_gpu::fallback::Failure::from(ConfigureErrorKind::DirectCompositionUnavailable),
        rune_gpu::fallback::Failure::DirectCompositionUnavailable,
    );
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::Refused), rune_gpu::fallback::Failure::SurfaceRefused);
}

#[test]
fn window_redirection_bitmap_omitted_on_plain_hwnd_fallback() {
    use crate::gpu::fallback::Presentation;
    use crate::window::spec::needs_no_redirection_bitmap;

    assert!(needs_no_redirection_bitmap(true, Presentation::DirectComposition));
    assert!(!needs_no_redirection_bitmap(true, Presentation::PlainHwnd));
    assert!(!needs_no_redirection_bitmap(false, Presentation::DirectComposition));
    assert!(!needs_no_redirection_bitmap(false, Presentation::PlainHwnd));
}
