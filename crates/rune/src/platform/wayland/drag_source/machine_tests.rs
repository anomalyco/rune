use std::path::PathBuf;

use super::machine::{Effect, Event, MAX_CONCURRENT_WRITERS, SourceAction, State, step};
use super::mime::{
    DND_ACTION_COPY, DND_ACTION_MOVE, DND_ACTION_NONE, MIME_PNG, MIME_TEXT_PLAIN, MIME_TEXT_UTF8,
    MIME_URI_LIST, assemble_offers, dnd_action_mask, encode_uri_list, offers_from_item,
};
use crate::drag_out::{DragItem, DragOutcome};

#[test]
fn uri_list_encodes_rfc2483_paths_and_roundtrips_through_drop_decoder() {
    let paths = vec![
        PathBuf::from("/home/user/My File #1?.txt"),
        PathBuf::from("/tmp/100%_done.png"),
        PathBuf::from("relative/ignored.txt"),
    ];
    let encoded = encode_uri_list(&paths);
    assert_eq!(
        std::str::from_utf8(&encoded).unwrap(),
        "file:///home/user/My%20File%20%231%3F.txt\r\nfile:///tmp/100%25_done.png\r\n"
    );
    let decoded = crate::platform::wayland::drop::uri::parse_uri_list(&encoded);
    assert_eq!(decoded, vec![paths[0].clone(), paths[1].clone()]);
}

#[test]
fn assemble_offers_preserves_custom_order_and_overrides_colliding_builtins() {
    let custom = [
        ("x-special/gnome-copied-files", b"copy\nsftp://host/file.txt".as_slice()),
        ("text/uri-list", b"sftp://host/file.txt\r\n".as_slice()),
        ("application/x-kde-cutselection", b"0".as_slice()),
    ];
    let paths = [PathBuf::from("/local/fallback.txt")];
    let offers = assemble_offers(custom, &paths, Some(b"PNGDATA"), Some("hello"));
    let mimes: Vec<&str> = offers.iter().map(|o| o.mime.as_str()).collect();
    assert_eq!(
        mimes,
        vec![
            "x-special/gnome-copied-files",
            "text/uri-list",
            "application/x-kde-cutselection",
            MIME_PNG,
            MIME_TEXT_UTF8,
            MIME_TEXT_PLAIN,
        ]
    );
    // Explicit custom text/uri-list wins over built-in paths encoding with no duplicates.
    assert_eq!(offers[1].bytes.as_ref(), b"sftp://host/file.txt\r\n");
    assert_eq!(offers[4].bytes.as_ref(), b"hello");
    assert_eq!(offers[5].bytes.as_ref(), b"hello");
}

