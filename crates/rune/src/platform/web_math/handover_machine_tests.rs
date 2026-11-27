use std::time::Duration;

use super::{
    FINISH_LIMIT, GlSyncPoll, GlSyncPollEffect, GlSyncPollEvent, HandoverEffect, HandoverEvent,
    HandoverState, TIMEOUT_PAD, should_redraw_after_gpu_check, web_wants_frame,
};
use crate::machine::Seq;

fn effects(e: crate::machine::Effects<HandoverEffect>) -> Vec<HandoverEffect> {
    e.into_iter().collect()
}

#[test]
fn webgpu_handover_submits_zero_gpu_frames_while_waiting_for_uber_and_breaker_checks() {
    let mut seq = Seq::default();
    let mut state = HandoverState::default();
    let t0 = crate::time::now();
    let mut gpu_submits_during_checks = 0usize;

    // 1. Before Built arrives, CPU frames animate/respond without submitting any GPU frame.
    for _ in 0..5 {
        assert!(!state.waiting_for_gpu());
        assert_eq!(effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: true })), vec![]);
    }

    // 2. Empty-queue Built barrier resolves while uber_check is still pending: enters WaitingChecks
    //    without requesting a redundant redraw.
    assert_eq!(effects(state.step(&mut seq, HandoverEvent::Built { checking_gpu: true })), vec![]);
    assert_eq!(state, HandoverState::WaitingChecks);

    // 3. 60 frames of CPU animation/input while waiting for uber_check -> 0 GPU submits.
    for _ in 0..60 {
        let eff = effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: true }));
        assert!(eff.is_empty());
        if eff.contains(&HandoverEffect::DrawGpuFrame) { gpu_submits_during_checks += 1; }
    }

    // 4. uber_check settles -> requests one redraw.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::ChecksSettled { still_checking: false })),
        vec![HandoverEffect::RequestRedraw],
    );
    assert_eq!(state, HandoverState::Building { built: true });

    // 5. Next frame prepares -> prechecks layer features -> Feature::Isolation starts checking.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: false })),
        vec![HandoverEffect::PrecheckFeatures],
    );
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::Prechecked { checking_gpu: true })),
        vec![],
    );
    assert_eq!(state, HandoverState::WaitingChecks);

    // 6. Another 60 frames of CPU animation/input while waiting for Feature::Isolation -> 0 GPU submits.
    for _ in 0..60 {
        let eff = effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: true }));
        assert!(eff.is_empty());
        if eff.contains(&HandoverEffect::DrawGpuFrame) { gpu_submits_during_checks += 1; }
    }
    assert_eq!(gpu_submits_during_checks, 0, "must submit 0 GPU frames while checks are pending");

    // 7. Feature::Isolation settles -> requests one redraw -> precheck passes -> draws ONE handover frame.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::ChecksSettled { still_checking: false })),
        vec![HandoverEffect::RequestRedraw],
    );
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: false })),
        vec![HandoverEffect::PrecheckFeatures],
    );
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::Prechecked { checking_gpu: false })),
        vec![HandoverEffect::DrawGpuFrame],
    );

    // 8. Handover frame submitted -> enters WaitingDone and arms finish barrier + timeout.
    let submitted = effects(state.step(&mut seq, HandoverEvent::GpuSubmitted { checking_gpu: false, now: t0 }));
    let attempt = match submitted.as_slice() {
        [HandoverEffect::ArmFinishWait { attempt, delay }] => {
            assert_eq!(*delay, FINISH_LIMIT + TIMEOUT_PAD);
            *attempt
        }
        other => panic!("expected ArmFinishWait, got {other:?}"),
    };
    assert!(state.waiting_for_gpu());

    // 9. Active CPU frames during WaitingDone submit zero extra GPU frames (no freeze).
    for _ in 0..10 {
        assert_eq!(
            effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: false })),
            vec![],
        );
    }
    assert!(state.waiting_for_gpu());

    // 10. onSubmittedWorkDone fires -> ReadyToFlip + RequestRedraw (atomic flip on next rAF).
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::GpuDone { attempt })),
        vec![HandoverEffect::RequestRedraw],
    );
    assert_eq!(state, HandoverState::ReadyToFlip);
    assert!(!state.waiting_for_gpu());

    // 11. Next rAF draws the up-to-date GPU flip frame and completes atomically on submit.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: false })),
        vec![HandoverEffect::PrecheckFeatures],
    );
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::Prechecked { checking_gpu: false })),
        vec![HandoverEffect::DrawFlipFrame],
    );
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::GpuSubmitted { checking_gpu: false, now: t0 + Duration::from_millis(32) })),
        vec![HandoverEffect::Complete],
    );
    assert_eq!(state, HandoverState::Done);
}

