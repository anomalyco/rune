//! A frame jump is one coupon inside `FrameMove`: the content size a resize request jumps to,
//! spent by the one `Resized` it causes, never read as motion, and never left for a later move.
//! The reference keeps the fields this replaced apart exactly as dev had them, plus this
//! branch's fixes, marked.

use super::events::{ResizeReply, resize_reply};
use super::stepping::FrameMove;
use crate::window::frame::FrameAnim;

/// The anim a walk's `Animate` requests start, and the origin it is taken from.
fn animation() -> FrameAnim {
    FrameAnim::new(None, 0.0, [10.0, 20.0, 300.0, 200.0], [0.0; 4], [30.0, 40.0, 300.0, 200.0])
}
const ORIGIN: [f32; 2] = [10.0, 20.0];

/// The scale a walk's window is at, and whether its backend is Wayland's, which converts a
/// request through whole logical pixels.
#[derive(Clone, Copy, Debug)]
struct Display {
    scale: f64,
    wayland: bool,
}
const RETINA: Display = Display { scale: 2.0, wayland: false };

/// At 2x, the content size the window has before a walk asks for anything: 500x400 pt.
const START: (u32, u32) = (1000, 800);
/// Requests for `START`, for a size rounding to its pixels, and for one pixel wider.
const START_PT: (f32, f32) = (500.0, 400.0);
const START_SAME: (f32, f32) = (500.2, 400.2);
const START_ADJACENT: (f32, f32) = (500.5, 400.5);
const JUMP: (f32, f32) = (300.0, 200.0);
const OTHER_JUMP: (f32, f32) = (420.0, 260.0);
/// The pixels `JUMP` reports, one more, and a user's drag.
const HIS: (u32, u32) = (600, 400);
const ADJACENT: (u32, u32) = (602, 400);
const USER: (u32, u32) = (14, 14);

/// The pixels a request asks the OS for, through winit's own conversion of the request as the
/// backend receives it (and Wayland's whole logical pixels): the reference never calls Rune.
fn native(size: (f32, f32), display: Display) -> (u32, u32) {
    let size = winit::dpi::Size::from(winit::dpi::LogicalSize::new(size.0, size.1));
    let pixels: winit::dpi::PhysicalSize<u32> = if display.wayland {
        size.to_logical::<u32>(display.scale).to_physical(display.scale)
    } else {
        size.to_physical(display.scale)
    };
    (pixels.width, pixels.height)
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
    /// `take_frame_request` elsewhere, for this content size in points.
    Jump((f32, f32)),
    /// The OS applies this content size in pixels; its `Resized` is still queued.
    Applied((u32, u32)),
    /// `WindowEvent::Resized` at these pixels, a size that changes the viewport.
    Resized((u32, u32)),
    /// `WindowEvent::Resized` at these pixels, one the window is already laid out at:
    /// `events.rs` returns early.
    Unchanged((u32, u32)),
    /// `land_frame`: a window that stops painting mid-move.
    Land,
}

/// The viewport `events.rs` lays a `Resized` out at.
fn viewport(pixels: (u32, u32), display: Display) -> (f32, f32) {
    let scale = display.scale as f32;
    (pixels.0 as f32 / scale, pixels.1 as f32 / scale)
}

