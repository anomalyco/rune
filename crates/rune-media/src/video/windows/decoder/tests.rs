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