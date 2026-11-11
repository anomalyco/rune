use super::*;

/// origin/dev's countdown, transcribed from `runtime/scroll/offset.rs`: a jump armed two
/// frames, and each frame took one off and reported whether one still held.
///
/// differs from dev: this holds on the clock, so its hold ends a fixed time after the last
/// instant scroll instead of a fixed number of frames; the two agree only at 60 Hz, the rate
/// its two frames were timed for (`the_hold_matches_the_countdown_at_sixty_hertz`), and part
/// company at every other rate (`the_hold_ends_a_fixed_time_after_the_land` drives 30 and 120).
#[derive(Default)]
struct Dev {
    nudged: u8,
}

impl Dev {
    fn arm(&mut self) {
        self.nudged = 2;
    }

    fn frame(&mut self) -> bool {
        self.nudged = self.nudged.saturating_sub(1);
        self.nudged > 0
    }

    fn held(&self) -> bool {
        self.nudged > 0
    }
}

/// One 60 Hz frame: the rate dev's two-frame countdown and this hold are the same at.
const FRAME: f64 = 1.0 / 60.0;

#[derive(Clone, Copy, Debug)]
enum Act {
    Arm,
    Frame,
}

/// Drives one sequence through the machine and through the countdown, one event at a time, and
/// compares what each reports after every event.
fn walk(acts: &[Act]) {
    let (mut nudge, mut dev, mut now) = (Nudge::default(), Dev::default(), 0.0);
    for (at, &act) in acts.iter().enumerate() {
        match act {
            Act::Arm => {
                let effects = nudge.arm(now);
                dev.arm();
                assert_eq!(effects, Effects::one(Effect::WakeAt(now + NUDGE_HOLD)), "{acts:?} at {at}: the arm asks for its due time");
            }
            Act::Frame => {
                now += FRAME;
                nudge.tick(now);
                dev.frame();
            }
        }
        assert_eq!(nudge.hold_at(now), dev.held(), "{acts:?}: held after event {at}, at {now}");
    }
}

/// Every sequence of arms and frames up to six long: an arm at a frame's clock time and the
/// frames after it, in any order, must hold hover exactly where dev's countdown did.
#[test]
fn the_hold_matches_the_countdown_at_sixty_hertz() {
    let mut acts = Vec::new();
    for len in 0..=6 {
        acts.clear();
        enumerate(&mut acts, len);
    }
}

fn enumerate(acts: &mut Vec<Act>, len: usize) {
    if acts.len() == len {
        walk(acts);
        return;
    }
    for act in [Act::Arm, Act::Frame] {
        acts.push(act);
        enumerate(acts, len);
        acts.pop();
    }
}

/// A hold that came due releases hover; before it, the arm's own frame and the one after hold.
#[test]
fn the_hold_covers_the_landing_frame_and_the_next() {
    let mut nudge = Nudge::default();
    let landed = 4.5;
    let _ = nudge.arm(landed);
    assert!(nudge.hold_at(landed), "the frame it landed holds");
    nudge.tick(landed + FRAME);
    assert!(nudge.hold_at(landed + FRAME), "so does the next");
    nudge.tick(landed + 2.0 * FRAME);
    assert!(!nudge.hold_at(landed + 2.0 * FRAME), "and it is over on the third");
    assert_eq!(nudge.state, Hold::Free);
    nudge.tick(landed + 3.0 * FRAME);
    assert!(!nudge.hold_at(landed + 3.0 * FRAME), "a spent hold stays free");
}

/// differs from dev: dev took a frame off the countdown whatever the frame rate, so it held
/// hover for two frames at 30 Hz as at 120. This one ends the same time after the land at
/// either rate, which is the whole point of the change.
#[test]
fn the_hold_ends_a_fixed_time_after_the_land() {
    for rate in [30.0, 120.0] {
        let (mut nudge, mut now) = (Nudge::default(), 0.0);
        let _ = nudge.arm(now);
        let mut freed = None;
        for _ in 0..10 {
            now += 1.0 / rate;
            nudge.tick(now);
            if !nudge.hold_at(now) {
                freed = Some(now);
                break;
            }
        }
        let freed = freed.unwrap_or_else(|| panic!("{rate} fps: the hold ends"));
        assert!(freed >= NUDGE_HOLD - 1e-9 && freed < NUDGE_HOLD + 1.0 / rate + 1e-9, "{rate} fps: held {freed} seconds");
    }
}

/// A due carried by an earlier arm changes nothing: the newer arm keeps its own deadline.
#[test]
fn a_due_from_an_earlier_arm_changes_nothing() {
    let (mut nudge, mut now) = (Nudge::default(), 0.0);
    let _ = nudge.arm(now);
    let Hold::Held { attempt: first, due: first_due } = nudge.state else { panic!("armed") };
    now += FRAME;
    let _ = nudge.arm(now);
    let Hold::Held { attempt: second, due } = nudge.state else { panic!("armed") };
    assert_ne!(second, first);
    assert_eq!(due, now + NUDGE_HOLD);
    let _ = step(&mut nudge.seq, &mut nudge.state, Event::Due { attempt: first, now: first_due + 1.0 });
    assert!(nudge.hold_at(now), "the newer arm still holds");
    assert_eq!(nudge.state, Hold::Held { attempt: second, due });
}
