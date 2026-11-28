use super::*;
use crate::ResizeAnchor;

#[test]
fn a_scale_change_keeps_the_logical_size() {
    // 980 x 760 points, dragged from a 150% laptop to a 100% display and back.
    assert_eq!(rescaled((1470, 1140), 1.5, 1.0), (980, 760));
    assert_eq!(rescaled((980, 760), 1.0, 1.5), (1470, 1140));
    // Odd scales round to the nearest pixel, and a round trip lands where it began.
    assert_eq!(rescaled((1225, 950), 1.25, 1.75), (1715, 1330));
    assert_eq!(rescaled(rescaled((1225, 950), 1.25, 1.75), 1.75, 1.25), (1225, 950));
    assert_eq!(rescaled((1001, 777), 1.0, 1.25), (1251, 971));
}

#[test]
fn a_maximized_or_fullscreen_scale_change_keeps_its_size() {
    // Normal window rescales from old DPI to new DPI.
    assert_eq!(scale_change_size((1470, 1140), 1.5, 1.0, false, false), (980, 760));
    assert_eq!(scale_change_size((980, 760), 1.0, 1.5, false, false), (1470, 1140));
    // Maximised window moved from 200% to 100% and back already carries the target monitor's size.
    assert_eq!(scale_change_size((2560, 1392), 2.0, 1.0, true, false), (2560, 1392));
    assert_eq!(scale_change_size((3840, 2064), 1.0, 2.0, true, false), (3840, 2064));
    // Full-screen window is also not resized by winit on WM_DPICHANGED.
    assert_eq!(scale_change_size((2560, 1440), 2.0, 1.0, false, true), (2560, 1440));
    assert_eq!(scale_change_size((2560, 1440), 1.0, 1.5, true, true), (2560, 1440));
}

const SCREEN: [f32; 4] = [0.0, 33.0, 1728.0, 1084.0];

#[test]
fn a_resize_keeps_the_top_edge_and_stays_on_screen() {
    let window = [100.0, 200.0, 400.0, 432.0];
    assert_eq!(target(window, Some((400.0, 632.0)), false, SCREEN, ResizeAnchor::TopLeft), [100.0, 200.0, 400.0, 632.0]);
    // Growing past the bottom of the screen lifts the window just enough.
    assert_eq!(target(window, Some((400.0, 1000.0)), false, SCREEN, ResizeAnchor::TopLeft), [100.0, 117.0, 400.0, 1000.0]);
    // Taller than the screen: the top pins to the visible top.
    assert_eq!(target(window, Some((400.0, 2000.0)), false, SCREEN, ResizeAnchor::TopLeft), [100.0, 33.0, 400.0, 2000.0]);
}

#[test]
fn bottom_anchor_keeps_bottom_edge_fixed_when_growing_and_shrinking() {
    let window = [100.0, 700.0, 400.0, 300.0]; // bottom = 1000.0
    // Growing moves the top upward while bottom stays at 1000.0.
    assert_eq!(target(window, Some((400.0, 500.0)), false, SCREEN, ResizeAnchor::Bottom), [100.0, 500.0, 400.0, 500.0]);
    assert_eq!(target(window, Some((400.0, 500.0)), false, SCREEN, ResizeAnchor::BottomLeft), [100.0, 500.0, 400.0, 500.0]);
    assert_eq!(target(window, Some((400.0, 500.0)), false, SCREEN, ResizeAnchor::BottomRight), [100.0, 500.0, 400.0, 500.0]);
    // Shrinking moves the top downward while bottom stays at 1000.0.
    assert_eq!(target(window, Some((400.0, 120.0)), false, SCREEN, ResizeAnchor::Bottom), [100.0, 880.0, 400.0, 120.0]);
    // Growing past the visible top clamps top to SCREEN[1] (33.0) without moving above the menu bar.
    assert_eq!(target(window, Some((400.0, 1050.0)), false, SCREEN, ResizeAnchor::Bottom), [100.0, 33.0, 400.0, 1050.0]);
    // Taller than the screen: bottom-anchored keeps the bottom pinned to visible bottom (33 + 1084 = 1117).
    assert_eq!(target(window, Some((400.0, 2000.0)), false, SCREEN, ResizeAnchor::Bottom), [100.0, 1117.0 - 2000.0, 400.0, 2000.0]);
}

