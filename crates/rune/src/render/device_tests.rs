use super::Gpu;

#[test]
fn default_instance_descriptor_hides_noncompliant_adapters() {
    let desc = Gpu::instance_descriptor(wgpu::Backends::all(), false);
    assert!(!desc.flags.contains(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER));
    assert!(!desc.flags.contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL));
    assert_eq!(
        desc.backend_options.dx12.presentation_system,
        Gpu::dx12_swapchain_kind(),
    );
    assert!(matches!(
        desc.backend_options.dx12.latency_waitable_object,
        wgpu::Dx12UseFrameLatencyWaitableObject::DontWait,
    ));
    assert!(matches!(desc.backend_options.dx12.shader_compiler, wgpu::Dx12Compiler::Fxc));
}

#[test]
fn fallback_instance_descriptor_allows_noncompliant_adapters() {
    let desc = Gpu::instance_descriptor(wgpu::Backends::all(), true);
    assert!(desc.flags.contains(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER));
    assert!(!desc.flags.contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore]
fn dx12_presentation_system_env_child() {
    let expected = match crate::env::var("RUNE_TEST_EXPECT_SWAPCHAIN").as_deref() {
        Some("hwnd") => wgpu::Dx12SwapchainKind::DxgiFromHwnd,
        Some("visual") => wgpu::Dx12SwapchainKind::DxgiFromVisual,
        other => panic!("unexpected RUNE_TEST_EXPECT_SWAPCHAIN: {other:?}"),
    };
    assert_eq!(Gpu::dx12_swapchain_kind(), expected);
    let desc = Gpu::instance_descriptor(wgpu::Backends::DX12, false);
    assert_eq!(desc.backend_options.dx12.presentation_system, expected);
    assert!(matches!(
        desc.backend_options.dx12.latency_waitable_object,
        wgpu::Dx12UseFrameLatencyWaitableObject::DontWait,
    ));
    assert!(matches!(desc.backend_options.dx12.shader_compiler, wgpu::Dx12Compiler::Fxc));
    #[cfg(target_os = "windows")]
    {
        let instance = Gpu::instance();
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
        assert!(!adapters.is_empty(), "DX12 instance ({expected:?}) must enumerate adapters offscreen");
    }
}

/// Verifies wgpu's `WGPU_DX12_PRESENTATION_SYSTEM` override (`Dx12SwapchainKind::with_env`) in
/// isolated child processes so parallel tests never race on environment variables: unset defaults
/// to DirectComposition (`DxgiFromVisual`), `hwnd` selects a plain HWND swap chain
/// (`DxgiFromHwnd`) and enumerates DX12 adapters offscreen on Windows, and `visual` selects
/// DirectComposition explicitly.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dx12_presentation_system_env_override_selects_hwnd_or_visual() {
    let exe = std::env::current_exe().expect("test binary path");
    for (env_value, expected) in [(None, "visual"), (Some("hwnd"), "hwnd"), (Some("visual"), "visual")] {
        let mut cmd = std::process::Command::new(&exe);
        cmd.args([
            "--exact",
            "render::device::tests::dx12_presentation_system_env_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNE_TEST_EXPECT_SWAPCHAIN", expected);
        match env_value {
            Some(val) => {
                cmd.env("WGPU_DX12_PRESENTATION_SYSTEM", val);
            }
            None => {
                cmd.env_remove("WGPU_DX12_PRESENTATION_SYSTEM");
            }
        }
        let output = cmd.output().expect("spawn child test");
        assert!(
            output.status.success(),
            "env={env_value:?} expected={expected} failed: status={:?}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
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

    if !Gpu::test_adapter_available() { return; }
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

/// Launch-speed guarantee on real Windows: when the first adapter succeeds, no fallback adapters
/// are tried, `make_surface` is never called a second time, and the default swap-chain kind stays
/// DirectComposition (`DxgiFromVisual`).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn happy_path_first_adapter_does_no_fallback_work() {
    use crate::gpu::fallback::Policy;

    if !Gpu::test_adapter_available() { return; }
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
    let (_gpu, (), value) = res.expect("first adapter succeeds");
    assert_eq!(value, 42);
    assert_eq!(tried, 1);
    assert_eq!(extra_surfaces, 0);
    if std::env::var_os("WGPU_DX12_PRESENTATION_SYSTEM").is_none() {
        assert_eq!(Gpu::dx12_swapchain_kind(), wgpu::Dx12SwapchainKind::DxgiFromVisual);
    }
}

/// Transient OOM, device loss, and pinned (named) adapter failures never switch to another
/// adapter.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn transient_and_pinned_failures_do_not_switch_real_adapters() {
    use crate::gpu::fallback::{Failure, Policy};
    use super::super::SurfaceSetupError;

    if !Gpu::test_adapter_available() { return; }
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

/// When every adapter refuses configuration (such as missing DirectComposition without the explicit
/// switch), the chain walks every adapter, preserves `Failure::SurfaceRefused` and the first
/// error, and `Host::create` hands it (with the Wine hint when under Wine) to `Life::fail_create`
/// / `run()` (PR #1762) with no blank window and no panic.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn exhausting_every_real_adapter_returns_first_error() {
    use crate::gpu::fallback::{Failure, Policy};
    use crate::window::{Life, finish_run};
    use super::super::{SurfaceSetupError, surface_setup::append_wine_hwnd_hint};

    if !Gpu::test_adapter_available() { return; }
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
    assert_eq!(err.kind, Failure::SurfaceRefused);
    assert!(err.message.starts_with("refused #1 on "), "got: {}", err.message);

    // Mirror the error reaching AppHandle::window_error and App::try_run under Wine.
    let hinted = append_wine_hwnd_hint(err.message.clone(), wgpu::Dx12SwapchainKind::DxgiFromVisual, true);
    assert!(hinted.contains("WGPU_DX12_PRESENTATION_SYSTEM=hwnd"), "got: {hinted}");
    let app = crate::App::new(crate::AppSpec::new("test", (40.0, 30.0), || (), |_, _| crate::el()));
    let handle = app.handle();
    let (_, _, shared, _) = app.into_parts(None);
    shared.core.set_window_error(Some(hinted.clone()));
    assert_eq!(handle.window_error().as_deref(), Some(hinted.as_str()));

    let mut life = Life::Unopened;
    assert!(life.fail_create(false, false), "first window failure ends try_run");
    let startup_error = format!("could not create the window main: {hinted}");
    assert_eq!(finish_run(Ok(()), None, Some(startup_error.clone()), None), Err(startup_error));
}

#[test]
fn wine_hwnd_hint_only_appended_for_visual_under_wine() {
    use super::super::surface_setup::append_wine_hwnd_hint;
    use wgpu::Dx12SwapchainKind::{DxgiFromHwnd, DxgiFromVisual};

    let base = "the window's GPU surface refused its configuration: Invalid surface";
    let hinted = append_wine_hwnd_hint(base.into(), DxgiFromVisual, true);
    assert!(hinted.starts_with(base));
    assert!(hinted.contains("WGPU_DX12_PRESENTATION_SYSTEM=hwnd"), "got: {hinted}");

    assert_eq!(append_wine_hwnd_hint(base.into(), DxgiFromVisual, false), base);
    assert_eq!(append_wine_hwnd_hint(base.into(), DxgiFromHwnd, true), base);
    assert_eq!(append_wine_hwnd_hint(base.into(), DxgiFromHwnd, false), base);

    #[cfg(target_os = "windows")]
    assert!(
        !crate::platform::win32::running_under_wine(),
        "real Windows ntdll.dll must not report wine_get_version"
    );
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
fn configure_error_classification_separates_oom_from_refusal() {
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

    assert_eq!(classify_scope_error(None, None, None).map(|(k, _)| k), None);
    assert_eq!(
        classify_scope_error(Some(mk_oom()), Some(mk_internal()), None).map(|(k, _)| k),
        Some(ConfigureErrorKind::OutOfMemory),
    );
    assert_eq!(
        classify_scope_error(None, Some(mk_internal()), Some(mk_validation())).map(|(k, _)| k),
        Some(ConfigureErrorKind::Refused),
    );
    assert_eq!(
        classify_scope_error(None, None, Some(mk_validation())).map(|(k, _)| k),
        Some(ConfigureErrorKind::Refused),
    );
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::DeviceLost), rune_gpu::fallback::Failure::DeviceLost);
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::OutOfMemory), rune_gpu::fallback::Failure::OutOfMemory);
    assert_eq!(rune_gpu::fallback::Failure::from(ConfigureErrorKind::Refused), rune_gpu::fallback::Failure::SurfaceRefused);
}

