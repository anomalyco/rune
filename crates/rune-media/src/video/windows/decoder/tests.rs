use super::*;

const FRAME: f64 = 1.0 / 30.0;

/// Feeds `landing` the pictures at `times` and returns the ones it keeps.
fn kept(landing: &mut Landing, times: &[f64]) -> Vec<f64> {
    times.iter().copied().filter(|t| landing.keeps(*t, FRAME)).collect()
}

#[test]
fn a_landing_keeps_the_picture_nearest_the_target_and_all_after_it() {
    let mut landing = Landing::at(1.0);
    assert_eq!(kept(&mut landing, &[0.9, 0.95, 0.99, 1.02, 1.06]), vec![0.99, 1.02, 1.06]);
    assert_eq!(landing.ended(), None);
}

#[test]
fn a_seek_past_the_last_picture_goes_back_once_for_it() {
    // A 10 s clip whose last picture is at 9.9667, and a seek to its duration.
    let mut landing = Landing::at(10.0);
    assert_eq!(kept(&mut landing, &[9.9, 9.9333, 9.9667]), Vec::<f64>::new());
    assert_eq!(landing.ended(), Some(9.9667));
    assert_eq!(landing.skip_before, 9.9667);
    // The second pass from the key frame keeps exactly the last picture.
    assert_eq!(kept(&mut landing, &[9.9, 9.9333, 9.9667]), vec![9.9667]);
    assert_eq!(landing.ended(), None);
}

#[test]
fn a_landing_goes_back_only_once() {
    let mut landing = Landing::at(10.0);
    assert!(!landing.keeps(9.0, FRAME));
    assert_eq!(landing.ended(), Some(9.0));
    // The second pass kept nothing either (a reader that won't give the picture again).
    assert_eq!(landing.ended(), None);
}

#[test]
fn a_stream_with_no_pictures_has_none_to_go_back_for() {
    assert_eq!(Landing::at(10.0).ended(), None);
}

#[test]
fn a_keyframe_landing_keeps_the_first_picture_without_discarding_frames() {
    let mut landing = Landing::keyframe();
    assert!(!landing.landed());
    // Keeps a keyframe well before any scrub target (and even a small negative edit-list PTS)
    // without discarding a single frame.
    assert_eq!(kept(&mut landing, &[-0.02, 0.0, 0.033, 0.066]), vec![-0.02, 0.0, 0.033, 0.066]);
    assert!(landing.landed());
    assert_eq!(landing.ended(), None);

    let mut empty = Landing::keyframe();
    assert_eq!(empty.ended(), None);
}

const SIXTIETH: f64 = 1.0 / 60.0;

#[test]
fn the_layer_skips_a_picture_whose_successor_is_already_due() {
    let just_presented = Some(Duration::from_millis(10));
    // On time, and late by less than a picture: shown.
    assert!(!superseded(4.99, 5.0, SIXTIETH, just_presented));
    assert!(!superseded(5.01, 5.0, SIXTIETH, just_presented));
    // A whole picture late: the next one is due, so this one is skipped.
    assert!(superseded(5.0 + SIXTIETH, 5.0, SIXTIETH, just_presented));
    assert!(superseded(6.2, 5.0, SIXTIETH, just_presented));
}

#[test]
fn the_layer_shows_a_late_picture_when_it_has_shown_none_for_a_while() {
    // The layer's first picture, however late.
    assert!(!superseded(6.2, 5.0, SIXTIETH, None));
    assert!(superseded(6.2, 5.0, SIXTIETH, Some(Duration::from_millis(249))));
    assert!(!superseded(6.2, 5.0, SIXTIETH, Some(LAYER_STARVED)));
}

/// A 60 fps clip on its layer, as `decode` plays it: the clock runs from 0, the decoder first
/// reads at `start` seconds and takes `cost` seconds a picture. Returns the time each picture
/// was presented at, with its own time.
fn layer_presents(start: f64, cost: f64, seconds: f64) -> Vec<(f64, f64)> {
    let (mut now, mut last, mut presents) = (start, None, Vec::new());
    for index in 0..(seconds / SIXTIETH) as u32 {
        let pts = f64::from(index) * SIXTIETH;
        now += cost;
        if superseded(now, pts, SIXTIETH, last.map(|last: f64| Duration::from_secs_f64(now - last))) {
            continue;
        }
        // `wait_until_due`, then the present.
        now = now.max(pts);
        presents.push((now, pts));
        last = Some(now);
    }
    presents
}

