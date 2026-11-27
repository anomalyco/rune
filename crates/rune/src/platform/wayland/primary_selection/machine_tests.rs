use std::sync::Arc;

use super::{
    Availability, Effect, Event, MAX_CONCURRENT_WRITERS, MAX_PENDING_OFFERS, MAX_PRIMARY_BYTES,
    State, TextEncoding, decode_text, is_offered_mime, mime_priority, select_best_mime, step,
};
use crate::platform::wayland::pipe::ReadOutcome;

fn state_with_seat(seat: u32) -> State {
    let mut s = State::new(Availability::Available);
    assert_eq!(step(&mut s, Event::SeatAdded { seat }), vec![Effect::GetDevice { seat }]);
    s
}

#[test]
fn idle_invariant_plain_window_and_incoming_offers_never_request_wake_on_read() {
    let mut s = state_with_seat(1);
    assert!(!s.needs_wake_on_read(), "fresh available controller must not wake on read");

    // Incoming offers from external apps must never toggle wake_on_read.
    assert!(step(&mut s, Event::DataOffer { seat: 1, offer_id: 10 }).is_empty());
    assert!(step(&mut s, Event::OfferMime { offer_id: 10, mime_type: "text/plain;charset=utf-8".into() }).is_empty());
    assert!(step(&mut s, Event::Selection { seat: 1, offer_id: Some(10) }).is_empty());
    assert!(!s.needs_wake_on_read(), "observing external offers must not toggle wake_on_read");

    // Acquiring ownership enables wake_on_read so Send/Cancelled wake an idle unfocused app.
    let effects = step(&mut s, Event::RequestWrite { text: "owned".into(), serial: Some((1, 42)) });
    assert_eq!(effects, vec![Effect::SetSelection { seat: 1, source_id: 1, serial: 42 }]);
    assert!(s.needs_wake_on_read());

    // Losing ownership returns immediately to zero idle wakeups.
    let cancel_effects = step(&mut s, Event::SourceCancelled { source_id: 1 });
    assert_eq!(cancel_effects, vec![Effect::DestroySource { source_id: 1 }, Effect::SelectionLost]);
    assert!(!s.needs_wake_on_read());

    let unavail = State::new(Availability::Unavailable);
    assert!(!unavail.needs_wake_on_read());
}

#[test]
fn mutter_none_then_some_replacement_and_hyprland_pointer_enter_delivery() {
    let mut s = state_with_seat(1);

    // Initial external offer 10.
    step(&mut s, Event::DataOffer { seat: 1, offer_id: 10 });
    step(&mut s, Event::OfferMime { offer_id: 10, mime_type: "UTF8_STRING".into() });
    assert!(step(&mut s, Event::Selection { seat: 1, offer_id: Some(10) }).is_empty());

    // Mutter replacement sequence: selection(None) destroys offer 10, then DataOffer(11) + Offer + selection(Some(11)).
    assert_eq!(
        step(&mut s, Event::Selection { seat: 1, offer_id: None }),
        vec![Effect::DestroyOffer { offer_id: 10 }]
    );
    assert!(step(&mut s, Event::RequestRead { target: "f".into(), preferred_seat: Some(1) }).is_empty());

    // Hyprland delivers new offer right before read (e.g. on wl_pointer.enter).
    step(&mut s, Event::DataOffer { seat: 1, offer_id: 11 });
    step(&mut s, Event::OfferMime { offer_id: 11, mime_type: "text/plain;charset=utf-8".into() });
    assert!(step(&mut s, Event::Selection { seat: 1, offer_id: Some(11) }).is_empty());

    let effects = step(&mut s, Event::RequestRead { target: "field-a".into(), preferred_seat: Some(1) });
    assert_eq!(effects.len(), 1);
    let Effect::StartRead { attempt, offer_id, mime } = &effects[0] else {
        panic!("expected StartRead, got {effects:?}");
    };
    assert_eq!(*offer_id, 11);
    assert_eq!(mime, "text/plain;charset=utf-8");

    let delivered = step(&mut s, Event::ReadFinished {
        attempt: *attempt,
        outcome: ReadOutcome::Bytes(b"from-hyprland".to_vec()),
    });
    assert_eq!(delivered, vec![Effect::DeliverText {
        attempt: Some(*attempt),
        target: "field-a".into(),
        text: Arc::from("from-hyprland"),
    }]);

    // Direct replacement without None still destroys previous selection offer 11.
    step(&mut s, Event::DataOffer { seat: 1, offer_id: 12 });
    assert_eq!(
        step(&mut s, Event::Selection { seat: 1, offer_id: Some(12) }),
        vec![Effect::DestroyOffer { offer_id: 11 }]
    );
}

