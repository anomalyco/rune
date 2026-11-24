//! Tests for `Player::presented_time`, `Player::frame_duration`, and `Player::step` through the
//! shared phase machine and scripted `Port`.

use super::*;

#[test]
fn presented_time_tracks_frame_pts_distinct_from_paused_seek_target_and_survives_parking() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    // 59.94 fps clip (1001/60000 s per frame), 20 s long.
    let frame = 1001.0 / 60_000.0;
    let fifty_nine = Meta { duration: 20.0, size: Some((1920, 1080)), frame: Some(frame), sound: false };
    assert_eq!(new.c.presented, None);
    new.c.open(Ok(fifty_nine));
    assert_eq!(new.c.presented, None, "opening metadata alone puts no picture on the surface");

    // Start player and land frame 0.
    new.run(Play);
    let f0 = Script { frame: Frame::New(0.0), item_time: 0.0, current: 0.0, ..Script::READY };
    new.run(Pull(f0));
    assert_eq!(new.c.presented, Some(0.0));
    new.run(Pause);
    new.run(Apply);

    // Exact seek to frame 959's centre (16.00766 s): while in flight, presented stays at 0.0.
    let target = fifty_nine.frame_time(959);
    let frame_959_pts = 959.0 * frame;
    new.send(Event::Seek { to: target, exact: true });
    new.run(Apply);
    assert!(new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.0), "previous frame remains shown while seek is in flight");

    // Frame 959 lands with its own PTS while AVPlayer.currentTime() still reads the seek target.
    let landed = Script { frame: Frame::New(frame_959_pts), item_time: target, current: target, player_rate: 0.0, ..Script::READY };
    new.run(Pull(landed));
    assert!(!new.c.is_seeking());
    assert!((new.c.time - target).abs() < 1e-9, "Player::time keeps currentTime when paused");
    assert!((new.c.presented.unwrap() - frame_959_pts).abs() < 1e-9, "presented_time reports the picture's own PTS");

    // Subsequent paused pull without a new frame keeps both values intact.
    let idle = Script { frame: Frame::Same, item_time: target, current: target, player_rate: 0.0, ..Script::READY };
    new.run(Pull(idle));
    assert!((new.c.presented.unwrap() - frame_959_pts).abs() < 1e-9);

    // Parking the player into Stage::Bare keeps the picture on the surface and its presented_time.
    new.run(Park);
    assert_eq!(new.c.stage, Stage::Bare);
    assert!((new.c.presented.unwrap() - frame_959_pts).abs() < 1e-9);

    // Failure clears presented_time (surface shows grey placeholder instead).
    new.run(Fail);
    assert_eq!(new.c.presented, None);
}

#[test]
fn poster_actual_time_sets_presented_time_and_audio_stays_none() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    new.fake.poster_pts = 0.0;
    new.run(Pull(Script::LOADING));
    assert_eq!(new.c.presented, None);
    new.run(Land(Done::Frame));
    new.run(Pull(Script::LOADING));
    assert_eq!(new.c.presented, Some(0.0));

    // Paused seek to 0.6 s (frame 2 of 0.25 s frames): poster worker reports actualTime = 0.5 s.
    new.fake.poster_pts = 0.5;
    new.send(Event::Seek { to: 0.6, exact: true });
    new.run(Pull(Script::LOADING));
    assert!(new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.0), "old poster stays shown until new poster lands");
    new.run(Land(Done::Frame));
    new.run(Pull(Script::LOADING));
    assert!(!new.c.is_seeking());
    assert_eq!(new.c.time, 0.6);
    assert_eq!(new.c.presented, Some(0.5));

    // Audio clips never report a presented video timestamp.
    let mut audio = New::new(true, now);
    audio.run(Open);
    audio.run(Play);
    audio.run(Pull(Script::READY));
    assert_eq!(audio.c.presented, None);
}

#[test]
fn step_on_ready_player_pauses_uses_step_by_and_falls_back_when_unsupported() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    new.run(Open); // 1.0 s, 4 frames of 0.25 s
    new.run(Play);
    let f1 = Script { frame: Frame::New(0.25), item_time: 0.25, current: 0.25, ..Script::READY };
    new.run(Pull(f1));
    assert!(new.c.playing());
    assert_eq!(new.c.presented, Some(0.25));

    // Step +1 while playing: pauses (SetRate(0.0)), calls step_by(1), enters is_seeking().
    new.fake.log.clear();
    new.send(Event::Step(1));
    assert!(!new.c.playing());
    assert!(new.c.is_seeking());
    new.run(Apply);
    assert_eq!(new.fake.steps, [1]);
    assert!(new.fake.seeks.is_empty(), "step_by succeeded, so no seek_to fallback");
    assert!(new.fake.log.contains(&Cmd::Rate(0.0)), "pauses native player");
    assert!(new.c.is_seeking());

    // Frame 2 (0.5 s) arrives: is_seeking clears, presented_time updates to 0.5.
    let f2 = Script { frame: Frame::New(0.5), item_time: 0.5, current: 0.5, player_rate: 0.0, ..Script::READY };
    new.run(Pull(f2));
    assert!(!new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.5));

    // When canStepBackward is false, step(-1) falls back cleanly to exact seek to frame 1 centre (0.375).
    new.fake.can_step_backward = false;
    new.send(Event::Step(-1));
    new.run(Apply);
    assert_eq!(new.fake.steps, [1], "step_by declined backward step");
    assert_eq!(new.fake.seeks, [(0.375, true)], "exact seek to frame 1 centre");
    assert!(new.c.is_seeking());
    let f1_back = Script { frame: Frame::New(0.25), item_time: 0.375, current: 0.375, player_rate: 0.0, ..Script::READY };
    new.run(Pull(f1_back));
    assert!(!new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.25));
}