#[test]
fn offers_from_drag_item_lets_custom_data_override_colliding_builtins_matching_macos_and_windows() {
    let item = DragItem::new()
        .file("/local/file.txt")
        .png(b"builtin-png".as_slice())
        .text("builtin-text")
        .data("x-special/gnome-copied-files", "copy
sftp://host/file.txt")
        .data("text/uri-list", "sftp://host/file.txt
")
        .data("image/png", b"override-png".as_slice())
        .data("text/plain;charset=utf-8", "override-utf8");
    let offers = offers_from_item(&item);
    let mimes: Vec<&str> = offers.iter().map(|o| o.mime.as_str()).collect();
    assert_eq!(
        mimes,
        vec![
            "x-special/gnome-copied-files",
            "text/uri-list",
            "image/png",
            "text/plain;charset=utf-8",
            "text/plain",
        ]
    );
    assert_eq!(offers[1].bytes.as_ref(), b"sftp://host/file.txt
");
    assert_eq!(offers[2].bytes.as_ref(), b"override-png");
    assert_eq!(offers[3].bytes.as_ref(), b"override-utf8");
    assert_eq!(offers[4].bytes.as_ref(), b"builtin-text");
}

#[test]
fn assemble_offers_falls_back_to_paths_for_text_when_plain_text_absent() {
    let paths = [PathBuf::from("/a/one.txt"), PathBuf::from("/b/two.txt")];
    let offers = assemble_offers([], &paths, None, None);
    assert_eq!(offers[0].mime, MIME_URI_LIST);
    assert_eq!(offers[1].mime, MIME_TEXT_UTF8);
    assert_eq!(offers[1].bytes.as_ref(), b"/a/one.txt\n/b/two.txt");
    assert_eq!(offers[2].mime, MIME_TEXT_PLAIN);
}

#[test]
fn dnd_action_mask_maps_copy_and_move_only() {
    assert_eq!(dnd_action_mask(false, false), DND_ACTION_NONE);
    assert_eq!(dnd_action_mask(true, false), DND_ACTION_COPY);
    assert_eq!(dnd_action_mask(false, true), DND_ACTION_MOVE);
    assert_eq!(dnd_action_mask(true, true), DND_ACTION_COPY | DND_ACTION_MOVE);
    assert_eq!(SourceAction::from_wayland_mask(DND_ACTION_COPY), SourceAction::Copy);
    assert_eq!(SourceAction::from_wayland_mask(DND_ACTION_MOVE), SourceAction::Move);
    assert_eq!(SourceAction::from_wayland_mask(DND_ACTION_NONE), SourceAction::None);
    assert_eq!(
        SourceAction::Copy.finished_outcome(DND_ACTION_COPY | DND_ACTION_MOVE),
        DragOutcome::Copied,
    );
    assert_eq!(
        SourceAction::Move.finished_outcome(DND_ACTION_COPY | DND_ACTION_MOVE),
        DragOutcome::Moved,
    );
    assert_eq!(SourceAction::None.finished_outcome(DND_ACTION_COPY), DragOutcome::Copied);
    assert_eq!(
        SourceAction::None.finished_outcome(DND_ACTION_COPY | DND_ACTION_MOVE),
        DragOutcome::Copied,
    );
    assert_eq!(SourceAction::None.finished_outcome(DND_ACTION_MOVE), DragOutcome::Moved);
    assert_eq!(SourceAction::None.finished_outcome(DND_ACTION_NONE), DragOutcome::Cancelled);
}

#[test]
fn v3_drag_lifecycle_drop_send_and_finish_delivers_action_outcome() {
    let mut state = State::Idle;
    let offers = assemble_offers([], &[], None, Some("hi"));
    let effects = step(&mut state, Event::Start {
        drag_id: 1, seat: 7, origin_surface: 42, serial: 99, version: 3,
        within_app: false, offers, actions_mask: DND_ACTION_COPY | DND_ACTION_MOVE, with_icon: true,
    });
    assert_eq!(state.active_seat_within_app(), Some((7, false)));
    assert!(matches!(&effects[0], Effect::StartDrag { actions_mask: Some(3), with_icon: true, .. }));
    assert_eq!(effects[1], Effect::Flush);

    assert!(step(&mut state, Event::Target { drag_id: 1, mime: Some(MIME_TEXT_UTF8.into()) }).is_empty());
    assert!(step(&mut state, Event::Action { drag_id: 1, mask: DND_ACTION_MOVE }).is_empty());
    assert!(step(&mut state, Event::DropPerformed { drag_id: 1 }).is_empty());
    assert!(matches!(state, State::Dropped(_)));

    // Send after DropPerformed still writes the payload.
    let send_fx = step(&mut state, Event::Send { drag_id: 1, mime: MIME_TEXT_UTF8.into(), fd_id: 10 });
    assert!(matches!(&send_fx[0], Effect::WritePipe { drag_id: 1, fd_id: 10, payload } if payload.as_ref() == b"hi"));

    // Unknown MIME closes fd immediately.
    let unknown_fx = step(&mut state, Event::Send { drag_id: 1, mime: "application/unknown".into(), fd_id: 11 });
    assert_eq!(unknown_fx, vec![Effect::CloseFd { fd_id: 11 }]);

    // Finished delivers Moved and tears down once.
    let end_fx = step(&mut state, Event::Finished { drag_id: 1 });
    assert_eq!(
        end_fx,
        vec![
            Effect::CancelWriters { drag_id: 1 },
            Effect::DestroySource { drag_id: 1 },
            Effect::Complete { drag_id: 1, outcome: DragOutcome::Moved },
            Effect::Flush,
        ]
    );
    assert_eq!(state, State::Idle);
    // Duplicate Finished/Cancelled after Idle are ignored.
    assert!(step(&mut state, Event::Cancelled { drag_id: 1 }).is_empty());
}

#[test]
fn v1_v2_source_skips_set_actions_and_infers_outcome_on_cancelled() {
    let mut state = State::Idle;
    let offers = assemble_offers([], &[], None, Some("v2"));
    let effects = step(&mut state, Event::Start {
        drag_id: 2, seat: 1, origin_surface: 10, serial: 5, version: 2,
        within_app: true, offers, actions_mask: DND_ACTION_COPY, with_icon: false,
    });
    assert!(matches!(&effects[0], Effect::StartDrag { actions_mask: None, .. }));
    let _ = step(&mut state, Event::Send { drag_id: 2, mime: MIME_TEXT_PLAIN.into(), fd_id: 1 });
    let end_fx = step(&mut state, Event::Cancelled { drag_id: 2 });
    assert!(end_fx.contains(&Effect::Complete { drag_id: 2, outcome: DragOutcome::Copied }));
}

#[test]
fn writer_cap_replacement_window_close_and_display_loss_cancel_cleanly() {
    let mut state = State::Idle;
    let offers = assemble_offers([], &[], None, Some("payload"));
    let _ = step(&mut state, Event::Start {
        drag_id: 10, seat: 1, origin_surface: 77, serial: 1, version: 3,
        within_app: false, offers: offers.clone(), actions_mask: DND_ACTION_COPY, with_icon: false,
    });
    for i in 0..MAX_CONCURRENT_WRITERS {
        let fx = step(&mut state, Event::Send { drag_id: 10, mime: MIME_TEXT_PLAIN.into(), fd_id: i as u64 });
        assert!(matches!(fx.as_slice(), [Effect::WritePipe { .. }]));
    }
    // Exceeding MAX_CONCURRENT_WRITERS closes the excess fd immediately.
    let excess = step(&mut state, Event::Send { drag_id: 10, mime: MIME_TEXT_PLAIN.into(), fd_id: 999 });
    assert_eq!(excess, vec![Effect::CloseFd { fd_id: 999 }]);
    // Completing one writer frees a slot.
    assert!(step(&mut state, Event::WriterFinished { drag_id: 10 }).is_empty());
    let freed = step(&mut state, Event::Send { drag_id: 10, mime: MIME_TEXT_PLAIN.into(), fd_id: 1000 });
    assert!(matches!(freed.as_slice(), [Effect::WritePipe { fd_id: 1000, .. }]));

    // Starting a new drag cancels the active drag once before starting the new one.
    let replace_fx = step(&mut state, Event::Start {
        drag_id: 11, seat: 1, origin_surface: 77, serial: 2, version: 3,
        within_app: false, offers, actions_mask: DND_ACTION_COPY, with_icon: false,
    });
    assert!(replace_fx.contains(&Effect::Complete { drag_id: 10, outcome: DragOutcome::Cancelled }));
    assert!(replace_fx.iter().any(|e| matches!(e, Effect::StartDrag { drag_id: 11, .. })));

    // Unrelated window close does not cancel drag 11.
    assert!(step(&mut state, Event::WindowClosed { surface: 999 }).is_empty());
    // Origin window close cancels drag 11 once.
    let close_fx = step(&mut state, Event::WindowClosed { surface: 77 });
    assert!(close_fx.contains(&Effect::Complete { drag_id: 11, outcome: DragOutcome::Cancelled }));
    assert_eq!(state, State::Idle);

    // DisplayLost cancels without flushing.
    let offers2 = assemble_offers([], &[], None, Some("lost"));
    let _ = step(&mut state, Event::Start {
        drag_id: 12, seat: 1, origin_surface: 77, serial: 3, version: 3,
        within_app: false, offers: offers2.clone(), actions_mask: DND_ACTION_COPY, with_icon: false,
    });
    let lost_fx = step(&mut state, Event::DisplayLost);
    assert!(lost_fx.contains(&Effect::Complete { drag_id: 12, outcome: DragOutcome::Cancelled }));
    assert!(!lost_fx.contains(&Effect::Flush));
    assert_eq!(state, State::Idle);

    // Shutdown cancels and flushes cleanly.
    let _ = step(&mut state, Event::Start {
        drag_id: 13, seat: 1, origin_surface: 77, serial: 4, version: 3,
        within_app: false, offers: offers2, actions_mask: DND_ACTION_COPY, with_icon: false,
    });
    let shut_fx = step(&mut state, Event::Shutdown);
    assert!(shut_fx.contains(&Effect::Complete { drag_id: 13, outcome: DragOutcome::Cancelled }));
    assert!(shut_fx.contains(&Effect::Flush));
    assert_eq!(state, State::Idle);
}

#[test]
fn compositor_event_order_variants_and_escape_after_hover_send_behave_per_protocol() {
    // 1. Action arrives before Target; target reads eagerly during hover; modifier flips Copy ->
    //    Move -> None (over non-accepting window) -> Copy before DropPerformed -> Send -> Finished.
    let mut state = State::Idle;
    let offers = assemble_offers([], &[], None, Some("data"));
    let _ = step(&mut state, Event::Start {
        drag_id: 20, seat: 1, origin_surface: 1, serial: 1, version: 3,
        within_app: false, offers: offers.clone(), actions_mask: DND_ACTION_COPY | DND_ACTION_MOVE, with_icon: false,
    });
    assert!(step(&mut state, Event::Action { drag_id: 20, mask: DND_ACTION_COPY }).is_empty());
    assert!(step(&mut state, Event::Target { drag_id: 20, mime: Some(MIME_TEXT_UTF8.into()) }).is_empty());
    let hover_send = step(&mut state, Event::Send { drag_id: 20, mime: MIME_TEXT_UTF8.into(), fd_id: 1 });
    assert!(matches!(hover_send.as_slice(), [Effect::WritePipe { fd_id: 1, .. }]));
    assert!(step(&mut state, Event::WriterFinished { drag_id: 20 }).is_empty());
    assert!(step(&mut state, Event::Action { drag_id: 20, mask: DND_ACTION_MOVE }).is_empty());
    assert!(step(&mut state, Event::Target { drag_id: 20, mime: None }).is_empty());
    assert!(step(&mut state, Event::Action { drag_id: 20, mask: DND_ACTION_NONE }).is_empty());
    assert!(step(&mut state, Event::Target { drag_id: 20, mime: Some(MIME_TEXT_UTF8.into()) }).is_empty());
    assert!(step(&mut state, Event::Action { drag_id: 20, mask: DND_ACTION_COPY }).is_empty());
    assert!(step(&mut state, Event::DropPerformed { drag_id: 20 }).is_empty());
    let end_fx = step(&mut state, Event::Finished { drag_id: 20 });
    assert!(end_fx.contains(&Effect::Complete { drag_id: 20, outcome: DragOutcome::Copied }));

    // 2. v3 hover-time Send followed by Escape/Cancelled MUST deliver Cancelled (not Copied).
    let _ = step(&mut state, Event::Start {
        drag_id: 21, seat: 1, origin_surface: 1, serial: 2, version: 3,
        within_app: false, offers: offers.clone(), actions_mask: DND_ACTION_COPY, with_icon: false,
    });
    let _ = step(&mut state, Event::Action { drag_id: 21, mask: DND_ACTION_COPY });
    let _ = step(&mut state, Event::Send { drag_id: 21, mime: MIME_TEXT_UTF8.into(), fd_id: 2 });
    let cancel_fx = step(&mut state, Event::Cancelled { drag_id: 21 });
    assert!(cancel_fx.contains(&Effect::Complete { drag_id: 21, outcome: DragOutcome::Cancelled }));

    // 3. v3 DropPerformed followed by target refusing (Cancelled instead of Finished) -> Cancelled.
    let _ = step(&mut state, Event::Start {
        drag_id: 22, seat: 1, origin_surface: 1, serial: 3, version: 3,
        within_app: false, offers, actions_mask: DND_ACTION_COPY, with_icon: false,
    });
    let _ = step(&mut state, Event::Action { drag_id: 22, mask: DND_ACTION_COPY });
    let _ = step(&mut state, Event::DropPerformed { drag_id: 22 });
    let drop_cancel_fx = step(&mut state, Event::Cancelled { drag_id: 22 });
    assert!(drop_cancel_fx.contains(&Effect::Complete { drag_id: 22, outcome: DragOutcome::Cancelled }));
    assert_eq!(state, State::Idle);
}

#[test]
fn v3_dnd_finished_without_action_falls_back_safely_to_offered_action() {
    // Reproduces Hyprland <= v0.57.2, which emits target -> (optional send / dnd_drop_performed)
    // -> dnd_finished without ever sending wl_data_source.action.
    let offers = assemble_offers([], &[], None, Some("payload"));

    // 1. Copy-only drag with DropPerformed -> Finished (no Action event) => Copied.
    let mut state = State::Idle;
    let _ = step(&mut state, Event::Start {
        drag_id: 30, seat: 1, origin_surface: 1, serial: 1, version: 3,
        within_app: false, offers: offers.clone(), actions_mask: DND_ACTION_COPY, with_icon: false,
    });
    let _ = step(&mut state, Event::Target { drag_id: 30, mime: Some(MIME_TEXT_UTF8.into()) });
    let _ = step(&mut state, Event::DropPerformed { drag_id: 30 });
    let fx = step(&mut state, Event::Finished { drag_id: 30 });
    assert!(fx.contains(&Effect::Complete { drag_id: 30, outcome: DragOutcome::Copied }));
    assert_eq!(state, State::Idle);

    // 2. Copy | Move drag with Target -> Send -> Finished (no DropPerformed, no Action) => Copied
    //    (never falsely reports Moved on a Copy | Move drag).
    let _ = step(&mut state, Event::Start {
        drag_id: 31, seat: 1, origin_surface: 1, serial: 2, version: 3,
        within_app: false, offers: offers.clone(),
        actions_mask: DND_ACTION_COPY | DND_ACTION_MOVE, with_icon: false,
    });
    let _ = step(&mut state, Event::Target { drag_id: 31, mime: Some(MIME_TEXT_UTF8.into()) });
    let _ = step(&mut state, Event::Send { drag_id: 31, mime: MIME_TEXT_UTF8.into(), fd_id: 1 });
    let fx = step(&mut state, Event::Finished { drag_id: 31 });
    assert!(fx.contains(&Effect::Complete { drag_id: 31, outcome: DragOutcome::Copied }));
    assert_eq!(state, State::Idle);

    // 3. Move-only drag without Action => Moved (single offered action).
    let _ = step(&mut state, Event::Start {
        drag_id: 32, seat: 1, origin_surface: 1, serial: 3, version: 3,
        within_app: false, offers: offers.clone(), actions_mask: DND_ACTION_MOVE, with_icon: false,
    });
    let _ = step(&mut state, Event::DropPerformed { drag_id: 32 });
    let fx = step(&mut state, Event::Finished { drag_id: 32 });
    assert!(fx.contains(&Effect::Complete { drag_id: 32, outcome: DragOutcome::Moved }));
    assert_eq!(state, State::Idle);

    // 4. Drag offering neither Copy nor Move (e.g. Link-only on Wayland) without Action => Cancelled.
    let _ = step(&mut state, Event::Start {
        drag_id: 33, seat: 1, origin_surface: 1, serial: 4, version: 3,
        within_app: false, offers: offers.clone(), actions_mask: DND_ACTION_NONE, with_icon: false,
    });
    let fx = step(&mut state, Event::Finished { drag_id: 33 });
    assert!(fx.contains(&Effect::Complete { drag_id: 33, outcome: DragOutcome::Cancelled }));
    assert_eq!(state, State::Idle);

    // 5. Explicit Cancelled without Action still reports Cancelled (not Copied).
    let _ = step(&mut state, Event::Start {
        drag_id: 34, seat: 1, origin_surface: 1, serial: 5, version: 3,
        within_app: false, offers: offers.clone(),
        actions_mask: DND_ACTION_COPY | DND_ACTION_MOVE, with_icon: false,
    });
    let _ = step(&mut state, Event::Target { drag_id: 34, mime: Some(MIME_TEXT_UTF8.into()) });
    let _ = step(&mut state, Event::Send { drag_id: 34, mime: MIME_TEXT_UTF8.into(), fd_id: 2 });
    let fx = step(&mut state, Event::Cancelled { drag_id: 34 });
    assert!(fx.contains(&Effect::Complete { drag_id: 34, outcome: DragOutcome::Cancelled }));
    assert_eq!(state, State::Idle);

    // 6. Explicit Action(Move) on Copy | Move drag still reports Moved on Finished.
    let _ = step(&mut state, Event::Start {
        drag_id: 35, seat: 1, origin_surface: 1, serial: 6, version: 3,
        within_app: false, offers,
        actions_mask: DND_ACTION_COPY | DND_ACTION_MOVE, with_icon: false,
    });
    let _ = step(&mut state, Event::Action { drag_id: 35, mask: DND_ACTION_MOVE });
    let _ = step(&mut state, Event::DropPerformed { drag_id: 35 });
    let fx = step(&mut state, Event::Finished { drag_id: 35 });
    assert!(fx.contains(&Effect::Complete { drag_id: 35, outcome: DragOutcome::Moved }));
    assert_eq!(state, State::Idle);
}
