use super::machine::*;
use super::{AudioEvent, AudioOutputState};
use std::time::Duration;

const RATE: u32 = 48_000;

fn run(phase: &mut Phase, event: Event) -> Vec<Effect> {
    step(phase, RATE, event).into_iter().collect()
}

#[test]
fn lifecycle_paused_playing_draining_ended_and_flush_reset() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    assert_eq!(phase.public_state(), AudioOutputState::Paused);
    assert!(!phase.is_playing());
    assert_eq!(phase.heard_at(t0, RATE), 0);

    // Play wakes worker and enters Playing.
    assert_eq!(run(&mut phase, Event::Play { now: t0 }), vec![Effect::WakeWorker]);
    assert_eq!(phase.public_state(), AudioOutputState::Playing);
    assert!(phase.is_playing());

    // Redundant Play is a no-op.
    assert_eq!(run(&mut phase, Event::Play { now: t0 }), vec![]);

    // Write 4800 frames (100 ms) with 1920 frames (40 ms) still in device -> 2880 heard (60 ms).
    let t1 = t0 + Duration::from_millis(60);
    assert_eq!(run(&mut phase, Event::Wrote { frames: 4_800, latency: 1_920, now: t1 }), vec![]);
    assert_eq!(phase.heard_at(t1, RATE), 2_880);
    assert_eq!(phase.latency_at(t1, RATE), 1_920);

    // Finish transitions Playing -> Draining (non-blocking).
    assert_eq!(run(&mut phase, Event::Finish { now: t1, has_buffered: false }), vec![Effect::WakeWorker]);
    assert_eq!(phase.public_state(), AudioOutputState::Draining);
    assert!(phase.is_playing());
    assert!(phase.rejects_push_draining());

    // Latency polls downward until 0 -> Ended + single AudioEvent::Ended.
    let t2 = t1 + Duration::from_millis(20);
    assert_eq!(run(&mut phase, Event::DrainPoll { latency: 960, now: t2 }), vec![]);
    assert_eq!(phase.public_state(), AudioOutputState::Draining);
    assert_eq!(phase.heard_at(t2, RATE), 3_840);

    let t3 = t2 + Duration::from_millis(20);
    assert_eq!(
        run(&mut phase, Event::DrainPoll { latency: 0, now: t3 }),
        vec![Effect::Emit(AudioEvent::Ended)]
    );
    assert_eq!(phase.public_state(), AudioOutputState::Ended);
    assert!(!phase.is_playing());
    assert_eq!(phase.heard_at(t3 + Duration::from_secs(5), RATE), 4_800);

    // Flush from Ended returns to Paused with clock reset to 0.
    assert_eq!(
        run(&mut phase, Event::Flush { now: t3 }),
        vec![Effect::ClearBuffer, Effect::WakeWorker]
    );
    assert_eq!(phase.public_state(), AudioOutputState::Paused);
    assert!(!phase.rejects_push_draining());
    assert_eq!(phase.heard_at(t3, RATE), 0);
}

#[test]
fn heard_clock_interpolates_smoothly_and_never_moves_backward_or_past_written() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    run(&mut phase, Event::Play { now: t0 });
    run(&mut phase, Event::Wrote { frames: 4_800, latency: 2_400, now: t0 });
    assert_eq!(phase.heard_at(t0, RATE), 2_400);

    // 10 ms later, interpolation advances by 480 frames -> 2_880.
    let t1 = t0 + Duration::from_millis(10);
    run(&mut phase, Event::Observe { now: t1 });
    assert_eq!(phase.heard_at(t1, RATE), 2_880);

    // Jittery higher latency reading from OS driver (would estimate 2_600) must not regress heard.
    run(&mut phase, Event::Wrote { frames: 0, latency: 2_200, now: t1 });
    assert_eq!(phase.heard_at(t1, RATE), 2_880);

    // Long jump forward caps at total written frames (4_800), never over-reporting.
    let t_far = t0 + Duration::from_secs(10);
    assert_eq!(phase.heard_at(t_far, RATE), 4_800);
    assert_eq!(phase.latency_at(t_far, RATE), 0);
}

