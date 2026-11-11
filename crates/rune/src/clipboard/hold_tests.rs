//! Named sequences through the hold, each checked against the reference on the way.
use super::*;

/// Runs `sequence` through the machines, checked against the reference, and hands back the rig
/// to look at.
fn run_on(counted: [bool; 2], sequence: &[Ev]) -> Rig<Core> {
    let start = Instant::now();
    check(sequence, counted, start).unwrap();
    let mut rig = Rig::<Core>::new(counted, start);
    for &ev in sequence { rig.apply(ev); }
    rig
}
fn run(sequence: &[Ev]) -> Rig<Core> { run_on([true, true], sequence) }
fn wipes(rig: &Rig<Core>, board: usize) -> usize {
    lock(&rig.log).iter().filter(|e| **e == Logged::Wiped(board)).count()
}

#[test]
fn a_cancel_during_the_write_is_wiped_before_a_queued_read() {
    let rig = run(&[WriteSACancelInWrite, ReadA]);
    assert_eq!(text_of(&rig.boards[0]), None);
    assert_eq!(rig.reads[0].as_ref().map(|r| r.0.text()), Some(""));
    assert_eq!(rig.s.state(), SecretState::Cleared);
}

#[test]
fn a_cancel_during_the_notification_is_wiped_before_a_queued_read() {
    let rig = run(&[WriteSACancelInWake, ReadA]);
    assert_eq!(rig.reads[0].as_ref().map(|r| r.0.text()), Some(""));
    let woke = lock(&rig.log).iter().filter(|e| matches!(e, Logged::Woke(..))).count();
    assert_eq!(woke, 2, "one for the hold, one for the clear");
}

#[test]
fn a_cancel_in_flight_spares_another_apps_copy() {
    let rig = run(&[WriteSACancelOtherInWrite]);
    assert_eq!(text_of(&rig.boards[0]), Some("theirs"));
    assert_eq!(wipes(&rig, 0), 0);
    assert_eq!(rig.s.state(), SecretState::Replaced);
}

#[test]
fn a_replay_after_clear_now_is_never_written() {
    let rig = run(&[WriteSA, CancelS, Pump, WriteSA]);
    assert_eq!(text_of(&rig.boards[0]), None);
    assert_eq!(rig.s.state(), SecretState::Cleared);
    let rig = run(&[WriteSA, Timeout, ReadA, WriteSA]);
    assert_eq!(text_of(&rig.boards[0]), Some(SECRET), "expiry alone doesn't stop a replay");
    assert_eq!(rig.s.state(), SecretState::Holding);
}

#[test]
fn a_replay_while_holding_stays_covered_by_cleanup() {
    let rig = run(&[WriteSA, WriteSA, Timeout, ReadA]);
    assert_eq!((text_of(&rig.boards[0]), wipes(&rig, 0)), (None, 1));
    let rig = run(&[WriteSA, WriteSA, ShutdownA]);
    assert_eq!((text_of(&rig.boards[0]), wipes(&rig, 0)), (None, 1));
}

#[test]
fn the_latest_failure_is_reported_and_keeps_earlier_cleanup() {
    let rig = run(&[ToggleFailA, WriteSA, ToggleFailA, ToggleFailA, WriteSA]);
    assert_eq!(rig.s.state(), SecretState::Failed("denied 3".into()));
    let rig = run(&[WriteSA, ToggleFailA, WriteSA, Timeout, ReadA]);
    assert_eq!(rig.s.state(), SecretState::Cleared);
    assert_eq!((text_of(&rig.boards[0]), wipes(&rig, 0)), (None, 1));
}

#[test]
fn a_clone_on_two_boards_is_cleaned_up_on_each() {
    let rig = run(&[WriteSA, WriteSB, WriteTA, Timeout, ReadA, ReadB]);
    assert_eq!(text_of(&rig.boards[1]), None, "A's later write doesn't disarm B");
    assert_eq!(wipes(&rig, 1), 1);
    let rig = run(&[WriteSA, WriteSB, OtherA, ReadA, Timeout, ReadB]);
    assert_eq!((text_of(&rig.boards[0]), text_of(&rig.boards[1])), (Some("theirs"), None));
}

#[test]
fn cancel_latches_in_every_report_and_asks_for_a_clear_only_when_it_may_be_held() {
    for report in [SecretState::Pending, SecretState::Holding, SecretState::Cleared, SecretState::Replaced, SecretState::Failed("x".into())] {
        for holds in [0, 1] {
            for mut secret in [Secret::Live { report: report.clone(), holds }, Secret::Cancelled { report: report.clone(), holds }] {
                let effects = step(&mut secret, SecretEvent::Cancel);
                assert_eq!(secret, Secret::Cancelled { report: report.clone(), holds });
                let held = report == SecretState::Holding || (matches!(report, SecretState::Failed(_)) && holds > 0);
                assert_eq!(effects, if held { Effects::one(SecretEffect::Clear) } else { Effects::none() });
            }
        }
    }
}

