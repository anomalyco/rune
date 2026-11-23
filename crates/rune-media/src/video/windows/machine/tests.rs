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
    let mut phase = Phase::Idle { at: 3.0 };
    assert_eq!(effects(&mut phase, Event::Open { play: true }), vec![Effect::Open { at: 3.0 }]);
    assert_eq!(phase, Phase::Opening { at: 3.0, play: true });
    assert_eq!(effects(&mut phase, Event::Opened), vec![]);
    assert_eq!(phase, Phase::Buffering { at: 3.0, play: true });
}

#[test]
fn the_first_picture_starts_the_clock_of_a_playing_clip() {
    let target = rune_core::time::now();
    let mut phase = Phase::Buffering { at: 3.0, play: true };
    let clock = Anchor { at: target, pos: 3.5, rate: 2.0 };
    assert_eq!(effects(&mut phase, Event::Shown { pts: 3.0, target, lead: 0.5, rate: 2.0 }), vec![Effect::Clock(Some(clock))]);
    assert_eq!(phase, Phase::Playing { clock });
}

#[test]
fn the_first_picture_holds_on_a_paused_clip() {
    let mut phase = Phase::Buffering { at: 3.0, play: false };
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
    assert_eq!(effects(&mut phase, Event::Seek { to: 9.0 }), vec![Effect::Clock(None), Effect::Seek { to: 9.0 }]);
    assert_eq!(phase, Phase::Buffering { at: 9.0, play: true });
}

#[test]
fn a_seek_while_paused_or_ended_waits_for_its_picture_paused() {
    for mut phase in [Phase::Paused { at: 5.0 }, Phase::Ended { at: 5.0 }] {
        assert_eq!(effects(&mut phase, Event::Seek { to: 9.0 }), vec![Effect::Seek { to: 9.0 }]);
        assert_eq!(phase, Phase::Buffering { at: 9.0, play: false });
    }
}

#[test]
fn a_stall_holds_the_clock_until_the_next_picture() {
    let (mut phase, _) = playing(5.0);
    assert_eq!(effects(&mut phase, Event::Stalled { at: 5.1 }), vec![Effect::Clock(None)]);
    assert_eq!(phase, Phase::Buffering { at: 5.1, play: true });
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
    assert_eq!(phase, Phase::Idle { at: 5.0 });
}

#[test]
fn a_failed_clip_stays_failed() {
    let mut phase = Phase::Failed;
    assert_eq!(effects(&mut phase, Event::Open { play: true }), vec![]);
    assert_eq!(phase, Phase::Failed);
}

#[test]
fn a_playing_seek_the_decoder_ends_with_no_picture_ends_where_it_was_asked_to_stand() {
    let mut phase = Phase::Buffering { at: 9.99, play: true };
    assert_eq!(effects(&mut phase, Event::Ended { at: 9.99 }), vec![]);
    assert_eq!(phase, Phase::Ended { at: 9.99 });
}

#[test]
fn a_paused_seek_the_decoder_ends_with_no_picture_holds_there() {
    let mut phase = Phase::Buffering { at: 9.99, play: false };
    assert_eq!(effects(&mut phase, Event::Ended { at: 9.99 }), vec![]);
    assert_eq!(phase, Phase::Paused { at: 9.99 });
}

#[test]
fn a_seek_while_parked_forgets_the_picture_and_opens_at_the_target() {
    let mut phase = Phase::Idle { at: 3.0 };
    assert_eq!(effects(&mut phase, Event::Seek { to: 7.5 }), vec![Effect::Seek { to: 7.5 }]);
    assert_eq!(phase, Phase::Idle { at: 7.5 });
    assert_eq!(effects(&mut phase, Event::Open { play: false }), vec![Effect::Open { at: 7.5 }]);
}