#[test]
fn checks_settling_before_built_waits_for_built_then_draws_once() {
    let mut seq = Seq::default();
    let mut state = HandoverState::default();

    // uber_check resolves before empty-queue Built finishes: stay in Building { built: false }.
    assert_eq!(effects(state.step(&mut seq, HandoverEvent::ChecksSettled { still_checking: false })), vec![]);
    assert_eq!(state, HandoverState::Building { built: false });

    // Built resolves with !checking_gpu -> Building { built: true } + RequestRedraw.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::Built { checking_gpu: false })),
        vec![HandoverEffect::RequestRedraw],
    );
    assert_eq!(state, HandoverState::Building { built: true });
}

#[test]
fn unexpected_check_during_gpu_encode_submits_at_most_one_warmup_frame_and_waits() {
    let mut seq = Seq::default();
    let mut state = HandoverState::Building { built: true };
    let t0 = crate::time::now();

    // Custom compositor triggers an async check during the first GPU encode.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::GpuSubmitted { checking_gpu: true, now: t0 })),
        vec![],
    );
    assert_eq!(state, HandoverState::WaitingChecks);
    assert!(!state.waiting_for_gpu());

    // Subsequent CPU frames while that check runs do NOT submit more GPU frames.
    for _ in 0..30 {
        assert_eq!(effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: true })), vec![]);
    }

    // Once that check answers, one redraw advances cleanly to the final handover frame.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::ChecksSettled { still_checking: false })),
        vec![HandoverEffect::RequestRedraw],
    );
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: false })),
        vec![HandoverEffect::PrecheckFeatures],
    );
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::Prechecked { checking_gpu: false })),
        vec![HandoverEffect::DrawGpuFrame],
    );
}

#[test]
fn waiting_done_ignores_stale_attempts_and_times_out_at_finish_limit() {
    let mut seq = Seq::default();
    let stale = seq.next();
    let mut state = HandoverState::Building { built: true };
    let t0 = crate::time::now();

    let eff = effects(state.step(&mut seq, HandoverEvent::GpuSubmitted { checking_gpu: false, now: t0 }));
    let active = match eff.as_slice() {
        [HandoverEffect::ArmFinishWait { attempt, .. }] => *attempt,
        other => panic!("unexpected: {other:?}"),
    };
    assert_ne!(stale, active);

    // Stale completion and early ticks do nothing.
    assert_eq!(effects(state.step(&mut seq, HandoverEvent::GpuDone { attempt: stale })), vec![]);
    assert_eq!(effects(state.step(&mut seq, HandoverEvent::Tick { now: t0 + Duration::from_secs(4) })), vec![]);
    assert!(state.waiting_for_gpu());

    // Reaching FINISH_LIMIT transitions to Failed once; subsequent events are ignored.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::Tick { now: t0 + FINISH_LIMIT })),
        vec![HandoverEffect::FailTimeout],
    );
    assert_eq!(state, HandoverState::Failed);
    assert_eq!(effects(state.step(&mut seq, HandoverEvent::GpuDone { attempt: active })), vec![]);
    assert_eq!(effects(state.step(&mut seq, HandoverEvent::Tick { now: t0 + FINISH_LIMIT + Duration::from_secs(1) })), vec![]);
}

