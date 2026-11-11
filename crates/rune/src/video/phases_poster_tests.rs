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
    assert!(new.c.meta.is_some(), "its facts still count");

    new.fake.done = Some((current, Done::Frame));
    new.run(Pull(Script::LOADING));
    assert_eq!(posters_shown(&new), 1);
}

#[test]
fn a_superseded_poster_keeps_the_clips_frame_timing() {
    let mut c = Control::default();
    let AskPoster(0.0, stale) = only(step(&mut c, Event::Preload)) else { panic!("asks for a poster") };
    step(&mut c, Event::Play);
    step(&mut c, Event::PlayerMade { created: Instant::now() });
    // The poster lands after the player took over, before its item is ready.
    let sixty = Meta { duration: 1.0, size: Some((64, 36)), frame: 1.0 / 60.0 };
    assert!(step(&mut c, Event::PosterDone { attempt: stale, result: Ok(sixty) }).is_empty());
    let now = Instant::now();
    for effect in step(&mut c, Event::Item { item: Item::Ready, now }) {
        if effect == Effect::ReadFacts {
            // What a ready item reports, which has no frame timing of its own.
            c.open(Ok(Meta { duration: 1.0, size: Some((64, 36)), frame: 1.0 / 30.0 }));
        }
    }
    step(&mut c, Event::Framed { shown: 0.0 });
    step(&mut c, Event::Pause);
    step(&mut c, Event::Seek(0.02));
    step(&mut c, Event::Reconcile { now });

    assert_eq!(c.meta.map(|m| m.frame), Some(1.0 / 60.0));
    let Stage::Ready { settle, .. } = c.stage else { panic!("ready") };
    assert!(settle.waiting(), "20 ms is the next frame at 60 fps, so the pull waits for it");
}

fn only(effects: Effects<Effect>) -> Effect {
    let mut effects = effects.into_iter();
    let effect = effects.next().unwrap();
    assert!(effects.next().is_none());
    effect
}

use Effect::AskPoster;

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

/// A request before a player and one after it, both owed: each answer carries its own request.
#[test]
fn overlapping_requests_either_side_of_a_player_answer_for_themselves() {
    walk(
        &[Preload, Play, Pull(Script::LOADING), Pause, Park, Pull(Script::LOADING)],
        &[Play, Pause, Seek(0.3), Land(Done::Err), Land(Done::Frame), Land(Done::Facts), Park, Pull(Script::LOADING), Pull(Script::FRAME)],
        6,
    );
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
