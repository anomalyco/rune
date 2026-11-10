use super::*;
use crate::growth::GROWTH;

/// A row growth from `h` toward `target`.
fn growth(h: f32, target: f32) -> GrowTrack {
    GrowTrack { target, ..GrowTrack::settled(h) }
}

#[test]
fn a_slide_shrinks_with_the_growth_left_to_open() {
    let phase = SlidePhase::Following { dy: -20.0, v: 0.0, at: 0.0 };
    let next = phase.step(&growth(50.0, 100.0), GROWTH, 12);
    let SlidePhase::Following { dy, v, at } = next else { panic!("still follows: {next:?}") };
    assert_eq!((dy, at), (-10.0, 50.0), "half the growth left, half the slide");
    assert!((v - 10.0 / (12.0 * SUBSTEP)).abs() < 1e-3, "its speed is measured for a spring to take over: {v}");
    // No time passed: it moves with the growth but keeps the speed it had.
    let same = next.step(&growth(75.0, 100.0), GROWTH, 0);
    assert_eq!(same, SlidePhase::Following { dy: -5.0, v, at: 75.0 });
}

#[test]
fn a_growth_that_stops_opening_toward_it_hands_its_speed_to_a_spring() {
    let phase = SlidePhase::Following { dy: -10.0, v: 300.0, at: 50.0 };
    // Retargeted below the height it had: the growth no longer opens toward the contents.
    let next = phase.step(&growth(50.0, 20.0), GROWTH, 1);
    let SlidePhase::Settling { dy, v } = next else { panic!("springs: {next:?}") };
    let (want_dy, want_v) = GROWTH.sample(-10.0, 300.0, 0.0, SUBSTEP);
    assert_eq!((dy, v), (want_dy, want_v), "from where it was, at the speed it had");
    // A growth opening toward it again doesn't bring the following back.
    assert!(matches!(next.step(&growth(0.0, 100.0), GROWTH, 1), SlidePhase::Settling { .. }));
}

#[test]
fn a_slide_ends_at_its_layout_and_a_spring_once_it_also_stops() {
    assert!(!SlidePhase::Following { dy: 0.001, v: 900.0, at: 10.0 }.live(), "a following slide at its layout is done");
    assert!(SlidePhase::Settling { dy: 0.001, v: 900.0 }.live(), "a spring passing its layout still moves");
    assert!(!SlidePhase::Settling { dy: 0.001, v: 0.1 }.live());
    let mut phase = SlidePhase::Settling { dy: -40.0, v: 0.0 };
    let mut frames = 0;
    while phase.live() {
        phase = phase.step(&growth(0.0, 0.0), GROWTH, 2);
        frames += 1;
        assert!(frames < 240, "springs home within 2 s: {phase:?}");
    }
}
