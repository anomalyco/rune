use super::{Candidate, Decision, Effect, Event, Failure, Policy, State, decide, order_fallbacks, step};
use rune_core::machine::Effects;

fn candidate(
    index: usize,
    name: &str,
    backend: wgpu::Backend,
    device_type: wgpu::DeviceType,
    vendor: u32,
    device: u32,
    pci: &str,
    surface_supported: bool,
    translucent: bool,
) -> Candidate {
    Candidate {
        index,
        name: name.into(),
        backend,
        device_type,
        vendor,
        device,
        pci: pci.into(),
        surface_supported,
        translucent,
    }
}

/// Complete decision table: which failures move on to the next adapter, which stop, and that a
/// named (`Pinned`) adapter is never replaced.
#[test]
fn decide_table_covers_every_policy_and_failure() {
    use Decision::*;
    use Failure::*;
    use Policy::*;

    let cases = [
        // Automatic selection: persistent adapter/surface incompatibilities move on.
        (Automatic, DeviceCreate, MoveOn),
        (Automatic, SurfaceUnsupported, MoveOn),
        (Automatic, SurfaceRefused, MoveOn),
        // Automatic selection: transient failures and native surface creation errors stop.
        (Automatic, OutOfMemory, Stop),
        (Automatic, DeviceLost, Stop),
        (Automatic, SurfaceCreate, Stop),
        // User- or app-named adapter: never replaced on any failure.
        (Pinned, DeviceCreate, Stop),
        (Pinned, SurfaceUnsupported, Stop),
        (Pinned, SurfaceRefused, Stop),
        (Pinned, OutOfMemory, Stop),
        (Pinned, DeviceLost, Stop),
        (Pinned, SurfaceCreate, Stop),
    ];

    for (policy, failure, expected) in cases {
        assert_eq!(
            decide(policy, failure),
            expected,
            "policy={policy:?}, failure={failure:?}"
        );
    }
}

#[test]
fn happy_path_finishes_without_enumerating_fallbacks() {
    let mut state = State::first(Policy::Automatic);
    assert_eq!(step(&mut state, Event::Succeeded), Effects::one(Effect::FinishOk { fell_back: false }));
    assert_eq!(state, State::Done);
    // Stale events in terminal state do nothing.
    assert_eq!(step(&mut state, Event::Failed { kind: Failure::SurfaceRefused, error: "late".into() }), Effects::none());
}

#[test]
fn refused_surface_walks_fallbacks_until_one_succeeds() {
    let mut state = State::first(Policy::Automatic);
    assert_eq!(
        step(&mut state, Event::Failed { kind: Failure::SurfaceRefused, error: "E_NOTIMPL".into() }),
        Effects::one(Effect::EnumerateFallbacks),
    );
    assert_eq!(state.first_error(), Some("E_NOTIMPL"));

    assert_eq!(
        step(&mut state, Event::Candidates(vec![2, 1])),
        Effects::one(Effect::TryCandidate(2)),
    );
    assert_eq!(
        step(&mut state, Event::Failed { kind: Failure::SurfaceRefused, error: "also refused".into() }),
        Effects::one(Effect::TryCandidate(1)),
    );
    assert_eq!(
        step(&mut state, Event::Succeeded),
        Effects::one(Effect::FinishOk { fell_back: true }),
    );
    assert_eq!(state, State::Done);
}

#[test]
fn exhausting_all_fallbacks_returns_the_first_error() {
    let mut state = State::first(Policy::Automatic);
    assert_eq!(
        step(&mut state, Event::Failed { kind: Failure::SurfaceRefused, error: "primary refused".into() }),
        Effects::one(Effect::EnumerateFallbacks),
    );
    assert_eq!(
        step(&mut state, Event::Candidates(vec![1])),
        Effects::one(Effect::TryCandidate(1)),
    );
    assert_eq!(
        step(&mut state, Event::Failed { kind: Failure::DeviceCreate, error: "warp failed".into() }),
        Effects::one(Effect::FinishErr("primary refused".into())),
    );
    assert_eq!(state, State::Exhausted { error: "primary refused".into() });
}

#[test]
fn empty_fallback_list_returns_the_first_error() {
    let mut state = State::first(Policy::Automatic);
    step(&mut state, Event::Failed { kind: Failure::SurfaceRefused, error: "primary refused".into() });
    assert_eq!(
        step(&mut state, Event::Candidates(vec![])),
        Effects::one(Effect::FinishErr("primary refused".into())),
    );
}

