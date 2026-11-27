use std::path::PathBuf;
use std::time::Duration;

use super::machine::{
    self, DND_ACTION_ASK, DND_ACTION_COPY, DND_ACTION_MOVE, DND_ACTION_NONE, DROP_READ_TIMEOUT,
    ENTER_READ_TIMEOUT, Effect, Event, MAX_PENDING_OFFERS, MAX_URI_LIST_BYTES, OfferInfo,
    OfferSet, ReadOutcome, ReadPhase, SelectedAction, State, WindowMap, parse_uri_list, pipe_read,
};
use crate::machine::{Attempt, Seq};
use crate::time::Instant;
use crate::window::FileStep;

fn uri_offer(id: u32, version: u32) -> OfferInfo {
    OfferInfo { id, version, has_uri_list: true, selected_action: SelectedAction::None }
}

fn non_uri_offer(id: u32, version: u32) -> OfferInfo {
    OfferInfo { id, version, has_uri_list: false, selected_action: SelectedAction::None }
}

fn step_vec(seq: &mut Seq, state: &mut State, event: Event) -> Vec<Effect> {
    machine::step(seq, state, event).into_iter().collect()
}

#[test]
fn enter_accepts_uri_list_copy_only_and_starts_eager_read() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let effects = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 7,
        surface: 10,
        target: Some(1),
        x: 42.5,
        y: 18.0,
        offer: Some(uri_offer(99, 3)),
    });
    assert_eq!(effects, vec![
        Effect::AcceptOffer { offer: 99, serial: 7, mime: Some("text/uri-list") },
        Effect::SetActions { offer: 99, copy: true },
        Effect::StartRead { offer: 99, attempt: Attempt(1), phase: ReadPhase::Enter },
        Effect::Flush,
    ]);
    assert!(matches!(state, State::Fetching { offer: 99, pos: (42.5, 18.0), attempt: Attempt(1), .. }));

    // On protocol v2, set_actions is not sent.
    let mut state_v2 = State::Idle;
    let effects_v2 = step_vec(&mut seq, &mut state_v2, Event::Enter {
        serial: 8,
        surface: 10,
        target: Some(1),
        x: 5.0,
        y: 6.0,
        offer: Some(uri_offer(100, 2)),
    });
    assert_eq!(effects_v2, vec![
        Effect::AcceptOffer { offer: 100, serial: 8, mime: Some("text/uri-list") },
        Effect::StartRead { offer: 100, attempt: Attempt(2), phase: ReadPhase::Enter },
        Effect::Flush,
    ]);
}

#[test]
fn enter_motion_drop_delivers_latest_position_and_finishes_v3_offer() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface: 10,
        target: Some(1),
        x: 10.0,
        y: 20.0,
        offer: Some(uri_offer(50, 3)),
    });
    // Action arrives asynchronously after set_actions.
    assert!(step_vec(&mut seq, &mut state, Event::OfferAction { offer: 50, action: SelectedAction::Copy }).is_empty());
    // Motion during fetch updates the point without emitting Move before Enter.
    assert!(step_vec(&mut seq, &mut state, Event::Motion { x: 30.0, y: 40.0 }).is_empty());

    let effects = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1),
        outcome: ReadOutcome::Bytes(b"file:///tmp/photo.png\r\n".to_vec()),
    });
    assert_eq!(effects, vec![Effect::EnqueueStep {
        surface: 10,
        token: 1,
        step: FileStep::Enter(vec![PathBuf::from("/tmp/photo.png")], Some((30.0, 40.0))),
    }]);

    let effects = step_vec(&mut seq, &mut state, Event::Motion { x: 64.0, y: 72.0 });
    assert_eq!(effects, vec![Effect::EnqueueStep {
        surface: 10,
        token: 1,
        step: FileStep::Move((64.0, 72.0)),
    }]);

    let effects = step_vec(&mut seq, &mut state, Event::Drop);
    assert_eq!(effects, vec![
        Effect::EnqueueStep {
            surface: 10,
            token: 1,
            step: FileStep::Drop(vec![PathBuf::from("/tmp/photo.png")], Some((64.0, 72.0))),
        },
        Effect::FinishOffer(50),
        Effect::DestroyOffer(50),
        Effect::Flush,
    ]);
    assert_eq!(state, State::Idle);

    // Immediate post-drop leave must not emit a spurious FileStep::Leave.
    assert!(step_vec(&mut seq, &mut state, Event::Leave).is_empty());
}

