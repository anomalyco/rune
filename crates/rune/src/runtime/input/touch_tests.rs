use super::*;
use crate::{ScrollSpec, col, el};

#[derive(Default)]
struct S { taps: Vec<usize>, drags: Vec<f32> }

fn view(_: &S, _: &mut Cx<S>) -> El {
    col().size(200.0, 300.0)
        .child(el().key("knob").size(200.0, 40.0).on_drag(|s: &mut S, d: &mut Drag, _| s.drags.push(d.dx)))
        .child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false })
            .children((0..30).map(|i| el().key(format!("row{i}")).w_full().h(40.0).on_press(move |s: &mut S| s.taps.push(i)))))
}

fn rig() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (200.0, 300.0), Env::default());
    rt.frame(0.0);
    rt
}

fn offset(rt: &Runtime<S>) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    rt.scroll.scrolls[&id].offset
}

/// A finger from (x, y) moving by `dy` over `steps` events 10 ms apart, then lifted.
fn swipe(rt: &mut Runtime<S>, (x, y): (f32, f32), dy: f32, steps: usize) {
    rt.touch(1, TouchPhase::Started, x, y, 0.0);
    for i in 1..=steps {
        rt.touch(1, TouchPhase::Moved, x, y + dy * i as f32 / steps as f32, i as f64 * 0.01);
    }
    rt.touch(1, TouchPhase::Ended, x, y + dy, steps as f64 * 0.01 + 0.005);
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_tap_clicks_what_it_lands_on_and_leaves_no_hover() {
    let mut rt = rig();
    rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 103.0, 72.0, 0.02);
    rt.touch(1, TouchPhase::Ended, 103.0, 72.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.taps, [0]);
    assert_eq!(rt.pointer(), None);
    assert!(!rt.touching());
}

#[test]
fn a_drag_on_a_list_scrolls_it_without_clicking_and_flings_on() {
    let mut rt = rig();
    swipe(&mut rt, (100.0, 200.0), -100.0, 5);
    assert!(rt.state.taps.is_empty(), "a pan presses nothing");
    let lifted = offset(&rt);
    assert!(lifted >= 90.0, "the list followed the finger: {lifted}");
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(offset(&rt) > lifted + 50.0, "and coasted on: {} after {lifted}", offset(&rt));
}

#[test]
fn a_finger_held_still_before_lifting_throws_nothing() {
    let mut rt = rig();
    rt.touch(1, TouchPhase::Started, 100.0, 200.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 100.0, 150.0, 0.01);
    rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.5);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert_eq!(offset(&rt), 50.0);
}

#[test]
fn a_drag_on_an_element_that_drags_drags_it_not_the_page() {
    let mut rt = rig();
    rt.touch(1, TouchPhase::Started, 20.0, 20.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 60.0, 30.0, 0.01);
    rt.touch(1, TouchPhase::Ended, 60.0, 30.0, 0.02);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.drags.last(), Some(&40.0));
    assert_eq!(offset(&rt), 0.0);
}

#[test]
fn a_second_finger_is_ignored_and_a_cancelled_touch_clicks_nothing() {
    let mut rt = rig();
    rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
    rt.touch(2, TouchPhase::Started, 100.0, 110.0, 0.0);
    rt.touch(2, TouchPhase::Ended, 100.0, 110.0, 0.01);
    rt.touch(1, TouchPhase::Cancelled, 100.0, 70.0, 0.02);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.taps.is_empty());
    assert!(!rt.touching());
}

#[test]
fn a_finger_past_the_slop_picks_one_mode_with_only_its_own_data() {
    let mut rt = rig();
    let list = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    rt.touch(1, TouchPhase::Started, 100.0, 200.0, 0.0);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Pending { scroll: Some(list) }), "pending, it holds the viewport under it");
    rt.touch(1, TouchPhase::Moved, 100.0, 150.0, 0.01);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Pan { scroll: list, horizontal: false }));
    rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.02);
    rt.touch(2, TouchPhase::Started, 20.0, 20.0, 1.0);
    rt.touch(2, TouchPhase::Moved, 60.0, 20.0, 1.01);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Drag), "on an element that drags it holds no viewport");
}