#[test]
fn a_failure_after_the_hold_ended_asks_for_no_clear() {
    let mut secret = Secret::default();
    let failed = || SecretEvent::Report(SecretState::Failed("x".into()));
    assert!(step(&mut secret, SecretEvent::Held).is_empty());
    assert!(step(&mut secret, SecretEvent::Released(SecretState::Replaced)).is_empty());
    assert!(step(&mut secret, failed()).is_empty());
    assert!(step(&mut secret, SecretEvent::Cancel).is_empty());
}

#[test]
fn clear_now_wipes_what_a_failed_replay_left_held() {
    let rig = run(&[WriteSA, ToggleFailA, WriteSA, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state(), wipes(&rig, 0)), (None, SecretState::Cleared, 1));
}

#[test]
fn clear_now_spares_another_apps_copy_after_a_failed_replay() {
    let rig = run(&[WriteSA, ToggleFailA, WriteSA, OtherA, CancelS, Pump, ReadA]);
    assert_eq!((text_of(&rig.boards[0]), wipes(&rig, 0)), (Some("theirs"), 0));
}

/// A failed replay, a `clear_now` that can't finish, then the secret's own timeout.
fn keeps_its_timeout(break_it: &[Ev], mend_it: &[Ev]) {
    let start = [WriteSA, ToggleFailA, WriteSA];
    let during: Vec<Ev> = start.iter().chain(break_it).chain(&[CancelS, Pump]).copied().collect();
    let rig = run(&during);
    assert_eq!(rig.boards[0].ours(), true, "the clear couldn't finish");
    assert!(rig.engines[0].watching(), "the hold stays");
    assert_eq!(rig.s.state(), SecretState::Failed("denied 1".into()));
    let after: Vec<Ev> = during.iter().chain(mend_it).chain(&[Timeout, ReadA]).copied().collect();
    let rig = run(&after);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state(), wipes(&rig, 0)), (None, SecretState::Cleared, 1));
}

#[test]
fn a_failed_replay_keeps_its_timeout_when_the_read_fails() {
    keeps_its_timeout(&[ToggleUnreadableA], &[ToggleUnreadableA]);
}

#[test]
fn a_failed_replay_keeps_its_timeout_when_the_wipe_is_refused() {
    keeps_its_timeout(&[CycleWipeA], &[CycleWipeA, CycleWipeA, CycleWipeA]);
}

#[test]
fn a_failed_replay_keeps_its_timeout_when_the_wipe_does_nothing() {
    keeps_its_timeout(&[CycleWipeA, CycleWipeA], &[CycleWipeA, CycleWipeA]);
}

#[test]
fn a_failed_replay_keeps_its_timeout_when_a_refused_wipe_locks_the_clipboard() {
    keeps_its_timeout(&[CycleWipeA, CycleWipeA, CycleWipeA], &[CycleWipeA, ToggleUnreadableA]);
}

#[test]
fn clear_now_still_reaches_only_the_first_board() {
    let rig = run(&[WriteSA, WriteSB, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[0]), text_of(&rig.boards[1])), (None, Some(SECRET)));
    let rig = run(&[WriteSA, WriteSB, ToggleFailA, WriteSA, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[0]), text_of(&rig.boards[1])), (None, Some(SECRET)));
}

#[test]
fn the_delay_starts_once_a_slow_write_is_done() {
    let rig = run(&[WriteSASlow, ReadA]);
    assert_eq!(rig.reads[0].as_ref().map(|r| r.0.text()), Some(SECRET));
    assert_eq!(rig.s.state(), SecretState::Holding);
    let rig = run(&[WriteSASlow, Timeout, ReadA]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (None, SecretState::Cleared));
}

#[test]
fn the_delay_runs_while_the_secret_is_hashed() {
    let rig = run(&[WriteSASlowHash, ReadA]);
    assert_eq!(rig.reads[0].as_ref().map(|r| r.0.text()), Some(""));
    assert_eq!((text_of(&rig.boards[0]), rig.s.state(), wipes(&rig, 0)), (None, SecretState::Cleared, 1));
}

#[test]
fn a_write_reports_holding_then_starts_the_delay_then_hashes() {
    let rig = run(&[WriteSA]);
    let (holding, pending) = (SecretState::Holding, SecretState::Pending);
    assert_eq!(*lock(&rig.log), [
        Logged::Wrote(0),
        Logged::ReadClock(holding.clone(), pending.clone()),
        Logged::Hashing(holding.clone(), pending.clone()),
        Logged::Woke(0, holding, pending),
    ]);
}

