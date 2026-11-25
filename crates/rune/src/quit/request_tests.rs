use super::*;

fn effects(e: Effects<RequestEffect>) -> Vec<RequestEffect> {
    e.into_iter().collect()
}

#[test]
fn no_hooks_or_already_quitting_returns_now_and_stays_idle() {
    let mut s = RequestState::Idle;
    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: false, quitting: false })),
        vec![RequestEffect::ReturnNow],
    );
    assert_eq!(s, RequestState::Idle);

    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: true })),
        vec![RequestEffect::ReturnNow],
    );
    assert_eq!(s, RequestState::Idle);
}

#[test]
fn clean_quit_asks_schedules_and_replies_yes_exactly_once() {
    let mut s = RequestState::Idle;
    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false })),
        vec![RequestEffect::WakeAndReturnLater],
    );
    assert_eq!(s, RequestState::Asking);

    assert_eq!(
        effects(step(&mut s, RequestEvent::Decided { allow: true })),
        vec![RequestEffect::ScheduleReply],
    );
    assert_eq!(s, RequestState::ReplyQueued { allow: true });

    assert_eq!(
        effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })),
        vec![RequestEffect::SendReply { allow: true }],
    );
    assert_eq!(s, RequestState::Idle);

    // A stale flush after replying changes nothing.
    assert!(effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })).is_empty());
    assert_eq!(s, RequestState::Idle);
}

#[test]
fn defer_replies_no_returns_to_idle_and_allows_second_request_then_programmatic_quit() {
    let mut s = RequestState::Idle;
    // First OS quit -> Defer -> reply(NO) -> Idle.
    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false })),
        vec![RequestEffect::WakeAndReturnLater],
    );
    assert_eq!(
        effects(step(&mut s, RequestEvent::Decided { allow: false })),
        vec![RequestEffect::ScheduleReply],
    );
    assert_eq!(
        effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })),
        vec![RequestEffect::SendReply { allow: false }],
    );
    assert_eq!(s, RequestState::Idle);

    // Second OS quit -> Defer again.
    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false })),
        vec![RequestEffect::WakeAndReturnLater],
    );
    assert_eq!(
        effects(step(&mut s, RequestEvent::Decided { allow: false })),
        vec![RequestEffect::ScheduleReply],
    );
    assert_eq!(
        effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })),
        vec![RequestEffect::SendReply { allow: false }],
    );
    assert_eq!(s, RequestState::Idle);

    // Later programmatic quit while Idle does not suppress exit or schedule a native reply.
    assert!(effects(step(&mut s, RequestEvent::ProgrammaticQuit)).is_empty());
    assert_eq!(s, RequestState::Idle);
}

#[test]
fn duplicate_os_quit_while_pending_coalesces_without_double_wake_or_double_reply() {
    let mut s = RequestState::Idle;
    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false })),
        vec![RequestEffect::WakeAndReturnLater],
    );
    // Duplicate while Asking.
    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false })),
        vec![RequestEffect::ReturnLater],
    );
    assert_eq!(s, RequestState::Asking);

    // Duplicate while ReplyQueued.
    assert_eq!(
        effects(step(&mut s, RequestEvent::Decided { allow: false })),
        vec![RequestEffect::ScheduleReply],
    );
    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false })),
        vec![RequestEffect::ReturnLater],
    );
    assert_eq!(s, RequestState::ReplyQueued { allow: false });

    // Duplicate while WaitingForIdle.
    assert!(effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Handling })).is_empty());
    assert_eq!(s, RequestState::WaitingForIdle { allow: false });
    assert_eq!(
        effects(step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false })),
        vec![RequestEffect::ReturnLater],
    );
    assert_eq!(s, RequestState::WaitingForIdle { allow: false });
}