#[test]
fn self_paste_fast_path_delivers_owned_text_without_pipe_even_with_no_compositor_offer() {
    let mut s = state_with_seat(1);
    step(&mut s, Event::RequestWrite { text: "self text".into(), serial: Some((1, 7)) });

    // No DataOffer/Selection has arrived (e.g. Hyprland without pointer leave/re-enter).
    let effects = step(&mut s, Event::RequestRead { target: "box".into(), preferred_seat: Some(1) });
    assert_eq!(effects, vec![Effect::DeliverText {
        attempt: None,
        target: "box".into(),
        text: Arc::from("self text"),
    }]);
}

#[test]
fn source_replacement_ignores_stale_cancelled_and_serves_retired_source_until_cancelled() {
    let mut s = state_with_seat(1);

    // First write -> source 1.
    assert_eq!(
        step(&mut s, Event::RequestWrite { text: "first".into(), serial: Some((1, 10)) }),
        vec![Effect::SetSelection { seat: 1, source_id: 1, serial: 10 }]
    );
    assert_eq!(s.active_source_id(), Some(1));

    // Second write -> source 2; source 1 stays retired until compositor sends Cancelled(1).
    assert_eq!(
        step(&mut s, Event::RequestWrite { text: "second".into(), serial: Some((1, 11)) }),
        vec![Effect::SetSelection { seat: 1, source_id: 2, serial: 11 }]
    );
    assert_eq!(s.active_source_id(), Some(2));

    // An in-flight Send on retired source 1 still serves "first".
    assert_eq!(
        step(&mut s, Event::SourceSend { source_id: 1, mime_type: "text/plain".into(), fd_id: 100 }),
        vec![Effect::SpawnWriter { fd_id: 100, bytes: Arc::from(&b"first"[..]) }]
    );
    step(&mut s, Event::WriterFinished);

    // Out-of-order stale Cancelled(1) destroys source 1 ONLY — must NOT emit SelectionLost or
    // clear active source 2.
    assert_eq!(
        step(&mut s, Event::SourceCancelled { source_id: 1 }),
        vec![Effect::DestroySource { source_id: 1 }]
    );
    assert_eq!(s.active_source_id(), Some(2));
    assert!(s.needs_wake_on_read());

    // Self-paste still delivers "second".
    assert_eq!(
        step(&mut s, Event::RequestRead { target: "t".into(), preferred_seat: Some(1) }),
        vec![Effect::DeliverText { attempt: None, target: "t".into(), text: Arc::from("second") }]
    );

    // Active source 2 cancelled -> destroys source 2, emits SelectionLost, clears wake_on_read.
    assert_eq!(
        step(&mut s, Event::SourceCancelled { source_id: 2 }),
        vec![Effect::DestroySource { source_id: 2 }, Effect::SelectionLost]
    );
    assert_eq!(s.active_source_id(), None);
    assert!(!s.needs_wake_on_read());

    // Duplicate/unknown Cancelled is a no-op.
    assert!(step(&mut s, Event::SourceCancelled { source_id: 999 }).is_empty());
}

