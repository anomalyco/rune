use super::*;

#[test]
fn oversized_state_machine_transition_table() {
    let last = OVERSIZED_IDLE_FLUSHES - 1;
    let cases = [
        // Cold
        (State::Cold, Event::OversizedWrite { poolable: true }, State::SeenOnce { idle_flushes: 0 }, Some(Effect::Chunk(Disposition::OneShot))),
        (State::Cold, Event::OversizedWrite { poolable: false }, State::Cold, Some(Effect::Chunk(Disposition::OneShot))),
        (State::Cold, Event::Flush { had_oversized: false }, State::Cold, None),
        (State::Cold, Event::Flush { had_oversized: true }, State::Cold, None),
        (State::Cold, Event::Trim, State::Cold, None),
        // SeenOnce
        (State::SeenOnce { idle_flushes: 0 }, Event::OversizedWrite { poolable: true }, State::Pooled, Some(Effect::Chunk(Disposition::Recycle))),
        (State::SeenOnce { idle_flushes: last }, Event::OversizedWrite { poolable: true }, State::Pooled, Some(Effect::Chunk(Disposition::Recycle))),
        (State::SeenOnce { idle_flushes: 5 }, Event::OversizedWrite { poolable: false }, State::SeenOnce { idle_flushes: 5 }, Some(Effect::Chunk(Disposition::OneShot))),
        (State::SeenOnce { idle_flushes: 5 }, Event::Flush { had_oversized: true }, State::SeenOnce { idle_flushes: 0 }, None),
        (State::SeenOnce { idle_flushes: 0 }, Event::Flush { had_oversized: false }, State::SeenOnce { idle_flushes: 1 }, None),
        (State::SeenOnce { idle_flushes: last }, Event::Flush { had_oversized: false }, State::Cold, None),
        (State::SeenOnce { idle_flushes: 3 }, Event::Trim, State::Cold, Some(Effect::Evict)),
        // Pooled
        (State::Pooled, Event::OversizedWrite { poolable: true }, State::Pooled, Some(Effect::Chunk(Disposition::Recycle))),
        (State::Pooled, Event::OversizedWrite { poolable: false }, State::Pooled, Some(Effect::Chunk(Disposition::OneShot))),
        (State::Pooled, Event::Flush { had_oversized: true }, State::Pooled, None),
        (State::Pooled, Event::Flush { had_oversized: false }, State::IdleCounting { idle_flushes: 1 }, None),
        (State::Pooled, Event::Trim, State::Cold, Some(Effect::Evict)),
        // IdleCounting
        (State::IdleCounting { idle_flushes: 10 }, Event::OversizedWrite { poolable: true }, State::Pooled, Some(Effect::Chunk(Disposition::Recycle))),
        (State::IdleCounting { idle_flushes: 10 }, Event::OversizedWrite { poolable: false }, State::IdleCounting { idle_flushes: 10 }, Some(Effect::Chunk(Disposition::OneShot))),
        (State::IdleCounting { idle_flushes: 10 }, Event::Flush { had_oversized: true }, State::Pooled, None),
        (State::IdleCounting { idle_flushes: 10 }, Event::Flush { had_oversized: false }, State::IdleCounting { idle_flushes: 11 }, None),
        (State::IdleCounting { idle_flushes: last }, Event::Flush { had_oversized: false }, State::Cold, Some(Effect::Evict)),
        (State::IdleCounting { idle_flushes: 10 }, Event::Trim, State::Cold, Some(Effect::Evict)),
    ];
    for (start, event, want_state, want_effect) in cases {
        let mut st = start;
        let got = step(&mut st, event);
        assert_eq!((st, got), (want_state, want_effect), "from {start:?} on {event:?}");
    }
}

#[test]
fn exact_64kib_alignment_and_2x_reuse_window() {
    // 1080x2400 RGBA8 padded to 256-byte rows: 4352 * 2400 = 10,444,800 B -> 10.0 MiB (not 16 MiB).
    assert_eq!(alloc_size(4352 * 2400), 10 << 20);
    assert_eq!(alloc_size((4 << 20) + 4), (4 << 20) + OVERSIZED_ALIGN);
    assert_eq!(alloc_size(8 << 20), 8 << 20);

    // Fits when chunk_size in [write_size, 2 * write_size].
    assert!(fits_reusable(10 << 20, 10 << 20));
    assert!(fits_reusable(10 << 20, 5 << 20));
    assert!(!fits_reusable(10 << 20, (5 << 20) - 1));
    assert!(!fits_reusable((10 << 20) - 1, 10 << 20));
    assert!(!fits_reusable(24 << 20, 8 << 20));
}

#[test]
fn state_predicates_match_lifecycle() {
    assert!(!State::Cold.is_active());
    assert!(!State::Cold.accepts_returns());
    assert!(State::SeenOnce { idle_flushes: 0 }.is_active());
    assert!(!State::SeenOnce { idle_flushes: 0 }.accepts_returns());
    assert!(State::Pooled.is_active());
    assert!(State::Pooled.accepts_returns());
    assert!(State::IdleCounting { idle_flushes: 1 }.is_active());
    assert!(State::IdleCounting { idle_flushes: 1 }.accepts_returns());
}