#[test]
fn a_decoder_that_fell_behind_on_the_layer_jumps_to_the_clock_instead_of_playing_its_backlog_fast() {
    // Held back for 1.2 s, then 2 ms a picture.
    let presents = layer_presents(1.2, 0.002, 4.0);
    // No backlog played fast. Pictures closer together than a picture's time: at most the one
    // that was less than a picture late as the decoder caught up, and its successor on time.
    let rushed = presents.windows(2).filter(|pair| pair[1].0 - pair[0].0 < SIXTIETH - 1e-9).count();
    assert!(rushed <= 1, "{rushed} pictures rushed: {:?}", &presents[..presents.len().min(12)]);
    // And back on the clock within half a second: each picture at its own time.
    let caught_up: Vec<_> = presents.iter().filter(|(at, _)| *at > 1.7).collect();
    assert!(caught_up.len() > 100, "{} pictures after catching up", caught_up.len());
    assert!(caught_up.iter().all(|(at, pts)| (at - pts).abs() < 1e-9));
}

#[test]
fn a_decoder_slower_than_the_clip_still_shows_a_picture_every_quarter_second() {
    // 20 ms a picture for a clip that has one every 16.7 ms: every picture is late.
    let presents = layer_presents(0.5, 0.02, 4.0);
    assert!(presents.len() > 10, "{presents:?}");
    for pair in presents.windows(2) {
        assert!(pair[1].0 - pair[0].0 <= LAYER_STARVED.as_secs_f64() + 0.02 + 1e-9, "presented {:?} then {:?}", pair[0], pair[1]);
    }
}

#[test]
fn each_stream_is_read_for_its_own_need() {
    assert_eq!(next_read(false, false, true, 0), None);
    // The picture queue is full (or the layer waits): the sound still reads ahead.
    assert_eq!(next_read(true, false, true, 0), Some(Stream::Sound));
    assert_eq!(next_read(false, true, true, 0), Some(Stream::Picture));
    // Both: the sound first, a few samples at a time.
    assert_eq!(next_read(true, true, true, 0), Some(Stream::Sound));
    assert_eq!(next_read(true, true, true, SOUND_BURST - 1), Some(Stream::Sound));
    assert_eq!(next_read(true, true, true, SOUND_BURST), Some(Stream::Picture));
    // A seek's first picture comes before its sound.
    assert_eq!(next_read(true, true, false, 0), Some(Stream::Picture));
    assert_eq!(next_read(true, false, false, 0), Some(Stream::Sound));
}

/// One AAC packet at 48 kHz, in seconds.
const PACKET: f64 = 1024.0 / 48_000.0;
/// How far ahead of the clock the sound output writes: its 100 ms buffer and the device's latency.
const WRITES_AHEAD: f64 = 0.12;

/// What a read policy sees.
struct Reads {
    /// The sound output holds less than a second.
    sound: bool,
    /// The picture queue has room.
    picture: bool,
    burst: u32,
    /// The times of the next sample of each stream.
    next_sound: f64,
    next_picture: f64,
}

/// What a played clip did.
struct Played {
    /// The least sound queued ahead of the clock, after the first two seconds.
    sound_ahead: f64,
    /// The longest the screen went without a new picture.
    picture_gap: f64,
}

