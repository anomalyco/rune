use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::*;

#[test]
fn latch_state_first_reason_wins_and_emits_wake_once() {
    let mut state = LatchState::Armed;
    let first = step(
        &mut state,
        LatchEvent::DeviceLost {
            destroyed: false,
            message: "VK_ERROR_DEVICE_LOST".into(),
        },
    );
    assert_eq!(first.into_iter().collect::<Vec<_>>(), vec![LatchEffect::Wake]);
    assert_eq!(
        state,
        LatchState::Failed {
            kind: GpuFailureKind::DeviceLost,
            message: "VK_ERROR_DEVICE_LOST".into(),
        }
    );

    // Subsequent errors, detach, or rearm never overwrite the first reason or fire Wake again.
    for ev in [
        LatchEvent::Failure {
            kind: GpuFailureKind::UncapturedError,
            message: "cascading validation error".into(),
        },
        LatchEvent::Failure {
            kind: GpuFailureKind::SurfaceFailed,
            message: "surface unconfigured".into(),
        },
        LatchEvent::DeviceLost {
            destroyed: true,
            message: "drop destroyed".into(),
        },
        LatchEvent::Detach,
        LatchEvent::Rearm,
    ] {
        let eff = step(&mut state, ev);
        assert!(eff.is_empty(), "terminal state must emit zero further effects");
        assert_eq!(
            state,
            LatchState::Failed {
                kind: GpuFailureKind::DeviceLost,
                message: "VK_ERROR_DEVICE_LOST".into(),
            }
        );
    }
}

#[test]
fn detached_ignores_drop_destroyed_and_rearms_cleanly_while_still_latching_real_failures() {
    let mut state = LatchState::Armed;
    assert!(step(&mut state, LatchEvent::Detach).is_empty());
    assert_eq!(state, LatchState::Detached);

    // Intentional clean GPU drop fires Destroyed: ignored while Detached.
    assert!(
        step(
            &mut state,
            LatchEvent::DeviceLost {
                destroyed: true,
                message: "device dropped".into(),
            },
        )
        .is_empty()
    );
    assert_eq!(state, LatchState::Detached);

    // Fresh device attaches: re-armed.
    assert!(step(&mut state, LatchEvent::Rearm).is_empty());
    assert_eq!(state, LatchState::Armed);

    // While Armed, Destroyed DOES latch (explicit external device.destroy()).
    let eff = step(
        &mut state,
        LatchEvent::DeviceLost {
            destroyed: true,
            message: "explicit destroy".into(),
        },
    );
    assert_eq!(eff.into_iter().collect::<Vec<_>>(), vec![LatchEffect::Wake]);
    assert!(matches!(state, LatchState::Failed { kind: GpuFailureKind::DeviceLost, .. }));

    // While Detached, a real non-Destroyed loss or failure still latches.
    let mut detached = LatchState::Detached;
    let eff = step(
        &mut detached,
        LatchEvent::DeviceLost {
            destroyed: false,
            message: "driver reset".into(),
        },
    );
    assert_eq!(eff.into_iter().collect::<Vec<_>>(), vec![LatchEffect::Wake]);
    assert!(matches!(detached, LatchState::Failed { kind: GpuFailureKind::DeviceLost, .. }));
}

#[test]
fn all_failure_kinds_latch_and_bound_utf8_reason_length() {
    for kind in [
        GpuFailureKind::DeviceLost,
        GpuFailureKind::DeviceRemoved,
        GpuFailureKind::UncapturedError,
        GpuFailureKind::InitFailed,
        GpuFailureKind::SurfaceFailed,
    ] {
        let mut state = LatchState::Armed;
        let eff = step(
            &mut state,
            LatchEvent::Failure {
                kind,
                message: String::new(),
            },
        );
        assert_eq!(eff.into_iter().collect::<Vec<_>>(), vec![LatchEffect::Wake]);
        match state {
            LatchState::Failed { kind: got_kind, message } => {
                assert_eq!(got_kind, kind);
                assert!(!message.is_empty(), "empty message gets a default fallback for {kind:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // Multibyte UTF-8 truncation respects char boundaries and never exceeds MAX_REASON_BYTES.
    let huge = "é".repeat(MAX_REASON_BYTES);
    let truncated = truncate_reason(&huge, GpuFailureKind::UncapturedError);
    assert!(truncated.len() <= MAX_REASON_BYTES, "len={}", truncated.len());
    assert!(truncated.ends_with("..."));
    assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
}

#[test]
fn gpu_loss_latch_fast_path_and_one_shot_wake() {
    let latch = GpuLossLatch::new();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    let latch_in_cb = Arc::clone(&latch);
    latch.add_wake(Arc::new(move |reason| {
        // Fast-path flag must already be visible when Wake fires, with the first reason.
        assert!(latch_in_cb.is_failed());
        assert_eq!(reason, "DXGI_ERROR_DEVICE_HUNG");
        wakes_cb.fetch_add(1, Ordering::SeqCst);
    }));

    assert!(!latch.is_failed());
    assert_eq!(latch.reason(), None);
    assert_eq!(latch.kind(), None);

    // Clean detach + drop-triggered Destroyed does not latch or wake.
    latch.detach();
    assert!(!latch.record_device_lost(true, "clean drop"));
    assert!(!latch.is_failed());
    assert_eq!(wakes.load(Ordering::SeqCst), 0);

    latch.rearm();
    assert!(latch.record_failure(GpuFailureKind::DeviceRemoved, "DXGI_ERROR_DEVICE_HUNG"));
    assert!(latch.is_failed());
    assert_eq!(latch.kind(), Some(GpuFailureKind::DeviceRemoved));
    assert_eq!(latch.reason().as_deref(), Some("DXGI_ERROR_DEVICE_HUNG"));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);

    // Second failure is ignored; reason stays first and wake stays 1.
    assert!(latch.record_failure(GpuFailureKind::UncapturedError, "second error"));
    assert_eq!(latch.kind(), Some(GpuFailureKind::DeviceRemoved));
    assert_eq!(latch.reason().as_deref(), Some("DXGI_ERROR_DEVICE_HUNG"));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
}

#[test]
fn web_terminal_policy_keeps_page_alive_without_cpu_fallback_or_webgl_restore() {
    use crate::frame::host_step::Gate;
    use crate::platform::web_math::{
        LossAction, WebDrawKind, loss_action_for_policy, web_frame_gate, webgl_prevent_default,
    };

    for is_webgl in [false, true] {
        for gpu_first in [false, true] {
            for embedded in [false, true] {
                for document in [false, true] {
                    assert_eq!(
                        loss_action_for_policy(GpuLossPolicy::Terminal, is_webgl, gpu_first, embedded, document),
                        LossAction::TerminalLatch,
                        "Terminal must latch without CPU fallback, restore pause, or Fatal end",
                    );
                }
            }
        }
    }
    assert!(!webgl_prevent_default(GpuLossPolicy::Terminal), "Terminal must omit event.preventDefault() on webglcontextlost");
    assert!(webgl_prevent_default(GpuLossPolicy::Recover), "Recover keeps event.preventDefault() on webglcontextlost");
    assert!(matches!(web_frame_gate(false, WebDrawKind::TerminalFailed, false), Gate::Skip));
    assert!(matches!(web_frame_gate(false, WebDrawKind::TerminalFailed, true), Gate::Skip));
}