#[test]
fn ask_action_on_drop_never_sends_finish_and_dismisses_cleanly() {
    // Reporter's case (#1857): compositor selects DndAction::Ask under wl_data_offer v3.
    // Calling finish() while Ask is active is a fatal invalid_finish protocol error.
    let ask = SelectedAction::from_wayland_mask(DND_ACTION_ASK);
    assert_eq!(ask, SelectedAction::Ask);
    assert!(!ask.is_final_action());
    assert!(!ask.allows_copy_finish());

    // Hovering + Ask -> Drop must clear hover (Leave) and destroy without FinishOffer.
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1, surface: 10, target: Some(1), x: 1.0, y: 2.0, offer: Some(uri_offer(51, 3)),
    });
    let _ = step_vec(&mut seq, &mut state, Event::OfferAction { offer: 51, action: ask });
    let _ = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1), outcome: ReadOutcome::Bytes(b"file:///tmp/a.txt\n".to_vec()),
    });
    let effects = step_vec(&mut seq, &mut state, Event::Drop);
    assert_eq!(effects, vec![
        Effect::EnqueueStep { surface: 10, token: 1, step: FileStep::Leave },
        Effect::DestroyOffer(51),
        Effect::Flush,
    ]);
    assert_eq!(state, State::Idle);

    // Fetching + Ask -> Drop cancels in-flight read and destroys without FinishOffer.
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 2, surface: 10, target: Some(1), x: 1.0, y: 2.0, offer: Some(uri_offer(52, 3)),
    });
    let _ = step_vec(&mut seq, &mut state, Event::OfferAction { offer: 52, action: ask });
    let attempt = seq.current();
    assert_eq!(step_vec(&mut seq, &mut state, Event::Drop), vec![
        Effect::CancelRead(attempt),
        Effect::DestroyOffer(52),
        Effect::Flush,
    ]);
    assert_eq!(state, State::Idle);

    // Deferred + Ask -> Drop destroys immediately without a fallback read or FinishOffer.
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 3, surface: 10, target: Some(1), x: 1.0, y: 2.0, offer: Some(uri_offer(53, 3)),
    });
    let _ = step_vec(&mut seq, &mut state, Event::OfferAction { offer: 53, action: ask });
    let attempt = seq.current();
    assert!(step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt, outcome: ReadOutcome::TimedOut,
    }).is_empty());
    assert_eq!(step_vec(&mut seq, &mut state, Event::Drop), vec![
        Effect::DestroyOffer(53),
        Effect::Flush,
    ]);
    assert_eq!(state, State::Idle);
}

#[test]
fn protocol_version_and_action_matrix_table() {
    // Mask decoding and protocol-vs-copy-only finish predicates.
    for (mask, expected, is_final, copy_ok) in [
        (DND_ACTION_NONE, SelectedAction::None, false, false),
        (DND_ACTION_COPY, SelectedAction::Copy, true, true),
        (DND_ACTION_MOVE, SelectedAction::Move, true, false),
        (DND_ACTION_ASK, SelectedAction::Ask, false, false),
        (DND_ACTION_COPY | DND_ACTION_MOVE, SelectedAction::None, false, false),
        (DND_ACTION_COPY | DND_ACTION_ASK, SelectedAction::None, false, false),
        (0xff, SelectedAction::None, false, false),
    ] {
        let action = SelectedAction::from_wayland_mask(mask);
        assert_eq!(action, expected, "mask {mask:#x}");
        assert_eq!(action.is_final_action(), is_final, "is_final {mask:#x}");
        assert_eq!(action.allows_copy_finish(), copy_ok, "allows_copy_finish {mask:#x}");
    }

    // Protocol version x action x mid-drag action change between last motion and drop.
    let cases: &[(u32, Option<SelectedAction>, Option<SelectedAction>, bool, bool)] = &[
        // (version, initial_action, after_motion_action, expect_drop, expect_finish)
        (1, None, None, true, false),
        (2, None, None, true, false),
        (3, None, None, false, false),
        (3, Some(SelectedAction::None), None, false, false),
        (3, Some(SelectedAction::Copy), None, true, true),
        (3, Some(SelectedAction::Move), None, false, false),
        (3, Some(SelectedAction::Ask), None, false, false),
        (4, Some(SelectedAction::Copy), None, true, true),
        (4, Some(SelectedAction::Ask), None, false, false),
        // Action changes between last motion and drop:
        (3, Some(SelectedAction::Copy), Some(SelectedAction::Ask), false, false),
        (3, Some(SelectedAction::Copy), Some(SelectedAction::Move), false, false),
        (3, Some(SelectedAction::Copy), Some(SelectedAction::None), false, false),
        (3, Some(SelectedAction::Ask), Some(SelectedAction::Copy), true, true),
        (3, Some(SelectedAction::None), Some(SelectedAction::Copy), true, true),
    ];
    for (idx, &(version, first, after_motion, expect_drop, expect_finish)) in cases.iter().enumerate() {
        let mut seq = Seq::default();
        let mut state = State::Idle;
        let offer = 200 + idx as u32;
        let enter_eff = step_vec(&mut seq, &mut state, Event::Enter {
            serial: 1, surface: 10, target: Some(1), x: 5.0, y: 6.0, offer: Some(uri_offer(offer, version)),
        });
        assert_eq!(enter_eff.iter().any(|e| matches!(e, Effect::SetActions { copy: true, .. })), version >= 3);
        if let Some(action) = first {
            assert!(step_vec(&mut seq, &mut state, Event::OfferAction { offer, action }).is_empty());
        }
        let _ = step_vec(&mut seq, &mut state, Event::ReadComplete {
            attempt: Attempt(1), outcome: ReadOutcome::Bytes(b"file:///tmp/t.txt\n".to_vec()),
        });
        let _ = step_vec(&mut seq, &mut state, Event::Motion { x: 22.0, y: 33.0 });
        if let Some(action) = after_motion {
            assert!(step_vec(&mut seq, &mut state, Event::OfferAction { offer, action }).is_empty());
        }
        let effects = step_vec(&mut seq, &mut state, Event::Drop);
        let mut expected = Vec::new();
        if expect_drop {
            expected.push(Effect::EnqueueStep {
                surface: 10, token: 1, step: FileStep::Drop(vec![PathBuf::from("/tmp/t.txt")], Some((22.0, 33.0))),
            });
        } else {
            expected.push(Effect::EnqueueStep { surface: 10, token: 1, step: FileStep::Leave });
        }
        if expect_finish {
            expected.push(Effect::FinishOffer(offer));
        }
        expected.push(Effect::DestroyOffer(offer));
        expected.push(Effect::Flush);
        assert_eq!(effects, expected, "case #{idx}: v{version} {first:?} -> {after_motion:?}");
        assert_eq!(state, State::Idle);
    }
}