/// The host's own writes, one event at a time, with the window's content size in pixels.
fn feed(slot: &mut Option<FrameMove>, inner: &mut (u32, u32), display: Display, step: Step) -> Observed {
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
            FrameMove::request_jump(slot, *inner, size, display.scale, display.wayland);
            false
        }
        Step::Applied(pixels) => {
            *inner = pixels;
            false
        }
        Step::Resized(pixels) => {
            *inner = pixels;
            super::events::resize_reply(slot, viewport(pixels, display), pixels, false) == ResizeReply::Stepped
        }
        // The host's own path, `events.rs`: an unchanged reply lays nothing out.
        Step::Unchanged(pixels) => {
            super::events::resize_reply(slot, viewport(pixels, display), pixels, true);
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
/// it, with the window's content size in pixels. The branches are dev's, copied literally, so a
/// change to the branch can't move both sides.
#[derive(Clone, Copy, Debug)]
struct Apart {
    frame_move: Option<()>,
    frame_jump: Option<(f32, f32)>,
    inner: (u32, u32),
    display: Display,
}

impl Apart {
    fn step(&mut self, step: Step) -> Observed {
        let mut asked = false;
        let display = self.display;
        match step {
            // take_frame_request, painting: `self.frame_move = Some(FrameMove { anim, origin })`.
            Step::Animate => self.frame_move = Some(()),
            // take_frame_request, not painting: `self.frame_move = None`, and the frame is set.
            Step::Cancel => self.frame_move = None,
            // differs from dev: dev's `self.frame_jump = Some((width, height))` held a coupon for
            // every request, including one for the pixels the window already has. No `Resized`
            // answers that one, so it stayed and the user's next resize spent it. Such a request
            // still retires a coupon for other pixels, as dev's replaced it, and replaces one for
            // these pixels, as dev did.
            Step::Jump(size) => {
                if native(size, display) != self.inner {
                    self.frame_jump = Some(size);
                } else if let Some(pending) = self.frame_jump {
                    self.frame_jump = (native(pending, display) == self.inner).then_some(size);
                }
            }
            Step::Applied(pixels) => self.inner = pixels,
            // WindowEvent::Resized: `frame_jump.take().is_some_and(|(w, h)| …)`, dev's.
            Step::Resized(pixels) => {
                asked = self.frame_jump.take().is_some_and(|(w, h)| within((w, h), viewport(pixels, display)));
                self.inner = pixels;
            }
            // The host's unchanged reply, differs from dev: dev returned with the coupon still
            // pending; the jump for exactly these pixels is answered there.
            Step::Unchanged(pixels) => {
                if self.frame_jump.is_some_and(|size| native(size, display) == pixels) {
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

/// Both sides fed every sequence from a window `start` pixels big, compared after every event:
/// what the window sees (motion and whether it asked for the size), what the move holds, and
/// the window's size.
fn holds_up(display: Display, start: (u32, u32)) -> impl FnMut(&[Step]) {
    move |seq| {
        let (mut slot, mut inner) = (None, start);
        let mut apart = Apart { frame_move: None, frame_jump: None, inner: start, display };
        for (k, step) in seq.iter().enumerate() {
            let seen = feed(&mut slot, &mut inner, display, *step);
            let expected = apart.step(*step);
            assert_eq!(seen, expected, "{display:?} {seq:?} at {k}");
            assert_eq!(inner, apart.inner, "the window's size after {seq:?} at {k}");
            // A platform's events never leave both of dev's fields set; the one value can't hold both.
            assert!(!(apart.frame_move.is_some() && apart.frame_jump.is_some()), "{seq:?} at {k}");
            let left = match (apart.frame_move, apart.frame_jump) {
                (Some(()), _) => Some(FrameMove::animating(animation(), ORIGIN)),
                (None, Some(size)) => Some(FrameMove::jumped(size, native(size, display))),
                (None, None) => None,
            };
            assert_eq!(slot, left, "the move after {display:?} {seq:?} at {k}");
        }
    }
}

/// A jump request, its `Resized` and a landing in every order: the coupon is spent by exactly
/// the event it announced, once, and is never motion. Requests for the pixels the window
/// already has hold none, and one for an adjacent pixel does.
#[test]
fn every_order_of_jumps_spends_one_coupon() {
    let all = [
        Step::Jump(JUMP), Step::Jump(OTHER_JUMP), Step::Jump(START_PT), Step::Jump(START_SAME),
        Step::Jump(START_ADJACENT), Step::Resized(HIS), Step::Resized(USER), Step::Land,
    ];
    every_sequence(&all, 6, holds_up(RETINA, START));
}

/// The host's reply path in every order: an unchanged `Resized` answers the jump for its exact
/// pixels and leaves another's, and a size that changes the viewport still spends its coupon.
#[test]
fn every_order_of_unchanged_replies_keeps_the_rules() {
    let all = [
        Step::Jump(JUMP), Step::Jump(OTHER_JUMP), Step::Jump(START_PT), Step::Unchanged(HIS),
        Step::Unchanged(ADJACENT), Step::Unchanged(START), Step::Resized(USER), Step::Land,
    ];
    every_sequence(&all, 6, holds_up(RETINA, START));
}

/// Requests merging back to the window's size in every order with the OS applying a size before
/// its `Resized`, and the user's drags: a request that needs no coupon retires one for other
/// pixels and keeps one for its own, whose reply is still queued (r301).
#[test]
fn every_order_of_requests_merging_back_keeps_the_rules() {
    // 420x260 pt is 840x520 px; the request is for 500x400 pt, 1000x800 px.
    let here = (840, 520);
    let all = [
        Step::Jump(START_PT), Step::Jump(OTHER_JUMP), Step::Applied(START), Step::Applied(here),
        Step::Unchanged(here), Step::Resized(START), Step::Resized(USER),
    ];
    every_sequence(&all, 6, holds_up(RETINA, here));
}

/// Fractional scales, where winit converts the request in f64: at 1.25x and 1.5x from 361 px, a
/// request that rounds to 360 px in f64 (but 361 in f32) holds its coupon, one for the window's
/// own pixels doesn't, and on Wayland whole logical pixels decide (r313).
#[test]
fn every_order_of_fractional_scale_jumps_converts_as_the_platform_does() {
    for (display, near, same, far) in [
        (Display { scale: 1.25, wayland: false }, 288.4, 288.8, 289.4),
        (Display { scale: 1.5, wayland: false }, 240.333_33, 240.666_67, 241.2),
        (Display { scale: 1.25, wayland: true }, 288.4, 288.8, 289.4),
    ] {
        let all = [
            Step::Jump((near, 100.0)), Step::Jump((same, 100.0)), Step::Jump((far, 100.0)),
            Step::Resized(native((near, 100.0), display)), Step::Unchanged(native((near, 100.0), display)),
            Step::Resized((361, native((0.0, 100.0), display).1)), Step::Applied((400, 300)),
        ];
        every_sequence(&all, 5, holds_up(display, (361, native((0.0, 100.0), display).1)));
    }
}

/// macOS's moving frames and their cancels in every order with the events that arrive beside
/// them: each sequence leaves the frame moving or still as dev did, including a request that
/// needs no coupon.
#[test]
fn every_order_of_animations_leaves_the_same_frame_moving() {
    let all = [Step::Animate, Step::Cancel, Step::Resized(HIS), Step::Land];
    every_sequence(&all, 7, holds_up(RETINA, START));
    let all = [Step::Animate, Step::Cancel, Step::Jump(START_PT), Step::Jump(START_SAME), Step::Unchanged(START), Step::Land];
    every_sequence(&all, 7, holds_up(RETINA, START));
}

/// The coupon's rules, named.
#[test]
fn a_jump_is_spent_by_the_resize_it_announced_once() {
    let (mut slot, mut inner) = (None, START);
    assert!(!feed(&mut slot, &mut inner, RETINA, Step::Resized(USER)).asked, "no request, no coupon");
    let requested = feed(&mut slot, &mut inner, RETINA, Step::Jump(JUMP));
    assert!(!requested.moving, "a jump names one event, it isn't motion");
    assert!(feed(&mut slot, &mut inner, RETINA, Step::Resized(HIS)).asked, "its own size lays out as a step");
    assert!(!feed(&mut slot, &mut inner, RETINA, Step::Resized(HIS)).asked, "once");
}

/// A later request replaces the coupon, a landing leaves it, and an animation moving beside it
/// is never read as the jump.
#[test]
fn a_later_request_or_landing_keeps_the_rules() {
    let (mut slot, mut inner) = (None, START);
    feed(&mut slot, &mut inner, RETINA, Step::Jump(JUMP));
    feed(&mut slot, &mut inner, RETINA, Step::Jump(OTHER_JUMP));
    assert!(!feed(&mut slot, &mut inner, RETINA, Step::Resized(HIS)).asked, "the earlier size is gone");
    feed(&mut slot, &mut inner, RETINA, Step::Jump(OTHER_JUMP));
    feed(&mut slot, &mut inner, RETINA, Step::Land);
    assert!(feed(&mut slot, &mut inner, RETINA, Step::Resized((840, 520))).asked, "a landing mid-move leaves the coupon for its Resized");
    assert!(feed(&mut slot, &mut inner, RETINA, Step::Animate).moving, "an animation is motion");
    feed(&mut slot, &mut inner, RETINA, Step::Animate);
    assert!(feed(&mut slot, &mut inner, RETINA, Step::Resized(HIS)).moving, "and no Resized spends it");
    assert!(!feed(&mut slot, &mut inner, RETINA, Step::Land).moving, "landing stops it");
}

/// A size request that moves no pixel gets no `Resized` back, so it holds no coupon; one that
/// moves a pixel keeps its coupon, rounded at the window's scale (r291, r299).
#[test]
fn only_a_request_that_moves_a_pixel_holds_a_coupon() {
    // At 2x, a 600x400 px window asks for its own 300x200 points.
    let (mut slot, mut inner) = (None, HIS);
    feed(&mut slot, &mut inner, RETINA, Step::Jump(JUMP));
    assert_eq!(slot, None, "no coupon for the size the window already has");
    assert!(!feed(&mut slot, &mut inner, RETINA, Step::Resized(USER)).asked, "so the user's resize stays the user's");
    // Rounded, not within a point: 300.2 pt still rounds to 600 px.
    let (mut slot, mut inner) = (None, HIS);
    feed(&mut slot, &mut inner, RETINA, Step::Jump((300.2, 200.2)));
    assert_eq!(slot, None, "same pixels at this scale");
    // 300.5 pt is 601 px: the window really moves, so the request keeps its coupon.
    let (mut slot, mut inner) = (None, HIS);
    feed(&mut slot, &mut inner, RETINA, Step::Jump((300.5, 200.0)));
    assert_eq!(slot, Some(FrameMove::jumped((300.5, 200.0), (601, 400))), "600 -> 601 px keeps its coupon");
    assert!(feed(&mut slot, &mut inner, RETINA, Step::Resized((601, 400))).asked, "and its reply steps");
}

/// At fractional scales the request is converted as winit and Win32 convert it, in f64: a
/// one-pixel resize that an f32 product would round back to the window's size keeps its
/// coupon (r313).
#[test]
fn a_fractional_scale_request_converts_as_the_platform_does() {
    for (scale, width) in [(1.25, 288.4_f32), (1.5, 240.333_33_f32)] {
        let display = Display { scale, wayland: false };
        // 361 px tall too, so both axes take the same path.
        let request = (width, width);
        let to_physical: winit::dpi::PhysicalSize<u32> = winit::dpi::LogicalSize::new(width, width).to_physical(scale);
        assert_eq!(to_physical.width, 360, "winit asks the OS for 360 px at {scale}x");
        assert_eq!((width * scale as f32).round(), 361.0, "where an f32 product rounds to the window's 361");
        let (mut slot, mut inner) = (None, (361, 361));
        feed(&mut slot, &mut inner, display, Step::Jump(request));
        assert_eq!(slot, Some(FrameMove::jumped(request, (360, 360))), "a real one-pixel resize holds its coupon at {scale}x");
        assert!(feed(&mut slot, &mut inner, display, Step::Resized((360, 360))).asked, "and its reply steps at {scale}x");
    }
    // Wayland rounds to whole logical pixels first: 289.4 pt is 289, 361 px at 1.25x, which the
    // window already has, where winit elsewhere asks for 362.
    let wayland = Display { scale: 1.25, wayland: true };
    let (mut slot, mut inner) = (None, (361, 361));
    feed(&mut slot, &mut inner, wayland, Step::Jump((289.4, 289.4)));
    assert_eq!(slot, None, "no coupon for Wayland's own 361 px");
    let (mut slot, mut inner) = (None, (361, 361));
    feed(&mut slot, &mut inner, Display { wayland: false, ..wayland }, Step::Jump((289.4, 289.4)));
    assert_eq!(slot, Some(FrameMove::jumped((289.4, 289.4), (362, 362))), "elsewhere 362 px holds one");
}

/// Requests merging back to the window's size: one for 500 pt then one for the 420 pt the
/// window has, coalesced with no reply. The second retires the first, so the user's later drag
/// to 500 pt is the user's; but when the OS already applied 500 pt and its `Resized` is queued,
/// a repeat of 500 pt keeps the coupon for it (r301).
#[test]
fn requests_merging_back_to_the_window_size_retire_the_earlier_jump() {
    let here = (840, 520);
    let (mut slot, mut inner) = (None, here);
    feed(&mut slot, &mut inner, RETINA, Step::Jump((500.0, 260.0)));
    feed(&mut slot, &mut inner, RETINA, Step::Jump(OTHER_JUMP));
    assert_eq!(slot, None, "the request for the window's own size retires the earlier one");
    feed(&mut slot, &mut inner, RETINA, Step::Unchanged(here));
    assert!(!feed(&mut slot, &mut inner, RETINA, Step::Resized((1000, 520))).asked, "so the user's drag to 500 pt sets the viewport");

    let (mut slot, mut inner) = (None, here);
    feed(&mut slot, &mut inner, RETINA, Step::Jump((500.0, 260.0)));
    feed(&mut slot, &mut inner, RETINA, Step::Applied((1000, 520)));
    feed(&mut slot, &mut inner, RETINA, Step::Jump((500.0, 260.0)));
    assert!(feed(&mut slot, &mut inner, RETINA, Step::Resized((1000, 520))).asked, "a repeat keeps the coupon its queued reply spends");
}

/// A reply that changes nothing answers its jump through the host's own path and lays nothing
/// out; another size's reply leaves it, and the user's drag then sets the viewport (r302).
#[test]
fn an_unchanged_reply_spends_its_coupon_through_the_host() {
    // A jump is pending and the window is already laid out at its size (the OS applied it, or
    // rt was initialised at it): its `Resized` changes nothing, and the host returns early.
    let mut slot = Some(FrameMove::jumped(JUMP, HIS));
    assert_eq!(resize_reply(&mut slot, viewport(HIS, RETINA), HIS, true), ResizeReply::Unchanged, "nothing to lay out");
    assert_eq!(slot, None, "but its own reply still spends its coupon");
    // A reply for a size the jump never asked for leaves it: another event is still coming.
    let mut slot = Some(FrameMove::jumped(JUMP, HIS));
    assert_eq!(resize_reply(&mut slot, viewport(ADJACENT, RETINA), ADJACENT, true), ResizeReply::Unchanged, "still nothing to lay out");
    assert_eq!(slot, Some(FrameMove::jumped(JUMP, HIS)), "not its pixels, so not its reply");
    // And the user's drag afterwards lands on the normal viewport path.
    let mut slot = None;
    assert_eq!(resize_reply(&mut slot, viewport(USER, RETINA), USER, false), ResizeReply::Set, "the user's drag sets the viewport");
}
