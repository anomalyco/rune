//! A frame jump is one coupon inside `FrameMove`: the content size a resize request jumps to,
//! spent by the one `Resized` it causes, never read as motion, and never left for a later move.
//! The reference keeps the two fields this replaced apart exactly as dev had them.

use super::stepping::{FrameMove, apply_immediate_resize};
use crate::window::frame::FrameAnim;
use crate::{Env, Runtime, col, el};

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

/// On Wayland, `Window::request_inner_size` applies the resize immediately and returns
/// `Some(size)` without emitting `WindowEvent::Resized`. The surface config, runtime viewport
/// and jump coupon must all update immediately, including at fractional scales.
#[test]
fn immediate_resize_reconfigures_surface_steps_viewport_and_consumes_jump() {
    let mut rt = Runtime::new((), |_, _| el().size(1200.0, 720.0), (1200.0, 720.0), Env::default());
    rt.frame(0.0);
    let mut surface = (1200u32, 720u32);
    let mut frame_move = Some(FrameMove::jumped((900.0, 600.0)));

    let changed = apply_immediate_resize(rt.core_mut(), &mut frame_move, (900, 600), 1.0, |next| {
        let resized = surface != next;
        surface = next;
        (resized, surface)
    });
    assert!(changed, "an immediate resize must mark the window changed so a redraw follows");
    assert_eq!(surface, (900, 600), "the surface config must follow the immediate size");
    assert_eq!(rt.core().window_viewport(), (900.0, 600.0));
    assert_eq!(frame_move, None, "the jump coupon must be consumed when no Resized will arrive");

    // Fractional scaling: logical (900, 601) at 1.25x rounds to physical (1125, 751) -> (900.0, 600.8).
    let mut rt_scaled = Runtime::new((), |_, _| el().size(1200.0, 720.0), (1200.0, 720.0), Env { scale: 1.25, ..Env::default() });
    rt_scaled.frame(0.0);
    let mut scaled_surface = (1500u32, 900u32);
    let mut scaled_move = Some(FrameMove::jumped((900.0, 601.0)));

    let changed = apply_immediate_resize(rt_scaled.core_mut(), &mut scaled_move, (1125, 751), 1.25, |next| {
        let resized = scaled_surface != next;
        scaled_surface = next;
        (resized, scaled_surface)
    });
    assert!(changed);
    assert_eq!(scaled_surface, (1125, 751));
    assert_eq!(rt_scaled.core().window_viewport(), (900.0, 600.8));
    assert_eq!(scaled_move, None);
}

/// A `fit_content_height` window resizing itself on Wayland updates its surface and viewport
/// through `step_viewport`, so it does not trigger `fit_resized` relayout as if a user dragged it.
#[test]
fn immediate_resize_for_fit_content_height_steps_without_fit_resized_relayout() {
    let mut rt = Runtime::new(200.0f32, |&h, _| col().w_full().child(el().size(100.0, h)), (400.0, 200.0), Env::default());
    rt.core_mut().host.set_fit_height(true);
    rt.core_mut().host.window_resting = true;
    rt.frame(0.0);
    rt.core_mut().host.window_frame.take();

    rt.update(|h, _| *h = 350.0);
    rt.frame(0.0);
    let requested = rt.core_mut().host.window_frame.take().and_then(|r| r.size).expect("fit height requests new size");
    assert_eq!(requested, (400.0, 350.0));

    let mut surface = (400u32, 200u32);
    let mut frame_move = Some(FrameMove::jumped(requested));
    let changed = apply_immediate_resize(rt.core_mut(), &mut frame_move, (400, 350), 1.0, |next| {
        let resized = surface != next;
        surface = next;
        (resized, surface)
    });
    assert!(changed);
    assert_eq!(surface, (400, 350));
    assert_eq!(rt.core().window_viewport(), (400.0, 350.0));
    assert_eq!(frame_move, None);
    assert!(!rt.core().stale(), "step_viewport must not mark a fit-height resize stale as a user resize");
}

/// When the compositor keeps the current size (e.g. a tiled or maximised Wayland window),
/// clamps it to a different size, or reports zero or an oversized dimension, the jump coupon is
/// still spent and the surface and viewport stay consistent.
#[test]
fn immediate_resize_handles_ignored_clamped_zero_and_oversized_results() {
    let mut rt = Runtime::new(200.0f32, |&h, _| col().w_full().child(el().size(100.0, h)), (400.0, 200.0), Env::default());
    rt.core_mut().host.set_fit_height(true);
    rt.core_mut().host.window_resting = true;
    rt.frame(0.0);
    rt.core_mut().host.window_frame.take();

    // Compositor ignores the request and returns the unchanged size: no redraw, coupon spent.
    let mut surface = (400u32, 200u32);
    let mut frame_move = Some(FrameMove::jumped((400.0, 350.0)));
    let changed = apply_immediate_resize(rt.core_mut(), &mut frame_move, (400, 200), 1.0, |next| {
        let resized = surface != next;
        surface = next;
        (resized, surface)
    });
    assert!(!changed, "unchanged size needs no redraw");
    assert_eq!(surface, (400, 200));
    assert_eq!(rt.core().window_viewport(), (400.0, 200.0));
    assert_eq!(frame_move, None, "ignored immediate resize must not leak the jump coupon");

    // Zero size leaves surface and viewport intact and spends the coupon.
    let mut frame_move = Some(FrameMove::jumped((400.0, 350.0)));
    let mut called = false;
    let changed = apply_immediate_resize(rt.core_mut(), &mut frame_move, (0, 200), 1.0, |_| {
        called = true;
        (false, surface)
    });
    assert!(!changed && !called);
    assert_eq!(frame_move, None);

    // Oversized dimension rejected by drawable_size keeps previous drawable and viewport.
    let mut frame_move = Some(FrameMove::jumped((20_000.0, 20_000.0)));
    let changed = apply_immediate_resize(rt.core_mut(), &mut frame_move, (20_000, 20_000), 1.0, |_| (false, surface));
    assert!(!changed);
    assert_eq!(rt.core().window_viewport(), (400.0, 200.0));
    assert_eq!(frame_move, None);

    // Compositor applies a different size than requested: falls back to set_viewport (triggers fit_resized).
    let mut frame_move = Some(FrameMove::jumped((400.0, 350.0)));
    let changed = apply_immediate_resize(rt.core_mut(), &mut frame_move, (400, 280), 1.0, |next| {
        let resized = surface != next;
        surface = next;
        (resized, surface)
    });
    assert!(changed);
    assert_eq!(surface, (400, 280));
    assert_eq!(rt.core().window_viewport(), (400.0, 280.0));
    assert_eq!(frame_move, None);
    assert!(rt.core().stale(), "a compositor-chosen height different from the request uses set_viewport");
}