#[test]
fn leave_while_hovering_emits_leave_and_destroys_without_finish() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface: 10,
        target: Some(1),
        x: 10.0,
        y: 10.0,
        offer: Some(uri_offer(60, 3)),
    });
    let _ = step_vec(&mut seq, &mut state, Event::OfferAction { offer: 60, action: SelectedAction::Copy });
    let _ = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1),
        outcome: ReadOutcome::Bytes(b"file:///tmp/doc.pdf\n".to_vec()),
    });
    let effects = step_vec(&mut seq, &mut state, Event::Leave);
    assert_eq!(effects, vec![
        Effect::EnqueueStep { surface: 10, token: 1, step: FileStep::Leave },
        Effect::DestroyOffer(60),
        Effect::Flush,
    ]);
    assert_eq!(state, State::Idle);
}

#[test]
fn leave_while_fetching_cancels_read_and_discards_stale_reply() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface: 10,
        target: Some(1),
        x: 10.0,
        y: 10.0,
        offer: Some(uri_offer(61, 3)),
    });
    let leave = step_vec(&mut seq, &mut state, Event::Leave);
    assert_eq!(leave, vec![
        Effect::CancelRead(Attempt(1)),
        Effect::DestroyOffer(61),
        Effect::Flush,
    ]);
    assert_eq!(state, State::Idle);

    // Late worker reply from cancelled read is silently discarded.
    let stale = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1),
        outcome: ReadOutcome::Bytes(b"file:///tmp/late.txt\n".to_vec()),
    });
    assert!(stale.is_empty());
    assert_eq!(state, State::Idle);
}

#[test]
fn reenter_invalidates_previous_attempt_and_discards_stale_reply() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface: 10,
        target: Some(1),
        x: 10.0,
        y: 10.0,
        offer: Some(uri_offer(70, 3)),
    });
    let effects = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 2,
        surface: 20,
        target: Some(2),
        x: 50.0,
        y: 60.0,
        offer: Some(uri_offer(71, 3)),
    });
    assert_eq!(effects, vec![
        Effect::CancelRead(Attempt(1)),
        Effect::DestroyOffer(70),
        Effect::Flush,
        Effect::AcceptOffer { offer: 71, serial: 2, mime: Some("text/uri-list") },
        Effect::SetActions { offer: 71, copy: true },
        Effect::StartRead { offer: 71, attempt: Attempt(2), phase: ReadPhase::Enter },
        Effect::Flush,
    ]);

    // Stale reply for Attempt(1) does not disturb Attempt(2).
    assert!(step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1),
        outcome: ReadOutcome::Bytes(b"file:///tmp/stale.txt\n".to_vec()),
    }).is_empty());

    // Real reply for Attempt(2) lands on surface 20 / token 2.
    let active = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(2),
        outcome: ReadOutcome::Bytes(b"file:///tmp/fresh.txt\n".to_vec()),
    });
    assert_eq!(active, vec![Effect::EnqueueStep {
        surface: 20,
        token: 2,
        step: FileStep::Enter(vec![PathBuf::from("/tmp/fresh.txt")], Some((50.0, 60.0))),
    }]);
}

#[test]
fn drop_while_fetching_waits_ignores_post_drop_leave_and_delivers() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface: 10,
        target: Some(1),
        x: 15.0,
        y: 25.0,
        offer: Some(uri_offer(80, 3)),
    });
    let _ = step_vec(&mut seq, &mut state, Event::OfferAction { offer: 80, action: SelectedAction::Copy });
    let _ = step_vec(&mut seq, &mut state, Event::Motion { x: 88.0, y: 99.0 });

    assert!(step_vec(&mut seq, &mut state, Event::Drop).is_empty());
    // Compositor emits immediate post-drop leave while pipe is still being read.
    assert!(step_vec(&mut seq, &mut state, Event::Leave).is_empty());
    assert!(matches!(state, State::Dropping { offer: 80, attempt: Attempt(1), fallback_available: true, .. }));

    let effects = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1),
        outcome: ReadOutcome::Bytes(b"file:///tmp/fast-drop.txt\n".to_vec()),
    });
    assert_eq!(effects, vec![
        Effect::EnqueueStep {
            surface: 10,
            token: 1,
            step: FileStep::Drop(vec![PathBuf::from("/tmp/fast-drop.txt")], Some((88.0, 99.0))),
        },
        Effect::FinishOffer(80),
        Effect::DestroyOffer(80),
        Effect::Flush,
    ]);
    assert_eq!(state, State::Idle);
}