#[test]
fn prepare_frame_recovers_if_reblend_starts_new_check_or_if_check_settled_before_wake() {
    let mut seq = Seq::default();
    let mut state = HandoverState::Building { built: true };

    // Reblend started a new uber_check right before PrepareFrame: defer to WaitingChecks.
    assert_eq!(effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: true })), vec![]);
    assert_eq!(state, HandoverState::WaitingChecks);

    // If a redraw runs after the check already answered (before Wake::GpuCheck), advance straight to PrecheckFeatures.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: false })),
        vec![HandoverEffect::PrecheckFeatures],
    );
    assert_eq!(state, HandoverState::Building { built: true });
}

#[test]
fn idle_frame_loop_does_not_spin_for_gpu_checks_and_wakes_once_when_settled() {
    // No animation, input, live pending, or compositor animation -> idle (no rAF spin).
    assert!(!web_wants_frame(false, false, false, false));
    assert!(web_wants_frame(true, false, false, false));
    assert!(web_wants_frame(false, true, false, false));
    assert!(web_wants_frame(false, false, true, false));
    assert!(web_wants_frame(false, false, false, true));

    // Draw::Gpu wake policy when Wake::GpuCheck fires:
    // Still checking -> do not redraw yet.
    assert!(!should_redraw_after_gpu_check(true, true, false));
    assert!(!should_redraw_after_gpu_check(true, false, false));
    // All checks cleared and a breaker settled -> redraw once even if last_presented existed.
    assert!(should_redraw_after_gpu_check(false, true, true));
    // All checks cleared and last_presented was None (uber_check just finished) -> redraw once.
    assert!(should_redraw_after_gpu_check(false, false, false));
    // All checks cleared, no breaker settled, and already presented cleanly -> stay idle.
    assert!(!should_redraw_after_gpu_check(false, false, true));
}

#[test]
fn ready_to_flip_ignores_late_timeout_and_defers_if_new_check_starts() {
    let mut seq = Seq::default();
    let mut state = HandoverState::Building { built: true };
    let t0 = crate::time::now();

    let eff = effects(state.step(&mut seq, HandoverEvent::GpuSubmitted { checking_gpu: false, now: t0 }));
    let attempt = match eff.as_slice() {
        [HandoverEffect::ArmFinishWait { attempt, .. }] => *attempt,
        other => panic!("unexpected: {other:?}"),
    };

    // GpuDone transitions to ReadyToFlip and requests the flip rAF.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::GpuDone { attempt })),
        vec![HandoverEffect::RequestRedraw],
    );
    assert_eq!(state, HandoverState::ReadyToFlip);

    // A stale timeout firing after GpuDone must NOT fail the completed handover.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::Tick { now: t0 + FINISH_LIMIT + Duration::from_secs(1) })),
        vec![],
    );
    assert_eq!(state, HandoverState::ReadyToFlip);

    // If a newly animated scene introduces an untried breaker feature on the flip frame,
    // ReadyToFlip defers back to WaitingChecks instead of flipping with an unverified pipeline.
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::PrepareFrame { checking_gpu: false })),
        vec![HandoverEffect::PrecheckFeatures],
    );
    assert_eq!(
        effects(state.step(&mut seq, HandoverEvent::Prechecked { checking_gpu: true })),
        vec![],
    );
    assert_eq!(state, HandoverState::WaitingChecks);
}