#[test]
fn pause_snaps_clock_rewinds_exact_unheard_tail_and_resumes_monotonically() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    run(&mut phase, Event::Play { now: t0 });
    run(&mut phase, Event::Wrote { frames: 4_800, latency: 2_400, now: t0 });

    // Pause while 2_400 frames are heard and 2_400 remain in the device.
    let t1 = t0 + Duration::from_millis(10); // interpolated heard = 2_880
    assert_eq!(run(&mut phase, Event::Pause { now: t1 }), vec![Effect::WakeWorker]);
    assert_eq!(phase.public_state(), AudioOutputState::Paused);
    assert_eq!(phase.heard_at(t1 + Duration::from_secs(5), RATE), 2_880);

    // Worker queries device latency = 1_920 (so 4_800 - 1_920 = 2_880 heard, 1_920 unheard) and flushes.
    assert_eq!(
        run(&mut phase, Event::PauseFlushed { latency: 1_920 }),
        vec![Effect::RewindTail(1_920)]
    );
    assert_eq!(phase.heard_at(t1 + Duration::from_secs(5), RATE), 2_880);
    assert_eq!(phase.pending_device(), None);

    // Even if device reports stale larger latency (e.g. 2_500 -> estimate 2_300 < 2_880 floor),
    // heard never moves backward and rewind only rewinds frames above the monotonic floor.
    let t2 = t1 + Duration::from_millis(50);
    run(&mut phase, Event::Play { now: t2 });
    run(&mut phase, Event::Wrote { frames: 1_920, latency: 1_920, now: t2 });
    assert_eq!(phase.heard_at(t2, RATE), 2_880);
    let t3 = t2 + Duration::from_millis(20);
    run(&mut phase, Event::Wrote { frames: 480, latency: 1_440, now: t3 });
    assert_eq!(phase.heard_at(t3, RATE), 3_840);
}

#[test]
fn play_immediately_after_pause_still_flushes_and_rewinds_before_writing() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    run(&mut phase, Event::Play { now: t0 });
    run(&mut phase, Event::Wrote { frames: 2_400, latency: 1_200, now: t0 });
    run(&mut phase, Event::Pause { now: t0 });
    // Resume before the worker ran PauseFlushed.
    run(&mut phase, Event::Play { now: t0 });
    assert!(matches!(phase.pending_device(), Some(PendingDevice::PauseRewind { .. })));

    // Worker completes the pending pause flush + rewind and anchors segment_base at 1_200.
    assert_eq!(
        run(&mut phase, Event::PauseFlushed { latency: 1_200 }),
        vec![Effect::RewindTail(1_200)]
    );
    assert_eq!(phase.pending_device(), None);
    assert_eq!(phase.heard_at(t0, RATE), 1_200);
}

#[test]
fn flush_while_playing_preserves_playing_and_suppresses_spurious_underrun() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    run(&mut phase, Event::Play { now: t0 });
    run(&mut phase, Event::Wrote { frames: 1_920, latency: 480, now: t0 });
    assert_eq!(phase.heard_at(t0, RATE), 1_440);

    let t1 = t0 + Duration::from_millis(5);
    assert_eq!(
        run(&mut phase, Event::Flush { now: t1 }),
        vec![Effect::ClearBuffer, Effect::WakeWorker]
    );
    assert_eq!(phase.public_state(), AudioOutputState::Playing);
    assert_eq!(phase.heard_at(t1, RATE), 0);
    run(&mut phase, Event::DeviceFlushed);

    // Empty buffer right after flush must NOT emit Underrun until new samples arrive.
    assert_eq!(run(&mut phase, Event::DryPoll { latency: 0, now: t1 }), vec![]);

    // Once new samples are written and later drain dry, Underrun fires once.
    run(&mut phase, Event::Wrote { frames: 480, latency: 480, now: t1 });
    assert_eq!(
        run(&mut phase, Event::DryPoll { latency: 0, now: t1 + Duration::from_millis(10) }),
        vec![Effect::Emit(AudioEvent::Underrun)]
    );
}

#[test]
fn underrun_is_edge_triggered_and_rearms_after_recovery() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    run(&mut phase, Event::Play { now: t0 });
    run(&mut phase, Event::Wrote { frames: 480, latency: 480, now: t0 });

    // Non-zero latency while dry: still draining device buffer, no underrun yet.
    assert_eq!(run(&mut phase, Event::DryPoll { latency: 240, now: t0 + Duration::from_millis(5) }), vec![]);
    // Latency reaches 0: emits Underrun once.
    assert_eq!(
        run(&mut phase, Event::DryPoll { latency: 0, now: t0 + Duration::from_millis(10) }),
        vec![Effect::Emit(AudioEvent::Underrun)]
    );
    // Repeated DryPoll or DeviceUnderrun while latched emits nothing.
    assert_eq!(run(&mut phase, Event::DryPoll { latency: 0, now: t0 + Duration::from_millis(20) }), vec![]);
    assert_eq!(run(&mut phase, Event::DeviceUnderrun { now: t0 + Duration::from_millis(25) }), vec![]);

    // Recovery: writing new samples clears the latch.
    let t1 = t0 + Duration::from_millis(30);
    run(&mut phase, Event::Wrote { frames: 480, latency: 480, now: t1 });
    assert_eq!(
        run(&mut phase, Event::DryPoll { latency: 0, now: t1 + Duration::from_millis(10) }),
        vec![Effect::Emit(AudioEvent::Underrun)]
    );
}

