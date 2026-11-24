use super::*;
use crate::Ime;
use crate::input::host_events::ImeStep;

fn collect(state: &mut State, event: Event) -> Vec<Effect> {
    step(state, event).into_iter().collect()
}

#[test]
fn sync_before_enter_replays_enable_content_type_and_cursor_rect_in_one_commit() {
    let mut state = State::default();
    let area = [12.4, 34.6, 2.0, 18.2];
    let rect = CursorRect { x: 12, y: 35, width: 2, height: 18 };

    // Before zwp_text_input_v3.enter, host sync records desired state without sending wire requests.
    assert_eq!(
        collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: Some(area) })),
        Vec::<Effect>::new(),
    );
    assert_eq!(state, State::Outside { desired: Desired::Enabled { cursor_rect: Some(rect) } });

    // Enter for another surface does not activate our overlay.
    assert_eq!(collect(&mut state, Event::Enter { ours: false }), Vec::<Effect>::new());

    // Enter for our surface replays enable + content_type + cursor_rectangle + a single commit.
    assert_eq!(
        collect(&mut state, Event::Enter { ours: true }),
        vec![
            Effect::Enable,
            Effect::SetContentTypeNormal,
            Effect::SetCursorRectangle(rect),
            Effect::Commit,
            Effect::Ime(Ime::Enabled),
        ],
    );

    // A duplicate cursor-area sync (e.g. after Ime::Enabled invalidates ImeSync) emits nothing.
    assert_eq!(
        collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: None, cursor_area: Some(area) })),
        Vec::<Effect>::new(),
    );

    // Moving the caret sends a single set_cursor_rectangle + commit.
    let moved = [40.0, 35.0, 2.0, 18.0];
    let moved_rect = CursorRect { x: 40, y: 35, width: 2, height: 18 };
    assert_eq!(
        collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: None, cursor_area: Some(moved) })),
        vec![Effect::SetCursorRectangle(moved_rect), Effect::Commit],
    );
}

#[test]
fn enter_before_sync_waits_for_enable_then_batches_single_commit() {
    let mut state = State::default();
    let area = [10.0, 20.0, 4.0, 16.0];
    let rect = CursorRect { x: 10, y: 20, width: 4, height: 16 };

    // Enter while IME is not allowed stays idle and emits nothing.
    assert_eq!(collect(&mut state, Event::Enter { ours: true }), Vec::<Effect>::new());
    assert_eq!(state, State::EnteredDisabled);

    // Stray events while disabled are ignored.
    assert_eq!(collect(&mut state, Event::Preedit { text: Some("a".into()), cursor_begin: 0, cursor_end: 1 }), Vec::<Effect>::new());
    assert_eq!(collect(&mut state, Event::Commit { text: Some("a".into()) }), Vec::<Effect>::new());
    assert_eq!(collect(&mut state, Event::Done), Vec::<Effect>::new());

    // Enabling batches enable + content_type + cursor_rectangle + one commit.
    assert_eq!(
        collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: Some(area) })),
        vec![
            Effect::Enable,
            Effect::SetContentTypeNormal,
            Effect::SetCursorRectangle(rect),
            Effect::Commit,
            Effect::Ime(Ime::Enabled),
        ],
    );

    // Disabling sends disable + commit + Ime::Disabled.
    assert_eq!(
        collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(false), cursor_area: None })),
        vec![Effect::Disable, Effect::Commit, Effect::Ime(Ime::Disabled)],
    );
    assert_eq!(state, State::EnteredDisabled);

    // Leaving while already disabled emits nothing (no spurious wake).
    assert_eq!(collect(&mut state, Event::Leave { ours: true }), Vec::<Effect>::new());
    assert_eq!(state, State::Outside { desired: Desired::Disabled });
}

#[test]
fn leave_while_enabled_disables_clears_pending_and_replays_on_reenter() {
    let mut state = State::default();
    let rect = CursorRect { x: 8, y: 12, width: 2, height: 16 };
    let _ = collect(&mut state, Event::Enter { ours: true });
    let _ = collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: Some([8.0, 12.0, 2.0, 16.0]) }));
    let _ = collect(&mut state, Event::Preedit { text: Some("にほん".into()), cursor_begin: 0, cursor_end: 9 });

    // Leave for another surface does not disturb our session.
    assert_eq!(collect(&mut state, Event::Leave { ours: false }), Vec::<Effect>::new());

    // Leave for our surface disables and clears pending composition, while preserving desired rect.
    assert_eq!(
        collect(&mut state, Event::Leave { ours: true }),
        vec![Effect::Disable, Effect::Commit, Effect::Ime(Ime::Disabled)],
    );

    // A stale done after leave emits nothing.
    assert_eq!(collect(&mut state, Event::Done), Vec::<Effect>::new());

    // Transient re-enter without keyboard blur replays enable + cursor rect cleanly.
    assert_eq!(
        collect(&mut state, Event::Enter { ours: true }),
        vec![
            Effect::Enable,
            Effect::SetContentTypeNormal,
            Effect::SetCursorRectangle(rect),
            Effect::Commit,
            Effect::Ime(Ime::Enabled),
        ],
    );
    // Pending preedit from before leave was dropped.
    assert_eq!(collect(&mut state, Event::Done), Vec::<Effect>::new());
}

