use super::SurfaceOutput;
use super::super::readiness::{self, CloseKind, Effect, Event, State};

#[test]
fn pinned_output_matches_only_its_own_global_remove_and_ignores_enter_leave() {
    let mut out = SurfaceOutput::new(Some(10));
    assert!(!out.follows_enter_leave());
    assert_eq!(out.current(), Some(10));

    out.enter(20);
    out.leave(10);
    assert_eq!(out.current(), Some(10), "pinned output ignores enter/leave");
    assert!(!out.reconcile_live_outputs(|id| id == 10));
    assert!(out.reconcile_live_outputs(|id| id == 20));
}

#[test]
fn followed_output_tracks_current_or_last_left_output() {
    let mut out = SurfaceOutput::new(None);
    assert!(out.follows_enter_leave());
    assert_eq!(out.current(), None);
    assert!(!out.reconcile_live_outputs(|_| false));

    // Entered output 10: removing another output (20) does not match; removing 10 does.
    out.enter(10);
    assert_eq!(out.current(), Some(10));
    assert!(!out.reconcile_live_outputs(|id| id == 10));

    // Leaving output 10 right before unplug remembers 10 as the last left output.
    out.leave(10);
    assert_eq!(out.current(), Some(10));
    assert!(!out.reconcile_live_outputs(|id| id == 10));
    assert!(out.reconcile_live_outputs(|id| id == 20));

    // Moving from 10 to 20: removing 10 no longer counts as our output going away.
    let mut moved = SurfaceOutput::new(None);
    moved.enter(10);
    moved.enter(20);
    moved.leave(10);
    assert_eq!(moved.current(), Some(20));
    assert!(!moved.reconcile_live_outputs(|id| id == 20));
    assert!(moved.reconcile_live_outputs(|id| id == 10));

    // While on two outputs [10, 20], removing 10 leaves the surface on 20.
    let mut spanning = SurfaceOutput::new(None);
    spanning.enter(10);
    spanning.enter(20);
    assert!(!spanning.reconcile_live_outputs(|id| id == 20));
    assert_eq!(spanning.current(), Some(20));
    assert!(spanning.reconcile_live_outputs(|_| false));

    // Leaving 10 then entering and leaving 20 remembers 20, not 10.
    let mut seq = SurfaceOutput::new(None);
    seq.enter(10);
    seq.leave(10);
    seq.enter(20);
    seq.leave(20);
    assert_eq!(seq.current(), Some(20));
    assert!(!seq.reconcile_live_outputs(|id| id == 20));
    assert!(seq.reconcile_live_outputs(|id| id == 10));
}

#[test]
fn readiness_waits_for_close_sync_before_reporting_closed_and_prefers_output_gone() {
    // 1. Plain closed: waits for CloseSyncDone before transitioning to Ended(Closed).
    let mut s = State::default();
    assert!(readiness::step(&mut s, Event::Configure { width: 100, height: 40 }).is_empty());
    let effects: Vec<_> = readiness::step(&mut s, Event::Closed).into_iter().collect();
    assert_eq!(effects, vec![Effect::SendCloseSync]);
    assert!(s.closing());
    assert!(!s.closed());
    assert_eq!(s.close_kind(), None);
    // Repeated Closed while already Closing emits no duplicate sync.
    assert!(readiness::step(&mut s, Event::Closed).is_empty());
    assert!(readiness::step(&mut s, Event::CloseSyncDone).is_empty());
    assert!(!s.closing());
    assert!(s.closed());
    assert_eq!(s.close_kind(), Some(CloseKind::Closed));

    // 2. Closed followed by OutputRemoved before sync done -> OutputGone.
    let mut s = State::default();
    readiness::step(&mut s, Event::Configure { width: 100, height: 40 });
    assert_eq!(readiness::step(&mut s, Event::Closed).into_iter().collect::<Vec<_>>(), vec![Effect::SendCloseSync]);
    assert!(readiness::step(&mut s, Event::OutputRemoved).is_empty());
    assert!(s.closed());
    assert_eq!(s.close_kind(), Some(CloseKind::OutputGone));
    // A late CloseSyncDone does not overwrite OutputGone.
    assert!(readiness::step(&mut s, Event::CloseSyncDone).is_empty());
    assert_eq!(s.close_kind(), Some(CloseKind::OutputGone));

    // 3. OutputRemoved without Closed -> OutputGone immediately, no sync needed.
    let mut s = State::default();
    readiness::step(&mut s, Event::Configure { width: 100, height: 40 });
    assert!(readiness::step(&mut s, Event::OutputRemoved).is_empty());
    assert!(s.closed());
    assert_eq!(s.close_kind(), Some(CloseKind::OutputGone));
    // A subsequent Closed does not emit a sync or overwrite OutputGone.
    assert!(readiness::step(&mut s, Event::Closed).is_empty());
    assert_eq!(s.close_kind(), Some(CloseKind::OutputGone));

    // 4. Closed during initial Configuring stays !configured() until OutputRemoved or CloseSyncDone.
    let mut s = State::default();
    assert!(!s.configured());
    assert_eq!(readiness::step(&mut s, Event::Closed).into_iter().collect::<Vec<_>>(), vec![Effect::SendCloseSync]);
    assert!(!s.configured(), "wait_for_configure must stay in the loop until sync or global_remove settles");
    assert!(readiness::step(&mut s, Event::OutputRemoved).is_empty());
    assert!(s.configured());
    assert_eq!(s.close_kind(), Some(CloseKind::OutputGone));

    // 5. ConnectionFailed while Closing ends immediately as Closed.
    let mut s = State::default();
    readiness::step(&mut s, Event::Configure { width: 100, height: 40 });
    readiness::step(&mut s, Event::Closed);
    assert!(readiness::step(&mut s, Event::ConnectionFailed).is_empty());
    assert!(s.closed());
    assert_eq!(s.close_kind(), Some(CloseKind::Closed));
}