/// Real `wgpu-core` device check for the shared `rune_gpu::health::with_error_scopes` helper:
/// captures a validation error without panicking on pop order and leaves no open scope on the
/// device afterwards.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn with_error_scopes_captures_error_and_leaves_real_device_stack_clean() {
    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    let ((), errors) = rune_gpu::health::with_error_scopes(&gpu.device, || {
        drop(gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("intentionally-invalid-scope-test"),
            source: wgpu::ShaderSource::Wgsl("@vertex fn vs() -> @builtin(position) vec4<f32> { return missing_ident; }".into()),
        }));
    });
    assert!(errors.validation.is_some(), "validation scope must capture invalid WGSL");
    assert!(errors.memory.is_none());
    assert!(errors.internal.is_none());

    // A second clean call must succeed with no leftover open scope on the device.
    let ((), clean) = rune_gpu::health::with_error_scopes(&gpu.device, || {});
    assert!(clean.validation.is_none());
    assert!(clean.memory.is_none());
    assert!(clean.internal.is_none());
}

#[test]
fn window_redirection_bitmap_omitted_on_plain_hwnd_swapchain() {
    use crate::window::spec::needs_no_redirection_bitmap;
    use wgpu::Dx12SwapchainKind::{DxgiFromHwnd, DxgiFromVisual};

    assert!(needs_no_redirection_bitmap(true, DxgiFromVisual));
    assert!(!needs_no_redirection_bitmap(true, DxgiFromHwnd));
    assert!(!needs_no_redirection_bitmap(false, DxgiFromVisual));
    assert!(!needs_no_redirection_bitmap(false, DxgiFromHwnd));
}

