use super::*;
use crate::native_surface::{ColorInfo, Format};

fn picture(generation: u32, pts: f64) -> Decoded {
    let bytes = ByteFrame { width: 2, height: 2, format: Format::Nv12Video, color: ColorInfo::default(), planes: [vec![0; 4], vec![0; 2]] };
    Decoded { generation, pts, picture: Pic::Cpu(bytes) }
}

fn inbox(pictures: &[(u32, f64)]) -> Inbox {
    let mut inbox = Inbox::default();
    for (generation, pts) in pictures {
        inbox.push(picture(*generation, *pts));
    }
    inbox
}

fn times(inbox: &Inbox) -> Vec<f64> {
    inbox.frames.iter().map(|f| f.pts).collect()
}

#[test]
fn a_take_drops_the_pictures_of_earlier_seeks_first() {
    let mut inbox = inbox(&[(1, 4.0), (1, 4.1), (2, 9.0), (2, 9.1)]);
    assert!(inbox.take(2, Pick::Hold).is_none());
    assert_eq!(times(&inbox), vec![9.0, 9.1]);
}

#[test]
fn the_first_pick_takes_the_front_picture() {
    let mut inbox = inbox(&[(1, 4.0), (2, 9.0), (2, 9.1)]);
    assert_eq!(inbox.take(2, Pick::First).map(|f| f.pts), Some(9.0));
    assert_eq!(times(&inbox), vec![9.1]);
}

#[test]
fn a_due_pick_takes_the_newest_picture_whose_time_has_come() {
    let mut inbox = inbox(&[(2, 9.0), (2, 9.1), (2, 9.2)]);
    assert!(inbox.take(2, Pick::Due(8.9)).is_none());
    assert_eq!(inbox.take(2, Pick::Due(9.15)).map(|f| f.pts), Some(9.1));
    assert_eq!(times(&inbox), vec![9.2]);
}

#[test]
fn the_newest_picture_is_the_current_seeks_last() {
    let inbox = inbox(&[(2, 9.0), (2, 9.1)]);
    assert_eq!(inbox.newest(2), Some(9.1));
    assert_eq!(inbox.newest(3), None);
}

#[test]
fn a_flush_empties_the_queue_and_forgets_the_end() {
    let mut inbox = inbox(&[(2, 9.0), (2, 9.1)]);
    inbox.ended = Some(2);
    inbox.flush();
    assert!(inbox.frames.is_empty());
    assert_eq!(inbox.ended, None);
}

#[test]
fn a_push_says_whether_the_queue_was_empty() {
    let mut inbox = Inbox::default();
    assert!(inbox.push(picture(1, 0.0)));
    assert!(!inbox.push(picture(1, 0.1)));
}

#[test]
fn keeping_the_newest_hands_back_the_older_pictures() {
    let mut inbox = inbox(&[(2, 9.0), (2, 9.1), (2, 9.2)]);
    let older: Vec<f64> = inbox.keep_newest().iter().map(|f| f.pts).collect();
    assert_eq!(older, vec![9.0, 9.1]);
    assert_eq!(times(&inbox), vec![9.2]);
    assert!(inbox.keep_newest().is_empty());
}
