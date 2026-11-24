use super::*;
use crate::shortcuts::{Shortcut, ShortcutDetails};

fn item(id: u32, chord: &str) -> PortalShortcut {
    let shortcut: Shortcut = chord.parse().unwrap();
    let details = ShortcutDetails::default();
    PortalShortcut {
        id: ShortcutId(id),
        portal_id: details.portal_id(&shortcut),
        description: details.portal_description(&shortcut),
        preferred_trigger: shortcut.portal_trigger(),
    }
}

fn named(id: u32, chord: &str, portal_id: &str, description: &str) -> PortalShortcut {
    let shortcut: Shortcut = chord.parse().unwrap();
    let details = ShortcutDetails::named(portal_id, description);
    PortalShortcut {
        id: ShortcutId(id),
        portal_id: details.portal_id(&shortcut),
        description: details.portal_description(&shortcut),
        preferred_trigger: shortcut.portal_trigger(),
    }
}

fn outcome(session: &'static str, ids: &[&str]) -> Result<BindOutcome<&'static str>, ShortcutError> {
    Ok(BindOutcome { session, bound_portal_ids: ids.iter().map(|s| (*s).into()).collect() })
}

fn run(
    seq: &mut Seq,
    state: &mut State<&'static str, u32, u32>,
    event: Event<&'static str, u32, u32>,
) -> Vec<Effect<&'static str, u32, u32>> {
    step(seq, state, event).into_iter().collect()
}

#[test]
fn startup_batch_binds_all_shortcuts_in_one_session_with_stable_ids() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(10, "ctrl+alt+c");
    let b = named(20, "ctrl+alt+space", "quick-panel", "Open quick panel");
    assert_eq!(a.portal_id, "ctrl+alt+c", "default portal id is canonical chord, independent of ShortcutId");
    assert_eq!(a.description, "Shortcut ctrl+alt+c");
    assert_eq!(a.preferred_trigger, "CTRL+ALT+c");
    assert_eq!(b.portal_id, "quick-panel");
    assert_eq!(b.description, "Open quick panel");
    assert_eq!(b.preferred_trigger, "CTRL+ALT+space");

    // Blank explicit id and description fall back to canonical defaults.
    let blank = named(30, "ctrl+alt+k", "   ", "");
    assert_eq!(blank.portal_id, "ctrl+alt+k");
    assert_eq!(blank.description, "Shortcut ctrl+alt+k");

    let prt = item(40, "printscreen");
    let ctrl_prt = item(41, "ctrl+printscreen");
    let alt_prt = item(42, "alt+printscreen");
    assert_eq!((prt.portal_id.as_str(), prt.preferred_trigger.as_str()), ("printscreen", "Print"));
    assert_eq!((ctrl_prt.portal_id.as_str(), ctrl_prt.preferred_trigger.as_str()), ("ctrl+printscreen", "CTRL+Print"));
    assert_eq!((alt_prt.portal_id.as_str(), alt_prt.preferred_trigger.as_str()), ("alt+printscreen", "ALT+Print"));

    let effects = run(&mut seq, &mut state, Event::Register { items: vec![a.clone(), b.clone()], reply: 1 });
    assert_eq!(effects, vec![Effect::Bind { attempt: Attempt(1), shortcuts: vec![a.clone(), b.clone()] }]);

    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+c", "quick-panel"]) });
    assert_eq!(effects, vec![Effect::ReplyRegister { reply: 1, results: vec![Ok(()), Ok(())] }]);

    // Activation matches by session handle and portal_id, mapping back to Rune's ShortcutId.
    let effects = run(&mut seq, &mut state, Event::Activated { session: "s1", portal_id: "quick-panel".into(), token: Some("tok".into()) });
    assert_eq!(effects, vec![Effect::Activate { id: ShortcutId(20), token: Some("tok".into()) }]);

    // Wrong session or unknown portal_id is ignored.
    assert!(run(&mut seq, &mut state, Event::Activated { session: "other", portal_id: "quick-panel".into(), token: None }).is_empty());
    assert!(run(&mut seq, &mut state, Event::Activated { session: "s1", portal_id: "unknown".into(), token: None }).is_empty());
}