/// Real offscreen multi-adapter fallback test: when the first hardware adapter(s) reject Rune's
/// last-resort Classic shader, the chain moves on across remaining hardware adapters before
/// software rendering (WARP / CPU), and the fallback adapter builds a working renderer.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn rejected_classic_shader_falls_back_across_real_adapters_hardware_before_software() {
    use crate::gpu::fallback::Policy;
    use super::super::{OFFSCREEN_FORMAT, Renderer, SurfaceSetupError, shader_check};

    if !Gpu::test_adapter_available() { return; }
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
            let refuse = if has_cpu {
                info.device_type != wgpu::DeviceType::Cpu
            } else {
                tried.len() == 1
            };
            shader_check::with_uber_shaders_refused(refuse, || {
                Renderer::try_new_classified(&gpu.device, OFFSCREEN_FORMAT)
                    .map_err(SurfaceSetupError::from_shader)
            })
        },
    );
    let (gpu, (), renderer) = res.expect("fallback chain must reach an adapter whose shader compiles");
    assert_eq!(renderer.format(), OFFSCREEN_FORMAT);
    assert!(tried.len() >= 2, "expected at least one fallback attempt, tried: {tried:?}");
    assert!(surfaces_made >= 1, "a fresh surface must be created after a shader rejection");
    if has_cpu {
        assert_eq!(gpu.adapter.get_info().device_type, wgpu::DeviceType::Cpu);
        let first_cpu = tried.iter().position(|(_, kind)| *kind == wgpu::DeviceType::Cpu).unwrap();
        assert_eq!(first_cpu, tried.len() - 1, "software adapter must be tried last: {tried:?}");
    }
}

