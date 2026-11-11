//! Poster answers that arrive too late to count: after the clip failed, or after a native player
//! took over from the request. Each runs the real [`step`] and turn on the scripted player.

use super::*;

fn clip(audio: bool, script: &[Sym]) -> New {
    let mut new = New::new(audio, Instant::now());
    for &sym in script {
        new.run(sym);
    }
    new
}

fn posters_shown(new: &New) -> usize {
    new.fake.log.iter().filter(|cmd| matches!(cmd, Cmd::Poster(_))).count()
}

#[test]
fn a_poster_after_a_failed_one_leaves_the_placeholder() {
    let mut new = clip(false, &[Pull(Script::LOADING), Seek(0.3), Pull(Script::LOADING), Land(Done::Err), Pull(Script::LOADING)]);
    assert_eq!(new.fake.log.iter().filter(|cmd| matches!(cmd, Cmd::Ask(_))).count(), 2, "two decodes queued");
    assert_eq!(new.c.run, Run::Failed { error: "load failed".into(), drawn: true });
    assert!(new.fake.log.contains(&Cmd::Placeholder));
    let frame_id = new.c.frame_id;

    new.run(Land(Done::Frame));
    new.run(Pull(Script::LOADING));

    assert_eq!(posters_shown(&new), 0, "nothing would redraw the placeholder it covered");
    assert_eq!(new.c.frame_id, frame_id);
    assert_eq!(new.c.run, Run::Failed { error: "load failed".into(), drawn: true });
}

#[test]
fn a_late_poster_error_leaves_a_working_player_playing() {
    let mut new = clip(false, &[Preload, Play, Pull(Script::FRAME)]);
    assert!(new.fake.log.contains(&Cmd::Frame(1)), "a native frame arrived");
    let generation = new.c.generation;

    new.run(Land(Done::Err));
    new.run(Pull(Script::FRAME));

    assert!(new.c.playing());
    assert!(new.c.stage.ready());
    assert!(!new.fake.log.contains(&Cmd::Drop));
    assert_eq!(new.c.generation, generation);
}

#[test]
fn a_request_from_before_a_player_does_not_answer_the_one_after_it() {
    let mut new = clip(false, &[Preload, Play, Pull(Script::LOADING), Pause, Park, Pull(Script::LOADING)]);
    let (stale, current) = (Attempt::default(), new.fake.asked);
    assert_ne!(stale, current);
    assert_eq!(new.c.stage, Stage::Poster);

    new.fake.done = Some((stale, Done::Frame));
    new.run(Pull(Script::LOADING));
    assert_eq!(posters_shown(&new), 0, "the stale frame is for an older position");
    assert_eq!(new.c.meta, None);

    new.fake.done = Some((current, Done::Frame));
    new.run(Pull(Script::LOADING));
    assert_eq!(posters_shown(&new), 1);
}

#[test]
fn an_audio_load_failure_fails_the_clip() {
    let new = clip(true, &[LoadAudio, Land(Done::Err), Pull(Script::READY)]);
    assert_eq!(new.c.state(), PlayerState::Failed("load failed".into()));
}

#[test]
fn an_audio_load_failure_counts_even_after_a_player_is_made() {
    let mut new = clip(true, &[LoadAudio, Open, Play, Pull(Script::READY)]);
    assert!(new.c.stage.has_player());

    new.run(Land(Done::Err));
    new.run(Pull(Script::READY));

    assert_eq!(new.c.state(), PlayerState::Failed("load failed".into()));
    assert!(new.fake.log.contains(&Cmd::Drop));
}

/// Every order of two requests either side of a player, with answers that fail or land.
#[test]
fn late_poster_answers_in_any_order_match_the_fenced_flags() {
    walk(
        &[],
        &[Play, Pause, Seek(0.3), Preload, Land(Done::Err), Land(Done::Frame), Land(Done::FrameFails), Fail, Park, Pull(Script::LOADING), Pull(Script::FRAME)],
        6,
    );
}