/// Plays a clip with `fps` pictures a second and AAC sound for six seconds in 1 ms steps, the
/// decoder reading by `policy` (up to four samples a step). Off the layer the window takes
/// each picture at its time from a queue of three; on it the decoder waits for each picture's
/// time itself. The sound's stream starts `sound_behind` seconds behind the clock (0: with it).
fn play(fps: f64, layer: bool, sound_behind: f64, policy: impl Fn(&Reads) -> Option<Stream>) -> Played {
    let frame = 1.0 / fps;
    let (mut pictures, mut next_picture, mut next_sound, mut sound_until) = (std::collections::VecDeque::new(), 0.0, -sound_behind, -sound_behind);
    let (mut burst, mut busy_until, mut shown_at) = (0, 0.0, 0.0);
    let mut played = Played { sound_ahead: f64::MAX, picture_gap: 0.0 };
    for step in 0..6000 {
        let now = f64::from(step) / 1000.0;
        // The window's frame.
        while pictures.front().is_some_and(|pts| *pts <= now) {
            pictures.pop_front();
            shown_at = now;
        }
        for _ in 0..4 {
            if now < busy_until {
                break;
            }
            let reads = Reads { sound: sound_until - (now + WRITES_AHEAD) < 1.0, picture: layer || pictures.len() < 3, burst, next_sound, next_picture };
            match policy(&reads) {
                Some(Stream::Sound) => {
                    burst += 1;
                    next_sound += PACKET;
                    sound_until = next_sound;
                }
                Some(Stream::Picture) => {
                    burst = 0;
                    if layer {
                        // `wait_until_due`, then the present.
                        busy_until = next_picture;
                        shown_at = now.max(next_picture);
                    } else {
                        pictures.push_back(next_picture);
                    }
                    next_picture += frame;
                }
                None => break,
            }
        }
        if now >= 2.0 {
            played.sound_ahead = played.sound_ahead.min(sound_until - now);
        }
        played.picture_gap = played.picture_gap.max(now - shown_at);
    }
    played
}

/// Reads as they come in the file, only while the picture queue has room: how the decoder read
/// before each stream had its own need.
fn as_they_come(reads: &Reads) -> Option<Stream> {
    reads.picture.then_some(if reads.next_sound <= reads.next_picture { Stream::Sound } else { Stream::Picture })
}

fn for_its_own_need(reads: &Reads) -> Option<Stream> {
    next_read(reads.sound, reads.picture, true, reads.burst)
}

#[test]
fn sound_read_only_with_the_pictures_comes_too_late_at_60_fps_and_on_the_layer() {
    // Three pictures ahead are 50 ms at 60 fps: less than the sound output writes ahead.
    assert!(play(60.0, false, 0.0, as_they_come).sound_ahead < WRITES_AHEAD);
    // On the layer the decoder is one picture ahead, at any rate.
    assert!(play(60.0, true, 0.0, as_they_come).sound_ahead < WRITES_AHEAD);
    assert!(play(24.0, true, 0.0, as_they_come).sound_ahead < WRITES_AHEAD);
}

#[test]
fn the_decoder_keeps_the_sound_a_second_ahead_at_any_picture_rate_and_on_the_layer() {
    for fps in [24.0, 30.0, 60.0, 120.0] {
        for layer in [false, true] {
            let played = play(fps, layer, 0.0, for_its_own_need);
            assert!(played.sound_ahead > 0.9, "{fps} fps, layer {layer}: {} s of sound ahead", played.sound_ahead);
            assert!(played.picture_gap <= 2.0 / fps + 0.002, "{fps} fps, layer {layer}: {} s without a picture", played.picture_gap);
        }
    }
}

#[test]
fn sound_with_a_lot_to_catch_up_on_never_holds_the_pictures_back() {
    // Turned on a minute into the clip, the audio stream reads from the clip's start.
    for layer in [false, true] {
        let played = play(60.0, layer, 60.0, for_its_own_need);
        assert!(played.picture_gap <= 2.0 / 60.0 + 0.002, "layer {layer}: {} s without a picture", played.picture_gap);
    }
    // Read as they came, the same sound stopped the pictures until it had caught up.
    assert!(play(60.0, false, 60.0, as_they_come).picture_gap > 0.5);
}