#[test]
fn utf8_preedit_cursor_validation_handles_multibyte_hidden_and_invalid_offsets() {
    let jp = "日本語"; // 3 bytes per char -> valid boundaries: 0, 3, 6, 9
    assert_eq!(validate_preedit_cursor(jp, 0, 9), Some((0, 9)));
    assert_eq!(validate_preedit_cursor(jp, 3, 6), Some((3, 6)));
    assert_eq!(validate_preedit_cursor(jp, -1, -1), None);
    assert_eq!(validate_preedit_cursor(jp, -1, 3), None);
    assert_eq!(validate_preedit_cursor(jp, 1, 6), None); // mid-codepoint begin
    assert_eq!(validate_preedit_cursor(jp, 10, 10), None); // out of bounds begin
    assert_eq!(validate_preedit_cursor(jp, 3, 4), Some((3, 3))); // mid-codepoint end falls back to begin
    assert_eq!(validate_preedit_cursor(jp, 3, 99), Some((3, 3))); // out of bounds end falls back to begin
    assert_eq!(validate_preedit_cursor("", 0, 0), None);
}

#[test]
fn done_applies_commit_before_preedit_independently_of_arrival_order() {
    let mut state = State::default();
    let _ = collect(&mut state, Event::Enter { ours: true });
    let _ = collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: None }));

    // Order 1: preedit_string before commit_string (winit drops preedit here; spec keeps both).
    assert_eq!(collect(&mut state, Event::Preedit { text: Some("ご".into()), cursor_begin: 0, cursor_end: 3 }), Vec::<Effect>::new());
    assert_eq!(collect(&mut state, Event::Commit { text: Some("日本".into()) }), Vec::<Effect>::new());
    assert_eq!(
        collect(&mut state, Event::Done),
        vec![
            Effect::ResetCompose,
            Effect::Ime(Ime::Commit("日本".into())),
            Effect::Ime(Ime::Preedit("ご".into(), Some((0, 3)), Vec::new())),
        ],
    );

    // Order 2: commit_string before preedit_string.
    assert_eq!(collect(&mut state, Event::Commit { text: Some("語".into()) }), Vec::<Effect>::new());
    assert_eq!(collect(&mut state, Event::Preedit { text: Some("です".into()), cursor_begin: 3, cursor_end: 6 }), Vec::<Effect>::new());
    assert_eq!(
        collect(&mut state, Event::Done),
        vec![
            Effect::ResetCompose,
            Effect::Ime(Ime::Commit("語".into())),
            Effect::Ime(Ime::Preedit("です".into(), Some((3, 6)), Vec::new())),
        ],
    );
}

#[test]
fn idle_done_is_silent_and_empty_done_clears_active_preedit_once() {
    let mut state = State::default();
    let _ = collect(&mut state, Event::Enter { ours: true });
    let _ = collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: None }));

    // Resting overlay receives done (e.g. reply to enable or set_cursor_rectangle): emits nothing.
    assert_eq!(collect(&mut state, Event::Done), Vec::<Effect>::new());
    assert_eq!(collect(&mut state, Event::Preedit { text: None, cursor_begin: 0, cursor_end: 0 }), Vec::<Effect>::new());
    assert_eq!(collect(&mut state, Event::Done), Vec::<Effect>::new());

    // Start composing.
    let _ = collect(&mut state, Event::Preedit { text: Some("か".into()), cursor_begin: 0, cursor_end: 3 });
    assert_eq!(
        collect(&mut state, Event::Done),
        vec![
            Effect::ResetCompose,
            Effect::Ime(Ime::Preedit("か".into(), Some((0, 3)), Vec::new())),
        ],
    );

    // A done with neither pending preedit nor commit clears the active preedit once.
    assert_eq!(
        collect(&mut state, Event::Done),
        vec![Effect::Ime(Ime::Preedit(String::new(), None, Vec::new()))],
    );
    // Subsequent idle done emits nothing so the overlay rests.
    assert_eq!(collect(&mut state, Event::Done), Vec::<Effect>::new());

    // A commit-only done while composing replaces the preedit without an extra empty preedit.
    let _ = collect(&mut state, Event::Preedit { text: Some("か".into()), cursor_begin: 0, cursor_end: 3 });
    let _ = collect(&mut state, Event::Done);
    let _ = collect(&mut state, Event::Commit { text: Some("家".into()) });
    assert_eq!(
        collect(&mut state, Event::Done),
        vec![Effect::ResetCompose, Effect::Ime(Ime::Commit("家".into()))],
    );
    assert_eq!(collect(&mut state, Event::Done), Vec::<Effect>::new());
}

