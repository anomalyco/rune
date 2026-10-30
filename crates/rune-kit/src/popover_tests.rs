use rune::{Cx, Env, Runtime, col, el, text};

use super::*;

const WIN: (f32, f32) = (400.0, 300.0);

#[test]
fn flips_above_near_the_bottom_edge() {
    let r = anchor_rect([20.0, 250.0, 60.0, 270.0], (100.0, 80.0), Placement::BottomStart, 4.0, WIN);
    assert_eq!(r, [20.0, 166.0, 120.0, 246.0]);
    let roomy = anchor_rect([20.0, 20.0, 60.0, 40.0], (100.0, 80.0), Placement::BottomStart, 4.0, WIN);
    assert_eq!(roomy, [20.0, 44.0, 120.0, 124.0]);
}

#[test]
fn flips_left_near_the_right_edge() {
    let r = anchor_rect([340.0, 100.0, 380.0, 120.0], (100.0, 80.0), Placement::Right, 4.0, WIN);
    assert_eq!(r[0], 236.0);
    assert_eq!(r[2], 336.0);
}

#[test]
fn clamps_inside_the_window() {
    // End-aligned under a trigger at the left edge would hang off the left; it flips to start.
    let r = anchor_rect([10.0, 20.0, 30.0, 40.0], (100.0, 40.0), Placement::BottomEnd, 4.0, WIN);
    assert_eq!(r[0], 10.0);
    // Centred over a trigger at the right edge shifts back to 8 points inside.
    let r = anchor_rect([380.0, 100.0, 398.0, 120.0], (100.0, 40.0), Placement::Bottom, 4.0, WIN);
    assert_eq!(r[2], 392.0);
}

#[test]
fn anchored_mounts_beside_its_trigger() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let surface = raised(&k, col().w(120.0).h(60.0).child(text("Menu")), k.radius.md);
        let layer = vec![anchored(&k, surface, "t", Placement::BottomStart, 4.0)];
        crate::root(col().padding(40.0).child(el().key("t").w(50.0).h(20.0)), layer)
    }
    let mut rt = Runtime::new((), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0);
    }
    let trigger = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap().target;
    let word = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Menu")).unwrap().target;
    assert!((word[0] - trigger[0]).abs() < 8.0, "{word:?} under {trigger:?}");
    assert!(word[1] >= trigger[1] + trigger[3] + 4.0, "{word:?} below {trigger:?}");
}
