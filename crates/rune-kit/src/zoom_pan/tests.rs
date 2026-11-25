use rune::Tx;
use rune::machine::Effects;

use super::gesture::{Effect, Event, Gesture, step};
use super::{Geom, View, ZoomPan};

/// A 784 × 600 tree in a 320 × 480 pane.
fn tree(viewport: [f32; 2], min_scale: f32) -> Geom {
    Geom { content: [784.0, 600.0], viewport, padding: 0.0, min_scale, max_scale: 8.0, grows: false }
}

fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-3 }

#[test]
fn untouched_fits_and_refits_on_resize() {
    let z = ZoomPan::new();
    z.measure(tree([392.0, 480.0], 0.25));
    assert!(z.fitted());
    assert!(close(z.scale(), 0.5));
    let [x, _, w, _] = z.content_rect();
    assert!(close(x, 0.0) && close(w, 392.0));
    z.measure(tree([784.0, 900.0], 0.25));
    assert!(close(z.scale(), 1.0), "a fit doesn't grow past the content's size by default");
}

#[test]
fn min_scale_bounds_the_fit_and_leaves_it_pannable() {
    let mut z = ZoomPan::new();
    z.measure(tree([320.0, 480.0], 0.5));
    assert!(close(z.scale(), 0.5));
    assert!(z.overflows());
    let mut tx = Tx::default();
    assert!(z.pan_by([1000.0, 0.0], &mut tx));
    // Panned as far as the left edge: 392 wide in 320 leaves 36 each way.
    assert!(close(z.content_rect()[0], 0.0));
    assert!(!z.fitted());
}

#[test]
fn a_touched_view_survives_a_resize() {
    let mut z = ZoomPan::new();
    z.measure(tree([392.0, 480.0], 0.25));
    let mut tx = Tx::default();
    z.zoom_at(1.0, [0.0, 0.0], &mut tx);
    assert!(!z.fitted());
    z.measure(tree([500.0, 480.0], 0.25));
    assert!(close(z.scale(), 1.0));
    z.fit(&mut tx);
    assert!(z.fitted());
    assert!(close(z.scale(), 500.0 / 784.0));
}

#[test]
fn zoom_keeps_the_point_under_the_pointer() {
    let mut z = ZoomPan::new();
    z.measure(tree([392.0, 480.0], 0.25));
    let at = [100.0, 200.0];
    let before = z.to_content(at);
    z.zoom_at(1.5, at, &mut Tx::default());
    let after = z.to_content(at);
    assert!(close(before[0], after[0]) && close(before[1], after[1]), "{before:?} {after:?}");
    let back = z.to_viewport(after);
    assert!(close(back[0], at[0]) && close(back[1], at[1]));
}

#[test]
fn a_pan_that_changes_nothing_stays_fitted() {
    let mut z = ZoomPan::new();
    z.measure(tree([1000.0, 1000.0], 0.25));
    assert!(!z.pan_by([20.0, 20.0], &mut Tx::default()));
    assert!(z.fitted());
}

#[test]
fn gesture_pans_past_the_threshold_and_double_click_fits() {
    let mut g = Gesture::default();
    assert!(step(&mut g, Event::Down { view: View::Fit }).is_empty());
    assert!(step(&mut g, Event::Moved { travel: [1.0, 1.0] }).is_empty());
    assert_eq!(step(&mut g, Event::Moved { travel: [5.0, 0.0] }), Effects::one(Effect::Pan([5.0, 0.0])));
    assert_eq!(step(&mut g, Event::Moved { travel: [6.0, 1.0] }), Effects::one(Effect::Pan([1.0, 1.0])));
    assert!(step(&mut g, Event::Up { clicks: 2 }).is_empty(), "a drag isn't a double-click");
    step(&mut g, Event::Down { view: View::Fit });
    assert_eq!(step(&mut g, Event::Up { clicks: 2 }), Effects::one(Effect::Fit));
}

#[test]
fn a_cancelled_pan_puts_the_view_back() {
    let held = View::Held { scale: 2.0, pan: [10.0, 0.0] };
    let mut g = Gesture::default();
    step(&mut g, Event::Down { view: held });
    step(&mut g, Event::Moved { travel: [40.0, 0.0] });
    assert_eq!(step(&mut g, Event::Cancel), Effects::one(Effect::Restore(held)));
    assert_eq!(g, Gesture::Idle);
}