/// A named (`Policy::Pinned`) adapter that rejects the last-resort shader is never replaced: its
/// failure goes straight to the error without enumerating other adapters.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pinned_adapter_rejected_shader_is_not_replaced() {
    use crate::gpu::fallback::{Failure, Policy};
    use super::super::{OFFSCREEN_FORMAT, Renderer, SurfaceSetupError, shader_check};

    if !Gpu::test_adapter_available() { return; }
    let mut tried = 0usize;
    let mut extra_surfaces = 0usize;
    let err = Gpu::try_setup_on(
        Gpu::instance(),
        (),
        Some(Policy::Pinned),
        wgpu::Features::empty(),
        false,
        |_| {
            extra_surfaces += 1;
            Ok(())
        },
        |_| None,
        |gpu, _| {
            tried += 1;
            shader_check::with_uber_shaders_refused(true, || {
                Renderer::try_new_classified(&gpu.device, OFFSCREEN_FORMAT)
                    .map_err(SurfaceSetupError::from_shader)
            })
        },
    )
    .err()
    .expect("pinned adapter shader rejection must fail");
    assert_eq!(err.kind, Failure::ShaderRejected);
    assert!(err.message.contains("rejected Rune's shader"), "got: {}", err.message);
    assert_eq!(tried, 1, "a pinned adapter must never fall back to another adapter");
    assert_eq!(extra_surfaces, 0);
}

/// When every adapter rejects the last-resort shader, the chain returns the first error so
/// `Host::create` records `AppHandle::window_error` and ends `App::try_run` with the error.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn exhausting_every_adapter_on_rejected_shader_reaches_window_error_and_try_run() {
    use crate::gpu::fallback::{Failure, Policy};
    use crate::window::{Life, finish_run};
    use super::super::{OFFSCREEN_FORMAT, Renderer, SurfaceSetupError, shader_check};

    if !Gpu::test_adapter_available() { return; }
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
            shader_check::with_uber_shaders_refused(true, || {
                Renderer::try_new_classified(&gpu.device, OFFSCREEN_FORMAT)
                    .map_err(SurfaceSetupError::from_shader)
            })
        },
    )
    .err()
    .expect("all adapters rejected the shader");
    assert!(tried >= 1);
    assert_eq!(err.kind, Failure::ShaderRejected);
    assert!(err.message.contains("rejected Rune's shader"), "got: {}", err.message);

    // Mirror Host::finish_create -> Life::fail_create -> finish_run (App::try_run).
    let app = crate::App::new(crate::AppSpec::new("test", (40.0, 30.0), || (), |_, _| crate::el()));
    let handle = app.handle();
    let (_, _, shared, _) = app.into_parts(None);
    shared.core.set_window_error(Some(err.message.clone()));
    assert_eq!(handle.window_error().as_deref(), Some(err.message.as_str()));

    let mut life = Life::Unopened;
    assert!(life.fail_create(false, false), "first window failure ends try_run");
    let startup_error = format!("could not create the window main: {}", err.message);
    let run_result = finish_run(Ok(()), None, Some(startup_error.clone()), None);
    assert_eq!(run_result, Err(startup_error));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore]
fn headless_run_rejected_shader_child() {
    use super::super::shader_check;
    let _turn = crate::tooling::headless_run::test_turn();
    let path = crate::env::var("RUNE_TEST_HEADLESS_SNAP").expect("snap path env");
    shader_check::with_uber_shaders_refused(true, || {
        crate::tooling::run_windows_with_args(
            crate::RunOptions::default(),
            ["--snap".into(), path],
            |_| [crate::WindowSpec::from_app("main", crate::App::new(crate::AppSpec::new("test", (40.0, 30.0), || (), |_, _| crate::el())))],
        )
    });
}