/// Reads the fixture clip as `Decoder::decode` reads a clip for a seek to its duration, with
/// Media Foundation's software decoder, and returns the times of the pictures kept, how often it
/// went back, the duration and the seconds a picture lasts.
fn land_at_the_duration() -> (Vec<f64>, u32, f64, f64) {
    // SAFETY: COM on this test's thread for the reader; left initialised, as the thread may open more.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().ok();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures/rgbw.mp4");
    let lib = mf::library().unwrap();
    let source = lib.resolve(&mf::Target::from_source(&MediaSource::File(path)).unwrap()).unwrap();
    let opened = mf::open(lib, &source, Kind::Video, Decode::Cpu, false).unwrap();
    let (video, frame) = (opened.video.unwrap(), opened.picture.unwrap().frame());
    let mut landing = Landing::at(opened.duration);
    mf::seek(&opened.reader, opened.duration).unwrap();
    let (mut kept, mut went_back) = (Vec::new(), 0);
    loop {
        let read = mf::read(&opened.reader, video).unwrap();
        if read.sample.is_some() && landing.keeps(read.time, frame) {
            kept.push(read.time);
        }
        if read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
            match landing.ended() {
                Some(last) => {
                    went_back += 1;
                    mf::seek(&opened.reader, last).unwrap();
                }
                None => break,
            }
        }
    }
    (kept, went_back, opened.duration, frame)
}

#[test]
fn a_seek_to_a_real_clips_duration_lands_on_its_last_picture() {
    let (kept, went_back, duration, frame) = land_at_the_duration();
    // The first pass keeps nothing: the last picture starts a whole picture before the duration.
    assert_eq!(went_back, 1);
    assert_eq!(kept.len(), 1, "{kept:?} of a {duration} s clip");
    assert!(kept[0] < duration && kept[0] >= duration - frame * 1.5, "{kept:?} of a {duration} s clip");
}

#[test]
fn a_real_clips_sound_reads_a_second_ahead_of_its_pictures_stream_by_stream() {
    // SAFETY: COM on this test's thread for the reader; left initialised, as the thread may open more.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().ok();
    // 60 pictures a second with AAC sound.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/rune.mp4");
    let lib = mf::library().unwrap();
    let source = lib.resolve(&mf::Target::from_source(&MediaSource::File(path)).unwrap()).unwrap();
    let opened = mf::open(lib, &source, Kind::Video, Decode::Cpu, true).unwrap();
    let (video, audio) = (opened.video.unwrap(), opened.audio.unwrap());
    // The sound alone, a second of it: no picture is read for it.
    let mut heard = Vec::new();
    while heard.last().is_none_or(|time| *time < 1.0) {
        let read = mf::read(&opened.reader, audio).unwrap();
        assert_eq!(read.stream, audio);
        assert!(read.sample.is_some() && read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 == 0);
        heard.push(read.time);
    }
    assert!(heard[0] < 0.05 && heard.is_sorted(), "{heard:?}");
    // The pictures then read from the clip's start.
    let picture = mf::read(&opened.reader, video).unwrap();
    assert_eq!(picture.stream, video);
    assert!(picture.sample.is_some() && picture.time < 0.1, "{}", picture.time);
    // And the sound goes on from where it was.
    let next = mf::read(&opened.reader, audio).unwrap();
    assert!((1.0..1.1).contains(&next.time), "{}", next.time);
}

#[test]
fn a_muted_clip_keeps_its_audio_stream_selected_and_with_no_sound_output() {
    // SAFETY: COM on this test's thread for the reader; left initialised, as the thread may open more.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().ok();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/rune.mp4");
    let job = Job {
        source: MediaSource::File(path),
        kind: Kind::Video,
        start: 0.0,
        exact: true,
        generation: 0,
        looping: false,
        sound: None,
        notify: Box::new(|| {}),
        clock: Arc::new(audio::Clock::default()),
        started: rune_core::time::now(),
    };
    let d = Decoder::open(&job).unwrap();
    // Muted: no WASAPI output opened (matching Chrome's suspended real sink).
    assert!(d.output.is_none());
    // Audio stream stays selected so unmuting mid-clip does not restart from 0.
    let audio = d.audio.expect("demo/rune.mp4 has an audio track");
    assert!(d.audio_open, "muted clip deselected its audio stream");
    // SAFETY: plain selection query on this thread's reader.
    let selected = unsafe { d.reader.GetStreamSelection(audio) }.unwrap();
    assert!(selected.as_bool(), "Media Foundation audio stream was deselected while muted");
}

