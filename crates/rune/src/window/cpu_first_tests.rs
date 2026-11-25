use super::{CpuFirst, WebStart};

#[test]
fn cpu_first_defaults_to_auto() {
    assert_eq!(CpuFirst::default(), CpuFirst::Auto);
}

#[test]
fn parse_env_recognizes_supported_values_and_rejects_unknowns() {
    assert_eq!(CpuFirst::parse_env("0"), Some(CpuFirst::Off));
    assert_eq!(CpuFirst::parse_env("off"), Some(CpuFirst::Off));
    assert_eq!(CpuFirst::parse_env("OFF"), Some(CpuFirst::Off));
    assert_eq!(CpuFirst::parse_env(" off "), Some(CpuFirst::Off));
    assert_eq!(CpuFirst::parse_env("only"), Some(CpuFirst::Only));
    assert_eq!(CpuFirst::parse_env("ONLY"), Some(CpuFirst::Only));
    assert_eq!(CpuFirst::parse_env("auto"), Some(CpuFirst::Auto));
    assert_eq!(CpuFirst::parse_env("AUTO"), Some(CpuFirst::Auto));
    assert_eq!(CpuFirst::parse_env(""), None);
    assert_eq!(CpuFirst::parse_env("   "), None);
    assert_eq!(CpuFirst::parse_env("1"), None);
    assert_eq!(CpuFirst::parse_env("gpu"), None);
}

#[test]
fn env_override_takes_precedence_over_builder_mode() {
    for configured in [CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off] {
        assert_eq!(configured.resolve(None), configured);
        assert_eq!(configured.resolve(Some("")), configured);
        assert_eq!(configured.resolve(Some("unknown")), configured);
        assert_eq!(configured.resolve(Some("0")), CpuFirst::Off);
        assert_eq!(configured.resolve(Some("off")), CpuFirst::Off);
        assert_eq!(configured.resolve(Some("only")), CpuFirst::Only);
        assert_eq!(configured.resolve(Some("auto")), CpuFirst::Auto);
    }
}

#[test]
fn desktop_opens_on_cpu_decision_matrix() {
    // Transparent windows or windows that already hold a GPU never open on CPU.
    for mode in [CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off] {
        for live_pool_gpu in [false, true] {
            assert!(!mode.opens_on_cpu(true, false, live_pool_gpu), "{mode:?} transparent");
            assert!(!mode.opens_on_cpu(false, true, live_pool_gpu), "{mode:?} has_gpu");
            assert!(!mode.opens_on_cpu(true, true, live_pool_gpu), "{mode:?} both");
        }
    }

    // Opaque window with no own GPU:
    // Auto matches existing default: CPU-first only when no other window holds a live GPU.
    assert!(CpuFirst::Auto.opens_on_cpu(false, false, false));
    assert!(!CpuFirst::Auto.opens_on_cpu(false, false, true));

    // Only opens on CPU even if another window already holds a live GPU.
    assert!(CpuFirst::Only.opens_on_cpu(false, false, false));
    assert!(CpuFirst::Only.opens_on_cpu(false, false, true));

    // Off never opens on CPU.
    assert!(!CpuFirst::Off.opens_on_cpu(false, false, false));
    assert!(!CpuFirst::Off.opens_on_cpu(false, false, true));
}

