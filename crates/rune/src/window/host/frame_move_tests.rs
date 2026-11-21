//! A frame jump is one coupon inside `FrameMove`: the content size a resize request jumps to,
//! spent by the one `Resized` it causes, never read as motion, and never left for a later move.
//! The reference keeps the two fields this replaced apart exactly as dev had them.

use super::stepping::FrameMove;
use crate::window::frame::FrameAnim;

/// The anim a walk's `Animate` requests start, and the origin it is taken from.
fn animation() -> FrameAnim {
    FrameAnim::new(None, 0.0, [10.0, 20.0, 300.0, 200.0], [0.0; 4], [30.0, 40.0, 300.0, 200.0])
}
const ORIGIN: [f32; 2] = [10.0, 20.0];
const JUMP: (f32, f32) = (300.0, 200.0);
const OTHER_JUMP: (f32, f32) = (420.0, 260.0);
/// The viewport `JUMP` announces, one within the pixel the window rounds by, and a user's.
const HIS: (f32, f32) = (300.0, 200.0);
const NEAR: (f32, f32) = (300.9, 200.9);
const USER: (f32, f32) = (7.0, 7.0);

/// What a step leaves to be seen: whether the frame animates (draw scheduling counts that as
/// motion), and whether the `Resized` says this window asked for the size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Observed {
    moving: bool,
    asked: bool,
}

/// The events that start, spend or end a frame move, as the host feeds them.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// `take_frame_request` where macOS paints the move.
    Animate,
    /// `take_frame_request` where the frame is set at once.
    Cancel,
    /// `take_frame_request` elsewhere, with a size the frame jumps to.
    Jump((f32, f32)),
    /// `WindowEvent::Resized`, laid out at this viewport.
    Resized((f32, f32)),
    /// `land_frame`: a window that stops painting mid-move.
    Land,
}

/// The host's own writes, one event at a time.
fn feed(slot: &mut Option<FrameMove>, step: Step) -> Observed {
    let asked = match step {
        Step::Animate => {
            *slot = Some(FrameMove::animating(animation(), ORIGIN));
            false
        }
        Step::Cancel => {
            *slot = None;
            false
        }
        Step::Jump(size) => {
            *slot = Some(FrameMove::jumped(size));
            false
        }
        Step::Resized(viewport) => FrameMove::take_jump(slot, viewport),
        Step::Land => {
            FrameMove::take_moving(slot);
            false
        }
    };
    Observed { moving: FrameMove::moving(slot), asked }
}

/// The fields this replaced, kept apart exactly as they were: `Host::frame_move` (its anim and
/// origin don't change here, so only its presence shows) and the loose `Host::frame_jump` beside
/// it. The branches are dev's, copied literally, so a change to the branch can't move both sides.
#[derive(Clone, Copy, Debug, Default)]
struct Apart {
    frame_move: Option<()>,
    frame_jump: Option<(f32, f32)>,
}

impl Apart {
    fn step(&mut self, step: Step) -> Observed {
        let mut asked = false;
        match step {
            // take_frame_request, painting: `self.frame_move = Some(FrameMove { anim, origin })`.
            Step::Animate => self.frame_move = Some(()),
            // take_frame_request, not painting: `self.frame_move = None`, and the frame is set.
            Step::Cancel => self.frame_move = None,
            // take_frame_request, elsewhere: `self.frame_jump = Some((width, height))`.
            Step::Jump(size) => self.frame_jump = Some(size),
            // WindowEvent::Resized: `frame_jump.take().is_some_and(|(w, h)| …)`.
            Step::Resized(viewport) => {
                asked = self.frame_jump.take().is_some_and(|(w, h)| (w - viewport.0).abs() < 1.0 && (h - viewport.1).abs() < 1.0);
            }
            // land_frame: `frame_move.take()`, which leaves the loose jump alone.
            Step::Land => self.frame_move = None,
        }
        Observed { moving: self.frame_move.is_some(), asked }
    }
}

