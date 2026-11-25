use std::time::Duration;

use super::{Effect, Event, State, SyncKey, TEARDOWN_GRACE, step};
use crate::machine::{Attempt, Seq};
use crate::time::Instant;

fn key(commit: u64, modal: Option<usize>) -> SyncKey {
    SyncKey { commit, modal, active_drag: false }
}

fn step_vec(seq: &mut Seq, state: &mut State, event: Event) -> Vec<Effect> {
    step(seq, state, event).into_iter().collect()
}

/// Instrumented harness proving exact bind/unbind/thread/scan counts with an injected clock.
struct Harness {
    seq: Seq,
    state: State,
    now: Instant,
    binds: usize,
    unbinds: usize,
    threads_started: usize,
    threads_joined: usize,
    active_threads: usize,
    leave_drags: usize,
    scans: usize,
    fail_bind: bool,
}

impl Harness {
    fn new() -> Self {
        Self {
            seq: Seq::default(),
            state: State::default(),
            now: crate::time::now(),
            binds: 0,
            unbinds: 0,
            threads_started: 0,
            threads_joined: 0,
            active_threads: 0,
            leave_drags: 0,
            scans: 0,
            fail_bind: false,
        }
    }

    fn apply(&mut self, event: Event) {
        for effect in step_vec(&mut self.seq, &mut self.state, event) {
            match effect {
                Effect::Bind => {
                    self.binds += 1;
                    if self.fail_bind {
                        self.apply(Event::BindFailed);
                    } else {
                        self.threads_started += 1;
                        self.active_threads += 1;
                    }
                }
                Effect::LeaveRuntimeDrag => self.leave_drags += 1,
                Effect::WakeAt(_) => {}
                Effect::Unbind => {
                    self.unbinds += 1;
                    if self.active_threads > 0 {
                        self.active_threads -= 1;
                        self.threads_joined += 1;
                    }
                }
            }
        }
    }

    fn sync_frame(&mut self, sync: SyncKey, has_targets: bool) {
        if !self.state.needs_sync(sync) {
            return;
        }
        self.scans += 1;
        let now = self.now;
        self.apply(Event::TreeSynced { key: sync, has_targets, now });
    }

    fn advance(&mut self, dt: Duration) {
        self.now += dt;
        if let Some(attempt) = self.state.grace_attempt()
            && self.state.deadline().is_some_and(|due| due <= self.now)
        {
            let now = self.now;
            self.apply(Event::GraceDue { attempt, now });
        }
    }
}

#[test]
fn app_without_drop_handlers_binds_nothing_starts_no_threads_and_skips_steady_frames() {
    let mut h = Harness::new();
    h.sync_frame(key(1, None), false);
    h.apply(Event::WindowReady);
    // 100 steady/animation frames with unchanged commit_generation and modal.
    for _ in 0..100 {
        h.sync_frame(key(1, None), false);
    }
    // Additional commits without drop handlers still bind nothing.
    h.sync_frame(key(2, None), false);
    h.sync_frame(key(3, Some(4)), false);
    assert_eq!(h.binds, 0);
    assert_eq!(h.unbinds, 0);
    assert_eq!(h.threads_started, 0);
    assert_eq!(h.active_threads, 0);
    assert_eq!(h.scans, 3);
    assert_eq!(h.state.deadline(), None);
}

#[test]
fn first_handler_binds_once_after_first_frame_and_steady_frames_do_zero_work() {
    let mut h = Harness::new();
    // Handler mounted during frame 1 before first frame lands: must NOT bind yet.
    h.sync_frame(key(1, None), true);
    assert_eq!(h.binds, 0);
    assert_eq!(h.threads_started, 0);
    assert!(matches!(h.state, State::PendingBind { .. }));

    // First frame lands -> binds once.
    h.apply(Event::WindowReady);
    assert_eq!(h.binds, 1);
    assert_eq!(h.threads_started, 1);
    assert_eq!(h.active_threads, 1);
    assert!(matches!(h.state, State::Bound { .. }));

    // Steady frames with same key do zero scans and zero binds.
    let scans_before = h.scans;
    for _ in 0..50 {
        h.sync_frame(key(1, None), true);
    }
    assert_eq!(h.scans, scans_before);
    assert_eq!(h.binds, 1);
}

