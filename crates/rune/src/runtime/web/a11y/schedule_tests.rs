use std::time::Duration;

use crate::machine::{Attempt, Seq};
use crate::time::Instant;

use super::{
    IDLE_TIMEOUT, SCROLL_SETTLE, ScheduleEffect, ScheduleEvent, ScheduleState, step,
};

fn effects(seq: &mut Seq, state: &mut ScheduleState, event: ScheduleEvent) -> Vec<ScheduleEffect> {
    step(seq, state, event).into_iter().collect()
}

#[test]
fn scrolling_and_virtual_shifts_defer_all_raf_syncs_until_settle_and_idle() {
    let t0 = Instant::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    // First scroll frame arms the 150 ms settle timer and performs zero synchronous flushes.
    let e0 = effects(&mut seq, &mut state, ScheduleEvent::Frame {
        now: t0,
        document: false,
        dirty: true,
        scrolling: true,
        non_virtual_commit: false,
    });
    assert_eq!(e0, vec![ScheduleEffect::ScheduleSettle { attempt: Attempt(1), delay: SCROLL_SETTLE }]);

    // 480 ms swipe at 60 Hz (including CommitWhy::VIRTUAL shifts): zero FlushNow and zero timer spam
    // while before the armed timer fires; when attempt 1 fires mid-swipe, it re-arms cleanly.
    let mut armed = Attempt(1);
    let mut timer_due = t0 + SCROLL_SETTLE;
    for frame in 1..=30u64 {
        let now = t0 + Duration::from_millis(frame * 16);
        let ef = effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now,
            document: false,
            dirty: true,
            scrolling: true,
            non_virtual_commit: false,
        });
        assert!(!ef.contains(&ScheduleEffect::FlushNow), "must not flush during scroll frame {frame}");
        if now >= timer_due {
            let te = effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
                attempt: armed,
                now,
                scrolling: true,
            });
            assert_eq!(te.len(), 1);
            match te[0] {
                ScheduleEffect::ScheduleSettle { attempt, delay } => {
                    assert_eq!(delay, SCROLL_SETTLE);
                    armed = attempt;
                    timer_due = now + delay;
                }
                other => panic!("expected ScheduleSettle mid-swipe, got {other:?}"),
            }
        }
    }

    // Swipe stops at 480 ms. An intermediate frame 16 ms later sees scrolling=false, still waits.
    let stop_at = t0 + Duration::from_millis(480);
    let after_stop = stop_at + Duration::from_millis(16);
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now: after_stop,
            document: false,
            dirty: true,
            scrolling: false,
            non_virtual_commit: false,
        }),
        vec![],
    );

    // If the previously armed timer fires before stop_at + 150 ms, it waits the exact remaining time.
    let mid = stop_at + Duration::from_millis(50);
    let rem_ef = effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
        attempt: armed,
        now: mid,
        scrolling: false,
    });
    assert_eq!(rem_ef.len(), 1);
    let final_attempt = match rem_ef[0] {
        ScheduleEffect::ScheduleSettle { attempt, delay } => {
            assert_eq!(delay, Duration::from_millis(100));
            attempt
        }
        other => panic!("expected remaining settle delay, got {other:?}"),
    };

    // Stale attempt from earlier is ignored.
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
            attempt: armed,
            now: stop_at + SCROLL_SETTLE,
            scrolling: false,
        }),
        vec![],
    );

    // At stop_at + 150 ms, settle completes and schedules a bounded off-rAF idle flush.
    let settled_at = stop_at + SCROLL_SETTLE;
    let settle_ef = effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
        attempt: final_attempt,
        now: settled_at,
        scrolling: false,
    });
    let idle_attempt = match settle_ef.as_slice() {
        [ScheduleEffect::ScheduleIdle { attempt, timeout }] => {
            assert_eq!(*timeout, IDLE_TIMEOUT);
            *attempt
        }
        other => panic!("expected ScheduleIdle after settle, got {other:?}"),
    };

    // When idle callback fires, mirror flushes once and returns to Clean.
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::IdleDue {
            attempt: idle_attempt,
            now: settled_at + Duration::from_millis(4),
            scrolling: false,
        }),
        vec![ScheduleEffect::FlushNow],
    );
    assert_eq!(state, ScheduleState::Clean);
}

#[test]
fn non_scroll_app_commits_never_flush_synchronously_in_raf() {
    let t0 = Instant::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    // First frame and subsequent non-scroll commits schedule off-rAF idle flush (never FlushNow in rAF).
    let e0 = effects(&mut seq, &mut state, ScheduleEvent::Frame {
        now: t0,
        document: false,
        dirty: true,
        scrolling: false,
        non_virtual_commit: true,
    });
    assert_eq!(e0, vec![ScheduleEffect::ScheduleIdle { attempt: Attempt(1), timeout: IDLE_TIMEOUT }]);

    // Another dirty frame before idle runs coalesces into the same pending idle callback.
    let e1 = effects(&mut seq, &mut state, ScheduleEvent::Frame {
        now: t0 + Duration::from_millis(16),
        document: false,
        dirty: true,
        scrolling: false,
        non_virtual_commit: true,
    });
    assert_eq!(e1, vec![]);

    // If scrolling starts before idle runs, IdleDue defers instead of flushing mid-scroll.
    let e2 = effects(&mut seq, &mut state, ScheduleEvent::IdleDue {
        attempt: Attempt(1),
        now: t0 + Duration::from_millis(20),
        scrolling: true,
    });
    assert_eq!(e2, vec![ScheduleEffect::ScheduleSettle { attempt: Attempt(2), delay: SCROLL_SETTLE }]);
    assert!(matches!(state, ScheduleState::ScrollDefer { attempt: Attempt(2), .. }));
}

#[test]
fn consumer_demand_cancels_pending_timers_and_flushes_immediately() {
    let t0 = Instant::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    // Enter ScrollDefer.
    let _ = effects(&mut seq, &mut state, ScheduleEvent::Frame {
        now: t0,
        document: false,
        dirty: true,
        scrolling: true,
        non_virtual_commit: false,
    });
    assert!(matches!(state, ScheduleState::ScrollDefer { .. }));

    // AT action / focusin / beforeprint / armMenu / Ctrl+F forces immediate flush and cancels timers.
    let demand = effects(&mut seq, &mut state, ScheduleEvent::ConsumerDemand);
    assert_eq!(demand, vec![ScheduleEffect::CancelTimers, ScheduleEffect::FlushNow]);
    assert_eq!(state, ScheduleState::Clean);

    // Late timer from before ConsumerDemand is ignored.
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
            attempt: Attempt(1),
            now: t0 + SCROLL_SETTLE,
            scrolling: false,
        }),
        vec![],
    );

    // ConsumerDemand when already Clean still flushes (without redundant CancelTimers).
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::ConsumerDemand),
        vec![ScheduleEffect::FlushNow],
    );
}

#[test]
fn document_mode_flushes_synchronously_on_first_frame_and_commits() {
    let t0 = Instant::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now: t0,
            document: true,
            dirty: true,
            scrolling: false,
            non_virtual_commit: true,
        }),
        vec![ScheduleEffect::FlushNow],
    );
    assert_eq!(state, ScheduleState::Clean);

    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now: t0 + Duration::from_millis(16),
            document: true,
            dirty: false,
            scrolling: false,
            non_virtual_commit: false,
        }),
        vec![],
    );
}