#[test]
fn drop_while_fetching_runs_one_fallback_receive_then_gives_up_on_second_timeout() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface: 10,
        target: Some(1),
        x: 10.0,
        y: 20.0,
        offer: Some(uri_offer(81, 3)),
    });
    let _ = step_vec(&mut seq, &mut state, Event::OfferAction { offer: 81, action: SelectedAction::Copy });
    assert!(step_vec(&mut seq, &mut state, Event::Drop).is_empty());

    // Enter-read timed out: triggers one post-drop fallback receive.
    let fallback = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1),
        outcome: ReadOutcome::TimedOut,
    });
    assert_eq!(fallback, vec![
        Effect::StartRead { offer: 81, attempt: Attempt(2), phase: ReadPhase::Drop },
        Effect::Flush,
    ]);

    // If the fallback also times out, tear down cleanly without retrying forever or calling finish.
    let done = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(2),
        outcome: ReadOutcome::TimedOut,
    });
    assert_eq!(done, vec![Effect::DestroyOffer(81), Effect::Flush]);
    assert_eq!(state, State::Idle);
}

#[test]
fn deferred_source_runs_post_drop_fallback_and_delivers() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface: 10,
        target: Some(1),
        x: 5.0,
        y: 5.0,
        offer: Some(uri_offer(82, 3)),
    });
    let _ = step_vec(&mut seq, &mut state, Event::OfferAction { offer: 82, action: SelectedAction::Copy });

    // Source deferred writing on enter (0 bytes / timeout).
    assert!(step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1),
        outcome: ReadOutcome::Bytes(Vec::new()),
    }).is_empty());
    assert!(matches!(state, State::Deferred { offer: 82, .. }));

    // Motion updates stored drop point without emitting Move.
    assert!(step_vec(&mut seq, &mut state, Event::Motion { x: 120.0, y: 80.0 }).is_empty());

    let drop_effects = step_vec(&mut seq, &mut state, Event::Drop);
    assert_eq!(drop_effects, vec![
        Effect::StartRead { offer: 82, attempt: Attempt(2), phase: ReadPhase::Drop },
        Effect::Flush,
    ]);

    let done = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(2),
        outcome: ReadOutcome::Bytes(b"file:///tmp/deferred.txt\r\n".to_vec()),
    });
    assert_eq!(done, vec![
        Effect::EnqueueStep {
            surface: 10,
            token: 1,
            step: FileStep::Drop(vec![PathBuf::from("/tmp/deferred.txt")], Some((120.0, 80.0))),
        },
        Effect::FinishOffer(82),
        Effect::DestroyOffer(82),
        Effect::Flush,
    ]);
    assert_eq!(state, State::Idle);
}

#[test]
fn rejects_non_uri_list_unregistered_surface_non_file_uris_and_oversized_payload() {
    let mut seq = Seq::default();

    // Missing text/uri-list MIME type.
    let mut state = State::Idle;
    let eff = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 3,
        surface: 10,
        target: Some(1),
        x: 0.0,
        y: 0.0,
        offer: Some(non_uri_offer(90, 3)),
    });
    assert_eq!(eff, vec![
        Effect::AcceptOffer { offer: 90, serial: 3, mime: None },
        Effect::SetActions { offer: 90, copy: false },
        Effect::Flush,
    ]);
    assert_eq!(state, State::Rejected { offer: 90 });
    assert_eq!(step_vec(&mut seq, &mut state, Event::Leave), vec![Effect::DestroyOffer(90), Effect::Flush]);

    // Unregistered surface.
    let eff = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 4,
        surface: 999,
        target: None,
        x: 0.0,
        y: 0.0,
        offer: Some(uri_offer(91, 3)),
    });
    assert_eq!(eff, vec![
        Effect::AcceptOffer { offer: 91, serial: 4, mime: None },
        Effect::SetActions { offer: 91, copy: false },
        Effect::Flush,
    ]);
    assert_eq!(step_vec(&mut seq, &mut state, Event::Leave), vec![Effect::DestroyOffer(91), Effect::Flush]);

    // Payload with only non-file URIs rejects the offer.
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 5,
        surface: 10,
        target: Some(1),
        x: 0.0,
        y: 0.0,
        offer: Some(uri_offer(92, 3)),
    });
    let cur = seq.current();
    let eff = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: cur,
        outcome: ReadOutcome::Bytes(b"https://example.com/file.png\r\nfile://remote.example/a.txt\r\n".to_vec()),
    });
    assert_eq!(eff, vec![
        Effect::AcceptOffer { offer: 92, serial: 5, mime: None },
        Effect::SetActions { offer: 92, copy: false },
        Effect::Flush,
    ]);
    assert_eq!(state, State::Rejected { offer: 92 });

    // Oversized payload rejects the offer.
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 6,
        surface: 10,
        target: Some(1),
        x: 0.0,
        y: 0.0,
        offer: Some(uri_offer(93, 3)),
    });
    let cur = seq.current();
    let eff = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: cur,
        outcome: ReadOutcome::TooLarge,
    });
    assert_eq!(eff, vec![
        Effect::AcceptOffer { offer: 93, serial: 6, mime: None },
        Effect::SetActions { offer: 93, copy: false },
        Effect::Flush,
    ]);
    assert_eq!(state, State::Rejected { offer: 93 });
}

