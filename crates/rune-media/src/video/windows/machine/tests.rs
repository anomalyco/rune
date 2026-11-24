use super::*;

fn effects(phase: &mut Phase, event: Event) -> Vec<Effect> {
    step(phase, event).into_iter().collect()
}

fn playing(pos: f64) -> (Phase, Anchor) {
    let clock = Anchor { at: rune_core::time::now(), pos, rate: 1.0 };
    (Phase::Playing { clock }, clock)
}

#[test]
fn a_clip_opens_where_it_was_parked() {
    let mut phase = Phase::Idle { at: 3.0, seek: None };
    assert_eq!(effects(&mut phase, Event::Open { play: true }), vec![Effect::Open { at: 3.0, exact: true }]);
    assert_eq!(phase, Phase::Opening { at: 3.0, play: true, seeking: false });
    assert!(!phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Opened), vec![]);
    assert_eq!(phase, Phase::Buffering { at: 3.0, play: true, seeking: false });
    assert!(!phase.is_seeking());
}

#[test]
fn the_first_picture_starts_the_clock_of_a_playing_clip() {
    let target = rune_core::time::now();
    let mut phase = Phase::Buffering { at: 3.0, play: true, seeking: false };
    let clock = Anchor { at: target, pos: 3.5, rate: 2.0 };
    assert_eq!(effects(&mut phase, Event::Shown { pts: 3.0, target, lead: 0.5, rate: 2.0 }), vec![Effect::Clock(Some(clock))]);
    assert_eq!(phase, Phase::Playing { clock });
}

#[test]
fn the_first_picture_holds_on_a_paused_clip() {
    let mut phase = Phase::Buffering { at: 3.0, play: false, seeking: false };
    assert_eq!(effects(&mut phase, Event::Shown { pts: 3.02, target: rune_core::time::now(), lead: 0.0, rate: 1.0 }), vec![]);
    assert_eq!(phase, Phase::Paused { at: 3.02 });
}

#[test]
fn a_pause_stops_the_clock_at_the_picture_on_screen() {
    let (mut phase, _) = playing(5.0);
    assert_eq!(effects(&mut phase, Event::Pause { at: 5.2 }), vec![Effect::Clock(None)]);
    assert_eq!(phase, Phase::Paused { at: 5.2 });
}

#[test]
fn a_seek_while_playing_stops_the_clock_and_plays_on_from_the_target() {
    let (mut phase, _) = playing(5.0);
    assert_eq!(effects(&mut phase, Event::Seek { to: 9.0, exact: true }), vec![Effect::Clock(None), Effect::Seek { to: 9.0, exact: true }]);
    assert_eq!(phase, Phase::Buffering { at: 9.0, play: true, seeking: true });
    assert!(phase.is_seeking());
}

#[test]
fn a_seek_while_paused_or_ended_waits_for_its_picture_paused() {
    for mut phase in [Phase::Paused { at: 5.0 }, Phase::Ended { at: 5.0 }] {
        assert_eq!(effects(&mut phase, Event::Seek { to: 9.0, exact: true }), vec![Effect::Seek { to: 9.0, exact: true }]);
        assert_eq!(phase, Phase::Buffering { at: 9.0, play: false, seeking: true });
        assert!(phase.is_seeking());
    }
}

#[test]
fn a_stall_holds_the_clock_until_the_next_picture() {
    let (mut phase, _) = playing(5.0);
    assert_eq!(effects(&mut phase, Event::Stalled { at: 5.1 }), vec![Effect::Clock(None)]);
    assert_eq!(phase, Phase::Buffering { at: 5.1, play: true, seeking: false });
    assert!(!phase.is_seeking());
}

#[test]
fn the_last_picture_ends_a_playing_clip() {
    let (mut phase, _) = playing(5.0);
    assert_eq!(effects(&mut phase, Event::Ended { at: 5.9 }), vec![Effect::Clock(None)]);
    assert_eq!(phase, Phase::Ended { at: 5.9 });
}

