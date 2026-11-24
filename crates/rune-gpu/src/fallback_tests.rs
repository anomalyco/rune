use std::time::Duration;

use super::{
    Candidate, Decision, Effect, Event, FORGIVE_LOSS, Failure, GpuMode, LOSS_LIMIT,
    LossEffect, LossEvent, LossState, Policy, State, decide, order_fallbacks, step, step_loss,
};
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
        // Automatic selection: persistent adapter/surface/shader incompatibilities and repeated losses move on.
        (Automatic, DeviceCreate, MoveOn),
        (Automatic, SurfaceUnsupported, MoveOn),
        (Automatic, SurfaceRefused, MoveOn),
        (Automatic, ShaderRejected, MoveOn),
        (Automatic, RepeatedDeviceLost, MoveOn),
        // Automatic selection: single transient failures and native surface creation errors do
        // not move on to another adapter.
        (Automatic, OutOfMemory, Stop),
        (Automatic, DeviceLost, Stop),
        (Automatic, SurfaceCreate, Stop),
        // User- or app-named adapter: never replaced on any failure, repeated loss included.
        (Pinned, DeviceCreate, Stop),
        (Pinned, SurfaceUnsupported, Stop),
        (Pinned, SurfaceRefused, Stop),
        (Pinned, ShaderRejected, Stop),
        (Pinned, OutOfMemory, Stop),
        (Pinned, DeviceLost, Stop),
        (Pinned, RepeatedDeviceLost, Stop),
        (Pinned, SurfaceCreate, Stop),
    ];

    assert_eq!(cases.len(), 16);
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
fn rejected_shader_walks_fallbacks_and_exhaustion_returns_first_error() {
    let mut state = State::first(Policy::Automatic);
    assert_eq!(
        step(&mut state, Event::Failed { kind: Failure::ShaderRejected, error: "classic shader rejected".into() }),
        Effects::one(Effect::EnumerateFallbacks),
    );
    assert_eq!(state.first_error(), Some("classic shader rejected"));
    assert_eq!(
        step(&mut state, Event::Candidates(vec![2, 1])),
        Effects::one(Effect::TryCandidate(2)),
    );
    assert_eq!(
        step(&mut state, Event::Failed { kind: Failure::ShaderRejected, error: "second rejected".into() }),
        Effects::one(Effect::TryCandidate(1)),
    );
    assert_eq!(
        step(&mut state, Event::Succeeded),
        Effects::one(Effect::FinishOk { fell_back: true }),
    );
    assert_eq!(state, State::Done);

    // Exhausting every adapter on ShaderRejected returns the first adapter's error.
    let mut exhausted = State::first(Policy::Automatic);
    step(&mut exhausted, Event::Failed { kind: Failure::ShaderRejected, error: "gpu0 rejected".into() });
    step(&mut exhausted, Event::Candidates(vec![1]));
    assert_eq!(
        step(&mut exhausted, Event::Failed { kind: Failure::ShaderRejected, error: "warp rejected".into() }),
        Effects::one(Effect::FinishErr("gpu0 rejected".into())),
    );
    assert_eq!(exhausted, State::Exhausted { error: "gpu0 rejected".into() });
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
        (Policy::Pinned, Failure::ShaderRejected),
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

#[test]
fn repeated_losses_within_forgive_window_fall_back_to_software_then_give_up() {
    let mut now = rune_core::time::now();
    let mut state = LossState::default();
    assert_eq!(state, LossState::Clean { mode: GpuMode::Hardware });
    assert_eq!(state.effective_losses(now), 0);

    // 3 hardware losses 10 s apart: attempts 1-2 stay on Hardware, attempt 3 switches to Software.
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false }),
    );
    assert_eq!(state.effective_losses(now), 1);

    now += Duration::from_secs(10);
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false }),
    );
    assert_eq!(state.effective_losses(now), 2);

    now += Duration::from_secs(10);
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::Replace { mode: GpuMode::Software, switched_to_software: true }),
    );
    assert_eq!(state, LossState::Clean { mode: GpuMode::Software });
    assert_eq!(state.effective_losses(now), 0);

    // 3 software losses 10 s apart: attempts 1-2 stay on Software, attempt 3 gives up.
    for expected_losses in 1..LOSS_LIMIT {
        now += Duration::from_secs(10);
        assert_eq!(
            step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
            Effects::one(LossEffect::Replace { mode: GpuMode::Software, switched_to_software: false }),
        );
        assert_eq!(state.effective_losses(now), expected_losses);
    }

    now += Duration::from_secs(10);
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::GiveUp),
    );
    assert_eq!(state, LossState::GivenUp);
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::GiveUp),
    );
}

