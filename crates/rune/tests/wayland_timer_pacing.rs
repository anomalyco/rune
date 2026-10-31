mod time { pub use std::time::Instant; }

#[path = "../src/window/pacing/timer_frame.rs"]
mod timer_frame;

use std::time::{Duration, Instant};
use timer_frame::{TIMER_FRAME, TimerFrame};

#[test]
fn ordinary_timers_keep_their_deadline() {
    let mut frame = TimerFrame::default();
    let at = Instant::now();
    assert_eq!(frame.deadline(Some(at)), Some(at));
    assert_eq!(frame.deadline(None), None);
    assert!(!frame.poll());
}

#[test]
fn a_withheld_redraw_replaces_an_expired_timer_with_a_future_wait() {
    let now = Instant::now();
    let overdue = now - Duration::from_secs(1);
    let mut frame = TimerFrame::default();
    frame.requested(now);
    assert_eq!(frame.deadline(Some(overdue)), Some(now + TIMER_FRAME));
    assert!(frame.deadline(Some(overdue)).unwrap() > now);
}

#[test]
fn a_parked_runtime_timer_still_has_a_fallback() {
    let now = Instant::now();
    let mut frame = TimerFrame::default();
    frame.requested(now);
    assert_eq!(frame.deadline(None), Some(now + TIMER_FRAME));
}

#[test]
fn other_wakes_cannot_extend_the_redraw_grace() {
    let now = Instant::now();
    let mut frame = TimerFrame::default();
    frame.requested(now);
    for step in 1..20 {
        frame.requested(now + Duration::from_millis(step));
        assert_eq!(frame.deadline(Some(now)), Some(now + TIMER_FRAME));
    }
}

#[test]
fn a_withheld_redraw_keeps_later_timers_on_the_background_path() {
    let now = Instant::now();
    let mut frame = TimerFrame::default();
    frame.requested(now);
    assert!(frame.poll());
    for step in 1..20 {
        let next = now + Duration::from_millis(step * 33);
        assert_eq!(frame.deadline(Some(next)), Some(next));
        assert!(frame.poll(), "later timers poll instead of requesting another redraw");
    }
    assert_eq!(frame.deadline(None), None, "no timers means no wake");
    frame.requested(now + Duration::from_secs(1));
    assert_eq!(frame.deadline(None), None, "a waiting redraw never rearms its grace");
}

#[test]
fn resetting_for_redraw_or_hide_restores_ordinary_timer_handling() {
    let now = Instant::now();
    for stalled in [false, true] {
        let mut frame = TimerFrame::default();
        frame.requested(now);
        if stalled { assert!(frame.poll()); }
        frame = TimerFrame::default();
        assert!(!frame.poll());
        assert_eq!(frame.deadline(Some(now)), Some(now));
        frame.requested(now + Duration::from_secs(1));
        assert_eq!(frame.deadline(None), Some(now + Duration::from_secs(1) + TIMER_FRAME));
    }
}