#[test]
fn unregister_window_cancels_matching_drag_only() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface: 10,
        target: Some(2),
        x: 10.0,
        y: 10.0,
        offer: Some(uri_offer(95, 3)),
    });
    // Old token on same surface ID does nothing.
    assert!(step_vec(&mut seq, &mut state, Event::UnregisterWindow { surface: 10, token: 1 }).is_empty());
    // Other surface does nothing.
    assert!(step_vec(&mut seq, &mut state, Event::UnregisterWindow { surface: 20, token: 2 }).is_empty());
    // Matching (surface, token) while Fetching cancels read and rejects without mid-drag destroy;
    // DestroyOffer is deferred until Leave/Drop.
    assert_eq!(
        step_vec(&mut seq, &mut state, Event::UnregisterWindow { surface: 10, token: 2 }),
        vec![
            Effect::CancelRead(Attempt(1)),
            Effect::AcceptOffer { offer: 95, serial: 1, mime: None },
            Effect::SetActions { offer: 95, copy: false },
            Effect::Flush,
        ],
    );
    assert_eq!(state, State::Rejected { offer: 95 });
    assert_eq!(step_vec(&mut seq, &mut state, Event::Leave), vec![Effect::DestroyOffer(95), Effect::Flush]);
    assert_eq!(state, State::Idle);

    // Matching (surface, token) while Hovering rejects and defers DestroyOffer to Leave.
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 11, surface: 10, target: Some(2), x: 1.0, y: 2.0, offer: Some(uri_offer(97, 3)),
    });
    let cur = seq.current();
    let _ = step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: cur, outcome: ReadOutcome::Bytes(b"file:///tmp/a.txt\n".to_vec()),
    });
    assert_eq!(
        step_vec(&mut seq, &mut state, Event::UnregisterWindow { surface: 10, token: 2 }),
        vec![
            Effect::AcceptOffer { offer: 97, serial: 11, mime: None },
            Effect::SetActions { offer: 97, copy: false },
            Effect::Flush,
        ],
    );
    assert_eq!(state, State::Rejected { offer: 97 });
    assert_eq!(step_vec(&mut seq, &mut state, Event::Leave), vec![Effect::DestroyOffer(97), Effect::Flush]);

    // Seat/controller teardown (Gone) cancels reads and destroys active offers without Leave.
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 2,
        surface: 10,
        target: Some(2),
        x: 1.0,
        y: 2.0,
        offer: Some(uri_offer(96, 3)),
    });
    assert_eq!(
        step_vec(&mut seq, &mut state, Event::Gone),
        vec![Effect::CancelRead(Attempt(3)), Effect::DestroyOffer(96), Effect::Flush],
    );
    assert_eq!(state, State::Idle);
}

#[test]
fn offer_set_bounds_pending_offers() {
    let mut set = OfferSet::default();
    for i in 1..=(MAX_PENDING_OFFERS as u32) {
        assert_eq!(set.insert(i, 3), None);
    }
    assert_eq!(set.insert(999, 3), Some(1));
    set.add_mime(999, "text/uri-list");
    set.set_action(999, SelectedAction::Copy);
    assert_eq!(set.take(999), Some(OfferInfo { id: 999, version: 3, has_uri_list: true, selected_action: SelectedAction::Copy }));
    assert_eq!(set.drain_ids().len(), MAX_PENDING_OFFERS - 1);
    assert!(set.drain_ids().is_empty());
    assert_eq!(ReadPhase::Enter.timeout(), ENTER_READ_TIMEOUT);
    assert_eq!(ReadPhase::Drop.timeout(), DROP_READ_TIMEOUT);
    assert_eq!(MAX_URI_LIST_BYTES, 1024 * 1024);
}

#[test]
fn window_map_routes_per_surface_coalesces_moves_and_guards_tokens() {
    let mut map = WindowMap::default();
    let t1 = map.register(10);
    let t2 = map.register(20);
    assert_ne!(t1, t2);
    assert_eq!(map.token(10), Some(t1));
    assert_eq!(map.token(20), Some(t2));

    assert!(map.push(10, t1, FileStep::Enter(vec![PathBuf::from("/a")], Some((1.0, 2.0)))));
    assert!(map.push(10, t1, FileStep::Move((5.0, 6.0))));
    assert!(map.push(10, t1, FileStep::Move((9.0, 12.0))));
    assert!(map.push(20, t2, FileStep::Leave));

    assert_eq!(map.take(10), vec![
        FileStep::Enter(vec![PathBuf::from("/a")], Some((1.0, 2.0))),
        FileStep::Move((9.0, 12.0)),
    ]);
    assert_eq!(map.take(20), vec![FileStep::Leave]);

    // Reusing surface ID 10 issues a new token; the old token cannot push or unregister it.
    let t1_new = map.register(10);
    assert_ne!(t1, t1_new);
    assert!(!map.push(10, t1, FileStep::Leave));
    assert!(!map.unregister(10, t1));
    assert_eq!(map.token(10), Some(t1_new));
    assert!(map.unregister(10, t1_new));
    assert!(map.unregister(20, t2));
    assert!(map.is_empty());
}

