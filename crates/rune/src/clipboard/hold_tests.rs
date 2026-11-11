//! Named sequences through the hold, each checked against the reference on the way.
use super::*;

fn write_memory(clipboard: &MemoryClipboard, delay: Duration) -> SecretCopy {
    let (write, handle) = ClipboardWrite::secret(SECRET.into(), delay);
    write.bind(Route::Memory(clipboard.clone()));
    clipboard.handle(Request::Write(write, None));
    handle
}

#[test]
fn a_held_secret_does_not_leak_its_memory_clipboard() {
    let clipboard = memory();
    let weak = Arc::downgrade(&clipboard.0);
    let handle = write_memory(&clipboard, LONG);
    assert!(handle.holds());
    drop(clipboard);
    drop(handle);
    assert!(weak.upgrade().is_none());
}

#[test]
fn either_surviving_handle_still_drives_expiry() {
    let clipboard = memory();
    let handle = write_memory(&clipboard, Duration::ZERO);
    drop(clipboard);
    assert_eq!(handle.state(), SecretState::Cleared);

    let clipboard = memory();
    drop(write_memory(&clipboard, Duration::ZERO));
    assert!(clipboard.text().is_none());
}

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
    let states = [SecretState::Pending, SecretState::Holding, SecretState::Cleared, SecretState::Replaced, SecretState::Failed("x".into())];
    for report in states.into_iter().map(Report::Said).chain([Report::Refused]) {
        for holds in [0, 1] {
            for mut secret in [Secret::Live { report: report.clone(), holds }, Secret::Cancelled { report: report.clone(), holds }] {
                let effects = step(&mut secret, SecretEvent::Cancel);
                assert_eq!(secret, Secret::Cancelled { report: report.clone(), holds });
                let held = report == Report::Said(SecretState::Holding) || (matches!(report, Report::Said(SecretState::Failed(_))) && holds > 0);
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
    assert!(step(&mut secret, SecretEvent::Released(Report::Said(SecretState::Replaced))).is_empty());
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
    assert!(step_hold(&mut seq, &mut hold, HoldEvent::Due { attempt: stale, ours: true }).is_empty());
    assert!(!hold.is_free());
    let attempt = seq.current();
    let effects: Vec<_> = step_hold(&mut seq, &mut hold, HoldEvent::Due { attempt, ours: true }).into_iter().collect();
    assert!(matches!(effects[..], [HoldEffect::Wipe]));
    drop(step_hold(&mut seq, &mut hold, HoldEvent::Swept { ok: true }));
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

fn board() -> Fake {
    Fake { id: 0, content: None, count: 0, counted: true, landed: None, others: false, fail: None, unreadable: false, wipe: WipeMode::Works, log: Log::default(), during_write: None }
}
fn woke(rig: &Rig<Core>) -> usize { lock(&rig.log).iter().filter(|e| matches!(e, Logged::Woke(0, ..))).count() }
fn refused() -> SecretState { SecretState::Failed(REFUSED.into()) }

// r128 (r208): a wipe the clipboard refuses is final.

#[test]
fn a_refused_wipe_is_final_and_reported_as_failed() {
    for counted in [true, false] {
        for clear in [&[CancelS, Pump][..], &[Timeout, ReadA]] {
            let rig = run_on([counted; 2], &[&[WriteSA, CycleWipeA][..], clear].concat());
            assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (Some(SECRET), refused()), "{counted} {clear:?}");
            assert!(!rig.engines[0].watching());
            assert_eq!(woke(&rig), 2, "once for the write, once for the refusal");
            let later = run_on([counted; 2], &[&[WriteSA, CycleWipeA][..], clear, &[CycleWipeA, CycleWipeA, CycleWipeA, Timeout, ReadA, CancelS, Pump]].concat());
            assert_eq!((text_of(&later.boards[0]), wipes(&later, 0), woke(&later)), (Some(SECRET), 0, 2), "never tried again");
        }
    }
}

/// A refused wipe's `Failed` is no failed write: `clear_now` doesn't send r86's recovery to a
/// board a clone also reached, as dev (which reported `Cleared`) didn't.
#[test]
fn a_refused_wipe_starts_no_failed_replay_recovery() {
    let rig = run(&[WriteSB, WriteSA, CycleWipeA, ClearHeldA, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[1]), rig.s.state(), wipes(&rig, 1)), (Some(SECRET), refused(), 0));
}

// r219: a read error ends the clear as dev's does; nothing decides it again later.

#[test]
fn a_read_error_ends_the_clear_as_before() {
    let rig = run(&[WriteSA, ToggleUnreadableA, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (Some(SECRET), SecretState::Replaced));
    assert!(!rig.engines[0].watching());
    for copy in [OtherA, OtherSameText] {
        let rig = run(&[WriteSA, ToggleUnreadableA, CancelS, Pump, ToggleUnreadableA, copy, ReadA, Timeout, ReadA]);
        assert_eq!((wipes(&rig, 0), rig.boards[0].others), (0, true), "{copy:?}");
    }
}

// r220: with no deferred release, a newer clone's Holding stays, and clear_now reaches it.

#[test]
fn a_newer_clone_s_holding_is_never_overwritten_by_an_older_board() {
    let rig = run(&[WriteSB, WriteSA, ToggleUnreadableA, ClearHeldA, WriteSB, OtherA, ReadA, CancelS, Pump]);
    assert_eq!((text_of(&rig.boards[1]), rig.s.state()), (None, SecretState::Cleared));
}

// r212: nothing is scheduled past the secret's own delay.

#[test]
fn a_failed_clear_schedules_nothing_past_the_delay() {
    // No deadline: an unreadable clipboard gives up at once, and nothing polls.
    let mut cb = board();
    let mut core = Core::default();
    let (write, forever) = ClipboardWrite::secret(SECRET.into(), Duration::MAX);
    core.write(&mut cb, write, None, &RealTime);
    cb.unreadable = true;
    core.clear(&mut cb, None);
    assert_eq!((forever.state(), core.next_check()), (SecretState::Replaced, None));
    // A refused wipe ends the hold too.
    let mut cb = board();
    let mut core = Core::default();
    let (write, refusing) = ClipboardWrite::secret(SECRET.into(), LONG);
    core.write(&mut cb, write, None, &RealTime);
    cb.wipe = WipeMode::Refused;
    core.clear(&mut cb, None);
    assert_eq!((refusing.state(), core.next_check()), (refused(), None));
    // A deadline makes no difference: an unreadable clipboard gives up at once.
    let mut cb = board();
    let mut core = Core::default();
    let (write, waiting) = ClipboardWrite::secret(SECRET.into(), LONG);
    core.write(&mut cb, write, None, &RealTime);
    cb.unreadable = true;
    core.clear(&mut cb, None);
    assert_eq!((waiting.state(), core.next_check()), (SecretState::Replaced, None));
}

// r210: the worker's exit settles the hold.

/// A clipboard that can be made unreadable, or that refuses wipes.
struct Stuck { text: String, revision: u64, unreadable: Arc<AtomicBool>, refuse: bool }
impl Backend for Stuck {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
        self.text = copy.content.text().to_string();
        self.revision += 1;
        Ok(())
    }
    fn text(&mut self) -> Option<Zeroizing<String>> {
        (!self.unreadable.load(Ordering::SeqCst)).then(|| Zeroizing::new(self.text.clone()))
    }
    fn revision(&self) -> Option<u64> { Some(self.revision) }
    fn clear(&mut self) -> Result<(), String> {
        if self.refuse { return Err("refused".into()); }
        self.text.clear();
        self.revision += 1;
        Ok(())
    }
}

#[test]
fn the_worker_s_exit_leaves_no_handle_holding() {
    for (unreadable, refuse, clear_first, expected) in [
        (true, false, false, SecretState::Replaced),
        (true, false, true, SecretState::Replaced),
        (false, true, false, refused()),
    ] {
        let lock_it = Arc::new(AtomicBool::new(false));
        let shared = lock_it.clone();
        let worker = native::Worker::spawn(move || Some(Stuck { text: String::new(), revision: 0, unreadable: shared.clone(), refuse }));
        let (woke, wakes) = mpsc::channel();
        let notify = Notify { changed: Arc::default(), wake: Arc::new(move || { let _ = woke.send(()); }) };
        let (write, handle) = ClipboardWrite::secret(SECRET.into(), LONG);
        write.bind(Route::System(worker.sender.clone()));
        worker.sender.send(Request::Write(write, Some(notify))).unwrap();
        wakes.recv_timeout(Duration::from_secs(1)).unwrap();
        lock_it.store(unreadable, Ordering::SeqCst);
        if clear_first { handle.clear_now(); }
        worker.stop();
        assert_eq!(handle.state(), expected, "unreadable {unreadable}, refuse {refuse}, clear first {clear_first}");
        wakes.recv_timeout(Duration::from_secs(1)).expect("the app hears of it");
        assert!(wakes.recv_timeout(Duration::from_millis(50)).is_err(), "once");
    }
}

// r211: the watch reads the clipboard's text as dev's does, formatting no error.

#[test]
fn the_watch_reads_an_unreadable_clipboard_as_before() {
    let rig = run_on([false, false], &[WriteSA, ToggleUnreadableA, ReadA]);
    assert_eq!(rig.s.state(), SecretState::Replaced);
}