#[test]
fn a_released_decoder_parks_the_clip_at_its_picture() {
    let mut phase = Phase::Paused { at: 5.0 };
    assert_eq!(effects(&mut phase, Event::Released { at: 5.0 }), vec![Effect::Clock(None)]);
    assert_eq!(phase, Phase::Idle { at: 5.0, seek: None });
}

#[test]
fn a_failed_clip_stays_failed() {
    let mut phase = Phase::Failed;
    assert_eq!(effects(&mut phase, Event::Open { play: true }), vec![]);
    assert_eq!(phase, Phase::Failed);
}

#[test]
fn a_playing_seek_the_decoder_ends_with_no_picture_ends_where_it_was_asked_to_stand() {
    let mut phase = Phase::Buffering { at: 9.99, play: true, seeking: true };
    assert_eq!(effects(&mut phase, Event::Ended { at: 9.99 }), vec![]);
    assert_eq!(phase, Phase::Ended { at: 9.99 });
    assert!(!phase.is_seeking());
}

#[test]
fn a_paused_seek_the_decoder_ends_with_no_picture_holds_there() {
    let mut phase = Phase::Buffering { at: 9.99, play: false, seeking: true };
    assert_eq!(effects(&mut phase, Event::Ended { at: 9.99 }), vec![]);
    assert_eq!(phase, Phase::Paused { at: 9.99 });
    assert!(!phase.is_seeking());
}

#[test]
fn a_seek_while_parked_forgets_the_picture_and_opens_at_the_target() {
    let mut phase = Phase::Idle { at: 3.0, seek: None };
    assert_eq!(effects(&mut phase, Event::Seek { to: 7.5, exact: true }), vec![Effect::Seek { to: 7.5, exact: true }]);
    assert_eq!(phase, Phase::Idle { at: 7.5, seek: Some(SeekMode::Exact) });
    assert!(phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Open { play: false }), vec![Effect::Open { at: 7.5, exact: true }]);
    assert_eq!(phase, Phase::Opening { at: 7.5, play: false, seeking: true });
    assert!(phase.is_seeking());
}

#[test]
fn seek_fast_propagates_exact_false_and_tracks_is_seeking_until_shown() {
    let now = rune_core::time::now();
    // Parked -> seek_fast -> Open -> Opened -> Shown.
    let mut phase = Phase::Idle { at: 1.0, seek: None };
    assert!(!phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Seek { to: 6.4, exact: false }), vec![Effect::Seek { to: 6.4, exact: false }]);
    assert_eq!(phase, Phase::Idle { at: 6.4, seek: Some(SeekMode::Keyframe) });
    assert!(phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Open { play: false }), vec![Effect::Open { at: 6.4, exact: false }]);
    assert_eq!(phase, Phase::Opening { at: 6.4, play: false, seeking: true });
    assert!(phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Opened), vec![]);
    assert_eq!(phase, Phase::Buffering { at: 6.4, play: false, seeking: true });
    assert!(phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Shown { pts: 5.0, target: now, lead: 0.0, rate: 1.0 }), vec![]);
    assert_eq!(phase, Phase::Paused { at: 5.0 });
    assert!(!phase.is_seeking());

    // Playing -> seek_fast -> Play/Pause preserve seeking -> Shown clears it.
    let (mut phase, _) = playing(2.0);
    assert_eq!(effects(&mut phase, Event::Seek { to: 7.5, exact: false }), vec![Effect::Clock(None), Effect::Seek { to: 7.5, exact: false }]);
    assert_eq!(phase, Phase::Buffering { at: 7.5, play: true, seeking: true });
    assert_eq!(effects(&mut phase, Event::Pause { at: 7.5 }), vec![]);
    assert_eq!(phase, Phase::Buffering { at: 7.5, play: false, seeking: true });
    assert!(phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Play { target: now, lead: 0.0, rate: 1.0 }), vec![]);
    assert_eq!(phase, Phase::Buffering { at: 7.5, play: true, seeking: true });
    assert!(phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Shown { pts: 6.0, target: now, lead: 0.0, rate: 1.0 }), vec![Effect::Clock(Some(Anchor { at: now, pos: 6.0, rate: 1.0 }))]);
    assert!(!phase.is_seeking());
}