/// Fallible headless entry points (`App::try_run`, `try_run_windows_with_args`) return `Err`
/// instead of ending the test process when GPU/shader setup fails.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn try_run_headless_returns_error_instead_of_exiting_process() {
    use super::super::shader_check;
    let _turn = crate::tooling::headless_run::test_turn();
    let snap = std::env::temp_dir().join(format!(
        "rune-try-run-headless-err-{}.png",
        std::process::id(),
    ));
    std::fs::remove_file(&snap).ok();
    let err = shader_check::with_uber_shaders_refused(true, || {
        crate::tooling::try_run_windows_with_args(
            crate::RunOptions::default(),
            ["--snap".into(), snap.to_string_lossy().into_owned()],
            |_| [crate::WindowSpec::from_app("main", crate::App::new(crate::AppSpec::new("test", (40.0, 30.0), || (), |_, _| crate::el())))],
        )
    })
    .expect_err("failed headless GPU setup must return Err, not exit the process");
    assert!(
        err.contains("rejected Rune's shader") || err.starts_with("no GPU adapter:"),
        "unexpected error: {err}",
    );
    assert!(!snap.exists(), "failed headless run must not write a snap image");
}

/// A headless run whose every adapter rejects the last-resort shader exits non-zero with the
/// shader error on stderr instead of writing a blank image or panicking.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn headless_run_exits_nonzero_when_every_adapter_rejects_classic_shader() {
    if !Gpu::test_adapter_available() { return; }
    let exe = std::env::current_exe().expect("test binary path");
    let snap = std::env::temp_dir().join(format!(
        "rune-rejected-shader-headless-{}.png",
        std::process::id(),
    ));
    std::fs::remove_file(&snap).ok();
    let output = std::process::Command::new(exe)
        .args([
            "--exact",
            "render::device::tests::headless_run_rejected_shader_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNE_TEST_HEADLESS_SNAP", &snap)
        .output()
        .expect("spawn headless child test");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && output.status.code() == Some(2),
        "headless run must exit non-zero (die = 2) on rejected shader, got {:?}, stderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stderr.contains("rejected Rune's shader"),
        "expected shader error on stderr, got:\n{stderr}",
    );
    assert!(!snap.exists(), "failed headless run must not write a snap image");
}

/// Simulates a Linux direct-driver instance whose single driver cannot serve a surface: when
/// `pick_first_adapter` finds no usable adapter on the restricted instance, the fallback chain
/// widens once to the unlimited instance, recreates the surface, and succeeds.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn direct_driver_pick_failure_widens_once_to_unlimited_instance() {
    use crate::gpu::fallback::Policy;

    if !Gpu::test_adapter_available() { return; }
    let mut empty_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    empty_desc.backends = wgpu::Backends::empty();
    let restricted = wgpu::Instance::new(empty_desc);
    let unlimited = Gpu::instance();

    let mut widen_calls = 0usize;
    let mut surfaces_made = 0usize;
    let res = Gpu::try_setup_on_with_widen(
        restricted,
        (),
        Some(Policy::Automatic),
        wgpu::Features::empty(),
        false,
        |_| {
            surfaces_made += 1;
            Ok(())
        },
        |_| {
            widen_calls += 1;
            Some(unlimited.clone())
        },
        |_| None,
        |_, _| Ok(7u32),
    );
    let (_gpu, (), value) = res.expect("widening to unlimited instance must recover");
    assert_eq!(value, 7);
    assert_eq!(widen_calls, 1, "widening runs at most once");
    assert_eq!(surfaces_made, 1, "surface is recreated on the widened instance");
}

