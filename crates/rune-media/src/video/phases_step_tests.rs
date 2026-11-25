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

    // Step +1 while playing: pauses (SetRate(0.0)), snaps the off-boundary cursor to the shown
    // frame first, then calls step_by(1) once the snap completes.
    new.fake.log.clear();
    new.send(Event::Step(1));
    assert!(!new.c.playing());
    assert!(new.c.is_seeking());
    new.run(Apply);
    assert!(new.fake.log.contains(&Cmd::Rate(0.0)), "pauses native player");
    assert_eq!(new.fake.snaps.len(), 1);
    assert_eq!(new.fake.snaps[0].1, 1);
    assert!(new.fake.steps.is_empty(), "waits for snap before step_by");
    assert!(new.fake.seeks.is_empty());
    assert!(new.c.is_seeking());

    // Snap finishes: emits step_by(1) and marks the cursor aligned.
    new.snap_done(new.fake.snaps[0].0, true);
    assert_eq!(new.fake.steps, [1]);
    assert!(new.fake.seeks.is_empty(), "step_by succeeded, so no seek_to fallback");
    assert!(new.c.is_seeking());

    // Frame 2 (0.5 s) arrives: is_seeking clears, presented_time updates to 0.5.
    let f2 = Script { frame: Frame::New(0.5), item_time: 0.5, current: 0.5, player_rate: 0.0, ..Script::READY };
    new.run(Pull(f2));
    assert!(!new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.5));

    // Cursor is now Aligned; when canStepBackward is false, step(-1) tries step_by directly and
    // falls back cleanly to exact seek to frame 1 centre (0.375), leaving the cursor OffBoundary.
    new.fake.can_step_backward = false;
    new.send(Event::Step(-1));
    new.run(Apply);
    assert_eq!(new.fake.snaps.len(), 1, "aligned cursor tries step_by directly without snapping");
    assert_eq!(new.fake.steps, [1], "step_by declined backward step");
    assert_eq!(new.fake.seeks, [(0.375, true)], "exact seek to frame 1 centre");
    assert!(new.c.is_seeking());
    let f1_back = Script { frame: Frame::New(0.25), item_time: 0.375, current: 0.375, player_rate: 0.0, ..Script::READY };
    new.run(Pull(f1_back));
    assert!(!new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.25));

    // Because the fallback used seek_to(0.375) (midpoint of frame 1), the next step(+1) must snap
    // back to frame 1's start rather than calling step_by directly from 0.375.
    new.send(Event::Step(1));
    new.run(Apply);
    assert_eq!(new.fake.snaps.len(), 2, "fallback seek left cursor OffBoundary, so next step snaps");
    assert_eq!(new.fake.snaps[1].1, 1);
}

