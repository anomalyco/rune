use super::*;
#[test]
fn sequence_delays_interpolates_holds_and_finishes_without_restarting() {
    const BEATS: &[Beat] = &[
        Beat::new(0.0).opacity(0.0).scale(0.8),
        Beat::new(0.3).easing(Easing::LINEAR),
        Beat::new(0.6).opacity(0.5).scale(1.1).easing(Easing::LINEAR),
        Beat::new(0.9).easing(Easing::LINEAR),
    ];
    let spec = LoopSpec::sequence(Sequence::new(BEATS).delay(0.2));
    let mut track = LoopTrack::new(spec, 4.0);
    assert_eq!(track.pending_start(4.0), Some(4.2));
    assert!(!track.active(4.1));
    let pose = track.pose(4.35, false).unwrap();
    assert!((pose.opacity - 0.5).abs() < 1e-5 && (pose.scale - 0.9).abs() < 1e-5);
    track.retime(spec, 4.35);
    assert_eq!(track.pose(4.35, false).unwrap(), pose);
    assert!(track.active(4.35));
    assert!(!track.active(5.2));
    assert_eq!(track.pose(5.2, false).unwrap().opacity, 1.0);
    assert_eq!(track.pose(4.35, true).unwrap().scale, 1.0);
    track.finish();
    assert!(!track.active(4.35));
    assert_eq!(track.pending_start(4.0), None);
    assert_eq!(track.pose(4.35, false).unwrap().scale, 1.0);
}
#[test]
fn a_finished_sequence_stays_done_when_enabled_again() {
    const BEATS: &[Beat] = &[Beat::new(0.0).opacity(0.0), Beat::new(0.5).opacity(1.0)];
    let spec = LoopSpec::sequence(Sequence::new(BEATS).delay(0.2));
    let mut t = LoopTrack::new(spec, 0.0);
    t.finish();
    assert!(!t.active(0.3), "a finished sequence asks for no frames");
    assert_eq!(t.pending_start(0.3), None);
    assert_eq!(t.pose(0.3, false).unwrap().opacity, 1.0, "and holds its last pose");
    t.advance(1.0);
    assert_eq!(t.pose(0.3, false).unwrap().opacity, 1.0, "a sequence keeps its place");
    t.enabled(false, 1.0);
    assert!(!t.active(10.0), "cooled to rest");
    t.enabled(true, 2.0);
    assert_eq!(t.pending_start(2.0), None, "enabling again doesn't wait on a delay");
    assert!(!t.active(2.1), "and asks for no frames");
    assert_eq!(t.pose(2.1, false).unwrap().opacity, 1.0, "holding its last pose");
    assert_eq!(t.pose(0.3, false).unwrap().opacity, 1.0, "it never replays");
}
#[test]
fn a_sequence_that_ran_out_while_cooled_rests_instead_of_replaying() {
    const BEATS: &[Beat] = &[Beat::new(0.0).opacity(0.0), Beat::new(0.5).opacity(1.0)];
    let spec = LoopSpec::sequence(Sequence::new(BEATS).delay(0.2));
    let mut t = LoopTrack::new(spec, 0.0);
    assert!(t.active(0.5), "playing until 0.7");
    t.enabled(false, 0.5);
    assert!(!t.active(0.5), "cooled to rest");
    t.enabled(true, 2.0);
    assert_eq!(t.pending_start(2.0), None, "the delay is not paid again");
    assert!(!t.active(2.1));
    assert_eq!(t.pose(2.1, false).unwrap().opacity, 1.0, "it rests where it ran out");
}
#[test]
fn enabling_a_cooled_sequence_carries_on_from_its_presented_phase() {
    const BEATS: &[Beat] = &[
        Beat::new(0.0).opacity(0.0).easing(Easing::LINEAR),
        Beat::new(1.0).opacity(1.0).easing(Easing::LINEAR),
    ];
    let spec = LoopSpec::sequence(Sequence::new(BEATS).delay(0.2));
    let mut t = LoopTrack::new(spec, 0.0);
    let cooled = t.phase(0.7);
    assert!((cooled - 0.5).abs() < 1e-9, "mid-run, half way");
    t.enabled(false, 0.7);
    assert!(!t.active(0.7), "cooled to rest");
    t.enabled(true, 0.9);
    assert!(t.active(0.9), "it warms back up and asks for frames");
    assert_eq!(t.pending_start(0.9), None, "it does not wait out the delay again");
    assert!((t.phase(0.9) - 0.7).abs() < 1e-9, "it carries on from where it had reached");
    assert!(
        (t.pose(0.9, false).unwrap().opacity - 0.7).abs() < 1e-5,
        "presenting the pose its phase reached, not its first pose"
    );
    assert!(!t.active(1.3), "and still ends where its own clock does");
    assert_eq!(t.pose(1.3, false).unwrap().opacity, 1.0);
}
#[test]
fn enabling_leaves_a_looping_tracks_phase_alone() {
    let specs = [
        LoopSpec::opacity(1.0, 0.0, 0.2, 0.8),
        LoopSpec::rotation(0.9),
        LoopSpec::slide(2.0, [-1.0, 0.0], [3.0, 0.0]),
    ];
    for spec in specs {
        let mut cooled = LoopTrack::new(spec, 0.0);
        let plain = LoopTrack::new(spec, 0.0);
        cooled.enabled(false, 0.4);
        assert!(cooled.active(0.41), "cooling keeps its settle frames");
        cooled.enabled(true, 1.1);
        for i in 0..50 {
            let now = 1.1 + i as f64 * 0.037;
            assert_eq!(cooled.phase(now), plain.phase(now), "{spec:?} keeps its phase");
            assert_eq!(cooled.active(now), plain.active(now));
            assert_eq!(cooled.pending_start(now), plain.pending_start(now));
        }
        assert!((cooled.sample(5.0, false) - plain.sample(5.0, false)).abs() < 1e-6);
        match (cooled.offset(5.0, false), plain.offset(5.0, false)) {
            (Some(warmed), Some(never)) => {
                assert!((warmed[0] - never[0]).abs() < 1e-6, "its amplitude warms back to full");
            }
            (None, None) => {}
            _ => panic!("the same spec asks for the same offsets"),
        }
    }
}
#[test]
fn a_delayed_loop_shifts_and_advances_its_start() {
    let mut t = LoopTrack::new(LoopSpec::opacity(1.0, 2.0, 0.0, 1.0), 3.0);
    assert_eq!(t.pending_start(3.0), Some(5.0));
    t.shift(0.5);
    assert_eq!(t.pending_start(3.0), Some(5.5), "a clock shift moves the start with it");
    t.advance(0.25);
    assert_eq!(t.pending_start(3.0), Some(5.25), "time the clock didn't see starts it sooner");
    assert!(!t.active(5.0));
    assert!(t.active(5.25));
}
#[test]
fn cycle_preserves_wall_phase_stops_still_and_cadence() {
    let c = Cycle { period: 1.7, phase: 0.3, stops: &[(0.0, 0.2), (0.2, 1.0), (0.7, 0.1), (1.0, 0.2)], still: 0.4, fps: 30.0 };
    let track = LoopTrack::new(LoopSpec::cycle(c), 4.0);
    for i in 0..200 {
        let wall = i as f64 * 0.037;
        assert_eq!(track.sample_at(4.0, wall, false), c.sample(wall));
        assert_eq!(track.sample_at(4.0, wall, true), c.still);
    }
    assert_eq!(track.frame_interval(), Some(f64::from(1.0 / 30.0_f32)));
}
#[test]
fn delay_has_one_wake_and_no_active_frames() {
    let track = LoopTrack::new(LoopSpec::opacity(1.0, 2.0, 0.0, 1.0), 3.0);
    assert_eq!(track.pending_start(3.0), Some(5.0));
    assert!(!track.active(4.999));
    assert_eq!(track.sample(4.999, false), 0.0);
    assert_eq!(track.pending_start(5.0), None);
    assert!(track.active(5.0));
    assert_eq!(track.phase(5.0), 0.0);
}
#[test]
fn phase_continuity_and_clock_sampling() {
    let mut t = LoopTrack::new(LoopSpec::rotation(0.9), 4.0);
    for i in 0..200 {
        let now = 4.0 + i as f64 * 0.037;
        let before = t.phase(now);
        t.retime(LoopSpec::rotation(0.2 + (i % 9) as f64 * 0.1), now);
        assert!((before - t.phase(now)).abs() < 1e-12);
        assert_eq!(t.sample(now, false), t.sample(now, false));
    }
    assert_eq!(t.sample(100.0, true), 0.0);
}
#[test]
fn slide_sweeps_wraps_holds_midpoint_reduced_and_cools_to_rest() {
    let mut t = LoopTrack::new(LoopSpec::slide(2.0, [-1.0, 0.0], [3.0, 0.0]), 0.0);
    assert_eq!(t.offset(0.0, false), Some([-1.0, 0.0]));
    assert_eq!(t.offset(1.0, false), Some([1.0, 0.0]));
    assert_eq!(t.offset(2.0, false), t.offset(0.0, false));
    let xs: Vec<f32> = (0..200).map(|i| t.offset(i as f64 * 0.01, false).unwrap()[0]).collect();
    assert!(xs.windows(2).all(|w| w[1] >= w[0]), "a sweep only moves forward within a period");
    assert_eq!(t.offset(0.37, true), Some([1.0, 0.0]));
    assert_eq!(t.sample(0.37, false), 1.0, "a slide never changes opacity");
    assert_eq!(LoopTrack::new(LoopSpec::rotation(1.0), 0.0).offset(0.5, false), None);
    t.enabled(false, 1.0);
    assert!(t.active(1.01));
    assert!(!t.active(10.0));
    assert_eq!(t.offset(10.0, false), Some([0.0, 0.0]));
}
#[test]
fn a_linear_slide_sweeps_at_a_constant_speed_and_holds_its_midpoint_reduced() {
    let spec = LoopSpec::slide(2.0, [-1.0, 0.0], [3.0, 0.0]).easing(Easing::LINEAR);
    let t = LoopTrack::new(spec, 0.0);
    // 240 frames at 120 Hz: one period, each frame the same step forward.
    let xs: Vec<f32> = (0..240).map(|i| t.offset(i as f64 / 120.0, false).unwrap()[0]).collect();
    assert_eq!(xs[0], -1.0);
    for pair in xs.windows(2) {
        assert!((pair[1] - pair[0] - 4.0 / 240.0).abs() < 1e-4, "step {} -> {}", pair[0], pair[1]);
    }
    assert_eq!(t.offset(2.0, false), Some([-1.0, 0.0]), "wraps after a period");
    assert_eq!(t.offset(0.37, true), Some([1.0, 0.0]), "reduced motion holds the midpoint");
    assert_eq!(t.sample(0.37, false), 1.0);
    // The default stays ease-in-out, and naming it is the same spec.
    let eased = LoopSpec::slide(2.0, [-1.0, 0.0], [3.0, 0.0]);
    assert_eq!(eased.easing(Easing::LINEAR).easing(Easing::EASE_IN_OUT).kind,
        LoopKind::SlideEased { from: [-1.0, 0.0], to: [3.0, 0.0], easing: Easing::EASE_IN_OUT });
    let e = LoopTrack::new(eased, 0.0);
    let steps: Vec<f32> = (0..121).map(|i| e.offset(i as f64 / 120.0, false).unwrap()[0]).collect::<Vec<_>>()
        .windows(2).map(|w| w[1] - w[0]).collect();
    assert!(steps[60] > steps[0] * 4.0, "ease-in-out starts slow");
    assert_eq!(LoopSpec::rotation(1.0).easing(Easing::LINEAR), LoopSpec::rotation(1.0), "only slides take a curve");
}
#[test]
fn cooling_keeps_phase_and_settles() {
    let mut t = LoopTrack::new(LoopSpec::opacity(1.0, 0.0, 0.15, 0.65), 0.0);
    let before = t.sample(0.3, false);
    t.enabled(false, 0.3);
    assert_eq!(t.sample(0.3, false), before);
    assert!(t.active(0.31));
    assert!(!t.active(10.0));
    assert_eq!(t.sample(10.0, false), 0.15);
}
#[test]
fn clock_pause_scale_step_and_seek_match_reference() {
    let spec = LoopSpec::rotation(0.9);
    let t = LoopTrack::new(spec, 0.0);
    let mut clock = crate::Clock::manual(120.0);
    for _ in 0..17 { clock.step(); }
    let first = t.sample(clock.now(), false);
    clock.pause(0.0);
    for _ in 0..40 { clock.step(); }
    assert_eq!(t.sample(clock.now(), false), first);
    clock.resume(0.0);
    clock.set_scale(0.25, 0.0);
    clock.step();
    let reference = ((17.0 / 120.0 + 0.25 / 120.0) / 0.9) as f32 * core::f32::consts::TAU;
    assert!((t.sample(clock.now(), false) - reference).abs() < 1e-6);
    clock.seek(0.45);
    assert!((t.sample(clock.now(), false) - core::f32::consts::PI).abs() < 1e-6);
}
