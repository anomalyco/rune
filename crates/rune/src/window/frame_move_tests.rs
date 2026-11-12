//! A frame jump is one coupon inside `FrameMove`: the content size a resize request jumps to,
//! spent by the one `Resized` it causes, never read as motion, and never left for a later move.
//! The reference keeps the fields this replaced apart exactly as dev had them, plus this
//! branch's one fix, marked.

use super::stepping::FrameMove;
use crate::window::frame::FrameAnim;

/// The anim a walk's `Animate` requests start, and the origin it is taken from.
fn animation() -> FrameAnim {
    FrameAnim::new(None, 0.0, [10.0, 20.0, 300.0, 200.0], [0.0; 4], [30.0, 40.0, 300.0, 200.0])
}
const ORIGIN: [f32; 2] = [10.0, 20.0];
/// The window's scale factor: every size here is in points, compared in doubled pixels.
const SCALE: f32 = 2.0;
/// The content size the window is laid out at before a walk asks for anything: 1000x800 px.
const START: (f32, f32) = (500.0, 400.0);
/// `START`, rounding to the same pixels at 2x, and one pixel wider.
const START_SAME: (f32, f32) = (500.2, 400.2);
const START_ADJACENT: (f32, f32) = (500.5, 400.5);
const JUMP: (f32, f32) = (300.0, 200.0);
/// `JUMP`, a pixel under and over what it rounds to at 2x.
const JUMP_SAME: (f32, f32) = (300.2, 200.2);
const ADJACENT: (f32, f32) = (301.0, 200.0);
const OTHER_JUMP: (f32, f32) = (420.0, 260.0);
/// The viewport `JUMP` announces, and a user's.
const HIS: (f32, f32) = (300.0, 200.0);
const USER: (f32, f32) = (7.0, 7.0);

/// A content size in the rounded physical pixels the OS reports it at, as the reference decides
/// it: it never calls production code.
fn pixels(size: (f32, f32), scale: f32) -> (u32, u32) {
    ((size.0 * scale).round().max(0.0) as u32, (size.1 * scale).round().max(0.0) as u32)
}

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
    /// `take_frame_request` elsewhere, for this content size.
    Jump((f32, f32)),
    /// `WindowEvent::Resized`, a size that changes the viewport.
    Resized((f32, f32)),
    /// `WindowEvent::Resized`, one the window is already laid out at: `events.rs` returns early.
    Unchanged((f32, f32)),
    /// `land_frame`: a window that stops painting mid-move.
    Land,
}

/// The host's own writes, one event at a time, with the content size the window is laid out at.
fn feed(slot: &mut Option<FrameMove>, current: &mut (f32, f32), step: Step) -> Observed {
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
            if let Some(jump) = FrameMove::jump_to(pixels(*current, SCALE), size, SCALE) { *slot = Some(jump); }
            false
        }
        Step::Resized(viewport) => {
            let asked = FrameMove::take_jump(slot, viewport);
            *current = viewport;
            asked
        }
        // The host's own path, `events.rs`: an unchanged reply lays nothing out.
        Step::Unchanged(viewport) => {
            super::events::resize_reply(slot, viewport, pixels(viewport, SCALE), SCALE, true);
            false
        }
        Step::Land => {
            FrameMove::take_moving(slot);
            false
        }
    };
    Observed { moving: FrameMove::moving(slot), asked }
}

/// Whether two sizes are the same within the point dev's `Resized` matched by.
fn within(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0
}

/// The fields this replaced, kept apart exactly as they were: `Host::frame_move` (its anim and
/// origin don't change here, so only its presence shows) and the loose `Host::frame_jump` beside
/// it, with the viewport the host last laid out. The branches are dev's, copied literally, so a
/// change to the branch can't move both sides.
#[derive(Clone, Copy, Debug)]
struct Apart {
    frame_move: Option<()>,
    frame_jump: Option<(f32, f32)>,
    current: (f32, f32),
}