#[test]
fn step_clamps_at_clip_ends_without_seeking_and_step_zero_only_pauses() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    new.run(Open); // 4 frames: 0.0, 0.25, 0.5, 0.75
    new.run(Play);
    let f0 = Script { frame: Frame::New(0.0), item_time: 0.0, current: 0.0, ..Script::READY };
    new.run(Pull(f0));

    // step(0) pauses without seeking, snapping, or calling step_by.
    new.send(Event::Step(0));
    assert!(!new.c.playing());
    assert!(!new.c.is_seeking());
    new.run(Apply);
    assert!(new.fake.snaps.is_empty());
    assert!(new.fake.steps.is_empty());
    assert!(new.fake.seeks.is_empty());
    assert!(!new.c.is_seeking());

    // Already on frame 0: step(-1) and step(i32::MIN) stay put without entering Settle::Seeking.
    new.send(Event::Step(-1));
    assert!(!new.c.is_seeking());
    new.run(Apply);
    assert!(new.fake.snaps.is_empty());
    assert!(new.fake.steps.is_empty());
    assert!(new.fake.seeks.is_empty());

    new.send(Event::Step(i32::MIN));
    assert!(!new.c.is_seeking());
    new.run(Apply);
    assert!(new.fake.snaps.is_empty());
    assert!(new.fake.steps.is_empty());

    // Step forward by a huge count clamps to last frame (frame 3 -> delta +3): snaps then steps.
    new.send(Event::Step(i32::MAX));
    new.run(Apply);
    assert_eq!(new.fake.snaps.len(), 1);
    assert_eq!(new.fake.snaps[0].1, 3);
    assert!(new.c.is_seeking());
    new.snap_done(new.fake.snaps[0].0, true);
    assert_eq!(new.fake.steps, [3]);
    let f3 = Script { frame: Frame::New(0.75), item_time: 0.75, current: 0.75, player_rate: 0.0, ..Script::READY };
    new.run(Pull(f3));
    assert!(!new.c.is_seeking());
    assert_eq!(new.c.presented, Some(0.75));

    // Already on last frame (3): step(+1) stays put without entering a 2 s SEEK_PATIENCE wait.
    new.send(Event::Step(1));
    assert!(!new.c.is_seeking());
    new.run(Apply);
    assert_eq!(new.fake.snaps.len(), 1);
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

    // First step(+1) after pausing starts a snap for step(+1) targeting frame 1 (0.375).
    new.send(Event::Step(1));
    new.run(Apply);
    assert_eq!(new.fake.snaps.len(), 1);
    let snap_attempt = new.fake.snaps[0].0;
    assert!(new.c.is_seeking());

    // Second step(+1) while the snap is still in flight steps from frame 1 to frame 2 (0.625) via
    // exact seek, cancelling the in-flight snap attempt.
    new.send(Event::Step(1));
    new.run(Apply);
    assert!(new.fake.steps.is_empty(), "in-flight step uses absolute seek_to to avoid relative race");
    assert_eq!(new.fake.seeks, [(0.625, true)]);
    assert!(new.c.is_seeking());

    // Late completion of the superseded snap attempt changes nothing.
    new.snap_done(snap_attempt, true);
    assert!(new.fake.steps.is_empty());
    assert_eq!(new.fake.seeks, [(0.625, true)]);
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


