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