#[test]
fn muted_audio_target_stays_behind_both_clock_and_video_position() {
    // 60 fps off-layer: front is one frame ahead of screen, capped by both clock and screen.
    let t = muted_audio_target(Some(60.008), Some(60.017), SIXTIETH, false, true).unwrap();
    assert!((t - (60.017 - SIXTIETH - MUTE_MARGIN)).abs() < 1e-6, "{t}");

    // 1 fps off-layer: front is 1 s ahead, so clock caps it and future sound isn't dropped.
    let t = muted_audio_target(Some(60.2), Some(61.0), 1.0, false, true).unwrap();
    assert!((t - (60.0 - MUTE_MARGIN)).abs() < 1e-6, "{t}");

    // Undrawn muted clip: clock ran ahead while picture queue froze at 1.017; stops at screen.
    let t = muted_audio_target(Some(3.0), Some(1.017), SIXTIETH, false, true).unwrap();
    assert!((t - (1.017 - SIXTIETH - MUTE_MARGIN)).abs() < 1e-6, "{t}");

    // On the layer: front is the presented picture itself.
    let t = muted_audio_target(Some(60.002), Some(60.0), SIXTIETH, true, true).unwrap();
    assert!((t - (60.0 - MUTE_MARGIN)).abs() < 1e-6, "{t}");

    // Paused after seek to 60.0 (no clock): advances up to 60.0 - frame - margin, then stops.
    let t = muted_audio_target(None, Some(60.0), SIXTIETH, false, true).unwrap();
    assert!((t - (60.0 - SIXTIETH - MUTE_MARGIN)).abs() < 1e-6, "{t}");
    assert!(!muted_audio_want(Some(t), 0.0, Some(t)));

    // Paused at 0.0: target is below 0, so no audio is read at all.
    let t = muted_audio_target(None, Some(0.0), SIXTIETH, false, true);
    assert!(!muted_audio_want(None, 0.0, t));

    // Loop transition (epoch = 10.0) while last frames of loop 1 still show (9.95):
    // does not read or discard loop 2's first packet.
    let t = muted_audio_target(Some(9.95), Some(9.967), SIXTIETH, false, true);
    assert!(!muted_audio_want(None, 10.0, t));

    // Audio-only clip muted follows the clock while playing and stops when paused.
    assert_eq!(muted_audio_target(Some(5.0), None, FRAME, false, false), Some(5.0 - MUTE_MARGIN));
    assert_eq!(muted_audio_target(None, None, FRAME, false, false), None);
}

#[test]
fn clip_done_ends_muted_video_with_pictures_and_waits_for_sound_when_unmuted_or_audio_only() {
    // Muted video ends/loops as soon as video hits EOS even if audio is still open.
    assert!(clip_done(false, true, true, false));
    assert!(!clip_done(true, false, true, false));
    // Unmuted video waits for both streams.
    assert!(!clip_done(false, true, true, true));
    assert!(clip_done(false, false, true, true));
    // Audio-only clip (muted or unmuted) waits for its audio stream.
    assert!(!clip_done(false, true, false, false));
    assert!(clip_done(false, false, false, false));
    assert!(!clip_done(false, true, false, true));
    assert!(clip_done(false, false, false, true));
}

#[test]
fn trailing_audio_extends_drain_end_and_next_loop_epoch() {
    // Audio ends at 12.5 s on a 10 s video clip: drain_end and next_loop_epoch reach 12.5 s.
    assert_eq!(drain_end(0.0, 10.0, Some(10.0), Some(12.5)), Some(12.5));
    assert_eq!(next_loop_epoch(0.0, 10.0, Some(12.5)), 12.5);
    // Audio shorter than video keeps duration end and regular loop epoch.
    assert_eq!(drain_end(0.0, 10.0, Some(10.0), Some(8.0)), Some(10.0));
    assert_eq!(next_loop_epoch(0.0, 10.0, Some(8.0)), 10.0);
    // Muted (no active audio end) uses duration/video end.
    assert_eq!(drain_end(0.0, 10.0, Some(10.0), None), Some(10.0));
    assert_eq!(next_loop_epoch(0.0, 10.0, None), 10.0);
    // Unknown duration (0.0) falls back to max(video_end, audio_end).
    assert_eq!(drain_end(0.0, 0.0, Some(9.967), Some(12.5)), Some(12.5));
}