#[test]
fn missing_serial_unknown_seat_empty_and_oversized_writes_are_skipped_safely() {
    let mut s = state_with_seat(1);

    // Missing serial: never fabricate serial 0.
    assert!(step(&mut s, Event::RequestWrite { text: "hello".into(), serial: None }).is_empty());
    assert!(!s.needs_wake_on_read());

    // Unknown seat.
    assert!(step(&mut s, Event::RequestWrite { text: "hello".into(), serial: Some((99, 5)) }).is_empty());
    // Empty text.
    assert!(step(&mut s, Event::RequestWrite { text: String::new(), serial: Some((1, 5)) }).is_empty());
    // Over MAX_PRIMARY_BYTES.
    let huge = "a".repeat(MAX_PRIMARY_BYTES + 1);
    assert!(step(&mut s, Event::RequestWrite { text: huge, serial: Some((1, 5)) }).is_empty());
    assert!(!s.needs_wake_on_read());
}

#[test]
fn mime_priority_case_insensitive_exact_string_and_latin1_string_decode() {
    assert_eq!(mime_priority("TEXT/PLAIN; charset=\"UTF-8\""), Some((0, TextEncoding::Utf8)));
    assert_eq!(mime_priority("text/plain;charset=utf-8"), Some((0, TextEncoding::Utf8)));
    assert_eq!(mime_priority("utf8_string"), Some((1, TextEncoding::Utf8)));
    assert_eq!(mime_priority("TEXT/PLAIN"), Some((2, TextEncoding::Utf8)));
    assert_eq!(mime_priority("text"), Some((3, TextEncoding::Utf8)));
    assert_eq!(mime_priority("string"), Some((4, TextEncoding::Latin1)));
    assert_eq!(mime_priority("image/png"), None);

    assert!(is_offered_mime("TEXT/PLAIN; CHARSET=UTF-8"));
    assert!(is_offered_mime("text/plain"));
    assert!(is_offered_mime("UTF8_STRING"));
    assert!(!is_offered_mime("STRING"));
    assert!(!is_offered_mime("image/png"));

    // Preference order selects highest priority and preserves exact advertised spelling.
    let mimes = vec![
        "image/png".into(),
        "STRING".into(),
        "TEXT".into(),
        "text/plain".into(),
        "UTF8_STRING".into(),
        "TEXT/PLAIN; CHARSET=UTF-8".into(),
    ];
    assert_eq!(select_best_mime(&mimes), Some(("TEXT/PLAIN; CHARSET=UTF-8", TextEncoding::Utf8)));

    // Latin-1 STRING decoding converts 0x80..=0xFF bytes to Unicode code points.
    assert_eq!(decode_text(vec![0x63, 0x61, 0x66, 0xE9, 0x20, 0xA3], TextEncoding::Latin1).as_deref(), Some("café £"));
    assert_eq!(decode_text(Vec::new(), TextEncoding::Utf8), None);

    // End-to-end state machine read of a STRING-only offer decodes Latin-1 and passes exact "String".
    let mut s = state_with_seat(1);
    step(&mut s, Event::DataOffer { seat: 1, offer_id: 20 });
    step(&mut s, Event::OfferMime { offer_id: 20, mime_type: "image/png".into() });
    step(&mut s, Event::OfferMime { offer_id: 20, mime_type: "String".into() });
    step(&mut s, Event::Selection { seat: 1, offer_id: Some(20) });

    let effects = step(&mut s, Event::RequestRead { target: "t".into(), preferred_seat: Some(1) });
    let Effect::StartRead { attempt, offer_id, mime } = &effects[0] else { panic!("expected StartRead") };
    assert_eq!(*offer_id, 20);
    assert_eq!(mime, "String");
    let out = step(&mut s, Event::ReadFinished {
        attempt: *attempt,
        outcome: ReadOutcome::Bytes(vec![0x6E, 0x61, 0xEF, 0x76, 0x65]),
    });
    assert_eq!(out, vec![Effect::DeliverText {
        attempt: Some(*attempt),
        target: "t".into(),
        text: Arc::from("naïve"),
    }]);

    // Non-text offer is a clean no-op.
    step(&mut s, Event::DataOffer { seat: 1, offer_id: 21 });
    step(&mut s, Event::OfferMime { offer_id: 21, mime_type: "image/png".into() });
    step(&mut s, Event::Selection { seat: 1, offer_id: Some(21) });
    assert!(step(&mut s, Event::RequestRead { target: "t".into(), preferred_seat: Some(1) }).is_empty());
}

