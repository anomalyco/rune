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
fn a_presented_picture_is_the_only_one_queued_and_the_one_the_layer_shows() {
    let mut inbox = inbox(&[(2, 9.0), (2, 9.1)]);
    let now = rune_core::time::now();
    inbox.push_presented(picture(2, 9.2), now);
    assert_eq!(times(&inbox), vec![9.2]);
    assert_eq!(inbox.unread.iter().map(|f| f.pts).collect::<Vec<_>>(), vec![9.0, 9.1]);
    assert_eq!(inbox.touched, Some(now));
    assert_eq!(inbox.presented(2), Some(9.2));
    // A seek since: nothing of the new one was presented yet.
    assert_eq!(inbox.presented(3), None);
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

fn unread(inbox: &Inbox) -> Vec<f64> {
    inbox.unread.iter().map(|f| f.pts).collect()
}

#[test]
fn keeping_the_newest_hands_back_the_older_pictures() {
    let mut inbox = inbox(&[(2, 9.0), (2, 9.1), (2, 9.2)]);
    inbox.keep_newest();
    assert_eq!(unread(&inbox), vec![9.0, 9.1]);
    assert_eq!(times(&inbox), vec![9.2]);
    inbox.keep_newest();
    assert_eq!(unread(&inbox), vec![9.0, 9.1]);
}

#[test]
fn pictures_of_earlier_seeks_are_handed_back_undrawn() {
    let mut inbox = inbox(&[(1, 4.0), (1, 4.1), (2, 9.0)]);
    assert!(inbox.take(2, Pick::Hold).is_none());
    assert_eq!(unread(&inbox), vec![4.0, 4.1]);
}

#[test]
fn late_pictures_a_due_pick_skips_are_handed_back_undrawn() {
    let mut inbox = inbox(&[(2, 9.0), (2, 9.1), (2, 9.2), (2, 9.3)]);
    assert_eq!(inbox.take(2, Pick::Due(9.25)).map(|f| f.pts), Some(9.2));
    assert_eq!(unread(&inbox), vec![9.0, 9.1]);
    assert_eq!(times(&inbox), vec![9.3]);
}

#[test]
fn a_flush_hands_back_the_pictures_it_drops() {
    let mut inbox = inbox(&[(2, 9.0), (2, 9.1)]);
    inbox.flush();
    assert_eq!(unread(&inbox), vec![9.0, 9.1]);
}

#[test]
fn a_picture_taken_for_the_surface_is_not_handed_back() {
    let mut inbox = inbox(&[(2, 9.0), (2, 9.1)]);
    assert!(inbox.take(2, Pick::First).is_some());
    assert!(inbox.take(2, Pick::Due(9.1)).is_some());
    assert!(inbox.unread.is_empty());
}

#[test]
fn the_inbox_is_unlocked_while_the_source_seeks_and_only_the_newest_seek_stays() {
    let pipe = Pipe::new(inbox(&[(1, 4.0), (1, 4.1)]));
    let mut asked = pipe.lock();
    asked.seek = Some((9.0, 2));
    asked.ended = Some(1);
    let mut landed = Vec::new();
    let after = pipe
        .land_seeks(asked, |to, generation| {
            // The UI thread's part while the source moves: it takes the lock without waiting,
            // and during the first seek it asks for a newer one.
            let mut ui = pipe.inbox.try_lock().expect("the inbox is locked while the source seeks");
            assert!(ui.frames.is_empty() && ui.ended.is_none());
            if landed.is_empty() {
                ui.seek = Some((12.0, 3));
            }
            landed.push((to, generation));
            Ok::<(), String>(())
        })
        .unwrap();
    assert_eq!(landed, vec![(9.0, 2), (12.0, 3)]);
    assert!(after.seek.is_none());
    assert_eq!(unread(&after), vec![4.0, 4.1]);
}

#[test]
fn a_source_that_fails_to_seek_leaves_the_inbox_unlocked() {
    let pipe = Pipe::new(Inbox::default());
    let mut asked = pipe.lock();
    asked.seek = Some((9.0, 2));
    assert_eq!(pipe.land_seeks(asked, |_, _| Err("seek: no")).err(), Some("seek: no"));
    assert!(pipe.inbox.try_lock().is_ok());
}