#[test]
fn grace_period_eliminates_churn_on_rapid_screen_toggles_and_tears_down_after_grace() {
    let mut h = Harness::new();
    h.apply(Event::WindowReady);
    h.sync_frame(key(1, None), true);
    assert_eq!((h.binds, h.unbinds, h.threads_started, h.threads_joined, h.active_threads), (1, 0, 1, 0, 1));

    // Rapidly toggle a drop-free modal/screen 10 times within TEARDOWN_GRACE: zero thread churn.
    let mut commit = 2;
    for _ in 0..10 {
        h.sync_frame(key(commit, Some(1)), false);
        commit += 1;
        assert!(matches!(h.state, State::TearingDown { .. }));
        h.advance(TEARDOWN_GRACE / 2);
        assert_eq!(h.active_threads, 1, "still within grace period");
        h.sync_frame(key(commit, None), true);
        commit += 1;
        assert!(matches!(h.state, State::Bound { .. }));
    }
    assert_eq!((h.binds, h.unbinds, h.threads_started, h.threads_joined, h.active_threads), (1, 0, 1, 0, 1));

    // Unmount permanently: unrelated commits during grace must NOT postpone the original deadline.
    h.sync_frame(key(commit, None), false);
    commit += 1;
    let armed_attempt = h.state.grace_attempt().expect("armed grace attempt");
    let due = h.state.deadline().expect("armed grace deadline");
    h.advance(TEARDOWN_GRACE / 2);
    h.sync_frame(key(commit, None), false);
    commit += 1;
    assert_eq!(h.state.deadline(), Some(due), "unrelated commits must not extend grace");

    // Stale attempt or premature firing does nothing.
    let now = h.now;
    h.apply(Event::GraceDue { attempt: Attempt(armed_attempt.0.wrapping_add(99)), now: due });
    h.apply(Event::GraceDue { attempt: armed_attempt, now });
    assert_eq!(h.active_threads, 1);

    // Once grace elapses, tears down cleanly and leaves zero threads behind.
    h.advance(TEARDOWN_GRACE);
    assert_eq!((h.binds, h.unbinds, h.threads_started, h.threads_joined, h.active_threads), (1, 1, 1, 1, 0));
    assert!(matches!(h.state, State::ReadyUnbound { .. }));
    assert_eq!(h.state.deadline(), None);

    // Mounting a drop handler again re-binds cleanly.
    h.sync_frame(key(commit, None), true);
    assert_eq!((h.binds, h.unbinds, h.threads_started, h.threads_joined, h.active_threads), (2, 1, 2, 1, 1));
    assert!(matches!(h.state, State::Bound { .. }));
}

#[test]
fn close_display_lost_and_bind_failed_transition_table() {
    let t0 = crate::time::now();
    let t1 = t0 + TEARDOWN_GRACE;
    let k1 = key(1, None);
    let k2 = key(2, None);

    // Table covering every state x key lifecycle event.
    let cases: &[(State, Event, State, &[Effect])] = &[
        (State::Unbound { last_sync: None }, Event::WindowReady, State::ReadyUnbound { last_sync: None }, &[]),
        (State::PendingBind { last_sync: Some(k1) }, Event::WindowReady, State::Bound { last_sync: Some(k1) }, &[Effect::Bind]),
        (State::PendingBind { last_sync: Some(k1) }, Event::TreeSynced { key: k2, has_targets: false, now: t0 }, State::Unbound { last_sync: Some(k2) }, &[]),
        (State::ReadyUnbound { last_sync: Some(k1) }, Event::TreeSynced { key: k2, has_targets: true, now: t0 }, State::Bound { last_sync: Some(k2) }, &[Effect::Bind]),
        (State::Bound { last_sync: Some(k1) }, Event::BindFailed, State::Unavailable, &[]),
        (State::Unavailable, Event::TreeSynced { key: k2, has_targets: true, now: t0 }, State::Unavailable, &[]),
        (State::Bound { last_sync: Some(k1) }, Event::WindowClosed, State::Unbound { last_sync: None }, &[Effect::LeaveRuntimeDrag, Effect::Unbind]),
        (State::TearingDown { last_sync: Some(k1), attempt: Attempt(1), due: t1 }, Event::WindowClosed, State::Unbound { last_sync: None }, &[Effect::Unbind]),
        (State::Bound { last_sync: Some(k1) }, Event::DisplayLost, State::Unavailable, &[Effect::LeaveRuntimeDrag, Effect::Unbind]),
        (State::TearingDown { last_sync: Some(k1), attempt: Attempt(1), due: t1 }, Event::DisplayLost, State::Unavailable, &[Effect::Unbind]),
        (State::ReadyUnbound { last_sync: Some(k1) }, Event::DisplayLost, State::Unavailable, &[]),
        (State::TearingDown { last_sync: Some(k1), attempt: Attempt(1), due: t1 }, Event::GraceDue { attempt: Attempt(1), now: t1 }, State::ReadyUnbound { last_sync: Some(k1) }, &[Effect::Unbind]),
    ];

    for (idx, &(initial, event, expected_state, expected_effects)) in cases.iter().enumerate() {
        let mut seq = Seq::default();
        let mut state = initial;
        let effects = step_vec(&mut seq, &mut state, event);
        assert_eq!(state, expected_state, "table case #{idx}: state mismatch");
        assert_eq!(effects.as_slice(), expected_effects, "table case #{idx}: effects mismatch");
    }

    // BindFailed in harness prevents retries forever.
    let mut h = Harness::new();
    h.fail_bind = true;
    h.apply(Event::WindowReady);
    h.sync_frame(key(1, None), true);
    assert_eq!(h.binds, 1);
    assert_eq!(h.state, State::Unavailable);
    h.sync_frame(key(2, None), true);
    assert_eq!(h.binds, 1, "must not retry after BindFailed");

    // WindowClosed / DisplayLost immediately join active threads without waiting for grace.
    let mut h2 = Harness::new();
    h2.apply(Event::WindowReady);
    h2.sync_frame(key(1, None), true);
    h2.sync_frame(key(2, None), false);
    assert!(matches!(h2.state, State::TearingDown { .. }));
    h2.apply(Event::WindowClosed);
    assert_eq!(h2.active_threads, 0);
    assert_eq!(h2.threads_joined, 1);
}