#[test]
fn transient_and_pinned_failures_never_enumerate_fallbacks() {
    for (policy, kind) in [
        (Policy::Automatic, Failure::OutOfMemory),
        (Policy::Automatic, Failure::DeviceLost),
        (Policy::Automatic, Failure::SurfaceCreate),
        (Policy::Pinned, Failure::DeviceCreate),
        (Policy::Pinned, Failure::SurfaceUnsupported),
        (Policy::Pinned, Failure::SurfaceRefused),
    ] {
        let mut state = State::first(policy);
        assert_eq!(
            step(&mut state, Event::Failed { kind, error: "stop here".into() }),
            Effects::one(Effect::FinishErr("stop here".into())),
            "policy={policy:?}, kind={kind:?}"
        );
        assert_eq!(state, State::Exhausted { error: "stop here".into() });
    }
}

#[test]
fn transient_failure_mid_fallback_stops_immediately() {
    let mut state = State::first(Policy::Automatic);
    step(&mut state, Event::Failed { kind: Failure::SurfaceRefused, error: "first".into() });
    step(&mut state, Event::Candidates(vec![1, 2]));
    assert_eq!(
        step(&mut state, Event::Failed { kind: Failure::OutOfMemory, error: "oom".into() }),
        Effects::one(Effect::FinishErr("oom".into())),
    );
}

#[test]
fn prewarmed_or_pooled_refusal_starts_at_need_fallbacks() {
    let mut state = State::need_fallbacks("warm refused".into());
    assert_eq!(
        step(&mut state, Event::Candidates(vec![3])),
        Effects::one(Effect::TryCandidate(3)),
    );
    assert_eq!(
        step(&mut state, Event::Succeeded),
        Effects::one(Effect::FinishOk { fell_back: true }),
    );
}

#[test]
fn order_fallbacks_tries_other_hardware_before_warp_and_skips_tried() {
    use wgpu::Backend::Dx12;
    use wgpu::DeviceType::*;

    let nvidia = candidate(0, "NVIDIA GeForce RTX 5070 Ti", Dx12, DiscreteGpu, 0x10de, 0x2c05, "", true, true);
    let warp = candidate(1, "Microsoft Basic Render Driver", Dx12, Cpu, 0x1414, 0x008c, "", true, true);
    let amd = candidate(2, "AMD Radeon(TM) Graphics", Dx12, IntegratedGpu, 0x1002, 0x13c0, "", true, true);
    let all = [nvidia.clone(), warp.clone(), amd.clone()];

    // Dual-GPU desktop: NVIDIA refused -> AMD iGPU first, then WARP.
    assert_eq!(
        order_fallbacks(&[nvidia.clone()], &all, &[(0x10de, 0x2c05)], None, false),
        vec![2, 1],
    );

    // Single-GPU VM (DXVK + WARP): DXVK refused -> WARP is the only candidate.
    let dxvk = candidate(0, "DXVK D3D12", Dx12, VirtualGpu, 0x1234, 0x5678, "", true, true);
    assert_eq!(
        order_fallbacks(&[dxvk.clone()], &[dxvk, warp], &[], None, false),
        vec![1],
    );
}

#[test]
fn order_fallbacks_prefers_display_compositor_and_translucency_while_keeping_software_last() {
    use wgpu::Backend::Vulkan;
    use wgpu::DeviceType::*;

    let tried = candidate(0, "NVIDIA RTX", Vulkan, DiscreteGpu, 0x10de, 0x2c05, "0000:01:00.0", true, true);
    let cpu_translucent = candidate(1, "llvmpipe", Vulkan, Cpu, 0x10005, 0, "", true, true);
    let hw_opaque = candidate(2, "Intel Arc", Vulkan, DiscreteGpu, 0x8086, 0x56a0, "0000:02:00.0", true, false);
    let hw_translucent = candidate(3, "AMD iGPU", Vulkan, IntegratedGpu, 0x1002, 0x13c0, "0000:0f:00.0", true, true);
    let unsupported = candidate(4, "Broken", Vulkan, IntegratedGpu, 0x9999, 0x1, "0000:03:00.0", false, true);
    let gl_on_compositor = candidate(5, "NVIDIA RTX", wgpu::Backend::Gl, DiscreteGpu, 0x10de, 0x2c05, "0000:01:00.0", true, false);
    let all = [tried.clone(), cpu_translucent, hw_opaque, hw_translucent, unsupported, gl_on_compositor];

    // Translucent request without compositor PCI: hardware translucent (3), then hardware opaque (2, 5), then CPU (1); unsupported (4) skipped.
    assert_eq!(
        order_fallbacks(&[tried.clone()], &all, &[], None, true),
        vec![3, 2, 5, 1],
    );

    // With compositor PCI matching 0000:01:00.0: other backend on compositor's device (5) comes first.
    assert_eq!(
        order_fallbacks(&[tried.clone()], &all, &[], Some("0000:01:00.0"), false),
        vec![5, 3, 2, 1],
    );

    // Windows display priority: display GPU (Intel Arc, 2) comes before non-display AMD iGPU (3), WARP/CPU (1) last.
    assert_eq!(
        order_fallbacks(&[tried], &all, &[(0x8086, 0x56a0)], None, false),
        vec![2, 3, 5, 1],
    );
}