#[test]
fn parse_uri_list_handles_rfc2483_encoding_and_rejects_invalid_entries() {
    let payload = concat!(
        "# RFC 2483 comment\r\n",
        "\r\n",
        "   \r\n",
        "file:///tmp/hello%20world.txt\r\n",
        "file://localhost/tmp/caf%C3%A9%231.png\n",
        "FILE:///tmp/query.rs?ignored=1#frag\r\n",
        "file://remote.example/share/foreign.txt\r\n",
        "https://example.com/not-a-file.png\r\n",
        "file:/missing-authority-slashes\r\n",
        "file:///tmp/bad%2\r\n",
        "file:///tmp/bad%ZZ.txt\r\n",
        "file:///tmp/nul%00byte.txt\r\n",
        "\0\0"
    );
    assert_eq!(parse_uri_list(payload.as_bytes()), vec![
        PathBuf::from("/tmp/hello world.txt"),
        PathBuf::from("/tmp/café#1.png"),
        PathBuf::from("/tmp/query.rs"),
    ]);
}

struct MockPipe {
    now: Instant,
    cancelled: bool,
    polls: std::collections::VecDeque<(Duration, pipe_read::Poll)>,
    reads: std::collections::VecDeque<Result<Vec<u8>, pipe_read::Read>>,
}

impl pipe_read::Driver for MockPipe {
    fn now(&mut self) -> Instant {
        self.now
    }

    fn cancelled(&self) -> bool {
        self.cancelled
    }