#[test]
fn step_after_exact_seek_snaps_once_then_steps_directly_with_no_extra_frame() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    // 59.94 fps clip (1001/60000 s per frame), 4 frames tested at boundary, first half,
    // second half, and 1 tick (1/60000 s) before the next frame boundary.
    let fd = 1001.0 / 60_000.0;
    let tick = 1.0 / 60_000.0;
    new.c.open(Ok(Meta { duration: 10.0, size: Some((1920, 1080)), frame: Some(fd), sound: false }));
    new.run(Play);
    new.run(Pull(Script { frame: Frame::New(0.0), item_time: 0.0, current: 0.0, ..Script::READY }));
    new.run(Pause);
    new.run(Apply);

    for (k, offset, delta) in [
        (10, 0.0, 1),
        (10, 0.25 * fd, 1),
        (10, 0.75 * fd, 1),
        (10, fd - tick, 1),
        (10, 0.0, -1),
        (10, 0.25 * fd, -1),
        (10, 0.75 * fd, -1),
        (10, fd - tick, -1),
    ] {
        let pts = f64::from(k) * fd;
        let seek_at = pts + offset;
        new.fake.snaps.clear();
        new.fake.steps.clear();
        new.fake.seeks.clear();

        // Exact seek lands on frame k (leaving native cursor OffBoundary).
        new.send(Event::Seek { to: seek_at, exact: true });
        new.run(Apply);
        assert_eq!(new.fake.seeks, [(seek_at, true)]);
        new.run(Pull(Script { frame: Frame::New(pts), item_time: seek_at, current: seek_at, player_rate: 0.0, ..Script::READY }));
        assert!(!new.c.is_seeking());
        assert_eq!(new.c.presented, Some(pts));

        // Step(delta) must snap before step_by.
        new.fake.log.clear();
        let frame_id_before = new.c.frame_id;
        new.send(Event::Step(delta));
        new.run(Apply);
        assert_eq!(new.fake.snaps.len(), 1, "seek at offset {offset} must snap before step({delta})");
        assert_eq!(new.fake.snaps[0].1, delta);
        assert!(new.fake.steps.is_empty());
        assert!(new.c.is_seeking());

        // A pull while snapping must NOT push any frame to the surface or clear is_seeking.
        let spurious = Script { frame: Frame::New(pts), item_time: pts, current: pts, player_rate: 0.0, ..Script::READY };
        new.run(Pull(spurious));
        assert_eq!(new.c.frame_id, frame_id_before, "snapping must not push an extra frame");
        assert!(new.c.is_seeking(), "snapping must not clear is_seeking before snap completes");

        // Snap finishes -> invokes step_by(delta) and aligns the cursor.
        let attempt = new.fake.snaps[0].0;
        new.snap_done(attempt, true);
        assert_eq!(new.fake.steps, [delta]);
        assert!(new.c.is_seeking());

        // Target frame lands: exactly one new frame pushed.
        let next_pts = f64::from(k + delta) * fd;
        new.run(Pull(Script { frame: Frame::New(next_pts), item_time: next_pts, current: next_pts, player_rate: 0.0, ..Script::READY }));
        assert!(!new.c.is_seeking());
        assert_eq!(new.c.frame_id, frame_id_before + 1);
        assert_eq!(new.c.presented, Some(next_pts));

        // A subsequent step-after-step uses direct step_by with ZERO extra snap or seek.
        new.fake.snaps.clear();
        new.fake.steps.clear();
        new.fake.seeks.clear();
        new.send(Event::Step(delta));
        new.run(Apply);
        assert!(new.fake.snaps.is_empty(), "step-after-step must not snap");
        assert!(new.fake.seeks.is_empty(), "step-after-step must not seek");
        assert_eq!(new.fake.steps, [delta]);
        let next2_pts = f64::from(k + 2 * delta) * fd;
        new.run(Pull(Script { frame: Frame::New(next2_pts), item_time: next2_pts, current: next2_pts, player_rate: 0.0, ..Script::READY }));
        assert!(!new.c.is_seeking());
    }
}

#[test]
fn seek_play_interruption_and_timeout_during_snap_cancel_stale_attempt() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    new.run(Open); // 4 frames of 0.25 s
    new.run(Play);
    new.run(Pull(Script { frame: Frame::New(0.0), item_time: 0.0, current: 0.0, ..Script::READY }));
    new.run(Pause);
    new.run(Apply);

    // 1. Seek arrives while snap is in flight: cancels snap; stale SnapDone changes nothing.
    new.send(Event::Step(1));
    new.run(Apply);
    let a1 = new.fake.snaps[0].0;
    new.send(Event::Seek { to: 0.80, exact: true });
    // Even if SnapDone(a1) arrives BEFORE Apply runs for the seek, it is ignored.
    new.snap_done(a1, true);
    assert!(new.fake.steps.is_empty(), "seek cancelled the snap attempt immediately");
    new.run(Apply);
    assert_eq!(new.fake.seeks, [(0.80, true)]);
    new.run(Pull(Script { frame: Frame::New(0.75), item_time: 0.80, current: 0.80, player_rate: 0.0, ..Script::READY }));
    assert!(!new.c.is_seeking());

    // 2. Play arrives while snap is in flight: cancels snap; stale SnapDone changes nothing.
    new.fake.snaps.clear();
    new.fake.steps.clear();
    new.fake.seeks.clear();
    new.send(Event::Step(-1));
    new.run(Apply);
    let a2 = new.fake.snaps[0].0;
    new.run(Play);
    new.snap_done(a2, true);
    assert!(new.fake.steps.is_empty(), "play cancelled the snap attempt");
    new.run(Apply);
    assert!(new.c.playing());

    // 3. Interrupted snap (finished == false) falls back cleanly to exact seek_to.
    new.run(Pause);
    new.run(Apply);
    new.fake.snaps.clear();
    new.fake.steps.clear();
    new.fake.seeks.clear();
    new.send(Event::Step(-1));
    new.run(Apply);
    let a3 = new.fake.snaps[0].0;
    new.snap_done(a3, false);
    assert!(new.fake.steps.is_empty(), "interrupted snap must not call step_by");
    assert_eq!(new.fake.seeks, [(0.625, true)], "interrupted snap falls back to exact seek");
    assert!(new.c.is_seeking());
    new.run(Pull(Script { frame: Frame::New(0.50), item_time: 0.625, current: 0.625, player_rate: 0.0, ..Script::READY }));

    // 4. Same-frame paused seek (Settle::Holding) still snaps before stepping (important for VFR).
    new.fake.snaps.clear();
    new.fake.steps.clear();
    new.fake.seeks.clear();
    new.send(Event::Seek { to: 0.68, exact: true });
    new.run(Apply);
    assert!(!new.c.is_seeking(), "same-frame seek enters Holding, not Seeking");
    new.send(Event::Step(1));
    new.run(Apply);
    assert_eq!(new.fake.snaps.len(), 1, "step after Holding same-frame seek snaps to shown frame");
    let a4 = new.fake.snaps[0].0;

    // 5. Expiry past SEEK_PATIENCE clears Settle::Snapping so a late SnapDone is ignored.
    new.run(Wait(3));
    new.run(Pull(Script { frame: Frame::Same, item_time: 0.68, current: 0.68, player_rate: 0.0, ..Script::READY }));
    assert!(!new.c.is_seeking());
    new.snap_done(a4, true);
    assert!(new.fake.steps.is_empty(), "expired snap attempt changes nothing");
}