#[test]
fn all_nine_resize_anchors_pin_their_edges_or_centers() {
    let window = [400.0, 400.0, 200.0, 100.0]; // right=600, bottom=500, center=(500, 450)
    let next = Some((300.0, 200.0));
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::TopLeft), [400.0, 400.0, 300.0, 200.0]);
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::Top), [350.0, 400.0, 300.0, 200.0]);
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::TopRight), [300.0, 400.0, 300.0, 200.0]);
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::Left), [400.0, 350.0, 300.0, 200.0]);
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::Center), [350.0, 350.0, 300.0, 200.0]);
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::Right), [300.0, 350.0, 300.0, 200.0]);
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::BottomLeft), [400.0, 300.0, 300.0, 200.0]);
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::Bottom), [350.0, 300.0, 300.0, 200.0]);
    assert_eq!(target(window, next, false, SCREEN, ResizeAnchor::BottomRight), [300.0, 300.0, 300.0, 200.0]);
    // Right-anchored wider than screen keeps the right edge pinned to visible right (1728.0).
    assert_eq!(target(window, Some((2000.0, 100.0)), false, SCREEN, ResizeAnchor::TopRight), [1728.0 - 2000.0, 400.0, 2000.0, 100.0]);
}

#[test]
fn a_window_already_off_screen_is_not_pulled_back() {
    let window = [-200.0, 900.0, 400.0, 400.0];
    // Shrinking leaves it where the user put it.
    assert_eq!(target(window, Some((400.0, 300.0)), false, SCREEN, ResizeAnchor::TopLeft), [-200.0, 900.0, 400.0, 300.0]);
    // Growing may go no further off than it already was.
    assert_eq!(target(window, Some((400.0, 600.0)), false, SCREEN, ResizeAnchor::TopLeft), [-200.0, 700.0, 400.0, 600.0]);
    // Bottom-anchored off-screen window preserves its existing bottom edge (1300.0).
    assert_eq!(target(window, Some((400.0, 300.0)), false, SCREEN, ResizeAnchor::Bottom), [-200.0, 1000.0, 400.0, 300.0]);
    assert_eq!(target(window, Some((400.0, 600.0)), false, SCREEN, ResizeAnchor::Bottom), [-200.0, 700.0, 400.0, 600.0]);
}

#[test]
fn centring_matches_appkit_regardless_of_resize_anchor() {
    // Measured from NSWindow.center on a 1728 x 1084 visible frame: a quarter of the spare
    // height above, three quarters below.
    for anchor in [ResizeAnchor::TopLeft, ResizeAnchor::Bottom, ResizeAnchor::BottomRight, ResizeAnchor::Center] {
        assert_eq!(target([0.0, 0.0, 400.0, 100.0], Some((400.0, 332.0)), true, [0.0, 0.0, 1728.0, 1084.0], anchor), [664.0, 188.0, 400.0, 332.0]);
        assert_eq!(target([0.0, 0.0, 400.0, 100.0], Some((300.0, 932.0)), true, [0.0, 0.0, 1728.0, 1084.0], anchor), [714.0, 38.0, 300.0, 932.0]);
        assert_eq!(target([5.0, 5.0, 800.0, 132.0], None, true, SCREEN, anchor), [464.0, 33.0 + 238.0, 800.0, 132.0]);
    }
}

#[test]
fn snap_anchored_eliminates_one_pixel_jitter_on_pinned_edges_across_spring_animation() {
    // At scale 2.0, 650.25 + 349.75 == 1000.0 exactly, yet naive snap rounds both half-pixels up
    // (650.5 + 350.0 = 1000.5). snap_anchored must keep bottom == 1000.0 and right == 500.0.
    let naive = snap([100.25, 650.25, 399.75, 349.75], 2.0);
    assert_eq!(naive[1] + naive[3], 1000.5, "naive snap jitters bottom edge by 1 device pixel");

    let bottom = snap_anchored([100.25, 650.25, 399.75, 349.75], 2.0, ResizeAnchor::Bottom);
    assert_eq!(bottom[1] + bottom[3], 1000.0, "Bottom anchor keeps bottom pixel exact");

    let br = snap_anchored([100.25, 650.25, 399.75, 349.75], 2.0, ResizeAnchor::BottomRight);
    assert_eq!(br[0] + br[2], 500.0, "BottomRight anchor keeps right pixel exact");
    assert_eq!(br[1] + br[3], 1000.0, "BottomRight anchor keeps bottom pixel exact");

    // Walk an entire spring resize + mid-flight retarget at 120 Hz on 1x, 1.5x, and 2x displays:
    // the bottom edge must never drift by even a subpixel on any frame.
    let spring = Some(Spring::new(0.3, 0.0));
    for scale in [1.0f32, 1.5, 2.0] {
        let from = snap_anchored([100.0, 700.0, 400.0, 300.0], scale, ResizeAnchor::Bottom);
        let to1 = snap_anchored(target(from, Some((400.0, 520.0)), false, SCREEN, ResizeAnchor::Bottom), scale, ResizeAnchor::Bottom);
        let anim1 = FrameAnim::retarget(None, spring, 0.0, from, to1);
        for step in 0..12 {
            let rect = snap_anchored(anim1.sample(step as f64 / 120.0).0, scale, ResizeAnchor::Bottom);
            assert!(((rect[1] + rect[3]) - 1000.0).abs() < 1e-4, "scale {scale} step {step}: {rect:?}");
        }
        // Retarget mid-flight to a shorter height (simulating fast typing in a launcher).
        let to2 = snap_anchored(target(anim1.to, Some((400.0, 180.0)), false, SCREEN, ResizeAnchor::Bottom), scale, ResizeAnchor::Bottom);
        let anim2 = FrameAnim::retarget(Some(&anim1), spring, 0.1, [0.0; 4], to2);
        for step in 12..140 {
            let rect = snap_anchored(anim2.sample(step as f64 / 120.0).0, scale, ResizeAnchor::Bottom);
            assert!(((rect[1] + rect[3]) - 1000.0).abs() < 1e-4, "scale {scale} retarget step {step}: {rect:?}");
        }
    }
}

