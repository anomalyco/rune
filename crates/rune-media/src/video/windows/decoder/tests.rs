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
    let (video, frame) = (opened.video.unwrap(), opened.picture.unwrap().frame);
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