#[test]
fn layer_due_follows_nudged_clock_at_same_rate_and_stops_on_pause_or_rate_change() {
    let at = rune_core::time::now();
    let initial = Anchor { at, pos: 2.0, rate: 1.0 };
    let base_due = layer_due(initial, Some(initial), 2.5).unwrap();
    // Audio nudged clock forward by 10 ms: picture at 2.5 s is due 10 ms earlier.
    let nudged = Anchor { pos: 2.01, ..initial };
    let nudged_due = layer_due(initial, Some(nudged), 2.5).unwrap();
    let shift = base_due.saturating_duration_since(nudged_due).as_secs_f64();
    assert!((shift - 0.010).abs() < 1e-6, "expected 10 ms earlier due, got {shift}");
    // Paused or rate changed: returns None so the window takes over.
    assert_eq!(layer_due(initial, None, 2.5), None);
    assert_eq!(layer_due(initial, Some(Anchor { rate: 2.0, ..initial }), 2.5), None);
}

#[test]
fn a_real_clip_unmuted_mid_clip_reads_audio_at_playback_position_without_seeking_video() {
    // SAFETY: COM on this test's thread for the reader; left initialised, as the thread may open more.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().ok();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/rune.mp4");
    let lib = mf::library().unwrap();
    let source = lib.resolve(&mf::Target::from_source(&MediaSource::File(path)).unwrap()).unwrap();
    let opened = mf::open(lib, &source, Kind::Video, Decode::Cpu, true).unwrap();
    let (video, audio, frame) = (opened.video.unwrap(), opened.audio.unwrap(), opened.picture.unwrap().frame());

    // Play muted up to 3.0 s: audio stream stays selected and advances via muted_audio_target,
    // without queuing samples or opening WASAPI.
    let mut video_pts = 0.0;
    let mut audio_pts: Option<f64> = None;
    let mut audio_reads = 0u32;
    while video_pts < 3.0 {
        let pic = mf::read(&opened.reader, video).unwrap();
        assert_eq!(pic.stream, video);
        assert!(pic.sample.is_some());
        video_pts = pic.time;
        let target = muted_audio_target(Some(video_pts), Some(video_pts), frame, false, true);
        while muted_audio_want(audio_pts, 0.0, target) {
            let snd = mf::read(&opened.reader, audio).unwrap();
            assert_eq!(snd.stream, audio);
            assert!(snd.sample.is_some());
            audio_pts = Some(snd.time);
            audio_reads += 1;
        }
    }
    assert!(audio_reads > 0);

    // Unmute at 3.0 s: the very next audio sample is right behind 3.0 s (not 0.0 s), and the
    // video stream continues past 3.0 s without any seek or gap.
    let next_audio = mf::read(&opened.reader, audio).unwrap();
    assert_eq!(next_audio.stream, audio);
    assert!(
        (2.8..3.05).contains(&next_audio.time),
        "expected audio near 3.0 s on mid-clip unmute, got {} s",
        next_audio.time
    );
    let next_video = mf::read(&opened.reader, video).unwrap();
    assert_eq!(next_video.stream, video);
    assert!(
        next_video.time >= video_pts && next_video.time < video_pts + frame * 2.0,
        "video jumped on unmute: was {video_pts} s, next {} s",
        next_video.time
    );
}