#[test]
fn partial_acceptance_commits_only_accepted_shortcuts_and_empty_closes_session() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(1, "ctrl+alt+a");
    let b = item(2, "ctrl+alt+b");

    let effects = run(&mut seq, &mut state, Event::Register { items: vec![a.clone(), b.clone()], reply: 1 });
    assert_eq!(effects, vec![Effect::Bind { attempt: Attempt(1), shortcuts: vec![a.clone(), b.clone()] }]);

    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a"]) });
    assert_eq!(effects, vec![Effect::ReplyRegister { reply: 1, results: vec![Ok(()), Err(ShortcutError::PermissionDenied)] }]);
    assert_eq!(
        run(&mut seq, &mut state, Event::Activated { session: "s1", portal_id: "ctrl+alt+a".into(), token: None }),
        vec![Effect::Activate { id: ShortcutId(1), token: None }]
    );
    assert!(run(&mut seq, &mut state, Event::Activated { session: "s1", portal_id: "ctrl+alt+b".into(), token: None }).is_empty());

    // Starting fresh: portal returns no bound shortcuts -> session is closed and state is Idle.
    let mut seq = Seq::default();
    let mut state = State::default();
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a], reply: 2 });
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s2", &[]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 2, results: vec![Err(ShortcutError::PermissionDenied)] },
            Effect::CloseSession("s2"),
        ]
    );
    assert!(matches!(state, State::Idle { ref committed } if committed.is_empty()));
}

#[test]
fn runtime_register_closes_old_session_and_binds_full_set() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(1, "ctrl+alt+a");
    let b = item(2, "ctrl+alt+b");

    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a.clone()], reply: 1 });
    let _ = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a"]) });

    let effects = run(&mut seq, &mut state, Event::Register { items: vec![b.clone()], reply: 2 });
    assert_eq!(
        effects,
        vec![
            Effect::CloseSession("s1"),
            Effect::Bind { attempt: Attempt(2), shortcuts: vec![a.clone(), b.clone()] },
        ]
    );

    // Activations while rebinding are ignored.
    assert!(run(&mut seq, &mut state, Event::Activated { session: "s1", portal_id: "ctrl+alt+a".into(), token: None }).is_empty());

    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(2), result: outcome("s2", &["ctrl+alt+a", "ctrl+alt+b"]) });
    assert_eq!(effects, vec![Effect::ReplyRegister { reply: 2, results: vec![Ok(())] }]);

    // Old session cannot fire; new session fires both.
    assert!(run(&mut seq, &mut state, Event::Activated { session: "s1", portal_id: "ctrl+alt+a".into(), token: None }).is_empty());
    assert_eq!(
        run(&mut seq, &mut state, Event::Activated { session: "s2", portal_id: "ctrl+alt+b".into(), token: None }),
        vec![Effect::Activate { id: ShortcutId(2), token: None }]
    );
}

#[test]
fn failed_runtime_register_restores_previous_shortcuts_without_looping() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(1, "ctrl+alt+a");
    let b = item(2, "ctrl+alt+b");

    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a.clone()], reply: 1 });
    let _ = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a"]) });

    // Runtime register of b is cancelled -> replies PermissionDenied and restores [a].
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![b.clone()], reply: 2 });
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(2), result: Err(ShortcutError::PermissionDenied) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 2, results: vec![Err(ShortcutError::PermissionDenied)] },
            Effect::Bind { attempt: Attempt(3), shortcuts: vec![a.clone()] },
        ]
    );
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(3), result: outcome("s3", &["ctrl+alt+a"]) });
    assert!(effects.is_empty());
    assert_eq!(
        run(&mut seq, &mut state, Event::Activated { session: "s3", portal_id: "ctrl+alt+a".into(), token: None }),
        vec![Effect::Activate { id: ShortcutId(1), token: None }]
    );

    // If a restore itself fails, stay Idle with committed intact and do not retry in a loop.
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![b], reply: 3 });
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(4), result: Err(ShortcutError::PermissionDenied) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 3, results: vec![Err(ShortcutError::PermissionDenied)] },
            Effect::Bind { attempt: Attempt(5), shortcuts: vec![a] },
        ]
    );
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(5), result: Err(ShortcutError::Backend("down".into())) });
    assert!(effects.is_empty(), "a failed restore must not loop");
    assert!(matches!(state, State::Idle { ref committed } if committed.len() == 1));
}