#[test]
fn vfr_clip_steps_via_snap_then_native_step_by_without_assuming_constant_frame_duration() {
    let now = rune_core::time::now();
    let mut new = New::new(false, now);
    // VFR clip: 1.0 s total, minFrameDuration = 0.125 s, real frames at 0.000, 0.125, 0.500, 0.625 s.
    new.c.open(Ok(Meta { duration: 1.0, size: Some((64, 36)), frame: Some(0.125), sound: false }));
    new.run(Play);
    new.run(Pull(Script { frame: Frame::New(0.0), item_time: 0.0, current: 0.0, ..Script::READY }));
    new.run(Pause);
    new.run(Apply);

    // Exact seek to 0.40 s (second half of long frame 1 [0.125, 0.500)): shown = 0.125.
    new.send(Event::Seek { to: 0.40, exact: true });
    new.run(Apply);
    new.run(Pull(Script { frame: Frame::New(0.125), item_time: 0.40, current: 0.40, player_rate: 0.0, ..Script::READY }));
    assert_eq!(new.c.presented, Some(0.125));

    // Step(+1): snaps to 0.125 then calls native step_by(+1) (which lands on VFR frame 2 at 0.500,
    // unlike a nominal-duration seek to 0.3125 which would stay stuck inside frame 1).
    new.fake.snaps.clear();
    new.fake.steps.clear();
    new.fake.seeks.clear();
    new.send(Event::Step(1));
    new.run(Apply);
    assert_eq!(new.fake.snaps.len(), 1);
    assert_eq!(new.fake.snaps[0].1, 1);
    new.snap_done(new.fake.snaps[0].0, true);
    assert_eq!(new.fake.steps, [1]);
    assert!(new.fake.seeks.is_empty());
    new.run(Pull(Script { frame: Frame::New(0.500), item_time: 0.500, current: 0.500, player_rate: 0.0, ..Script::READY }));
    assert_eq!(new.c.presented, Some(0.500));

    // Follow-up step(-1) is step-after-step (Aligned): direct step_by(-1) with no snap.
    new.fake.snaps.clear();
    new.send(Event::Step(-1));
    new.run(Apply);
    assert!(new.fake.snaps.is_empty());
    assert_eq!(new.fake.steps, [1, -1]);
}