#[test]
fn loss_forgiven_after_five_minutes_stays_on_hardware() {
    let mut now = rune_core::time::now();
    let mut state = LossState::default();

    // Loss 1 at t = 0 -> count = 1.
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false }),
    );
    assert_eq!(state.effective_losses(now), 1);

    // 4m59s later: not yet forgiven -> count = 2.
    now += FORGIVE_LOSS - Duration::from_secs(1);
    assert_eq!(state.effective_losses(now), 1);
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false }),
    );
    assert_eq!(state.effective_losses(now), 2);

    // Exactly 5 minutes later: 1 loss forgiven (2 - 1 = 1), so next loss brings count to 2 (stays Hardware).
    now += FORGIVE_LOSS;
    assert_eq!(state.effective_losses(now), 1);
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false }),
    );
    assert_eq!(state.effective_losses(now), 2);
    assert_eq!(state.mode(), Some(GpuMode::Hardware));

    // 10 minutes later: both losses forgiven (2 - 2 = 0), so next loss brings count back to 1.
    now += FORGIVE_LOSS * 2;
    assert_eq!(state.effective_losses(now), 0);
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false }),
    );
    assert_eq!(state.effective_losses(now), 1);
    assert_eq!(state.mode(), Some(GpuMode::Hardware));
}

#[test]
fn pinned_or_unavailable_software_gives_up_instead_of_switching() {
    for (policy, software_available) in [
        (Policy::Pinned, true),
        (Policy::Pinned, false),
        (Policy::Automatic, false),
    ] {
        let mut now = rune_core::time::now();
        let mut state = LossState::default();
        assert_eq!(
            step_loss(&mut state, LossEvent::Replace { now, policy, software_available }),
            Effects::one(LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false }),
        );
        now += Duration::from_secs(10);
        assert_eq!(
            step_loss(&mut state, LossEvent::Replace { now, policy, software_available }),
            Effects::one(LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false }),
        );
        assert_eq!(state.next_backoff(now, policy, software_available, Duration::from_millis(100)), None);
        now += Duration::from_secs(10);
        assert_eq!(
            step_loss(&mut state, LossEvent::Replace { now, policy, software_available }),
            Effects::one(LossEffect::GiveUp),
            "policy={policy:?}, software_available={software_available}",
        );
        assert_eq!(state, LossState::GivenUp);
    }
}

#[test]
fn on_software_no_software_adapter_and_replace_failed_transitions() {
    let mut now = rune_core::time::now();

    let mut state = LossState::default();
    assert_eq!(step_loss(&mut state, LossEvent::OnSoftware), Effects::none());
    assert_eq!(state, LossState::Clean { mode: GpuMode::Software });
    for _ in 0..2 {
        assert_eq!(
            step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
            Effects::one(LossEffect::Replace { mode: GpuMode::Software, switched_to_software: false }),
        );
        now += Duration::from_secs(5);
    }
    assert_eq!(
        step_loss(&mut state, LossEvent::Replace { now, policy: Policy::Automatic, software_available: true }),
        Effects::one(LossEffect::GiveUp),
    );

    let mut switched = LossState::for_mode(GpuMode::Software);
    assert_eq!(step_loss(&mut switched, LossEvent::ReplaceFailed { now }), Effects::none());
    assert_eq!(switched.effective_losses(now), 1);

    let mut missing = LossState::for_mode(GpuMode::Software);
    assert_eq!(step_loss(&mut missing, LossEvent::NoSoftwareAdapter), Effects::none());
    assert_eq!(missing, LossState::GivenUp);
}