#[test]
fn unregister_rebinds_remaining_shortcuts_and_last_unregister_only_closes() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(1, "ctrl+alt+a");
    let b = item(2, "ctrl+alt+b");

    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a.clone(), b.clone()], reply: 1 });
    let _ = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a", "ctrl+alt+b"]) });

    // Unknown id does not disturb the active session.
    assert_eq!(
        run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(99), reply: 10 }),
        vec![Effect::ReplyUnregister { reply: 10, result: Err(ShortcutError::UnknownId) }]
    );

    // Unregistering one of two shortcuts closes s1 and rebinds [a].
    let effects = run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(2), reply: 11 });
    assert_eq!(
        effects,
        vec![
            Effect::CloseSession("s1"),
            Effect::Bind { attempt: Attempt(2), shortcuts: vec![a.clone()] },
        ]
    );
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(2), result: outcome("s2", &["ctrl+alt+a"]) });
    assert_eq!(effects, vec![Effect::ReplyUnregister { reply: 11, result: Ok(()) }]);

    // Unregistering the last shortcut closes s2 with no BindShortcuts call.
    let effects = run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 12 });
    assert_eq!(
        effects,
        vec![
            Effect::CloseSession("s2"),
            Effect::ReplyUnregister { reply: 12, result: Ok(()) },
        ]
    );
    assert!(matches!(state, State::Idle { ref committed } if committed.is_empty()));
}

#[test]
fn registering_while_bind_is_pending_queues_and_coalesces() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(1, "ctrl+alt+a");
    let b = item(2, "ctrl+alt+b");
    let c = item(3, "ctrl+alt+c");

    let effects = run(&mut seq, &mut state, Event::Register { items: vec![a.clone()], reply: 1 });
    assert_eq!(effects, vec![Effect::Bind { attempt: Attempt(1), shortcuts: vec![a.clone()] }]);

    // Both b and c arrive while attempt 1 is still pending: no overlapping dialogs.
    assert!(run(&mut seq, &mut state, Event::Register { items: vec![b.clone()], reply: 2 }).is_empty());
    assert!(run(&mut seq, &mut state, Event::Register { items: vec![c.clone()], reply: 3 }).is_empty());

    // Once attempt 1 succeeds, b and c coalesce into a single rebind with [a, b, c].
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a"]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 1, results: vec![Ok(())] },
            Effect::CloseSession("s1"),
            Effect::Bind { attempt: Attempt(2), shortcuts: vec![a, b, c] },
        ]
    );

    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(2), result: outcome("s2", &["ctrl+alt+a", "ctrl+alt+b", "ctrl+alt+c"]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 2, results: vec![Ok(())] },
            Effect::ReplyRegister { reply: 3, results: vec![Ok(())] },
        ]
    );
    assert!(matches!(state, State::Bound { session: "s2", ref committed } if committed.len() == 3));
}

#[test]
fn unregistering_while_bind_is_pending_waits_and_cleans_up() {
    let a = item(1, "ctrl+alt+a");
    let b = item(2, "ctrl+alt+b");

    // 1. Unregistering the only in-flight shortcut when the bind succeeds closes its session with no rebind.
    let mut seq = Seq::default();
    let mut state = State::default();
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a.clone()], reply: 1 });
    assert!(run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 10 }).is_empty());
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a"]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 1, results: vec![Ok(())] },
            Effect::CloseSession("s1"),
            Effect::ReplyUnregister { reply: 10, result: Ok(()) },
        ]
    );
    assert!(matches!(state, State::Idle { ref committed } if committed.is_empty()));

    // 2. Unregistering an in-flight shortcut whose bind fails returns UnknownId.
    let mut seq = Seq::default();
    let mut state = State::default();
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a.clone()], reply: 1 });
    assert!(run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 10 }).is_empty());
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: Err(ShortcutError::PermissionDenied) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 1, results: vec![Err(ShortcutError::PermissionDenied)] },
            Effect::ReplyUnregister { reply: 10, result: Err(ShortcutError::UnknownId) },
        ]
    );

    // 3. Unregistering an already-committed shortcut while a second register is pending rebinds without it.
    let mut seq = Seq::default();
    let mut state = State::default();
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a.clone()], reply: 1 });
    let _ = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a"]) });
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![b.clone()], reply: 2 });
    assert!(run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 11 }).is_empty());

    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(2), result: outcome("s2", &["ctrl+alt+a", "ctrl+alt+b"]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 2, results: vec![Ok(())] },
            Effect::CloseSession("s2"),
            Effect::Bind { attempt: Attempt(3), shortcuts: vec![b] },
        ]
    );
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(3), result: outcome("s3", &["ctrl+alt+b"]) });
    assert_eq!(effects, vec![Effect::ReplyUnregister { reply: 11, result: Ok(()) }]);
}