#[test]
fn drag_out_only_tree_triggers_lazy_binding_and_respects_exiting_modal_and_active_drag() {
    use crate::{Cx, DragItem, DragOutcome, El, Env, Runtime, Transition, col, el};

    #[derive(Default)]
    struct S {
        card: bool,
        card_exit: bool,
        modal: bool,
        modal_card: bool,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().size(200.0, 200.0);
        if s.card {
            let mut card = el()
                .key("card")
                .size(100.0, 40.0)
                .drag_out(|_: &S| Some(DragItem::new().text("card")));
            if s.card_exit {
                card = card.transition(Transition::opacity());
            }
            root = root.child(card);
        }
        if s.modal {
            let mut m = col().key("modal").modal(true, |_: &mut S| {}).size(150.0, 150.0);
            if s.modal_card {
                m = m.child(
                    el().key("mcard")
                        .size(80.0, 30.0)
                        .drag_out(|_: &S| Some(DragItem::new().text("mcard"))),
                );
            }
            root = root.child(m);
        }
        root
    }

    let mut rt = Runtime::new(S { card: true, ..Default::default() }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.core().has_drop_targets());
    assert!(rt.core().has_drag_out());

    // Lazy binding binds once after WindowReady for a drag_out-only tree.
    let mut h = Harness::new();
    let sync = |rt: &Runtime<S>, h: &mut Harness| {
        let k = SyncKey {
            commit: rt.core().commit.commits,
            modal: rt.core().active_modal(),
            active_drag: rt.core().has_active_drag_out(),
        };
        let has = rt.core().has_drop_targets() || rt.core().has_drag_out() || rt.core().has_active_drag_out();
        h.sync_frame(k, has);
    };
    sync(&rt, &mut h);
    assert_eq!(h.binds, 0, "must defer before first frame");
    h.apply(Event::WindowReady);
    assert_eq!(h.binds, 1, "drag_out-only tree binds once after first frame");

    // os_drag = false disables has_drag_out.
    rt.set_os_drag(false);
    assert!(!rt.core().has_drag_out());
    rt.set_os_drag(true);
    assert!(rt.core().has_drag_out());

    // Exiting card does not count as mounted drag_out.
    rt.update(|s, _| s.card_exit = true);
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.card = false);
    rt.frame(1.0 / 60.0);
    assert!(!rt.core().has_drag_out(), "exiting drag_out element must not count");

    // Active modal without drag_out hides background drag_out.
    rt.update(|s, _| { s.card = true; s.card_exit = false; s.modal = true; });
    rt.frame(1.0 / 60.0);
    assert!(!rt.core().has_drag_out());
    rt.update(|s, _| s.modal_card = true);
    rt.frame(1.0 / 60.0);
    assert!(rt.core().has_drag_out());

    // An in-flight drag keeps has_active_drag_out() true even if the card unmounts on drag start.
    rt.update(|s, _| { s.modal = false; s.modal_card = false; s.card = true; });
    rt.frame(1.0 / 60.0);
    rt.pointer_move(20.0, 20.0);
    rt.pointer_down(20.0, 20.0);
    rt.pointer_move(50.0, 20.0);
    let start = rt.take_drag_out().expect("drag-out started");
    rt.update(|s, _| s.card = false);
    rt.frame(1.0 / 60.0);
    assert!(!rt.core().has_drag_out());
    assert!(rt.core().has_active_drag_out(), "active drag stays tracked until on_drag_end");
    sync(&rt, &mut h);
    h.advance(TEARDOWN_GRACE * 2);
    assert_eq!(h.unbinds, 0, "binding must stay alive while drag is in flight");

    // Once the drag finishes, grace timer unbinds cleanly.
    rt.drag_end_sink().borrow_mut().push((start.path, DragOutcome::Copied));
    rt.frame(1.0 / 60.0);
    assert!(!rt.core().has_active_drag_out());
    sync(&rt, &mut h);
    h.advance(TEARDOWN_GRACE);
    assert_eq!(h.unbinds, 1);
}