    fn poll(&mut self, timeout_ms: i32) -> pipe_read::Poll {
        assert!(timeout_ms > 0);
        match self.polls.pop_front() {
            Some((advance, poll)) => {
                self.now += advance;
                poll
            }
            None => pipe_read::Poll::TimedOut,
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> pipe_read::Read {
        match self.reads.pop_front() {
            Some(Ok(bytes)) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                if n < bytes.len() {
                    self.reads.push_front(Ok(bytes[n..].to_vec()));
                }
                pipe_read::Read::Bytes(n)
            }
            Some(Err(kind)) => kind,
            None => pipe_read::Read::Eof,
        }
    }
}

#[test]
fn pipe_read_timeout_ms_rounds_up_submillisecond_and_clamps() {
    assert_eq!(pipe_read::timeout_ms(Duration::from_nanos(1)), 1);
    assert_eq!(pipe_read::timeout_ms(Duration::from_nanos(999_999)), 1);
    assert_eq!(pipe_read::timeout_ms(Duration::from_millis(1)), 1);
    assert_eq!(pipe_read::timeout_ms(Duration::from_nanos(1_000_001)), 2);
    assert_eq!(pipe_read::timeout_ms(Duration::from_secs(2)), 2_000);
    assert_eq!(pipe_read::timeout_ms(Duration::MAX), i32::MAX);
}

#[test]
fn pipe_read_chunked_with_eintr_and_wouldblock() {
    let mut driver = MockPipe {
        now: crate::time::now(),
        cancelled: false,
        polls: [
            (Duration::from_millis(5), pipe_read::Poll::Interrupted),
            (Duration::from_millis(5), pipe_read::Poll::Readable),
            (Duration::from_millis(10), pipe_read::Poll::Readable),
        ].into(),
        reads: [
            Err(pipe_read::Read::Interrupted),
            Ok(b"file:///a".to_vec()),
            Err(pipe_read::Read::WouldBlock),
            Ok(b"/b.txt\n".to_vec()),
            Err(pipe_read::Read::Eof),
        ].into(),
    };
    let out = pipe_read::read_bounded(&mut driver, 64, Duration::from_millis(500));
    assert_eq!(out, Some(ReadOutcome::Bytes(b"file:///a/b.txt\n".to_vec())));
}

#[test]
fn pipe_read_times_out_silent_peer_and_bounds_size_and_cancels() {
    // Silent peer never writes: stops at deadline.
    let mut silent = MockPipe {
        now: crate::time::now(),
        cancelled: false,
        polls: [(Duration::from_millis(500), pipe_read::Poll::TimedOut)].into(),
        reads: Default::default(),
    };
    assert_eq!(
        pipe_read::read_bounded(&mut silent, 64, Duration::from_millis(500)),
        Some(ReadOutcome::TimedOut),
    );

    // Exact bound succeeds; 1 byte over returns TooLarge.
    let mut exact = MockPipe {
        now: crate::time::now(),
        cancelled: false,
        polls: [(Duration::from_millis(1), pipe_read::Poll::Readable)].into(),
        reads: [Ok(vec![b'x'; 16]), Err(pipe_read::Read::Eof)].into(),
    };
    assert_eq!(
        pipe_read::read_bounded(&mut exact, 16, Duration::from_millis(500)),
        Some(ReadOutcome::Bytes(vec![b'x'; 16])),
    );

    let mut overflow = MockPipe {
        now: crate::time::now(),
        cancelled: false,
        polls: [(Duration::from_millis(1), pipe_read::Poll::Readable)].into(),
        reads: [Ok(vec![b'x'; 16]), Ok(vec![b'y'; 1])].into(),
    };
    assert_eq!(
        pipe_read::read_bounded(&mut overflow, 16, Duration::from_millis(500)),
        Some(ReadOutcome::TooLarge),
    );

    // Cancellation aborts immediately with None.
    let mut cancelled = MockPipe {
        now: crate::time::now(),
        cancelled: false,
        polls: [(Duration::from_millis(1), pipe_read::Poll::Cancelled)].into(),
        reads: Default::default(),
    };
    assert_eq!(pipe_read::read_bounded(&mut cancelled, 64, Duration::from_millis(500)), None);

    // Poll and read errors propagate as ReadOutcome::Error.
    let mut poll_err = MockPipe {
        now: crate::time::now(),
        cancelled: false,
        polls: [(Duration::from_millis(1), pipe_read::Poll::Error)].into(),
        reads: Default::default(),
    };
    assert_eq!(
        pipe_read::read_bounded(&mut poll_err, 64, Duration::from_millis(500)),
        Some(ReadOutcome::Error),
    );
    let mut read_err = MockPipe {
        now: crate::time::now(),
        cancelled: false,
        polls: [(Duration::from_millis(1), pipe_read::Poll::Readable)].into(),
        reads: [Err(pipe_read::Read::Error)].into(),
    };
    assert_eq!(
        pipe_read::read_bounded(&mut read_err, 64, Duration::from_millis(500)),
        Some(ReadOutcome::Error),
    );
}

#[test]
fn wayland_file_steps_drive_nested_targets_drop_move_and_drop_point() {
    use crate::{Cx, El, Env, Runtime, col, el};

    #[derive(Default)]
    struct App {
        hovered: Vec<(&'static str, bool)>,
        moves: Vec<(f32, f32)>,
        dropped: Vec<(&'static str, Vec<PathBuf>, Option<(f32, f32)>)>,
    }

    fn view(_: &App, _: &mut Cx<App>) -> El {
        col().size(300.0, 200.0).child(
            el().key("outer").size(300.0, 200.0)
                .on_drop_hover(|s: &mut App, over, _| s.hovered.push(("outer", over)))
                .on_drop_files(|s: &mut App, files, tx| {
                    s.dropped.push(("outer", files.to_vec(), tx.drop_point().map(|p| (p.x, p.y))));
                })
                .child(
                    el().key("inner").absolute(20.0, 30.0).size(100.0, 80.0)
                        .on_drop_hover(|s: &mut App, over, _| s.hovered.push(("inner", over)))
                        .on_drop_move(|s: &mut App, p, _| s.moves.push((p.x, p.y)))
                        .on_drop_files(|s: &mut App, files, tx| {
                            s.dropped.push(("inner", files.to_vec(), tx.drop_point().map(|p| (p.x, p.y))));
                        }),
                ),
        )
    }

    let mut rt = Runtime::new(App::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.016);

    let mut seq = Seq::default();
    let mut state = State::Idle;
    let mut windows = WindowMap::default();
    let surface = 42;
    let token = windows.register(surface);

    let apply = |windows: &mut WindowMap, effects: Vec<Effect>| {
        for e in effects {
            if let Effect::EnqueueStep { surface, token, step } = e {
                windows.push(surface, token, step);
            }
        }
    };
    let flush = |rt: &mut Runtime<App>, windows: &mut WindowMap| {
        for step in windows.take(surface) {
            match step {
                FileStep::Enter(paths, at) => rt.drag_files(paths, at),
                FileStep::EnterWith { paths, at, mods, allowed } => rt.drag_files_with(paths, Some(at), mods, allowed),
                FileStep::Move(at) => { let _ = rt.drag_files_to(Some(at)); }
                FileStep::MoveWith { at, mods, allowed } => { let _ = rt.drag_files_to_with(Some(at), mods, allowed); }
                FileStep::Drop(paths, at) => rt.drop_files(paths, at),
                FileStep::DropWith { paths, at, mods, allowed, operation } => rt.drop_files_with(paths, Some(at), mods, allowed, operation),
                FileStep::Leave => rt.drag_files_leave(),
            }
        }
        rt.frame(0.016);
    };

    // Enter over nested inner target (20..120, 30..110) at window point (50, 70) -> local (30, 40).
    let _ = step_vec(&mut seq, &mut state, Event::Enter {
        serial: 1,
        surface,
        target: Some(token),
        x: 50.0,
        y: 70.0,
        offer: Some(uri_offer(1, 3)),
    });
    let _ = step_vec(&mut seq, &mut state, Event::OfferAction { offer: 1, action: SelectedAction::Copy });
    apply(&mut windows, step_vec(&mut seq, &mut state, Event::ReadComplete {
        attempt: Attempt(1),
        outcome: ReadOutcome::Bytes(b"file:///tmp/nested%20file.txt\r\n".to_vec()),
    }));
    flush(&mut rt, &mut windows);

    // Move within inner to (70, 90) -> local (50, 60).
    apply(&mut windows, step_vec(&mut seq, &mut state, Event::Motion { x: 60.0, y: 80.0 }));
    apply(&mut windows, step_vec(&mut seq, &mut state, Event::Motion { x: 70.0, y: 90.0 }));
    flush(&mut rt, &mut windows);

    // Drop on inner at (70, 90) -> local (50, 60).
    apply(&mut windows, step_vec(&mut seq, &mut state, Event::Drop));
    flush(&mut rt, &mut windows);

    assert_eq!(rt.state.hovered, vec![("inner", true), ("inner", false)]);
    assert_eq!(rt.state.moves, vec![(30.0, 40.0), (50.0, 60.0)]);
    assert_eq!(rt.state.dropped, vec![(
        "inner",
        vec![PathBuf::from("/tmp/nested file.txt")],
        Some((50.0, 60.0)),
    )]);
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod unix_pipe_tests {
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::time::Duration;

    use wayland_client::protocol::wl_data_device_manager::DndAction;
    use super::super::machine::{ReadOutcome, SelectedAction};
    use crate::platform::wayland::pipe::unix::{CancelToken, DoneFlag, SignalOnDrop, THREAD_JOIN_BOUND, join_bounded, read_fd_bounded};

    fn nonblocking_pair() -> (OwnedFd, UnixStream) {
        let (rx, tx) = UnixStream::pair().expect("UnixStream::pair");
        rx.set_nonblocking(true).expect("set_nonblocking");
        (OwnedFd::from(rx), tx)
    }

    #[test]
    fn unix_pipe_reads_uri_list_over_socket_pair() {
        let (rx, mut tx) = nonblocking_pair();
        let cancel = CancelToken::new().expect("CancelToken");
        tx.write_all(b"file:///tmp/one.txt\r\nfile:///tmp/two.txt\r\n").unwrap();
        drop(tx);
        let out = read_fd_bounded(rx, &cancel, 1024, Duration::from_millis(500));
        assert_eq!(out, Some(ReadOutcome::Bytes(b"file:///tmp/one.txt\r\nfile:///tmp/two.txt\r\n".to_vec())));
    }

    #[test]
    fn unix_pipe_enforces_size_bound_over_socket_pair() {
        let (rx, mut tx) = nonblocking_pair();
        let cancel = CancelToken::new().expect("CancelToken");
        tx.write_all(&[b'a'; 65]).unwrap();
        drop(tx);
        let out = read_fd_bounded(rx, &cancel, 64, Duration::from_millis(500));
        assert_eq!(out, Some(ReadOutcome::TooLarge));
    }

    #[test]
    fn unix_pipe_times_out_on_silent_peer_over_socket_pair() {
        let (rx, _tx) = nonblocking_pair();
        let cancel = CancelToken::new().expect("CancelToken");
        let start = crate::time::now();
        let out = read_fd_bounded(rx, &cancel, 1024, Duration::from_millis(40));
        assert_eq!(out, Some(ReadOutcome::TimedOut));
        assert!(start.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn unix_pipe_cancels_immediately_over_socket_pair() {
        let (rx, _tx) = nonblocking_pair();
        let cancel = Arc::new(CancelToken::new().expect("CancelToken"));
        let killer = cancel.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            killer.cancel();
        });
        let out = read_fd_bounded(rx, &cancel, 1024, Duration::from_secs(60));
        handle.join().unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn wayland_dnd_action_bits_match_selected_action() {
        assert_eq!(SelectedAction::from_wayland_mask(DndAction::None.bits()), SelectedAction::None);
        assert_eq!(SelectedAction::from_wayland_mask(DndAction::Copy.bits()), SelectedAction::Copy);
        assert_eq!(SelectedAction::from_wayland_mask(DndAction::Move.bits()), SelectedAction::Move);
        assert_eq!(SelectedAction::from_wayland_mask(DndAction::Ask.bits()), SelectedAction::Ask);
        assert_eq!(SelectedAction::from_wayland_mask((DndAction::Copy | DndAction::Move).bits()), SelectedAction::None);
        assert_eq!(SelectedAction::from_wayland_mask((DndAction::Copy | DndAction::Ask).bits()), SelectedAction::None);
    }

    #[test]
    fn unix_join_bounded_reaps_cancelled_thread_within_bound() {
        let (rx, _tx) = nonblocking_pair();
        let cancel = Arc::new(CancelToken::new().expect("CancelToken"));
        let worker_cancel = cancel.clone();
        let (done, signal_done) = DoneFlag::new();
        let handle = std::thread::spawn(move || {
            let _signal = SignalOnDrop(signal_done);
            let _ = read_fd_bounded(rx, &worker_cancel, 1024, Duration::from_secs(10));
        });
        cancel.cancel();
        assert!(join_bounded(handle, &done, THREAD_JOIN_BOUND), "cancelled worker must join within bound");
    }
}