#[test]
fn reregistering_while_bind_is_pending_handles_same_and_new_portal_id() {
    let a1 = named(1, "ctrl+alt+a", "toggle", "Toggle panel");
    let a2 = named(2, "ctrl+alt+x", "toggle", "Toggle panel");
    let x = item(10, "ctrl+alt+k");

    // 1. Only shortcut: Unregister(1) + Register(2, same portal_id) while attempt 1 is pending.
    let mut seq = Seq::default();
    let mut state = State::default();
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a1.clone()], reply: 1 });
    assert!(run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 10 }).is_empty());
    assert!(run(&mut seq, &mut state, Event::Register { items: vec![a2.clone()], reply: 2 }).is_empty());

    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["toggle"]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 1, results: vec![Ok(())] },
            Effect::CloseSession("s1"),
            Effect::ReplyUnregister { reply: 10, result: Ok(()) },
            Effect::Bind { attempt: Attempt(2), shortcuts: vec![a2.clone()] },
        ]
    );
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(2), result: outcome("s2", &["toggle"]) });
    assert_eq!(effects, vec![Effect::ReplyRegister { reply: 2, results: vec![Ok(())] }]);
    assert_eq!(
        run(&mut seq, &mut state, Event::Activated { session: "s2", portal_id: "toggle".into(), token: None }),
        vec![Effect::Activate { id: ShortcutId(2), token: None }]
    );

    // 2. With another shortcut present: Unregister(1) rebinds [x], then Register(2) binds [x, a2].
    let mut seq = Seq::default();
    let mut state = State::default();
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![x.clone(), a1.clone()], reply: 1 });
    assert!(run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 10 }).is_empty());
    assert!(run(&mut seq, &mut state, Event::Register { items: vec![a2.clone()], reply: 2 }).is_empty());

    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+k", "toggle"]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 1, results: vec![Ok(()), Ok(())] },
            Effect::CloseSession("s1"),
            Effect::Bind { attempt: Attempt(2), shortcuts: vec![x.clone()] },
        ]
    );
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(2), result: outcome("s2", &["ctrl+alt+k"]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyUnregister { reply: 10, result: Ok(()) },
            Effect::CloseSession("s2"),
            Effect::Bind { attempt: Attempt(3), shortcuts: vec![x, a2.clone()] },
        ]
    );
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(3), result: outcome("s3", &["ctrl+alt+k", "toggle"]) });
    assert_eq!(effects, vec![Effect::ReplyRegister { reply: 2, results: vec![Ok(())] }]);

    // 3. Re-registering the same portal_id without unregistering conflicts if attempt 1 succeeds,
    //    but succeeds if attempt 1 fails.
    let mut seq = Seq::default();
    let mut state = State::default();
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a1.clone()], reply: 1 });
    assert!(run(&mut seq, &mut state, Event::Register { items: vec![a2.clone()], reply: 2 }).is_empty());
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["toggle"]) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 1, results: vec![Ok(())] },
            Effect::ReplyRegister { reply: 2, results: vec![Err(ShortcutError::Conflict)] },
        ]
    );
    assert!(matches!(state, State::Bound { session: "s1", .. }), "conflict leaves the active session alone");

    let mut seq = Seq::default();
    let mut state = State::default();
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a1], reply: 1 });
    assert!(run(&mut seq, &mut state, Event::Register { items: vec![a2.clone()], reply: 2 }).is_empty());
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: Err(ShortcutError::PermissionDenied) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 1, results: vec![Err(ShortcutError::PermissionDenied)] },
            Effect::Bind { attempt: Attempt(2), shortcuts: vec![a2] },
        ]
    );
}