#[test]
fn step_clamps_at_clip_ends_without_seeking_and_step_zero_only_pauses() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    new.run(Open); // 4 frames: 0.0, 0.25, 0.5, 0.75
    new.run(Play);
    let f0 = Script { frame: Frame::New(0.0), item_time: 0.0, current: 0.0, ..Script::READY };
    new.run(Pull(f0));

    // step(0) pauses without seeking or calling step_by.
    new.send(Event::Step(0));
    assert!(!new.c.playing());
    assert!(!new.c.is_seeking());
    new.run(Apply);
    assert!(new.fake.steps.is_empty());
    assert!(new.fake.seeks.is_empty());
    assert!(!new.c.is_seeking());

    // Already on frame 0: step(-1) and step(i32::MIN) stay put without entering Settle::Seeking.
    new.send(Event::Step(-1));
    assert!(!new.c.is_seeking());
    new.run(Apply);
    assert!(new.fake.steps.is_empty());
    assert!(new.fake.seeks.is_empty());

    new.send(Event::Step(i32::MIN));
    assert!(!new.c.is_seeking());
    new.run(Apply);
    assert!(new.fake.steps.is_empty());

    // Step forward by a huge count clamps to last frame (frame 3 -> delta +3).
    new.send(Event::Step(i32::MAX));
    new.run(Apply);
    assert_eq!(new.fake.steps, [3]);
    assert!(new.c.is_seeking());
    let f3 = Script { frame: Frame::New(0.75), item_time: 0.75, current: 0.75, player_rate: 0.0, ..Script::READY };
    new.run(Pull(f3));
    assert!(!new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.75));

    // Already on last frame (3): step(+1) stays put without entering a 2 s SEEK_PATIENCE wait.
    new.send(Event::Step(1));
    assert!(!new.c.is_seeking());
    new.run(Apply);
    assert_eq!(new.fake.steps, [3]);
    assert!(new.fake.seeks.is_empty());
    assert!(!new.c.is_seeking());
}

#[test]
fn step_while_already_seeking_steps_from_in_flight_target_and_uses_exact_seek() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    new.run(Open); // 4 frames of 0.25 s
    new.run(Play);
    let f0 = Script { frame: Frame::New(0.0), item_time: 0.0, current: 0.0, ..Script::READY };
    new.run(Pull(f0));
    new.run(Pause);
    new.run(Apply);

    // First step(+1) uses step_by(1) targeting frame 1 (0.375).
    new.send(Event::Step(1));
    new.run(Apply);
    assert_eq!(new.fake.steps, [1]);
    assert!(new.c.is_seeking());

    // Second step(+1) before frame 1 arrives steps from frame 1 to frame 2 (0.625) via exact seek.
    new.send(Event::Step(1));
    new.run(Apply);
    assert_eq!(new.fake.steps, [1], "in-flight step uses absolute seek_to to avoid relative race");
    assert_eq!(new.fake.seeks, [(0.625, true)]);
    assert!(new.c.is_seeking());
}

#[test]
fn step_works_on_poster_and_parked_clips_and_ignores_audio_and_failed() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    // Poster at frame 0 lands.
    new.fake.poster_pts = 0.0;
    new.run(Pull(Script::LOADING));
    new.run(Land(Done::Frame));
    new.run(Pull(Script::LOADING));
    assert_eq!(new.c.stage, Stage::Poster);
    assert_eq!(new.c.presented, Some(0.0));
    assert!(!new.c.is_seeking());

    // Step +1 while paused on poster: asks poster worker for frame 1 centre (0.375).
    new.fake.poster_pts = 0.25;
    new.send(Event::Step(1));
    assert!(new.c.is_seeking());
    new.run(Pull(Script::LOADING));
    assert!(new.fake.log.contains(&Cmd::Ask(0.375)));
    assert!(new.c.is_seeking());
    new.run(Land(Done::Frame));
    new.run(Pull(Script::LOADING));
    assert!(!new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.25));

    // Step -1 from Ended transitions back to Ready (Paused).
    new.c.run = Run::Ended;
    new.send(Event::Step(-1));
    assert_eq!(new.c.state(), PlayerState::Ready);
    assert!(new.c.is_seeking());

    // Audio ignores step completely (stays playing).
    let mut audio = New::new(true, now);
    audio.run(Open);
    audio.run(Play);
    audio.run(Pull(Script::READY));
    audio.send(Event::Step(1));
    assert!(audio.c.playing());
    assert!(!audio.c.is_seeking());

    // Failed clips ignore step completely.
    new.run(Fail);
    new.send(Event::Step(1));
    assert!(!new.c.is_seeking());
}