#[test]
fn gl_sync_poll_runs_on_animation_frames_and_stops_cleanly() {
    let mut seq = Seq::default();
    let mut poll = GlSyncPoll::default();
    let t0 = crate::time::now();
    let deadline = t0 + FINISH_LIMIT;

    // Start always waits for the next rAF (never polls synchronously in the submit turn).
    let start_eff: Vec<_> = poll.step(&mut seq, GlSyncPollEvent::Start { deadline }).into_iter().collect();
    let a1 = match start_eff.as_slice() {
        [GlSyncPollEffect::RequestAnimationFrame { attempt }] => *attempt,
        other => panic!("expected RequestAnimationFrame, got {other:?}"),
    };

    // Stale rAF attempt is ignored.
    let stale = seq.next();
    assert!(poll.step(&mut seq, GlSyncPollEvent::AnimationFrame { attempt: stale, now: t0 }).is_empty());

    // Frame 1 rAF -> PollDevice -> still pending -> schedules Frame 2 rAF with a new attempt.
    let f1: Vec<_> = poll.step(&mut seq, GlSyncPollEvent::AnimationFrame {
        attempt: a1,
        now: t0 + Duration::from_millis(16),
    }).into_iter().collect();
    assert_eq!(f1, vec![GlSyncPollEffect::PollDevice { attempt: a1 }]);

    let p1: Vec<_> = poll.step(&mut seq, GlSyncPollEvent::Polled {
        attempt: a1,
        completed: false,
        device_error: false,
        now: t0 + Duration::from_millis(16),
    }).into_iter().collect();
    let a2 = match p1.as_slice() {
        [GlSyncPollEffect::RequestAnimationFrame { attempt }] => *attempt,
        other => panic!("expected next RequestAnimationFrame, got {other:?}"),
    };
    assert_ne!(a1, a2);

    // Frame 2 rAF -> PollDevice -> completed -> Done with zero further rAFs.
    let f2: Vec<_> = poll.step(&mut seq, GlSyncPollEvent::AnimationFrame {
        attempt: a2,
        now: t0 + Duration::from_millis(33),
    }).into_iter().collect();
    assert_eq!(f2, vec![GlSyncPollEffect::PollDevice { attempt: a2 }]);

    let p2: Vec<_> = poll.step(&mut seq, GlSyncPollEvent::Polled {
        attempt: a2,
        completed: true,
        device_error: false,
        now: t0 + Duration::from_millis(33),
    }).into_iter().collect();
    assert!(p2.is_empty());
    assert_eq!(poll, GlSyncPoll::Done);
}

#[test]
fn gl_sync_poll_stops_on_deadline_device_error_or_cancel() {
    let t0 = crate::time::now();
    let deadline = t0 + FINISH_LIMIT;

    // 1. Reaching deadline at rAF stops without polling.
    let mut seq = Seq::default();
    let mut poll = GlSyncPoll::default();
    let a1 = match poll.step(&mut seq, GlSyncPollEvent::Start { deadline }).into_iter().next() {
        Some(GlSyncPollEffect::RequestAnimationFrame { attempt }) => attempt,
        other => panic!("unexpected: {other:?}"),
    };
    assert!(poll.step(&mut seq, GlSyncPollEvent::AnimationFrame { attempt: a1, now: deadline }).is_empty());
    assert_eq!(poll, GlSyncPoll::Done);

    // 2. Device error on poll stops immediately without scheduling another rAF.
    let mut poll = GlSyncPoll::default();
    let a1 = match poll.step(&mut seq, GlSyncPollEvent::Start { deadline }).into_iter().next() {
        Some(GlSyncPollEffect::RequestAnimationFrame { attempt }) => attempt,
        other => panic!("unexpected: {other:?}"),
    };
    let _ = poll.step(&mut seq, GlSyncPollEvent::AnimationFrame { attempt: a1, now: t0 + Duration::from_millis(8) });
    assert!(poll.step(&mut seq, GlSyncPollEvent::Polled {
        attempt: a1,
        completed: false,
        device_error: true,
        now: t0 + Duration::from_millis(8),
    }).is_empty());
    assert_eq!(poll, GlSyncPoll::Done);

    // 3. Cancel stops pending wait immediately.
    let mut poll = GlSyncPoll::default();
    let a1 = match poll.step(&mut seq, GlSyncPollEvent::Start { deadline }).into_iter().next() {
        Some(GlSyncPollEffect::RequestAnimationFrame { attempt }) => attempt,
        other => panic!("unexpected: {other:?}"),
    };
    assert!(poll.step(&mut seq, GlSyncPollEvent::Cancel).is_empty());
    assert_eq!(poll, GlSyncPoll::Done);
    assert!(poll.step(&mut seq, GlSyncPollEvent::AnimationFrame { attempt: a1, now: t0 + Duration::from_millis(8) }).is_empty());
}