#[test]
fn operations_queued_during_restore_run_after_restore_settles() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(1, "ctrl+alt+a");
    let b = item(2, "ctrl+alt+b");
    let c = item(3, "ctrl+alt+c");

    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a.clone()], reply: 1 });
    let _ = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a"]) });

    // Runtime register of b fails -> starts Restore(attempt 3) for [a].
    let _ = run(&mut seq, &mut state, Event::Register { items: vec![b], reply: 2 });
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(2), result: Err(ShortcutError::PermissionDenied) });
    assert_eq!(
        effects,
        vec![
            Effect::ReplyRegister { reply: 2, results: vec![Err(ShortcutError::PermissionDenied)] },
            Effect::Bind { attempt: Attempt(3), shortcuts: vec![a] },
        ]
    );

    // While Restore is in flight, caller unregisters a and registers c.
    assert!(run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 10 }).is_empty());
    assert!(run(&mut seq, &mut state, Event::Register { items: vec![c.clone()], reply: 3 }).is_empty());

    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(3), result: outcome("s3", &["ctrl+alt+a"]) });
    assert_eq!(
        effects,
        vec![
            Effect::CloseSession("s3"),
            Effect::ReplyUnregister { reply: 10, result: Ok(()) },
            Effect::Bind { attempt: Attempt(4), shortcuts: vec![c] },
        ]
    );
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(4), result: outcome("s4", &["ctrl+alt+c"]) });
    assert_eq!(effects, vec![Effect::ReplyRegister { reply: 3, results: vec![Ok(())] }]);
}

#[test]
fn stale_bind_completion_is_ignored_and_closes_stray_session() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(1, "ctrl+alt+a");

    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a], reply: 1 });
    let effects = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(99), result: outcome("stray", &["ctrl+alt+a"]) });
    assert_eq!(effects, vec![Effect::CloseSession("stray")]);
    assert!(matches!(state, State::Binding { attempt: Attempt(1), .. }));

    // Stale error changes nothing.
    assert!(run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(99), result: Err(ShortcutError::PermissionDenied) }).is_empty());
    assert!(matches!(state, State::Binding { attempt: Attempt(1), .. }));
}

#[test]
fn session_closed_and_shutdown_cancel_in_flight_and_fail_all_waiters() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let a = item(1, "ctrl+alt+a");
    let b = item(2, "ctrl+alt+b");

    let _ = run(&mut seq, &mut state, Event::Register { items: vec![a.clone()], reply: 1 });
    let _ = run(&mut seq, &mut state, Event::BindFinished { attempt: Attempt(1), result: outcome("s1", &["ctrl+alt+a"]) });

    // External SessionClosed transitions to Idle while remembering committed shortcuts.
    assert!(run(&mut seq, &mut state, Event::SessionClosed { session: "wrong" }).is_empty());
    assert!(run(&mut seq, &mut state, Event::SessionClosed { session: "s1" }).is_empty());
    assert!(matches!(state, State::Idle { ref committed } if committed.len() == 1));
    assert!(run(&mut seq, &mut state, Event::Activated { session: "s1", portal_id: "ctrl+alt+a".into(), token: None }).is_empty());

    // Registering b from Idle rebinds the full set [a, b].
    let effects = run(&mut seq, &mut state, Event::Register { items: vec![b.clone()], reply: 2 });
    assert_eq!(effects, vec![Effect::Bind { attempt: Attempt(2), shortcuts: vec![a, b] }]);

    // Queue an unregister while attempt 2 is pending, then shut down.
    assert!(run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 10 }).is_empty());
    let effects = run(&mut seq, &mut state, Event::Shutdown);
    assert_eq!(
        effects,
        vec![
            Effect::CancelBind(Attempt(2)),
            Effect::ReplyRegister { reply: 2, results: vec![Err(ShortcutError::NotRunning)] },
            Effect::ReplyUnregister { reply: 10, result: Err(ShortcutError::NotRunning) },
        ]
    );
    assert!(matches!(state, State::Closed));

    // Late commands after Shutdown fail immediately with NotRunning.
    assert_eq!(
        run(&mut seq, &mut state, Event::Unregister { id: ShortcutId(1), reply: 11 }),
        vec![Effect::ReplyUnregister { reply: 11, result: Err(ShortcutError::NotRunning) }]
    );
    assert_eq!(
        run(&mut seq, &mut state, Event::Register { items: vec![item(3, "ctrl+alt+c")], reply: 3 }),
        vec![Effect::ReplyRegister { reply: 3, results: vec![Err(ShortcutError::NotRunning)] }]
    );
}