/// When all adapters on a direct-driver instance refuse configuration with `SurfaceRefused`,
/// the fallback chain widens once to the unlimited instance (skipping already-tried adapters),
/// and never loops or widens on transient (`OutOfMemory`/`DeviceLost`) or `Pinned` failures.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn direct_driver_surface_refused_widens_once_and_never_widens_on_stop_or_happy_path() {
    use crate::gpu::fallback::{Failure, Policy};
    use super::super::SurfaceSetupError;

    if !Gpu::test_adapter_available() { return; }
    // 1. Exhausting adapters with SurfaceRefused triggers widen_instance exactly once and
    // preserves the first error if the widened instance has no untried adapters left.
    let mut widen_calls = 0usize;
    let mut surfaces_made = 0usize;
    let err = Gpu::try_setup_on_with_widen(
        Gpu::instance(),
        (),
        Some(Policy::Automatic),
        wgpu::Features::empty(),
        false,
        |_| {
            surfaces_made += 1;
            Ok(())
        },
        |reason| {
            assert!(reason.starts_with("refused #1"), "got reason={reason}");
            widen_calls += 1;
            Some(Gpu::instance())
        },
        |_| None,
        |gpu, _| {
            Err::<(), _>(SurfaceSetupError {
                kind: Failure::SurfaceRefused,
                message: format!("refused #1 on {}", gpu.adapter.get_info().name),
            })
        },
    )
    .err()
    .expect("all adapters refused");
    assert_eq!(widen_calls, 1, "must widen once when direct instance exhausts candidates");
    assert!(surfaces_made >= 2, "must recreate surface on widened instance");
    assert!(err.message.starts_with("refused #1 on "), "got: {}", err.message);

    // 2. Happy path, OOM, DeviceLost, and Pinned never call widen_instance.
    let mut happy_widens = 0usize;
    let ok = Gpu::try_setup_on_with_widen(
        Gpu::instance(),
        (),
        Some(Policy::Automatic),
        wgpu::Features::empty(),
        false,
        |_| Ok(()),
        |_| {
            happy_widens += 1;
            Some(Gpu::instance())
        },
        |_| None,
        |_, _| Ok(1u32),
    );
    assert!(ok.is_ok());
    assert_eq!(happy_widens, 0, "happy path must never widen");

    for (policy, kind) in [
        (Policy::Automatic, Failure::OutOfMemory),
        (Policy::Automatic, Failure::DeviceLost),
        (Policy::Pinned, Failure::SurfaceRefused),
    ] {
        let mut stop_widens = 0usize;
        let err = Gpu::try_setup_on_with_widen(
            Gpu::instance(),
            (),
            Some(policy),
            wgpu::Features::empty(),
            false,
            |_| Ok(()),
            |_| {
                stop_widens += 1;
                Some(Gpu::instance())
            },
            |_| None,
            |_, _| Err::<(), _>(SurfaceSetupError { kind, message: "stop".into() }),
        )
        .err()
        .expect("must stop");
        assert_eq!(err.kind, kind);
        assert_eq!(stop_widens, 0, "must not widen on policy={policy:?} kind={kind:?}");
    }
}

#[test]
fn gpu_memory_policy_defaults_to_memory_usage_and_maps_to_wgpu_hints() {
    use crate::GpuMemoryPolicy;
    use crate::gpu::memory::to_wgpu_hints;
    const MIB: u64 = 1024 * 1024;

    let assert_manual = |policy: GpuMemoryPolicy, expected: std::ops::Range<u64>| match to_wgpu_hints(&policy) {
        wgpu::MemoryHints::Manual { suballocated_device_memory_block_size } => {
            assert_eq!(suballocated_device_memory_block_size, expected);
        }
        other => panic!("expected Manual({expected:?}), got {other:?}"),
    };

    assert_eq!(GpuMemoryPolicy::default(), GpuMemoryPolicy::MemoryUsage);
    assert!(matches!(to_wgpu_hints(&GpuMemoryPolicy::default()), wgpu::MemoryHints::MemoryUsage));
    assert!(matches!(to_wgpu_hints(&GpuMemoryPolicy::Performance), wgpu::MemoryHints::Performance));
    assert_manual(GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(8 * MIB) }, (4 * MIB)..(8 * MIB));
    assert_manual(GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(4 * MIB) }, (4 * MIB)..(4 * MIB));
    assert_manual(GpuMemoryPolicy::Manual { block_size: 0..0 }, 1..1);
    let range = |start, end| std::ops::Range { start, end };
    assert_manual(GpuMemoryPolicy::Manual { block_size: range(16 * MIB, 4 * MIB) }, (16 * MIB)..(16 * MIB));
    assert_manual(GpuMemoryPolicy::Manual { block_size: range(u64::MAX, 0) }, u64::MAX..u64::MAX);
}

