use std::time::{Duration, Instant};

use super::{Cloak, CloakEvent, CloakSeam};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Cloak(bool),
    TransitionsDisabled(bool),
    DwmFlush,
    Activate,
}

struct FakeSeam {
    allow_cloak: bool,
    calls: Vec<Call>,
}

impl FakeSeam {
    fn ok() -> Self { Self { allow_cloak: true, calls: Vec::new() } }
    fn deny_cloak() -> Self { Self { allow_cloak: false, calls: Vec::new() } }
}

impl CloakSeam for FakeSeam {
    fn set_cloaked(&mut self, cloaked: bool) -> bool {
        self.calls.push(Call::Cloak(cloaked));
        !cloaked || self.allow_cloak
    }
    fn set_transitions_disabled(&mut self, disabled: bool) {
        self.calls.push(Call::TransitionsDisabled(disabled));
    }
    fn dwm_flush(&mut self) {
        self.calls.push(Call::DwmFlush);
    }
    fn activate(&mut self) {
        self.calls.push(Call::Activate);
    }
}

#[test]
fn fullscreen_transition_cloak_disables_transitions_before_cloak_and_flushes_before_restoring() {
    let cloak = Cloak::default();
    let mut seam = FakeSeam::ok();
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_millis(250);

    cloak.apply_with(CloakEvent::Hold { until: deadline, activate: false, suppress_transitions: true }, &mut seam);
    assert!(cloak.held());
    assert_eq!(seam.calls, vec![Call::TransitionsDisabled(true), Call::Cloak(true)]);

    seam.calls.clear();
    cloak.apply_with(CloakEvent::Lift, &mut seam);
    assert!(!cloak.held());
    assert_eq!(
        seam.calls,
        vec![Call::Cloak(false), Call::DwmFlush, Call::TransitionsDisabled(false)],
        "must uncloak, flush DWM composition, and then re-enable transitions without re-activating",
    );
}

#[test]
fn failed_transition_cloak_restores_transitions_immediately_without_leaking() {
    let cloak = Cloak::default();
    let mut seam = FakeSeam::deny_cloak();
    let deadline = Instant::now() + Duration::from_millis(250);

    cloak.apply_with(CloakEvent::Hold { until: deadline, activate: true, suppress_transitions: true }, &mut seam);
    assert!(!cloak.held());
    assert_eq!(
        seam.calls,
        vec![
            Call::TransitionsDisabled(true),
            Call::Cloak(true),
            Call::TransitionsDisabled(false),
            Call::Activate,
        ],
    );
}

#[test]
fn ordinary_launch_cloak_never_touches_transitions_or_dwm_flush() {
    let cloak = Cloak::default();
    let mut seam = FakeSeam::ok();
    let deadline = Instant::now() + Duration::from_millis(250);

    cloak.apply_with(CloakEvent::Hold { until: deadline, activate: true, suppress_transitions: false }, &mut seam);
    cloak.apply_with(CloakEvent::Lift, &mut seam);
    assert_eq!(seam.calls, vec![Call::Cloak(true), Call::Cloak(false), Call::Activate]);
}

#[test]
fn upgrading_existing_cloak_to_suppress_transitions_and_timing_out_or_hiding_restores_transitions() {
    let t0 = Instant::now();
    let d1 = t0 + Duration::from_millis(100);
    let d2 = t0 + Duration::from_millis(200);

    // Timeout path restores transitions and preserves earlier activate request.
    let cloak = Cloak::default();
    let mut seam = FakeSeam::ok();
    cloak.apply_with(CloakEvent::Hold { until: d1, activate: true, suppress_transitions: false }, &mut seam);
    cloak.apply_with(CloakEvent::Hold { until: d2, activate: false, suppress_transitions: true }, &mut seam);
    assert_eq!(seam.calls, vec![Call::Cloak(true), Call::TransitionsDisabled(true)]);

    seam.calls.clear();
    cloak.apply_with(CloakEvent::Tick { now: d1 }, &mut seam);
    assert!(cloak.held(), "updated deadline d2 has not expired at d1");
    assert!(seam.calls.is_empty());

    cloak.apply_with(CloakEvent::Tick { now: d2 }, &mut seam);
    assert!(!cloak.held());
    assert_eq!(seam.calls, vec![Call::Cloak(false), Call::DwmFlush, Call::TransitionsDisabled(false), Call::Activate]);

    // Hidden path restores transitions without activating.
    let cloak = Cloak::default();
    let mut seam = FakeSeam::ok();
    cloak.apply_with(CloakEvent::Hold { until: d1, activate: true, suppress_transitions: true }, &mut seam);
    seam.calls.clear();
    cloak.apply_with(CloakEvent::Hidden, &mut seam);
    assert!(!cloak.held());
    assert_eq!(seam.calls, vec![Call::Cloak(false), Call::DwmFlush, Call::TransitionsDisabled(false)]);
}

#[test]
fn cloak_size_matches_blocks_stale_pre_resize_frames_and_accepts_resized_or_clamped_frames() {
    // Entering fullscreen: window is already 1920x1080, stale swap chain is still 620x640.
    assert!(!Cloak::size_matches((620, 640), (1920, 1080), 16384));
    // Once resized to 1920x1080, lifting is allowed.
    assert!(Cloak::size_matches((1920, 1080), (1920, 1080), 16384));
    // Leaving fullscreen: window is already 620x640, stale swap chain is still 1920x1080.
    assert!(!Cloak::size_matches((1920, 1080), (620, 640), 16384));
    assert!(Cloak::size_matches((620, 640), (620, 640), 16384));
    // Zero inner size (minimized) or texture-limit-clamped size never stalls uncloaking.
    assert!(Cloak::size_matches((620, 640), (0, 0), 16384));
    assert!(Cloak::size_matches((620, 640), (20000, 1080), 16384));
}