impl Apart {
    fn step(&mut self, step: Step) -> Observed {
        let mut asked = false;
        match step {
            // take_frame_request, painting: `self.frame_move = Some(FrameMove { anim, origin })`.
            Step::Animate => self.frame_move = Some(()),
            // take_frame_request, not painting: `self.frame_move = None`, and the frame is set.
            Step::Cancel => self.frame_move = None,
            // differs from dev: dev's `self.frame_jump = Some((width, height))` held a coupon for
            // every request, including one for the pixels the window already has. No `Resized`
            // answers that one, so it stayed and the user's next resize spent it.
            Step::Jump(size) => {
                if pixels(self.current, SCALE) != pixels(size, SCALE) { self.frame_jump = Some(size); }
            }
            // WindowEvent::Resized: `frame_jump.take().is_some_and(|(w, h)| …)`, dev's.
            Step::Resized(viewport) => {
                asked = self.frame_jump.take().is_some_and(|(w, h)| within((w, h), viewport));
                self.current = viewport;
            }
            // The host's unchanged reply, differs from dev: dev returned with the coupon still
            // pending; the jump for exactly these pixels is answered there.
            Step::Unchanged(viewport) => {
                if self.frame_jump.is_some_and(|size| pixels(size, SCALE) == pixels(viewport, SCALE)) {
                    self.frame_jump = None;
                }
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
/// whether it asked for the size), what the move holds, and the size it is laid out at.
fn holds_up(seq: &[Step]) {
    let (mut slot, mut current) = (None, START);
    let mut apart = Apart { frame_move: None, frame_jump: None, current: START };
    for (k, step) in seq.iter().enumerate() {
        let seen = feed(&mut slot, &mut current, *step);
        let expected = apart.step(*step);
        assert_eq!(seen, expected, "{seq:?} at {k}");
        assert_eq!(current, apart.current, "the laid-out size after {seq:?} at {k}");
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
/// the event it announced, once, and is never motion. Requests for the pixels the window
/// already has hold none, and one for an adjacent pixel does.
#[test]
fn every_order_of_jumps_spends_one_coupon() {
    let all = [
        Step::Jump(JUMP), Step::Jump(OTHER_JUMP), Step::Jump(START), Step::Jump(START_SAME),
        Step::Jump(START_ADJACENT), Step::Resized(HIS), Step::Resized(USER), Step::Land,
    ];
    every_sequence(&all, 6, holds_up);
}

/// The host's reply path in every order: an unchanged `Resized` answers the jump for its exact
/// pixels and leaves another's, and a size that changes the viewport still spends its coupon.
#[test]
fn every_order_of_unchanged_replies_keeps_the_rules() {
    let all = [
        Step::Jump(JUMP), Step::Jump(OTHER_JUMP), Step::Jump(START), Step::Unchanged(JUMP),
        Step::Unchanged(ADJACENT), Step::Unchanged(START_SAME), Step::Resized(USER), Step::Land,
    ];
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
    let (mut slot, mut current) = (None, START);
    assert!(!feed(&mut slot, &mut current, Step::Resized(USER)).asked, "no request, no coupon");
    let requested = feed(&mut slot, &mut current, Step::Jump(JUMP));
    assert!(!requested.moving, "a jump names one event, it isn't motion");
    assert!(feed(&mut slot, &mut current, Step::Resized(HIS)).asked, "its own size lays out as a step");
    assert!(!feed(&mut slot, &mut current, Step::Resized(HIS)).asked, "once");
}

/// A later request replaces the coupon, a landing leaves it, and an animation moving beside it
/// is never read as the jump.
#[test]
fn a_later_request_or_landing_keeps_the_rules() {
    let (mut slot, mut current) = (None, START);
    feed(&mut slot, &mut current, Step::Jump(JUMP));
    feed(&mut slot, &mut current, Step::Jump(OTHER_JUMP));
    assert!(!feed(&mut slot, &mut current, Step::Resized(HIS)).asked, "the earlier size is gone");
    feed(&mut slot, &mut current, Step::Jump(OTHER_JUMP));
    feed(&mut slot, &mut current, Step::Land);
    assert!(feed(&mut slot, &mut current, Step::Resized(OTHER_JUMP)).asked, "a landing mid-move leaves the coupon for its Resized");
    assert!(feed(&mut slot, &mut current, Step::Animate).moving, "an animation is motion");
    feed(&mut slot, &mut current, Step::Animate);
    assert!(feed(&mut slot, &mut current, Step::Resized(HIS)).moving, "and no Resized spends it");
    assert!(!feed(&mut slot, &mut current, Step::Land).moving, "landing stops it");
}

/// A size request that moves no pixel gets no `Resized` back, so it holds no coupon; one that
/// moves a pixel keeps its coupon, rounded at the window's scale (r291, r299).
#[test]
fn only_a_request_that_moves_a_pixel_holds_a_coupon() {
    // At 2x, a 600x400 px window asks for its own 300x200 points.
    let (mut slot, mut current) = (None, (300.0, 200.0));
    feed(&mut slot, &mut current, Step::Jump(HIS));
    assert_eq!(slot, None, "no coupon for the size the window already has");
    assert!(!feed(&mut slot, &mut current, Step::Resized(USER)).asked, "so the user's resize stays the user's");
    // Rounded, not within a point: 300.2 pt still rounds to 600 px.
    let (mut slot, mut current) = (None, (300.0, 200.0));
    feed(&mut slot, &mut current, Step::Jump(JUMP_SAME));
    assert_eq!(slot, None, "same pixels at this scale");
    // 300.5 pt is 601 px: the window really moves, so the request keeps its coupon.
    let (mut slot, mut current) = (None, (300.0, 200.0));
    feed(&mut slot, &mut current, Step::Jump((300.5, 200.0)));
    assert_eq!(slot, Some(FrameMove::jumped((300.5, 200.0))), "600 -> 601 px keeps its coupon");
    assert!(feed(&mut slot, &mut current, Step::Resized((300.5, 200.0))).asked, "and its reply steps");
}

/// A reply that changes nothing answers its jump through the host's own path and lays nothing
/// out; another size's reply leaves it, and the user's drag then sets the viewport (r302).
#[test]
fn an_unchanged_reply_spends_its_coupon_through_the_host() {
    use super::events::{ResizeReply, resize_reply};
    // A jump is pending and the window is already laid out at its size (the OS applied it, or
    // rt was initialised at it): its `Resized` changes nothing, and the host returns early.
    let mut slot = Some(FrameMove::jumped(JUMP));
    assert_eq!(resize_reply(&mut slot, HIS, pixels(HIS, SCALE), SCALE, true), ResizeReply::Unchanged, "nothing to lay out");
    assert_eq!(slot, None, "but its own reply still spends its coupon");
    // A reply for a size the jump never asked for leaves it: another event is still coming.
    let mut slot = Some(FrameMove::jumped(JUMP));
    assert_eq!(resize_reply(&mut slot, ADJACENT, pixels(ADJACENT, SCALE), SCALE, true), ResizeReply::Unchanged, "still nothing to lay out");
    assert_eq!(slot, Some(FrameMove::jumped(JUMP)), "not its pixels, so not its reply");
    // And the user's drag afterwards lands on the normal viewport path.
    let mut slot = None;
    assert_eq!(resize_reply(&mut slot, USER, pixels(USER, SCALE), SCALE, false), ResizeReply::Set, "the user's drag sets the viewport");
}