#[test]
fn desktop_gpu_owed_decision_matrix() {
    // Not ready or retry backoff still active: never owed in any mode.
    for mode in [CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off] {
        assert!(!mode.gpu_owed(false), "{mode:?} not due");
    }

    // Ready and due: Auto starts background handover, Only suppresses it, Off does not suppress.
    assert!(CpuFirst::Auto.gpu_owed(true));
    assert!(!CpuFirst::Only.gpu_owed(true));
    assert!(CpuFirst::Off.gpu_owed(true));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn linux_transparent_gpu_fallback_decision_matrix() {
    use crate::gpu::fallback::{Failure, Policy};
    use crate::render::cpu_boot::NeedsGpu;

    let pre_swapchain = [Failure::SurfaceCreate, Failure::DeviceCreate, Failure::SurfaceUnsupported];
    let post_or_transient = [
        Failure::SurfaceRefused,
        Failure::ShaderRejected,
        Failure::OutOfMemory,
        Failure::DeviceLost,
        Failure::RepeatedDeviceLost,
    ];

    // Transparent windows never open on CPU before trying the GPU (hardware, software, or none):
    // machines that can create a GPU surface stay 100% untouched for first window, second window,
    // and WindowOptions::overlay alike.
    for mode in [CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off] {
        for live_pool_gpu in [false, true] {
            assert!(!mode.opens_on_cpu(true, false, live_pool_gpu));
            assert!(!mode.opens_on_cpu(true, true, live_pool_gpu));
        }
    }

    // After pre-swapchain GPU startup fails on Linux:
    // Auto and Only with Automatic policy fall back to CpuPainter for visible transparent windows
    // (including a first transparent window, a second transparent window, or WindowOptions::overlay).
    for mode in [CpuFirst::Auto, CpuFirst::Only] {
        for failure in pre_swapchain {
            assert!(mode.transparent_gpu_fallback(true, true, Policy::Automatic, failure), "{mode:?} {failure:?}");
            // Opaque or hidden windows never take this branch.
            assert!(!mode.transparent_gpu_fallback(true, false, Policy::Automatic, failure));
            assert!(!mode.transparent_gpu_fallback(false, true, Policy::Automatic, failure));
            // Pinned adapter (WGPU_ADAPTER_NAME) never silently falls back to CPU.
            assert!(!mode.transparent_gpu_fallback(true, true, Policy::Pinned, failure));
        }
        // Post-swapchain or transient errors never fall back to CPU (protects Wayland explicit sync).
        for failure in post_or_transient {
            assert!(!mode.transparent_gpu_fallback(true, true, Policy::Automatic, failure), "{mode:?} {failure:?}");
        }
    }

    // CpuFirst::Off explicitly requests GPU-only startup: never falls back to CPU.
    for policy in [Policy::Automatic, Policy::Pinned] {
        for failure in pre_swapchain.into_iter().chain(post_or_transient) {
            assert!(!CpuFirst::Off.transparent_gpu_fallback(true, true, policy, failure), "Off {policy:?} {failure:?}");
        }
    }

    // Degraded CPU paint on Linux applies only after the GPU job has permanently failed, and only
    // for degradable NeedsGpu reasons.
    for reason in [
        NeedsGpu::LayerBlur,
        NeedsGpu::TurnedLayer,
        NeedsGpu::BackdropBlur,
        NeedsGpu::Effect,
        NeedsGpu::NativeSurface,
        NeedsGpu::LinearBlend,
    ] {
        assert!(CpuFirst::linux_degraded_when_gpu_failed(true, reason));
        assert!(!CpuFirst::linux_degraded_when_gpu_failed(false, reason));
    }
    for reason in [NeedsGpu::SlugText, NeedsGpu::OutlineGlyph, NeedsGpu::UnknownKind, NeedsGpu::Target] {
        assert!(!CpuFirst::linux_degraded_when_gpu_failed(true, reason));
        assert!(!CpuFirst::linux_degraded_when_gpu_failed(false, reason));
    }
}

#[test]
fn web_start_decision_matrix() {
    // Document and inline embedded pages always start on the GPU regardless of CpuFirst.
    for mode in [CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off] {
        assert_eq!(mode.web_start(true, false), WebStart::GpuFirst, "{mode:?} document");
        assert_eq!(mode.web_start(false, true), WebStart::GpuFirst, "{mode:?} embedded");
        assert_eq!(mode.web_start(true, true), WebStart::GpuFirst, "{mode:?} document+embedded");
    }

    // Full-window pages follow the resolved CpuFirst mode.
    assert_eq!(CpuFirst::Auto.web_start(false, false), WebStart::CpuFirst);
    assert_eq!(CpuFirst::Only.web_start(false, false), WebStart::CpuOnly);
    assert_eq!(CpuFirst::Off.web_start(false, false), WebStart::GpuFirst);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "invoked by effective_reads_process_env_override_without_mutating_parent"]
fn effective_env_child() {
    let expected = match crate::env::var("RUNE_TEST_CPU_FIRST_CASE").as_deref() {
        Some("unset") => (CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off),
        Some("zero") | Some("off") => (CpuFirst::Off, CpuFirst::Off, CpuFirst::Off),
        Some("only") => (CpuFirst::Only, CpuFirst::Only, CpuFirst::Only),
        Some("auto") => (CpuFirst::Auto, CpuFirst::Auto, CpuFirst::Auto),
        Some("bogus") => (CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off),
        other => panic!("unexpected test case: {other:?}"),
    };
    assert_eq!(CpuFirst::Auto.effective(), expected.0);
    assert_eq!(CpuFirst::Only.effective(), expected.1);
    assert_eq!(CpuFirst::Off.effective(), expected.2);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn effective_reads_process_env_override_without_mutating_parent() {
    let exe = std::env::current_exe().expect("test binary path");
    for (case, env_value) in [
        ("unset", None),
        ("zero", Some("0")),
        ("off", Some("off")),
        ("only", Some("only")),
        ("auto", Some("auto")),
        ("bogus", Some("bogus")),
    ] {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--exact")
            .arg("window::cpu_first::tests::effective_env_child")
            .arg("--ignored")
            .arg("--nocapture")
            .env("RUNE_TEST_CPU_FIRST_CASE", case);
        match env_value {
            Some(v) => { cmd.env("RUNE_CPU_FIRST", v); }
            None => { cmd.env_remove("RUNE_CPU_FIRST"); }
        }
        let output = cmd.output().expect("spawn cpu_first env child");
        assert!(
            output.status.success(),
            "case={case} env={env_value:?} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