#[test]
fn flush_while_handling_defers_until_outermost_handling_drops_and_replies_once() {
    let mut s = RequestState::Idle;
    let _ = step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false });
    let _ = step(&mut s, RequestEvent::Decided { allow: true });

    // Block fires while winit handler is active: transitions to WaitingForIdle without replying.
    assert!(effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Handling })).is_empty());
    assert_eq!(s, RequestState::WaitingForIdle { allow: true });

    // A stale flush while WaitingForIdle changes nothing.
    assert!(effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })).is_empty());
    assert_eq!(s, RequestState::WaitingForIdle { allow: true });

    // Outermost Handling guard drops -> re-schedules common-mode block.
    assert_eq!(
        effects(step(&mut s, RequestEvent::LoopIdle)),
        vec![RequestEffect::ScheduleReply],
    );
    assert_eq!(s, RequestState::ReplyQueued { allow: true });

    // Second LoopIdle while already ReplyQueued does not double-schedule.
    assert!(effects(step(&mut s, RequestEvent::LoopIdle)).is_empty());

    // Now block runs while Idle -> replies YES once.
    assert_eq!(
        effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })),
        vec![RequestEffect::SendReply { allow: true }],
    );
    assert_eq!(s, RequestState::Idle);
}

#[test]
fn programmatic_quit_overrides_asking_reply_queued_and_waiting_for_idle() {
    // 1. Tx::quit() called inside on_quit_request (while Asking), before Decided(false).
    let mut s = RequestState::Idle;
    let _ = step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false });
    assert_eq!(
        effects(step(&mut s, RequestEvent::ProgrammaticQuit)),
        vec![RequestEffect::ScheduleReply, RequestEffect::SuppressExit],
    );
    assert_eq!(s, RequestState::ReplyQueued { allow: true });
    // Subsequent Decided(false) from the hook returning Defer cannot downgrade or double-schedule.
    assert!(effects(step(&mut s, RequestEvent::Decided { allow: false })).is_empty());
    assert_eq!(s, RequestState::ReplyQueued { allow: true });
    assert_eq!(
        effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })),
        vec![RequestEffect::SendReply { allow: true }],
    );
    assert_eq!(s, RequestState::Idle);

    // 2. ProgrammaticQuit / last-window closing while ReplyQueued { allow: false }.
    let _ = step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false });
    let _ = step(&mut s, RequestEvent::Decided { allow: false });
    assert_eq!(
        effects(step(&mut s, RequestEvent::ProgrammaticQuit)),
        vec![RequestEffect::SuppressExit],
    );
    assert_eq!(s, RequestState::ReplyQueued { allow: true });
    assert_eq!(
        effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })),
        vec![RequestEffect::SendReply { allow: true }],
    );
    assert_eq!(s, RequestState::Idle);

    // 3. ProgrammaticQuit while WaitingForIdle { allow: false }.
    let _ = step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false });
    let _ = step(&mut s, RequestEvent::Decided { allow: false });
    let _ = step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Handling });
    assert_eq!(
        effects(step(&mut s, RequestEvent::ProgrammaticQuit)),
        vec![RequestEffect::SuppressExit],
    );
    assert_eq!(s, RequestState::WaitingForIdle { allow: true });
    assert_eq!(
        effects(step(&mut s, RequestEvent::LoopIdle)),
        vec![RequestEffect::ScheduleReply],
    );
    assert_eq!(
        effects(step(&mut s, RequestEvent::FlushReply { loop_state: Loop::Idle })),
        vec![RequestEffect::SendReply { allow: true }],
    );
    assert_eq!(s, RequestState::Idle);
}

#[test]
fn wake_failed_falls_back_to_terminate_now_and_resets_to_idle() {
    let mut s = RequestState::Idle;
    let _ = step(&mut s, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false });
    assert_eq!(s, RequestState::Asking);
    assert_eq!(
        effects(step(&mut s, RequestEvent::WakeFailed)),
        vec![RequestEffect::ReturnNow],
    );
    assert_eq!(s, RequestState::Idle);
    // WakeFailed in Idle is a no-op.
    assert!(effects(step(&mut s, RequestEvent::WakeFailed)).is_empty());
}