#[test]
fn apply_gpu_memory_policy_chooses_before_invalidating_and_skips_when_unchanged() {
    let log = std::cell::RefCell::new(Vec::new());
    super::apply_gpu_memory_policy(
        crate::GpuMemoryPolicy::Performance,
        |p| {
            log.borrow_mut().push(format!("choose {p:?}"));
            true
        },
        || log.borrow_mut().push("invalidate".into()),
    );
    assert_eq!(*log.borrow(), ["choose Performance", "invalidate"]);

    log.borrow_mut().clear();
    super::apply_gpu_memory_policy(
        crate::GpuMemoryPolicy::Performance,
        |p| {
            log.borrow_mut().push(format!("choose {p:?}"));
            false
        },
        || log.borrow_mut().push("invalidate".into()),
    );
    assert_eq!(*log.borrow(), ["choose Performance"]);
}

/// Matches an initial suballocation block size against wgpu-hal's
/// `AllocationSizes::from_memory_hints` `(device_block, host_block)`.
///
/// D3D12 always places `STORAGE | COPY_DST` buffers in `D3D12_HEAP_TYPE_DEFAULT` (device blocks)
/// and discrete Vulkan GPUs expose a non-host-visible `DEVICE_LOCAL` heap (device blocks). On
/// CPU-only or unified-memory Vulkan adapters (such as Mesa llvmpipe), `DEVICE_LOCAL` memory
/// types are also `HOST_VISIBLE`, so `gpu-allocator` allocates from the host block size (half
/// the device block size, clamped to 4 MiB..=256 MiB).
#[cfg(not(target_arch = "wasm32"))]
fn expected_memory_block_matches(
    backend: wgpu::Backend,
    device_type: wgpu::DeviceType,
    device_block: u64,
    host_block: u64,
    actual: u64,
) -> bool {
    actual == device_block
        || (backend == wgpu::Backend::Vulkan
            && device_type != wgpu::DeviceType::DiscreteGpu
            && actual == host_block)
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn expected_memory_block_matches_allows_host_block_only_on_non_discrete_vulkan() {
    use wgpu::{Backend, DeviceType};
    const MIB: u64 = 1024 * 1024;
    for (backend, kind) in [
        (Backend::Dx12, DeviceType::DiscreteGpu),
        (Backend::Dx12, DeviceType::Cpu),
        (Backend::Vulkan, DeviceType::DiscreteGpu),
    ] {
        assert!(expected_memory_block_matches(backend, kind, 8 * MIB, 4 * MIB, 8 * MIB));
        assert!(!expected_memory_block_matches(backend, kind, 8 * MIB, 4 * MIB, 4 * MIB));
        assert!(expected_memory_block_matches(backend, kind, 128 * MIB, 64 * MIB, 128 * MIB));
        assert!(!expected_memory_block_matches(backend, kind, 128 * MIB, 64 * MIB, 64 * MIB));
    }
    for kind in [DeviceType::Cpu, DeviceType::IntegratedGpu] {
        assert!(expected_memory_block_matches(Backend::Vulkan, kind, 8 * MIB, 4 * MIB, 4 * MIB));
        assert!(expected_memory_block_matches(Backend::Vulkan, kind, 8 * MIB, 4 * MIB, 8 * MIB));
        assert!(!expected_memory_block_matches(Backend::Vulkan, kind, 8 * MIB, 4 * MIB, 64 * MIB));
        assert!(expected_memory_block_matches(Backend::Vulkan, kind, 128 * MIB, 64 * MIB, 64 * MIB));
        assert!(!expected_memory_block_matches(Backend::Vulkan, kind, 128 * MIB, 64 * MIB, 4 * MIB));
        assert!(expected_memory_block_matches(Backend::Vulkan, kind, 4 * MIB, 4 * MIB, 4 * MIB));
        assert!(!expected_memory_block_matches(Backend::Vulkan, kind, 4 * MIB, 4 * MIB, 8 * MIB));
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore]
fn gpu_memory_policy_child() {
    use crate::GpuMemoryPolicy;
    const MIB: u64 = 1024 * 1024;

    let mode = crate::env::var("RUNE_TEST_GPU_MEMORY_POLICY").expect("policy test mode");
    let (expected_device, expected_host) = match mode.as_str() {
        "default" => (8 * MIB, 4 * MIB),
        "manual-setter" => {
            crate::set_gpu_memory_policy(GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(8 * MIB) });
            (4 * MIB, 4 * MIB)
        }
        "manual-app" => {
            let _app = crate::App::new(crate::AppSpec::new("test", (40.0, 30.0), || (), |_, _| crate::el()))
                .gpu_memory_policy(GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(8 * MIB) });
            (4 * MIB, 4 * MIB)
        }
        "performance" => {
            crate::set_gpu_memory_policy(GpuMemoryPolicy::Performance);
            (128 * MIB, 64 * MIB)
        }
        "parked-invalidation" => {
            #[cfg(not(target_os = "macos"))]
            {
                let Some(gpu) = Gpu::test_headless_or_noop() else { return };
                let renderer = super::super::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
                super::super::prewarm::park(gpu, renderer, super::super::prewarm::PARKED);
                // Setting the active policy again leaves the parked device alone.
                crate::set_gpu_memory_policy(GpuMemoryPolicy::MemoryUsage);
                let (gpu, renderer) = super::super::prewarm::take().expect("unchanged policy keeps parked GPU");
                super::super::prewarm::park(gpu, renderer, super::super::prewarm::PARKED);
                // Changing the policy discards the parked device so it is never silently reused.
                crate::set_gpu_memory_policy(GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(8 * MIB) });
                assert!(super::super::prewarm::take().is_none(), "changed policy must discard parked GPU");
            }
            return;
        }
        other => panic!("unexpected RUNE_TEST_GPU_MEMORY_POLICY: {other}"),
    };

    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    let small = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("memory-policy-small"),
        size: 64 * 1024,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let info = gpu.adapter.get_info();
    if matches!(info.backend, wgpu::Backend::Dx12 | wgpu::Backend::Vulkan) {
        if let Some(report) = gpu.device.generate_allocator_report() {
            if let Some(max_block) = report.blocks.iter().map(|b| b.size).max() {
                assert!(
                    expected_memory_block_matches(info.backend, info.device_type, expected_device, expected_host, max_block),
                    "mode={mode} backend={:?} device_type={:?} expected max block size {expected_device} (or host {expected_host}), got blocks: {:?}",
                    info.backend,
                    info.device_type,
                    report.blocks.iter().map(|b| b.size).collect::<Vec<_>>(),
                );
            }
        }
    }
    // Resources larger than a Manual block range still succeed via dedicated allocations.
    if mode.starts_with("manual-") {
        let (large, errors) = rune_gpu::health::with_error_scopes(&gpu.device, || {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("memory-policy-large"),
                size: 10 * MIB,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        assert!(errors.validation.is_none(), "unexpected validation error: {:?}", errors.validation);
        assert!(errors.memory.is_none(), "unexpected OOM error: {:?}", errors.memory);
        assert!(errors.internal.is_none(), "unexpected internal error: {:?}", errors.internal);
        drop(large);
    }
    drop(small);
}

/// Verifies startup `GpuMemoryPolicy` end-to-end in isolated child processes so parallel tests
/// never race on the process-wide policy or parked warm-up slot: default stays `MemoryUsage`,
/// `set_gpu_memory_policy` and `App::gpu_memory_policy` apply `Manual` (4..8 MiB, with larger
/// dedicated resources still succeeding) and `Performance` to created devices, and changing the
/// policy discards a parked warm-up device.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn gpu_memory_policy_reaches_offscreen_device_and_invalidates_parked_gpu() {
    let exe = std::env::current_exe().expect("test binary path");
    for mode in ["default", "manual-setter", "manual-app", "performance", "parked-invalidation"] {
        let output = std::process::Command::new(&exe)
            .args([
                "--exact",
                "render::device::tests::gpu_memory_policy_child",
                "--ignored",
                "--nocapture",
            ])
            .env("RUNE_TEST_GPU_MEMORY_POLICY", mode)
            .output()
            .expect("spawn child test");
        assert!(
            output.status.success(),
            "mode={mode} failed: status={:?}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