#[test]
fn a_hold_with_no_deadline_keeps_no_earlier_alarm() {
    let mut cb = Fake { id: 0, content: None, count: 0, counted: true, landed: None, others: false, fail: None, unreadable: false, wipe: WipeMode::Works, log: Log::default(), during_write: None };
    let mut core = Core::default();
    core.write(&mut cb, ClipboardWrite::secret("first".into(), LONG).0, None, &RealTime);
    assert!(core.alarm.is_some());
    core.write(&mut cb, ClipboardWrite::secret("second".into(), Duration::MAX).0, None, &RealTime);
    assert!(core.alarm.is_none(), "a wrapped attempt could otherwise match it");
}

#[test]
fn a_timer_from_an_earlier_hold_changes_nothing() {
    let slot = Arc::new(Slot::default());
    let (mut seq, mut hold) = (Seq::default(), Hold::default());
    let due = Some(Instant::now());
    let written = |slot: &Arc<Slot>| HoldEvent::Written { slot: slot.clone(), notify: None, due, revision: None, print: 0 };
    drop(step_hold(&mut seq, &mut hold, written(&slot)));
    let stale = seq.current();
    drop(step_hold(&mut seq, &mut hold, written(&slot)));
    assert!(step_hold(&mut seq, &mut hold, HoldEvent::Due { attempt: stale, owner: Owner::Ours }).is_empty());
    assert!(!hold.is_free());
    let attempt = seq.current();
    let effects: Vec<_> = step_hold(&mut seq, &mut hold, HoldEvent::Due { attempt, owner: Owner::Ours }).into_iter().collect();
    assert!(matches!(effects[..], [HoldEffect::Wipe { last: true }]));
    drop(step_hold(&mut seq, &mut hold, HoldEvent::Swept { ok: true, last: true }));
    assert!(hold.is_free());
}

#[test]
fn no_clipboard_reports_a_failed_write_and_wakes() {
    let worker = native::Worker::spawn(|| None::<Board>);
    let (woke, wakes) = mpsc::channel();
    let notify = Notify { changed: Arc::default(), wake: Arc::new(move || { let _ = woke.send(()); }) };
    let (write, handle) = ClipboardWrite::secret("fixture".into(), LONG);
    worker.sender.send(Request::Write(write, Some(notify))).unwrap();
    wakes.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(handle.state(), SecretState::Failed("no clipboard".into()));
    worker.stop();
}

// r128 and r129: before the delay runs out, a clipboard that couldn't be wiped or read keeps the
// hold, and the delay tries again.

#[test]
fn a_refused_wipe_before_the_delay_keeps_the_hold_for_the_delay() {
    let rig = run(&[WriteSA, CycleWipeA, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (Some(SECRET), SecretState::Holding));
    assert!(rig.engines[0].watching());
    let rig = run(&[WriteSA, CycleWipeA, CancelS, Pump, CycleWipeA, CycleWipeA, CycleWipeA, Timeout, ReadA]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state(), wipes(&rig, 0)), (None, SecretState::Cleared, 1));
}

#[test]
fn an_unreadable_clipboard_before_the_delay_keeps_the_hold_for_the_delay() {
    let rig = run(&[WriteSA, ToggleUnreadableA, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (Some(SECRET), SecretState::Holding));
    let rig = run(&[WriteSA, ToggleUnreadableA, CancelS, Pump, ToggleUnreadableA, Timeout, ReadA]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (None, SecretState::Cleared));
}

/// The delay's try is the last: a clipboard that still refuses is given up on, as before, so
/// nothing keeps retrying or waking.
#[test]
fn the_delay_s_try_is_the_last() {
    let rig = run(&[WriteSA, CycleWipeA, CancelS, Pump, Timeout, ReadA]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (Some(SECRET), SecretState::Cleared));
    assert!(!rig.engines[0].watching());
    let rig = run(&[WriteSA, ToggleUnreadableA, Timeout, ReadA]);
    assert_eq!(rig.s.state(), SecretState::Replaced);
    assert!(!rig.engines[0].watching());
}

/// The delay's try checks the change count first, so a copy made after the failed wipe is
/// never taken.
#[test]
fn the_delay_s_try_spares_a_copy_made_after_the_failed_wipe() {
    for copy in [OtherA, OtherSameText] {
        let rig = run(&[WriteSA, CycleWipeA, CancelS, Pump, CycleWipeA, CycleWipeA, CycleWipeA, copy, Timeout, ReadA]);
        assert_eq!((rig.s.state(), wipes(&rig, 0)), (SecretState::Replaced, 0), "{copy:?}");
        assert!(rig.boards[0].others);
    }
}

/// Without a change count a later copy of the same text can't be told apart, so the first
/// failure gives up, as before.
#[test]
fn without_a_change_count_the_first_failure_gives_up_as_before() {
    let rig = run_on([false, false], &[WriteSA, CycleWipeA, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (Some(SECRET), SecretState::Cleared));
    assert!(!rig.engines[0].watching());
    let rig = run_on([false, false], &[WriteSA, ToggleUnreadableA, CancelS, Pump]);
    assert_eq!(rig.s.state(), SecretState::Replaced);
}