#[test]
fn pause_during_draining_resumes_draining_and_flush_cancels_drain() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    run(&mut phase, Event::Play { now: t0 });
    run(&mut phase, Event::Wrote { frames: 1_920, latency: 960, now: t0 });
    run(&mut phase, Event::Finish { now: t0, has_buffered: true });
    assert_eq!(phase.public_state(), AudioOutputState::Draining);

    // Pause while draining preserves resume_draining and rewinds unheard frames.
    run(&mut phase, Event::Pause { now: t0 });
    assert_eq!(phase.public_state(), AudioOutputState::Paused);
    assert!(phase.rejects_push_draining());
    assert_eq!(run(&mut phase, Event::PauseFlushed { latency: 960 }), vec![Effect::RewindTail(960)]);

    // Play resumes Draining.
    run(&mut phase, Event::Play { now: t0 });
    assert_eq!(phase.public_state(), AudioOutputState::Draining);

    // Flush returns Draining to normal Paused.
    run(&mut phase, Event::Flush { now: t0 });
    assert_eq!(phase.public_state(), AudioOutputState::Paused);
    assert!(!phase.rejects_push_draining());
}

#[test]
fn draining_stall_timer_bounds_stuck_device_latency() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    run(&mut phase, Event::Play { now: t0 });
    run(&mut phase, Event::Wrote { frames: 960, latency: 480, now: t0 });
    run(&mut phase, Event::Finish { now: t0, has_buffered: false });

    // Stuck latency at 480 frames: stays Draining before DRAIN_STALL_PATIENCE.
    assert_eq!(run(&mut phase, Event::DrainPoll { latency: 480, now: t0 }), vec![]);
    assert_eq!(
        run(&mut phase, Event::DrainPoll { latency: 480, now: t0 + DRAIN_STALL_PATIENCE - Duration::from_millis(1) }),
        vec![]
    );
    // At DRAIN_STALL_PATIENCE with no progress: finishes cleanly instead of hanging.
    assert_eq!(
        run(&mut phase, Event::DrainPoll { latency: 480, now: t0 + DRAIN_STALL_PATIENCE }),
        vec![Effect::Emit(AudioEvent::Ended)]
    );
    assert_eq!(phase.public_state(), AudioOutputState::Ended);
    assert_eq!(phase.heard_at(t0 + DRAIN_STALL_PATIENCE, RATE), 960);
}

#[test]
fn device_lost_is_terminal_and_emits_once() {
    let t0 = rune_core::time::now();
    let mut phase = Phase::default();
    run(&mut phase, Event::Play { now: t0 });
    run(&mut phase, Event::Wrote { frames: 1_000, latency: 200, now: t0 });

    assert_eq!(
        run(&mut phase, Event::DeviceLost { reason: "unplugged".into(), now: t0 }),
        vec![Effect::ClearBuffer, Effect::Emit(AudioEvent::DeviceLost("unplugged".into()))]
    );
    assert_eq!(phase.public_state(), AudioOutputState::DeviceLost("unplugged".into()));
    assert_eq!(phase.heard_at(t0 + Duration::from_secs(5), RATE), 800);

    // Subsequent events cannot leave DeviceLost or emit duplicate DeviceLost.
    assert_eq!(run(&mut phase, Event::Play { now: t0 }), vec![]);
    assert_eq!(run(&mut phase, Event::Pause { now: t0 }), vec![]);
    assert_eq!(run(&mut phase, Event::Finish { now: t0, has_buffered: false }), vec![]);
    assert_eq!(run(&mut phase, Event::DeviceLost { reason: "again".into(), now: t0 }), vec![]);
    assert_eq!(run(&mut phase, Event::Flush { now: t0 }), vec![Effect::ClearBuffer]);
    assert!(phase.is_lost());
}

#[test]
fn frames_duration_conversions_are_exact() {
    assert_eq!(frames_to_duration(0, 48_000), Duration::ZERO);
    assert_eq!(frames_to_duration(48_000, 48_000), Duration::from_secs(1));
    assert_eq!(frames_to_duration(480, 48_000), Duration::from_millis(10));
    assert_eq!(frames_to_duration(44_100, 44_100), Duration::from_secs(1));
    assert_eq!(duration_to_frames(Duration::from_millis(10), 48_000), 480);
    assert_eq!(duration_to_frames(Duration::from_secs(1), 48_000), 48_000);
    assert_eq!(frames_to_duration(100, 0), Duration::ZERO);
    assert_eq!(duration_to_frames(Duration::from_secs(1), 0), 0);
}