fn every_sequence(alphabet: &[Step], max: usize, mut f: impl FnMut(&[Step])) {
    fn walk(alphabet: &[Step], seq: &mut Vec<Step>, max: usize, f: &mut impl FnMut(&[Step])) {
        f(seq);
        if seq.len() == max { return; }
        for step in alphabet {
            seq.push(*step);
            walk(alphabet, seq, max, f);
            seq.pop();
        }
    }
    walk(alphabet, &mut Vec::new(), max, &mut f);
}

/// Both sides fed every sequence, compared after every event: what the window sees (motion and
/// whether it asked for the size) and what the move holds.
fn holds_up(seq: &[Step]) {
    let (mut slot, mut apart) = (None, Apart::default());
    for (k, step) in seq.iter().enumerate() {
        let seen = feed(&mut slot, *step);
        let expected = apart.step(*step);
        assert_eq!(seen, expected, "{seq:?} at {k}");
        // A platform's events never leave both of dev's fields set; the one value can't hold both.
        assert!(!(apart.frame_move.is_some() && apart.frame_jump.is_some()), "{seq:?} at {k}");
        let left = match (apart.frame_move, apart.frame_jump) {
            (Some(()), _) => Some(FrameMove::animating(animation(), ORIGIN)),
            (None, Some(size)) => Some(FrameMove::jumped(size)),
            (None, None) => None,
        };
        assert_eq!(slot, left, "the move after {seq:?} at {k}");
    }
}

/// A jump request, its `Resized` and a landing in every order: the coupon is spent by exactly
/// the event it announced, once, and is never motion.
#[test]
fn every_order_of_jumps_spends_one_coupon() {
    let all = [Step::Jump(JUMP), Step::Jump(OTHER_JUMP), Step::Resized(HIS), Step::Resized(NEAR), Step::Resized(USER), Step::Land];
    every_sequence(&all, 6, holds_up);
}

/// macOS's moving frames and their cancels in every order with the events that arrive beside
/// them: each sequence leaves the frame moving or still as dev did.
#[test]
fn every_order_of_animations_leaves_the_same_frame_moving() {
    let all = [Step::Animate, Step::Cancel, Step::Resized(HIS), Step::Land];
    every_sequence(&all, 7, holds_up);
}

/// The coupon's rules, named.
#[test]
fn a_jump_is_spent_by_the_resize_it_announced_once() {
    let mut slot = None;
    assert!(!feed(&mut slot, Step::Resized(HIS)).asked, "no request, no coupon");
    let requested = feed(&mut slot, Step::Jump(JUMP));
    assert!(!requested.moving, "a jump names one event, it isn't motion");
    assert!(!feed(&mut slot, Step::Resized(USER)).asked, "another size is the user's");
    assert!(!feed(&mut slot, Step::Resized(HIS)).asked, "and the coupon is spent, so it's the user's too");
    feed(&mut slot, Step::Jump(JUMP));
    assert!(feed(&mut slot, Step::Resized(HIS)).asked, "its own size lays out as a step");
    assert!(!feed(&mut slot, Step::Resized(HIS)).asked, "once");
}

/// A later request replaces the coupon, a landing leaves it, and an animation moving beside it
/// is never read as the jump.
#[test]
fn a_later_request_or_landing_keeps_the_rules() {
    let mut slot = None;
    feed(&mut slot, Step::Jump(JUMP));
    feed(&mut slot, Step::Jump(OTHER_JUMP));
    assert!(!feed(&mut slot, Step::Resized(HIS)).asked, "the earlier size is gone");
    feed(&mut slot, Step::Jump(JUMP));
    feed(&mut slot, Step::Land);
    assert!(feed(&mut slot, Step::Resized(HIS)).asked, "a landing mid-move leaves the coupon for its Resized");
    assert!(feed(&mut slot, Step::Animate).moving, "an animation is motion");
    feed(&mut slot, Step::Animate);
    assert!(feed(&mut slot, Step::Resized(HIS)).moving, "and no Resized spends it");
    assert!(!feed(&mut slot, Step::Land).moving, "landing stops it");
}
