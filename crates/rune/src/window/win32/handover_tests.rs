use super::warm_frame;
use crate::render::{Gpu, Instance, KIND_RECT, NO_CLIP, Renderer};

fn rect_instance() -> Instance {
    Instance {
        rect: [0.0, 0.0, 16.0, 16.0],
        radii: [0.0; 4],
        fill: [0.5, 0.25, 0.125, 1.0],
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

/// A pre-warmed `Bgra8Unorm` renderer (`render/warm.rs` `FORMAT`) handed to a window whose swap
/// chain target is `Bgra8UnormSrgb` (`srgb_blend == false`) must switch its pipeline format
/// before `warm_frame` draws, or wgpu rejects the render pass with a validation error; likewise
/// for a parked `Bgra8UnormSrgb` renderer handed to a `Bgra8Unorm` (`srgb_blend == true`) window,
/// and `srgb_blend` must match the window's mode even when the format already agrees.
#[test]
fn warm_frame_rebuilds_renderer_for_target_format() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let instances = [rect_instance()];
    for (initial, target, srgb) in [
        (wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Bgra8UnormSrgb, false),
        (wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Bgra8Unorm, true),
        (wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Bgra8Unorm, true),
        (wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Bgra8UnormSrgb, false),
    ] {
        let mut renderer = Renderer::new(&gpu.device, initial);
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        warm_frame(&gpu, &mut renderer, target, srgb, (32, 32), &instances);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "warm_frame({initial:?} -> {target:?}, srgb={srgb}) hit validation error: {error:?}");
        assert_eq!(renderer.format(), target);
        assert_eq!(renderer.srgb_blend(), srgb);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdapterEnv {
    Hardware(wgpu::DeviceType),
    SoftwareOnly,
    HeadlessWarp0x1414,
    NonD3d12Fallback,
}

#[derive(Default, Debug, PartialEq, Eq)]
struct FakeCounters {
    d3d12_devices: usize,
    swapchains: usize,
    shader_compiles: usize,
    committed: bool,
}

/// Off-screen simulation of the Windows idle -> prepare -> NeedsGpu handover state machine,
/// driving the exact production policy helpers in `win32::handover` and `rune_gpu::pick`.
fn simulate_handover(
    env: AdapterEnv,
    mode: crate::window::CpuFirst,
    policy: crate::gpu::fallback::Policy,
    needs_gpu_event: Option<(bool /* first_frame_ready */, crate::render::cpu_boot::NeedsGpu)>,
) -> (FakeCounters, &'static str, Option<super::NeedsGpuFrameAction>) {
    use crate::gpu::pick::{WARP_DEVICE_ID, WARP_VENDOR_ID, has_hardware_dxgi_in};

    let dxgi_list: Vec<(u32, u32, bool)> = match env {
        AdapterEnv::Hardware(_) => vec![(0x10DE, 0x2C05, false), (WARP_VENDOR_ID, WARP_DEVICE_ID, true)],
        AdapterEnv::SoftwareOnly => vec![(WARP_VENDOR_ID, WARP_DEVICE_ID, true)],
        AdapterEnv::HeadlessWarp0x1414 => vec![
            (WARP_VENDOR_ID, WARP_DEVICE_ID, false),
            (WARP_VENDOR_ID, WARP_DEVICE_ID, true),
        ],
        AdapterEnv::NonD3d12Fallback => vec![(0x1AB8, 0x4005, false), (WARP_VENDOR_ID, WARP_DEVICE_ID, true)],
    };
    let picked_device_type = match env {
        AdapterEnv::Hardware(dt) => dt,
        AdapterEnv::SoftwareOnly | AdapterEnv::HeadlessWarp0x1414 | AdapterEnv::NonD3d12Fallback => {
            wgpu::DeviceType::Cpu
        }
    };

    let mut counters = FakeCounters::default();
    let mut job_state = "waiting";

    // Idle turn after frame 1 (plain frame).
    if needs_gpu_event.is_none() || needs_gpu_event.is_some_and(|(ready, _)| ready) {
        match super::idle_handover_action(mode, policy, true, || Some(has_hardware_dxgi_in(&dxgi_list))) {
            super::IdleHandover::Skip => {}
            super::IdleHandover::DeferSoftware => {
                job_state = "software_deferred";
            }
            super::IdleHandover::Start { defer_software } => {
                counters.d3d12_devices += 1;
                if super::should_defer_prepared_adapter(defer_software, picked_device_type) {
                    job_state = "software_deferred";
                } else {
                    counters.swapchains += 1;
                    counters.shader_compiles += 1;
                    counters.committed = true;
                    job_state = "handed";
                }
            }
        }
    }

    let mut frame_action = None;
    if let Some((first_frame_ready, reason)) = needs_gpu_event {
        frame_action = Some(super::needs_gpu_frame_action(first_frame_ready, reason));
        if job_state != "handed" {
            // Urgent NeedsGpu handover always allows software adapters (defer_software = false).
            counters.d3d12_devices += 1;
            assert!(!super::should_defer_prepared_adapter(false, picked_device_type));
            counters.swapchains += 1;
            counters.shader_compiles += 1;
            counters.committed = true;
            job_state = "handed";
        }
    }

    (counters, job_state, frame_action)
}

#[test]
fn windows_handover_state_machine_matrix_software_deferred_and_hardware_bit_identical() {
    use crate::gpu::fallback::Policy;
    use crate::render::cpu_boot::NeedsGpu;
    use crate::window::CpuFirst;

    // 1. Software-only & headless 0x1414:0x008C under Auto + Automatic:
    // plain frames create 0 D3D12 devices, 0 swapchains, and 0 shader compiles, parking in
    // SoftwareDeferred with no retry timer until NeedsGpu arrives.
    for env in [AdapterEnv::SoftwareOnly, AdapterEnv::HeadlessWarp0x1414] {
        let (c, state, action) = simulate_handover(env, CpuFirst::Auto, Policy::Automatic, None);
        assert_eq!(state, "software_deferred", "{env:?}");
        assert_eq!(c, FakeCounters::default(), "{env:?} must do zero GPU work on plain frames");
        assert_eq!(action, None);

        // Mid-session degradable NeedsGpu (LayerBlur, TurnedLayer, BackdropBlur, Effect,
        // NativeSurface, LinearBlend): paints degraded live and hands over to WARP.
        for reason in [
            NeedsGpu::LayerBlur,
            NeedsGpu::TurnedLayer,
            NeedsGpu::BackdropBlur,
            NeedsGpu::Effect,
            NeedsGpu::NativeSurface,
            NeedsGpu::LinearBlend,
        ] {
            let (c, state, action) = simulate_handover(env, CpuFirst::Auto, Policy::Automatic, Some((true, reason)));
            assert_eq!(state, "handed", "{env:?} {reason:?}");
            assert_eq!(action, Some(super::NeedsGpuFrameAction::PaintDegraded), "{env:?} {reason:?}");
            assert_eq!(c.d3d12_devices, 1);
            assert_eq!(c.swapchains, 1);
            assert_eq!(c.shader_compiles, 1);
            assert!(c.committed);
        }

        // Mid-session non-degradable NeedsGpu (SlugText, OutlineGlyph, UnknownKind, Target):
        // holds previous frame and hands over to WARP.
        for reason in [NeedsGpu::SlugText, NeedsGpu::OutlineGlyph, NeedsGpu::UnknownKind, NeedsGpu::Target] {
            let (c, state, action) = simulate_handover(env, CpuFirst::Auto, Policy::Automatic, Some((true, reason)));
            assert_eq!(state, "handed", "{env:?} {reason:?}");
            assert_eq!(action, Some(super::NeedsGpuFrameAction::KeepPreviousFrame), "{env:?} {reason:?}");
            assert!(c.committed);
        }

        // First-frame NeedsGpu: never paints degraded (stays cloaked until GPU draws frame 1).
        let (c, state, action) = simulate_handover(env, CpuFirst::Auto, Policy::Automatic, Some((false, NeedsGpu::LayerBlur)));
        assert_eq!(state, "handed");
        assert_eq!(action, Some(super::NeedsGpuFrameAction::WaitForGpu));
        assert!(c.committed);
    }

    // 2. Non-D3D12 virtual adapter falling through to WARP during idle: belt-and-suspenders
    // aborts before configuring a swapchain or compiling shaders, and parks in SoftwareDeferred.
    let (c, state, _) = simulate_handover(AdapterEnv::NonD3d12Fallback, CpuFirst::Auto, Policy::Automatic, None);
    assert_eq!(state, "software_deferred");
    assert_eq!(c.swapchains, 0);
    assert_eq!(c.shader_compiles, 0);
    assert!(!c.committed);

    // 3. Hardware GPUs (IntegratedGpu, DiscreteGpu, VirtualGpu): bit-identical immediate idle handover.
    for dt in [wgpu::DeviceType::IntegratedGpu, wgpu::DeviceType::DiscreteGpu, wgpu::DeviceType::VirtualGpu] {
        for policy in [Policy::Automatic, Policy::Pinned] {
            let (c, state, _) = simulate_handover(AdapterEnv::Hardware(dt), CpuFirst::Auto, policy, None);
            assert_eq!(state, "handed", "{dt:?} {policy:?}");
            assert_eq!(c.d3d12_devices, 1);
            assert_eq!(c.swapchains, 1);
            assert_eq!(c.shader_compiles, 1);
            assert!(c.committed);
        }
    }

    // 4. Pinned adapter (WGPU_ADAPTER_NAME) and CpuFirst::Off: bit-identical immediate handover
    // even on software-only machines.
    for env in [AdapterEnv::SoftwareOnly, AdapterEnv::HeadlessWarp0x1414, AdapterEnv::NonD3d12Fallback] {
        let (c_pin, s_pin, _) = simulate_handover(env, CpuFirst::Auto, Policy::Pinned, None);
        assert_eq!(s_pin, "handed", "pinned {env:?}");
        assert!(c_pin.committed);

        let (c_off, s_off, _) = simulate_handover(env, CpuFirst::Off, Policy::Automatic, None);
        assert_eq!(s_off, "handed", "off {env:?}");
        assert!(c_off.committed);
    }

    // 5. CpuFirst::Only: stays on CPU at idle with zero GPU work, hands over on NeedsGpu with
    // non-freezing mid-session degraded paint.
    let (c_only, s_only, _) = simulate_handover(AdapterEnv::Hardware(wgpu::DeviceType::DiscreteGpu), CpuFirst::Only, Policy::Automatic, None);
    assert_eq!(s_only, "waiting");
    assert_eq!(c_only, FakeCounters::default());
    let (c_only_ng, s_only_ng, a_only_ng) = simulate_handover(
        AdapterEnv::SoftwareOnly,
        CpuFirst::Only,
        Policy::Automatic,
        Some((true, NeedsGpu::TurnedLayer)),
    );
    assert_eq!(s_only_ng, "handed");
    assert_eq!(a_only_ng, Some(super::NeedsGpuFrameAction::PaintDegraded));
    assert!(c_only_ng.committed);
}

#[test]
fn urgent_needs_gpu_handover_bypasses_quiet_wait_and_skips_degraded_drift() {
    let now = crate::time::now();
    // Normal idle handover while animating defers during QUIET_WAIT (1 s).
    assert!(super::should_defer_quiet_wait(true, true, false, now, now));
    assert!(super::should_defer_quiet_wait(true, true, false, now + std::time::Duration::from_millis(500), now));
    assert!(!super::should_defer_quiet_wait(true, true, false, now + super::QUIET_WAIT, now));
    // At rest: no deferral.
    assert!(!super::should_defer_quiet_wait(true, false, false, now, now));
    // Urgent NeedsGpu handover while animating: 0 ms QUIET_WAIT deferral.
    assert!(!super::should_defer_quiet_wait(true, true, true, now, now));

    // Drift counting runs only when a non-degraded CPU frame was copied to the window.
    assert!(super::should_count_drift(true, false));
    assert!(!super::should_count_drift(true, true));
    assert!(!super::should_count_drift(false, false));
    assert!(!super::should_count_drift(false, true));
}

#[test]
fn opaque_second_window_ignores_software_gpu_in_pool_under_auto_automatic() {
    use super::super::cpu_frame::win32_opens_on_cpu;
    use crate::gpu::fallback::Policy;
    use crate::window::CpuFirst;
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, VirtualGpu};

    // Opaque second window on software-only machine whose transparent window/overlay populated
    // gpu_pool with WARP (DeviceType::Cpu): stays on CpuPainter under Auto + Automatic.
    assert!(win32_opens_on_cpu(CpuFirst::Auto, Policy::Automatic, false, false, Some(Cpu)));

    // Pinned WARP (WGPU_ADAPTER_NAME): bit-identical reuse of pooled GPU.
    assert!(!win32_opens_on_cpu(CpuFirst::Auto, Policy::Pinned, false, false, Some(Cpu)));

    // Hardware GPUs in gpu_pool: bit-identical reuse of pooled hardware GPU.
    for hw in [IntegratedGpu, DiscreteGpu, VirtualGpu] {
        assert!(!win32_opens_on_cpu(CpuFirst::Auto, Policy::Automatic, false, false, Some(hw)), "{hw:?}");
        assert!(!win32_opens_on_cpu(CpuFirst::Auto, Policy::Pinned, false, false, Some(hw)), "{hw:?} pinned");
    }

    // Transparent windows and overlays always open on GPU regardless of mode/policy/pool.
    for mode in [CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off] {
        for policy in [Policy::Automatic, Policy::Pinned] {
            for pool in [None, Some(Cpu), Some(DiscreteGpu)] {
                assert!(!win32_opens_on_cpu(mode, policy, true, false, pool), "transparent {mode:?} {policy:?} {pool:?}");
            }
        }
    }

    // CpuFirst::Off never opens on CPU; CpuFirst::Only always opens opaque windows on CPU.
    assert!(!win32_opens_on_cpu(CpuFirst::Off, Policy::Automatic, false, false, None));
    assert!(!win32_opens_on_cpu(CpuFirst::Off, Policy::Automatic, false, false, Some(Cpu)));
    assert!(win32_opens_on_cpu(CpuFirst::Only, Policy::Automatic, false, false, Some(DiscreteGpu)));
    assert!(win32_opens_on_cpu(CpuFirst::Only, Policy::Automatic, false, false, Some(Cpu)));
}
