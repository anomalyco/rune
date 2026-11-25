use super::*;

#[test]
fn idle_draws_nothing_and_arms_no_wake() {
    let mut fill = Fill::default();
    let fx = fill.step(FillEvent::Frame { now: 0.0, wanted: false, missing: 0, version_changed: false });
    assert!(fx.is_empty());
    assert_eq!(fill.state(), State::Idle);
    assert!(!fill.resuming(100.0));
}

#[test]
fn steady_budgeted_drain_finishes_without_pausing_past_stagnant_limit() {
    let mut fill = Fill::default();
    // 30 frames of monotonic draining (exceeding STAGNANT_LIMIT = 12) never backs off.
    let mut t = 0.0;
    for remaining in (1..=30).rev() {
        let fx: Vec<_> = fill.step(FillEvent::Frame { now: t, wanted: true, missing: remaining, version_changed: false }).into_iter().collect();
        assert_eq!(fx, vec![FillEffect::Draw], "frame at remaining={remaining}");
        assert!(matches!(fill.state(), State::Filling { .. }));
        t += 1.0 / 60.0;
    }
    let fx = fill.step(FillEvent::Frame { now: t, wanted: false, missing: 0, version_changed: false });
    assert!(fx.is_empty());
    assert_eq!(fill.state(), State::Idle);
}

#[test]
fn runaway_version_churn_backs_off_then_stalls_and_resets_when_settled() {
    let mut fill = Fill::default();
    let mut t = 0.0;
    let mut waits = Vec::new();
    for _ in 0..200 {
        if let Some(attempt) = fill.wake(t) {
            let fx: Vec<_> = fill.step(FillEvent::Due { attempt, now: t }).into_iter().collect();
            assert_eq!(fx, vec![FillEffect::Draw]);
        }
        let fx: Vec<_> = fill.step(FillEvent::Frame { now: t, wanted: true, missing: 20, version_changed: true }).into_iter().collect();
        if let Some(FillEffect::WakeAt(due)) = fx.first().copied() {
            waits.push(due - t);
            t = due;
            continue;
        }
        if fill.state() == State::Stalled {
            break;
        }
        t += 1.0 / 60.0;
    }
    assert_eq!(fill.state(), State::Stalled, "runaway churn must stall");
    assert_eq!(waits.len(), 3, "three exponentially increasing waits before stall: {waits:?}");
    assert!(waits[1] > waits[0] && waits[2] > waits[1], "backoff must double: {waits:?}");

    // Further frames while churning draw nothing.
    let fx = fill.step(FillEvent::Frame { now: t + 1.0, wanted: true, missing: 20, version_changed: true });
    assert!(fx.is_empty());
    assert!(!fill.resuming(t + 10.0));

    // Settling (wanted: false) or Reset returns cleanly to Idle.
    let fx = fill.step(FillEvent::Frame { now: t + 2.0, wanted: false, missing: 0, version_changed: false });
    assert!(fx.is_empty());
    assert_eq!(fill.state(), State::Idle);
}

#[test]
fn stale_attempt_never_restarts_a_retired_wait() {
    let mut fill = Fill::default();
    let mut t = 0.0;
    let mut armed_due = None;
    for _ in 0..20 {
        let fx: Vec<_> = fill.step(FillEvent::Frame { now: t, wanted: true, missing: 10, version_changed: true }).into_iter().collect();
        if let Some(FillEffect::WakeAt(due)) = fx.first().copied() {
            armed_due = Some(due);
            break;
        }
        t += 1.0 / 60.0;
    }
    let due = armed_due.expect("entered Waiting");
    // Unmount / settle retires the wait and drops the deadline.
    let _ = fill.step(FillEvent::Frame { now: due - 0.01, wanted: false, missing: 0, version_changed: false });
    assert_eq!(fill.state(), State::Idle);
    assert_eq!(fill.wake(due + 1.0), None);
    assert!(!fill.resuming(due + 1.0));
}