#[test]
fn pending_offer_eviction_and_read_outcomes_and_latest_wins_cancellation() {
    let mut s = state_with_seat(1);

    // Fill MAX_PENDING_OFFERS without Selection; offer 17 evicts offer 1.
    for id in 1..=(MAX_PENDING_OFFERS as u32) {
        assert!(step(&mut s, Event::DataOffer { seat: 1, offer_id: id }).is_empty());
    }
    assert_eq!(
        step(&mut s, Event::DataOffer { seat: 1, offer_id: (MAX_PENDING_OFFERS as u32) + 1 }),
        vec![Effect::DestroyOffer { offer_id: 1 }]
    );

    // Select latest offer and test TooLarge / TimedOut / latest-wins cancellation.
    let offer_id = (MAX_PENDING_OFFERS as u32) + 1;
    step(&mut s, Event::OfferMime { offer_id, mime_type: "text/plain".into() });
    step(&mut s, Event::Selection { seat: 1, offer_id: Some(offer_id) });

    let r1 = step(&mut s, Event::RequestRead { target: "a".into(), preferred_seat: Some(1) });
    let Effect::StartRead { attempt: att1, .. } = r1[0] else { panic!("expected StartRead") };
    assert!(step(&mut s, Event::ReadFinished { attempt: att1, outcome: ReadOutcome::TooLarge }).is_empty());

    let r2 = step(&mut s, Event::RequestRead { target: "a".into(), preferred_seat: Some(1) });
    let Effect::StartRead { attempt: att2, .. } = r2[0] else { panic!("expected StartRead") };
    assert!(step(&mut s, Event::ReadFinished { attempt: att2, outcome: ReadOutcome::TimedOut }).is_empty());

    // Newer RequestRead cancels stalled earlier read; stale ReadFinished is ignored.
    let r3 = step(&mut s, Event::RequestRead { target: "first".into(), preferred_seat: Some(1) });
    let Effect::StartRead { attempt: att3, .. } = r3[0] else { panic!("expected StartRead") };

    let r4 = step(&mut s, Event::RequestRead { target: "second".into(), preferred_seat: Some(1) });
    assert_eq!(r4.len(), 2);
    assert_eq!(r4[0], Effect::CancelRead { attempt: att3 });
    let Effect::StartRead { attempt: att4, .. } = r4[1] else { panic!("expected StartRead") };

    assert!(step(&mut s, Event::ReadFinished {
        attempt: att3,
        outcome: ReadOutcome::Bytes(b"stale".to_vec()),
    }).is_empty());
    assert_eq!(
        step(&mut s, Event::ReadFinished { attempt: att4, outcome: ReadOutcome::Bytes(b"fresh".to_vec()) }),
        vec![Effect::DeliverText { attempt: Some(att4), target: "second".into(), text: Arc::from("fresh") }]
    );
}