#[test]
fn cancel_reset_commits_disable_before_batched_reenable_and_clears_pending() {
    let mut state = State::default();
    let _ = collect(&mut state, Event::Enter { ours: true });
    let _ = collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: Some([4.0, 6.0, 2.0, 14.0]) }));
    let _ = collect(&mut state, Event::Preedit { text: Some("abc".into()), cursor_begin: 0, cursor_end: 3 });
    let _ = collect(&mut state, Event::Done);
    // Pending preedit queued before reset must be dropped too.
    let _ = collect(&mut state, Event::Preedit { text: Some("stale".into()), cursor_begin: 0, cursor_end: 5 });

    let next_rect = CursorRect { x: 80, y: 100, width: 2, height: 18 };
    assert_eq!(
        collect(&mut state, Event::Sync(ImeStep { cancel: true, set_allowed: Some(true), cursor_area: Some([80.0, 100.0, 2.0, 18.0]) })),
        vec![
            Effect::Disable,
            Effect::Commit,
            Effect::Enable,
            Effect::SetContentTypeNormal,
            Effect::SetCursorRectangle(next_rect),
            Effect::Commit,
        ],
    );

    // Neither stale pending preedit nor old composing flag survives reset.
    assert_eq!(collect(&mut state, Event::Done), Vec::<Effect>::new());

    // Cancel while moving to non-editable element only disables once.
    assert_eq!(
        collect(&mut state, Event::Sync(ImeStep { cancel: true, set_allowed: Some(false), cursor_area: None })),
        vec![Effect::Disable, Effect::Commit, Effect::Ime(Ime::Disabled)],
    );
}

#[test]
fn drain_disables_while_preserving_enter_and_gone_emits_no_wire_requests() {
    let mut state = State::default();
    let _ = collect(&mut state, Event::Enter { ours: true });
    let _ = collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: Some([1.0, 2.0, 3.0, 4.0]) }));
    let _ = collect(&mut state, Event::Preedit { text: Some("pending".into()), cursor_begin: 0, cursor_end: 7 });

    // Drain disables on the wire and stays EnteredDisabled so showing later still works.
    assert_eq!(collect(&mut state, Event::Drain), vec![Effect::Disable, Effect::Commit]);
    assert_eq!(state, State::EnteredDisabled);
    assert_eq!(collect(&mut state, Event::Drain), Vec::<Effect>::new());

    // Re-enabling after drain works without needing another enter event.
    assert_eq!(
        collect(&mut state, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: None })),
        vec![Effect::Enable, Effect::SetContentTypeNormal, Effect::Commit, Effect::Ime(Ime::Enabled)],
    );

    // Drain while outside clears pending desired enable.
    let mut outside = State::default();
    let _ = collect(&mut outside, Event::Sync(ImeStep { cancel: false, set_allowed: Some(true), cursor_area: Some([1.0, 2.0, 3.0, 4.0]) }));
    assert_eq!(collect(&mut outside, Event::Drain), Vec::<Effect>::new());
    assert_eq!(outside, State::Outside { desired: Desired::Disabled });

    // Connection failure / drop emits zero wire requests.
    assert_eq!(collect(&mut state, Event::Gone), Vec::<Effect>::new());
    assert_eq!(state, State::Outside { desired: Desired::Disabled });
}

#[test]
fn cursor_rect_handles_rounding_negative_sizes_and_non_finite_values() {
    assert_eq!(
        CursorRect::from_logical([10.6, -2.4, -5.0, 17.5]),
        CursorRect { x: 11, y: -2, width: 0, height: 18 },
    );
    assert_eq!(
        CursorRect::from_logical([f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::NAN]),
        CursorRect { x: 0, y: 0, width: 0, height: 0 },
    );
}