#[test]
fn a_synced_clock_updates_a_playing_clip_without_emitting_effects() {
    let (mut phase, clock) = playing(5.0);
    let nudged = Anchor { pos: 5.006, ..clock };
    assert_eq!(effects(&mut phase, Event::Synced { clock: nudged }), vec![]);
    assert_eq!(phase, Phase::Playing { clock: nudged });
}

#[test]
fn a_synced_clock_with_a_different_rate_or_outside_playing_is_ignored() {
    let (mut phase, clock) = playing(5.0);
    let other_rate = Anchor { pos: 5.01, rate: 2.0, ..clock };
    assert_eq!(effects(&mut phase, Event::Synced { clock: other_rate }), vec![]);
    assert_eq!(phase, Phase::Playing { clock });

    let mut paused = Phase::Paused { at: 5.0 };
    assert_eq!(effects(&mut paused, Event::Synced { clock }), vec![]);
    assert_eq!(paused, Phase::Paused { at: 5.0 });
}

#[test]
fn step_always_pauses_and_issues_exact_seek_across_every_phase() {
    let now = rune_core::time::now();
    // Playing -> stops clock, pauses (play: false), exact seek, is_seeking == true.
    let (mut phase, _) = playing(2.0);
    assert_eq!(effects(&mut phase, Event::Step { to: 2.0 + 1.0 / 30.0 }), vec![Effect::Clock(None), Effect::Seek { to: 2.0 + 1.0 / 30.0, exact: true }]);
    assert_eq!(phase, Phase::Buffering { at: 2.0 + 1.0 / 30.0, play: false, seeking: true });
    assert!(phase.is_seeking());
    assert_eq!(effects(&mut phase, Event::Shown { pts: 2.0 + 1.0 / 30.0, target: now, lead: 0.0, rate: 1.0 }), vec![]);
    assert_eq!(phase, Phase::Paused { at: 2.0 + 1.0 / 30.0 });
    assert!(!phase.is_seeking());

    // Paused and Ended -> Buffering { play: false, seeking: true } with exact seek.
    for mut phase in [Phase::Paused { at: 5.0 }, Phase::Ended { at: 10.0 }] {
        assert_eq!(effects(&mut phase, Event::Step { to: 4.5 }), vec![Effect::Seek { to: 4.5, exact: true }]);
        assert_eq!(phase, Phase::Buffering { at: 4.5, play: false, seeking: true });
        assert!(phase.is_seeking());
    }

    // Idle (parked) -> remembers exact seek and opens paused when drawn.
    let mut idle = Phase::Idle { at: 3.0, seek: None };
    assert_eq!(effects(&mut idle, Event::Step { to: 3.1 }), vec![Effect::Seek { to: 3.1, exact: true }]);
    assert_eq!(idle, Phase::Idle { at: 3.1, seek: Some(SeekMode::Exact) });
    assert!(idle.is_seeking());
    assert_eq!(effects(&mut idle, Event::Open { play: false }), vec![Effect::Open { at: 3.1, exact: true }]);
    assert_eq!(idle, Phase::Opening { at: 3.1, play: false, seeking: true });

    // Opening (playing) -> flips to paused (play: false) and exact seek.
    let mut opening = Phase::Opening { at: 1.0, play: true, seeking: false };
    assert_eq!(effects(&mut opening, Event::Step { to: 1.1 }), vec![Effect::Seek { to: 1.1, exact: true }]);
    assert_eq!(opening, Phase::Opening { at: 1.1, play: false, seeking: true });

    // Failed -> no-op.
    let mut failed = Phase::Failed;
    assert_eq!(effects(&mut failed, Event::Step { to: 1.0 }), vec![]);
    assert_eq!(failed, Phase::Failed);
}