#[test]
fn writer_cap_unknown_mime_and_lifecycle_teardown() {
    let mut s = state_with_seat(1);
    step(&mut s, Event::RequestWrite { text: "payload".into(), serial: Some((1, 50)) });

    // Unknown MIME closes fd immediately without consuming a writer slot.
    assert_eq!(
        step(&mut s, Event::SourceSend { source_id: 1, mime_type: "image/png".into(), fd_id: 1 }),
        vec![Effect::CloseFd { fd_id: 1 }]
    );
    assert_eq!(s.active_writers(), 0);

    // Fill all MAX_CONCURRENT_WRITERS slots; all share the same Arc<[u8]>.
    let mut first_ptr: Option<*const u8> = None;
    for i in 0..MAX_CONCURRENT_WRITERS {
        let effects = step(&mut s, Event::SourceSend {
            source_id: 1,
            mime_type: "text/plain;charset=utf-8".into(),
            fd_id: (i as u64) + 10,
        });
        let Effect::SpawnWriter { bytes, .. } = &effects[0] else { panic!("expected SpawnWriter") };
        let ptr = bytes.as_ptr();
        if let Some(expected) = first_ptr {
            assert_eq!(ptr, expected, "concurrent writers must share payload bytes via Arc<[u8]>");
        } else {
            first_ptr = Some(ptr);
        }
    }
    assert_eq!(s.active_writers(), MAX_CONCURRENT_WRITERS);

    // Excess send closes fd immediately.
    assert_eq!(
        step(&mut s, Event::SourceSend { source_id: 1, mime_type: "text/plain".into(), fd_id: 99 }),
        vec![Effect::CloseFd { fd_id: 99 }]
    );

    // Finishing one writer frees a slot.
    step(&mut s, Event::WriterFinished);
    assert_eq!(s.active_writers(), MAX_CONCURRENT_WRITERS - 1);
    assert!(matches!(
        step(&mut s, Event::SourceSend { source_id: 1, mime_type: "UTF8_STRING".into(), fd_id: 100 })[0],
        Effect::SpawnWriter { fd_id: 100, .. }
    ));

    // SeatRemoved cleans up pending/selection offers, device, active source, and emits SelectionLost.
    step(&mut s, Event::DataOffer { seat: 1, offer_id: 30 });
    step(&mut s, Event::DataOffer { seat: 1, offer_id: 31 });
    step(&mut s, Event::Selection { seat: 1, offer_id: Some(30) });
    let rm = step(&mut s, Event::SeatRemoved { seat: 1 });
    assert!(rm.contains(&Effect::DestroyOffer { offer_id: 31 }));
    assert!(rm.contains(&Effect::DestroyOffer { offer_id: 30 }));
    assert!(rm.contains(&Effect::DestroyDevice { seat: 1 }));
    assert!(rm.contains(&Effect::DestroySource { source_id: 1 }));
    assert!(rm.contains(&Effect::SelectionLost));
    assert!(!s.needs_wake_on_read());

    // Shutdown / DisplayLost tears down remaining seat, active read, and sources cleanly.
    step(&mut s, Event::SeatAdded { seat: 2 });
    step(&mut s, Event::DataOffer { seat: 2, offer_id: 40 });
    step(&mut s, Event::OfferMime { offer_id: 40, mime_type: "text/plain".into() });
    step(&mut s, Event::Selection { seat: 2, offer_id: Some(40) });
    let r = step(&mut s, Event::RequestRead { target: "x".into(), preferred_seat: Some(2) });
    let Effect::StartRead { attempt, .. } = r[0] else { panic!("expected StartRead") };
    step(&mut s, Event::RequestWrite { text: "bye".into(), serial: Some((2, 60)) });

    let sd = step(&mut s, Event::Shutdown);
    assert_eq!(s.availability(), Availability::Dead);
    assert!(!s.needs_wake_on_read());
    assert!(sd.contains(&Effect::CancelRead { attempt }));
    assert!(sd.contains(&Effect::DestroyOffer { offer_id: 40 }));
    assert!(sd.contains(&Effect::DestroyDevice { seat: 2 }));
    assert!(sd.contains(&Effect::DestroySource { source_id: 2 }));

    // DisplayLost tears down identically and marks Dead.
    let mut s2 = state_with_seat(3);
    step(&mut s2, Event::RequestWrite { text: "lost".into(), serial: Some((3, 7)) });
    let dl = step(&mut s2, Event::DisplayLost);
    assert_eq!(s2.availability(), Availability::Dead);
    assert!(!s2.needs_wake_on_read());
    assert!(dl.contains(&Effect::DestroyDevice { seat: 3 }));
    assert!(dl.contains(&Effect::DestroySource { source_id: 1 }));
    assert_eq!(super::OFFERED_MIMES, ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"]);
    assert_eq!(super::PRIMARY_READ_TIMEOUT, std::time::Duration::from_millis(2_000));
}

#[cfg(unix)]
mod unix_pipe_integration_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::super::MAX_PRIMARY_BYTES;
    use crate::platform::wayland::pipe::ReadOutcome;
    use crate::platform::wayland::pipe::unix::{CancelToken, make_pipe, read_fd_bounded};
    use crate::platform::wayland::pipe_write::{PIPE_WRITE_TIMEOUT, WriteOutcome};
    use crate::platform::wayland::pipe_write::unix::{WriteCancel, write_fd_bounded};

    #[test]
    fn real_pipe_round_trip_over_64kib_and_4mib_too_large_and_timeout_and_cancel_and_epipe() {
        // 1. >64 KiB (256 KiB) round-trip through shared pipe_write + pipe reader.
        let (read_fd, write_fd) = make_pipe().expect("make_pipe");
        let payload: Arc<[u8]> = Arc::from(vec![b'R'; 256 * 1024]);
        let w_cancel = Arc::new(WriteCancel::new().expect("WriteCancel"));
        let r_cancel = CancelToken::new().expect("CancelToken");
        let w_bytes = Arc::clone(&payload);
        let wc = Arc::clone(&w_cancel);
        let writer = std::thread::spawn(move || {
            write_fd_bounded(write_fd, &w_bytes, &wc, MAX_PRIMARY_BYTES, PIPE_WRITE_TIMEOUT)
        });
        let read_out = read_fd_bounded(read_fd, &r_cancel, MAX_PRIMARY_BYTES, Duration::from_secs(5));
        assert_eq!(writer.join().unwrap(), WriteOutcome::Complete);
        assert_eq!(read_out, Some(ReadOutcome::Bytes(payload.to_vec())));

        // 2. >4 MiB triggers ReadOutcome::TooLarge cleanly.
        let (read_fd, write_fd) = make_pipe().expect("make_pipe");
        let oversize: Arc<[u8]> = Arc::from(vec![b'X'; MAX_PRIMARY_BYTES + 128]);
        let w_cancel = Arc::new(WriteCancel::new().expect("WriteCancel"));
        let r_cancel = CancelToken::new().expect("CancelToken");
        let wc = Arc::clone(&w_cancel);
        let writer = std::thread::spawn(move || {
            write_fd_bounded(write_fd, &oversize, &wc, MAX_PRIMARY_BYTES + 1024, PIPE_WRITE_TIMEOUT)
        });
        let read_out = read_fd_bounded(read_fd, &r_cancel, MAX_PRIMARY_BYTES, Duration::from_secs(5));
        assert_eq!(read_out, Some(ReadOutcome::TooLarge));
        // Reader closed early -> writer observes PeerClosed (EPIPE without SIGPIPE).
        assert_eq!(writer.join().unwrap(), WriteOutcome::PeerClosed);

        // 3. Stalled peer times out.
        let (read_fd, _held_write_fd) = make_pipe().expect("make_pipe");
        let r_cancel = CancelToken::new().expect("CancelToken");
        assert_eq!(
            read_fd_bounded(read_fd, &r_cancel, MAX_PRIMARY_BYTES, Duration::from_millis(40)),
            Some(ReadOutcome::TimedOut)
        );

        // 4. CancelToken aborts a blocked reader promptly.
        let (read_fd, _held_write_fd) = make_pipe().expect("make_pipe");
        let r_cancel = Arc::new(CancelToken::new().expect("CancelToken"));
        let rc = Arc::clone(&r_cancel);
        let t0 = std::time::Instant::now();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            rc.cancel();
        });
        assert_eq!(read_fd_bounded(read_fd, &r_cancel, MAX_PRIMARY_BYTES, Duration::from_secs(30)), None);
        canceller.join().unwrap();
        assert!(t0.elapsed() < Duration::from_secs(2));
    }
}