#[test]
fn the_spring_lands_on_the_target_and_retargets_with_its_speed() {
    let spring = Some(Spring::new(0.3, 0.0));
    let anim = FrameAnim::new(spring, 1.0, [0.0, 0.0, 400.0, 400.0], [0.0; 4], [0.0, 0.0, 400.0, 600.0]);
    let (mid, v) = anim.sample(1.1);
    assert!(mid[3] > 400.0 && mid[3] < 600.0 && v[3] > 0.0, "{mid:?} {v:?}");
    assert_eq!(mid[0..3], [0.0, 0.0, 400.0]);
    assert!(!anim.done(1.1));
    assert!(anim.done(2.0));
    assert_eq!(anim.sample(2.0).0, anim.to);
    let back = FrameAnim::retarget(Some(&anim), spring, 1.1, [9.0; 4], [0.0, 0.0, 400.0, 400.0]);
    assert_eq!(back.sample(1.1).0, mid);
    assert!(back.sample(1.11).0[3] > mid[3], "keeps moving the way it was going for a moment");
    let jump = FrameAnim::retarget(Some(&anim), None, 1.1, [9.0; 4], [1.0; 4]);
    assert!(jump.done(1.1));
    assert_eq!(jump.sample(1.1).0, [1.0; 4]);
}

#[test]
fn a_drag_mid_flight_keeps_the_new_origin_and_the_size_carries_on() {
    let spring = Some(Spring::new(0.3, 0.0));
    let mut anim = FrameAnim::new(spring, 0.0, [10.0, 20.0, 400.0, 400.0], [0.0; 4], [10.0, 20.0, 400.0, 600.0]);
    let before = anim.sample(0.1).0;
    anim.shift(50.0, -5.0);
    let after = anim.sample(0.1).0;
    assert_eq!(after, [before[0] + 50.0, before[1] - 5.0, before[2], before[3]]);
    assert_eq!(anim.to, [60.0, 15.0, 400.0, 600.0]);
}

#[test]
fn requested_sizes_stay_inside_the_window_limits() {
    assert_eq!(clamp_size((300.0, 900.0), (420.0, 0.0), Some((800.0, 700.0))), (420.0, 700.0));
    assert_eq!(clamp_size((500.0, 100.0), (420.0, 200.0), None), (500.0, 200.0));
}

#[test]
fn later_sizes_win_and_centring_sticks() {
    let a = FrameRequest { size: Some((1.0, 2.0)), animate: true, spring: None, center: true, resize_drag: None };
    let b = FrameRequest { size: Some((3.0, 4.0)), animate: false, spring: None, center: false, resize_drag: None };
    assert_eq!(a.merge(b), FrameRequest { size: Some((3.0, 4.0)), animate: false, spring: None, center: true, resize_drag: None });
    assert_eq!(b.merge(FrameRequest { center: true, ..Default::default() }), FrameRequest { center: true, ..b });
}

#[test]
fn resize_drag_merges_with_size_and_center_and_later_edge_wins() {
    let resize = FrameRequest { resize_drag: Some(crate::ResizeEdge::SouthEast), ..Default::default() };
    let sized = FrameRequest { size: Some((500.0, 400.0)), animate: true, center: true, ..Default::default() };
    assert_eq!(resize.merge(sized), FrameRequest { resize_drag: Some(crate::ResizeEdge::SouthEast), ..sized });
    assert_eq!(sized.merge(resize), FrameRequest { resize_drag: Some(crate::ResizeEdge::SouthEast), ..sized });
    let newer = FrameRequest { resize_drag: Some(crate::ResizeEdge::NorthWest), ..Default::default() };
    assert_eq!(resize.merge(newer).resize_drag, Some(crate::ResizeEdge::NorthWest));
}