#[test]
fn a_real_clip_keyframe_seek_lands_on_the_first_decoded_frame_without_skipping_forward() {
    // SAFETY: COM on this test's thread for the reader; left initialised, as the thread may open more.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().ok();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/rune.mp4");
    let lib = mf::library().unwrap();
    let source = lib.resolve(&mf::Target::from_source(&MediaSource::File(path)).unwrap()).unwrap();
    let opened = mf::open(lib, &source, Kind::Video, Decode::Cpu, false).unwrap();
    let (video, frame) = (opened.video.unwrap(), opened.picture.unwrap().frame());

    // Pick a target mid-clip where MF seeks back to an earlier keyframe.
    let target = (opened.duration * 0.5).min(1.5);
    assert!(target > frame * 2.0, "fixture too short: {}", opened.duration);

    // Keyframe-only seek: keeps the very first picture returned after mf::seek.
    mf::seek(&opened.reader, target).unwrap();
    let mut fast = Landing::keyframe();
    let mut fast_reads = 0u32;
    let fast_pts = loop {
        let read = mf::read(&opened.reader, video).unwrap();
        assert!(read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 == 0, "unexpected EOS during fast seek");
        if read.sample.is_some() {
            fast_reads += 1;
            if fast.keeps(read.time, frame) {
                break read.time;
            }
        }
    };
    assert_eq!(fast_reads, 1, "keyframe seek must keep the first decoded picture");
    assert!(fast_pts <= target + frame * 0.5, "keyframe at {fast_pts} s should be at or before target {target} s");

    // Exact seek to the same target: lands within half a frame of target, reading at least as many frames.
    mf::seek(&opened.reader, target).unwrap();
    let mut exact = Landing::at(target);
    let mut exact_reads = 0u32;
    let exact_pts = loop {
        let read = mf::read(&opened.reader, video).unwrap();
        assert!(read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 == 0, "unexpected EOS during exact seek");
        if read.sample.is_some() {
            exact_reads += 1;
            if exact.keeps(read.time, frame) {
                break read.time;
            }
        }
    };
    assert!((exact_pts - target).abs() <= frame, "exact seek landed at {exact_pts} s for target {target} s");
    assert!(exact_reads >= fast_reads, "exact seek ({exact_reads}) must read at least as many frames as keyframe seek ({fast_reads})");
    if fast_pts + frame * 0.5 < target {
        assert!(exact_reads > 1, "mid-GOP target {target} s after keyframe {fast_pts} s should decode intermediate frames for exact seek");
    }
}

/// Multiple Media Foundation CPU readers seeking concurrently must never deadlock inside
/// `IMFSourceReader::ReadSample`. Each worker runs behind a bounded timeout so a regression
/// fails with a message instead of hanging the test binary.
#[test]
fn parallel_cpu_readers_seeking_repeatedly_complete_within_timeout() {
    use std::sync::{Barrier, mpsc};
    const WORKERS: usize = 6;
    const TIMEOUT: Duration = Duration::from_secs(15);

    let rgbw = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures/rgbw.mp4");
    let demo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/rune.mp4");
    let barrier = Arc::new(Barrier::new(WORKERS));
    let (tx, rx) = mpsc::channel();

    for id in 0..WORKERS {
        let path = if id % 2 == 0 { rgbw.clone() } else { demo.clone() };
        let barrier = barrier.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            // SAFETY: MTA COM for this worker thread; matched on exit after all COM objects drop.
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            let result = (|| -> Result<(), String> {
                let lib = mf::library()?;
                let source = lib.resolve(&mf::Target::from_source(&MediaSource::File(path))?)?;
                let opened = mf::open(lib, &source, Kind::Video, Decode::Cpu, false)?;
                let video = opened.video.ok_or("no video track")?;
                let targets: &[f64] = if id % 2 == 0 {
                    &[0.125, 0.375, 0.125, 0.625, 0.375, 0.125, 0.875, 0.375]
                } else {
                    &[0.5, 0.0, 1.0, 0.25, 0.75, 0.0]
                };
                barrier.wait();
                for _round in 0..3 {
                    for &at in targets {
                        mf::seek(&opened.reader, at)?;
                        loop {
                            let read = mf::read(&opened.reader, video)?;
                            if read.sample.is_some() || read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
                                break;
                            }
                        }
                    }
                }
                Ok(())
            })();
            if com {
                // SAFETY: matches `CoInitializeEx` above after `opened` and `source` dropped.
                unsafe { CoUninitialize() };
            }
            // The receiver may have already timed out and dropped.
            tx.send((id, result)).ok();
        });
    }
    drop(tx);

    let deadline = rune_core::time::now() + TIMEOUT;
    let mut completed = 0usize;
    while completed < WORKERS {
        let left = deadline.saturating_duration_since(rune_core::time::now());
        match rx.recv_timeout(left) {
            Ok((id, Ok(()))) => {
                let _ = id;
                completed += 1;
            }
            Ok((id, Err(e))) => panic!("parallel MF seek worker {id} failed: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!(
                    "parallel MF seek timed out after {}s ({completed}/{WORKERS} workers completed): IMFSourceReader::ReadSample deadlocked",
                    TIMEOUT.as_secs()
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("parallel MF seek worker panicked ({completed}/{WORKERS} completed)");
            }
        }
    }
}


