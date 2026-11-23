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
