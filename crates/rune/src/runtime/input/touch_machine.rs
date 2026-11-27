//! Pure touchscreen contact state machine (`rune::machine`): tracks single-finger
//! tap/drag/pan/window-drag/long-press alongside two-finger pinch recognition and quiescent
//! post-pinch handling.
//!
//! `step_touch` is plain data in and `Effects<TouchEffect>` out; `runtime/input/touch.rs`
//! queries read-only hit-test facts, feeds `TouchInput`, and carries out the returned effects.
//! Thresholds follow Chromium `ScaleGestureDetector` / Android `ViewConfiguration`
//! (`SPAN_SLOP = 2 * TOUCH_SLOP = 16 pt`, `MIN_SPAN = 16 pt`) with slop subtracted on activation
//! so the first `Moved` delta has no initial jump.
use rune_motion::NodeId;

use crate::GesturePhase;
use crate::machine::Effects;

/// How far (points) a single finger travels before its touch is a drag or a pan, not a tap.
pub(super) const TOUCH_SLOP: f32 = 8.0;
/// Midpoint travel (points) before a two-finger pan emits `Began` (subtracted on activation).
pub(super) const PAN_SLOP: f32 = TOUCH_SLOP;
/// Span change (points) two fingers travel before a pinch emits `Began` (Chromium/Android
/// `span_slop = 2 * touch_slop`). Subtracted on activation so the first `Moved` does not jump.
pub(super) const SPAN_SLOP: f32 = TOUCH_SLOP * 2.0;
/// Safety floor (points) for active pinch span ratios and minimum span for touch rotation so
/// coincident or collapsing fingers never divide by zero or produce spurious angles.
pub(super) const MIN_SPAN: f32 = 16.0;
/// Angular turn (degrees) before two-finger rotation emits `Began` (subtracted on activation).
pub(super) const ROTATE_SLOP_DEG: f32 = 8.0;
/// A finger held this long (seconds) before lifting throws nothing.
pub(super) const FLING_HOLD: f64 = 0.1;

/// One active finger on a touchscreen: its platform id and latest window-point position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Contact {
    pub(super) id: u64,
    pub(super) pos: (f32, f32),
}

/// Phase of a single active finger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// Still within `TOUCH_SLOP`: lifting here is a tap. `scroll` is the vertical viewport
    /// under the finger, held until the pan picks its axis; `edge` holds the innermost matching
    /// `El::on_edge_drag` claimant for each physical edge whose zone the finger landed in;
    /// `tap_focus` records whether lifting as a tap should commit deferred focus and selection.
    Pending { scroll: Option<NodeId>, edge: super::edge_drag::EdgeCandidates, tap_focus: bool },
    /// Landed on a bare `.window_drag()` region and still within `TOUCH_SLOP`: holds the drag
    /// until the finger moves past the slop, so taps, double-taps and long presses still work.
    WindowDragPending,
    /// Crossed `TOUCH_SLOP` on a bare `.window_drag()` region and emitted `TitlebarAction::Drag`
    /// (or immediately triggered `TitlebarAction::DoubleClick`): moves and release do nothing.
    WindowDragging,
    /// It drags the element it pressed.
    Drag,
    /// An `El::on_edge_drag` element claimed it: the press under it was let go.
    Edge { node: NodeId, edge: crate::SwipeEdge },
    /// An active edge drag was cancelled mid-touch (Escape, unmount, focus loss, modal, or
    /// `cancel_pointer`): moves emit nothing and release returns to `Idle` without clicking.
    Cancelled,
    /// It pans viewport `scroll` along one axis.
    Pan { scroll: NodeId, horizontal: bool },
    /// A long press opened the menu under it: it lifts on nothing.
    Menu,
}

impl Mode {
    /// The viewport the touch holds: the one it pans, or while pending the one under it.
    pub(super) fn scroll(self) -> Option<NodeId> {
        match self {
            Mode::Pending { scroll, .. } => scroll,
            Mode::Pan { scroll, .. } => Some(scroll),
            Mode::WindowDragPending | Mode::WindowDragging | Mode::Drag | Mode::Edge { .. } | Mode::Cancelled | Mode::Menu => None,
        }
    }

    pub(super) fn horizontal(self) -> bool {
        matches!(self, Mode::Pan { horizontal: true, .. })
    }

    pub(super) fn hold_eligible(self) -> bool {
        matches!(self, Mode::Pending { .. } | Mode::WindowDragPending)
    }

    /// Whether a second finger landing may hand this touch off into a two-finger gesture.
    /// Active (or cancelled) edge swipes ignore a second finger so a stray contact cannot abort a
    /// back/drawer drag; an unclaimed `Pending` edge candidate is discarded when two fingers pinch.
    pub(super) fn pinch_eligible(self) -> bool {
        !matches!(self, Mode::WindowDragging | Mode::Edge { .. } | Mode::Cancelled | Mode::Menu)
    }
}

/// A single tracked finger and its pan-velocity history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Finger {
    pub(super) id: u64,
    pub(super) origin: (f32, f32),
    /// Latest window-point position, used as the anchor if a second finger joins.
    pub(super) pos: (f32, f32),
    /// Travel along the pan's axis (vertical until a pan picks horizontal) at the last event,
    /// and that event's time.
    pub(super) last: f32,
    pub(super) at: f64,
    /// Points per second along the pan's axis.
    pub(super) velocity: f32,
    pub(super) mode: Mode,
}

/// Which two-finger gesture channels have a live, allowed handler under the initial midpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TwoFingerTargets {
    pub(super) wheel: bool,
    pub(super) magnify: bool,
    pub(super) rotate: bool,
}

impl TwoFingerTargets {
    #[inline]
    pub(super) const fn any(self) -> bool {
        self.wheel || self.magnify || self.rotate
    }
}

/// Per-channel recognition phase for two-finger midpoint pan (`El::on_wheel`, `touch: true`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PanChannel {
    /// No `on_wheel` handler lay under the initial midpoint.
    Disabled,
    /// Waiting for midpoint displacement `> PAN_SLOP`, or for `pinch` to begin.
    Armed,
    /// `Began` was emitted; `prev_mid` is the last emitted midpoint.
    Active { prev_mid: (f32, f32) },
}

/// Per-channel recognition phase for two-finger pinch (`El::on_magnify`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PinchChannel {
    /// No `on_magnify` handler lay under the initial midpoint.
    Disabled,
    /// Waiting for `|span - initial_span| > SPAN_SLOP` before emitting `Began`.
    Armed { initial_span: f32 },
    /// `Began` was emitted; `prev_span` is the last clamped span (`>= MIN_SPAN`).
    Active { prev_span: f32 },
}

/// Per-channel recognition phase for two-finger rotation (`El::on_rotate`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum RotateChannel {
    /// No `on_rotate` handler lay under the initial midpoint.
    Disabled,
    /// Waiting for `span >= MIN_SPAN` and cumulative turn `> ROTATE_SLOP_DEG`.
    /// `anchored` is false until the fingers first separate to `MIN_SPAN`.
    Armed { anchored: bool, prev_angle: f32, cumulative: f32 },
    /// `Began` was emitted; `prev_angle` is the last valid finger-line angle (at `span >= MIN_SPAN`).
    Active { prev_angle: f32 },
}

/// Two active fingers tracked together after cancelling any single-finger press/hold/pan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TwoFinger {
    pub(super) first: Contact,
    pub(super) second: Contact,
    /// Midpoint where the two-finger gesture began; used to resolve `Began`'s handler chain
    /// even if the midpoint drifted slightly past an edge while crossing slop.
    pub(super) initial_mid: (f32, f32),
    /// Latest finite midpoint between the two fingers in window points.
    pub(super) mid: (f32, f32),
    pub(super) pan: PanChannel,
    pub(super) pinch: PinchChannel,
    pub(super) rotate: RotateChannel,
}

impl TwoFinger {
    pub(super) fn new(first: Contact, second: Contact, targets: TwoFingerTargets) -> Self {
        let initial_mid = midpoint(first.pos, second.pos);
        let initial_span = span(first.pos, second.pos);
        let pan = if targets.wheel { PanChannel::Armed } else { PanChannel::Disabled };
        let pinch = if targets.magnify {
            PinchChannel::Armed { initial_span }
        } else {
            PinchChannel::Disabled
        };
        let rotate = if targets.rotate {
            if initial_span.is_finite() && initial_span >= MIN_SPAN {
                let angle = finger_angle_deg(first.pos, second.pos);
                RotateChannel::Armed { anchored: angle.is_finite(), prev_angle: if angle.is_finite() { angle } else { 0.0 }, cumulative: 0.0 }
            } else {
                RotateChannel::Armed { anchored: false, prev_angle: 0.0, cumulative: 0.0 }
            }
        } else {
            RotateChannel::Disabled
        };
        Self { first, second, initial_mid, mid: initial_mid, pan, pinch, rotate }
    }
}

/// Whole touchscreen contact lifecycle, stepped by [`step_touch`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum TouchState {
    #[default]
    Idle,
    Single(Finger),
    TwoFinger(TwoFinger),
    /// One finger remaining down after a two-finger gesture ended: keeps `touching()` true,
    /// emits no tap/drag/pan/fling on moves or release, and allows a returning second finger to
    /// begin a fresh two-finger gesture (pinch-ratcheting).
    PostPinch(Contact),
}

impl TouchState {
    #[inline]
    pub(super) fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    #[inline]
    pub(super) fn single(&self) -> Option<&Finger> {
        match self {
            Self::Single(f) => Some(f),
            Self::Idle | Self::TwoFinger(_) | Self::PostPinch(_) => None,
        }
    }

    #[inline]
    pub(super) fn in_menu(&self) -> bool {
        matches!(self, Self::Single(f) if f.mode == Mode::Menu)
    }

    #[inline]
    pub(super) fn hold_eligible(&self) -> bool {
        matches!(self, Self::Single(f) if f.mode.hold_eligible())
    }

    /// If a newly landed finger `id` at `pos` is eligible to enter a two-finger gesture from
    /// this state, returns the initial midpoint to query handlers at.
    #[inline]
    pub(super) fn second_down_midpoint(&self, id: u64, pos: (f32, f32)) -> Option<(f32, f32)> {
        match *self {
            Self::Single(f) if f.id != id && f.mode.pinch_eligible() => Some(midpoint(f.pos, pos)),
            Self::PostPinch(c) if c.id != id => Some(midpoint(c.pos, pos)),
            Self::Idle | Self::Single(_) | Self::TwoFinger(_) | Self::PostPinch(_) => None,
        }
    }

    /// If a move of finger `id` to `pos` crosses `TOUCH_SLOP` out of `Mode::Pending`, returns
    /// `(origin, horizontal_bias, vertical_scroll)` so the caller can supply read-only
    /// `SlopResolution` facts to `TouchInput::Move`.
    #[inline]
    pub(super) fn single_slop_query(&self, id: u64, pos: (f32, f32)) -> Option<((f32, f32), bool, Option<NodeId>)> {
        let Self::Single(f) = *self else { return None };
        if f.id != id { return None; }
        let Mode::Pending { scroll, edge, .. } = f.mode else { return None };
        let (dx, dy) = (pos.0 - f.origin.0, pos.1 - f.origin.1);
        if dx.hypot(dy) <= TOUCH_SLOP {
            return None;
        }
        if super::edge_drag::moves_away_edge(dx, dy).is_some_and(|e| edge[e.index()].is_some()) {
            return None;
        }
        Some((f.origin, dx.abs() > dy.abs(), scroll))
    }
}

/// Read-only facts queried by `touch.rs` when a `Mode::Pending` finger crosses `TOUCH_SLOP`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SlopResolution {
    pub(super) press_drags: bool,
    pub(super) across: Option<NodeId>,
}

/// Events fed into [`step_touch`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TouchInput {
    StartSingle(Finger),
    SecondDown { id: u64, pos: (f32, f32), targets: TwoFingerTargets },
    Move { id: u64, pos: (f32, f32), time: f64, slop: Option<SlopResolution> },
    End { id: u64, pos: (f32, f32), time: f64, cancelled: bool },
    HoldFired,
    CancelEdge,
}

/// Side effects requested by [`step_touch`] and executed by `touch.rs` in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TouchEffect {
    CancelHold,
    StartWindowDrag,
    ClearTitlebar,
    BeginPan { origin: (f32, f32), horizontal: bool, vertical: Option<NodeId> },
    PanMove { scroll: NodeId, along: f32 },
    EndScroll { scroll: Option<NodeId>, velocity: f32 },
    BeginTouchSelectionDrag { origin: (f32, f32) },
    CommitTap((f32, f32)),
    CommitWindowDragTap,
    PointerMove((f32, f32)),
    PointerUp((f32, f32)),
    PointerLeave,
    ReleaseWithoutClick,
    CancelPointer,
    EdgeDrag { node: NodeId, edge: crate::SwipeEdge, phase: crate::Phase, origin: (f32, f32), pos: (f32, f32), cancelled: bool },
    CommitHold { origin: (f32, f32) },
    EmitTouchWheel { dx: f32, dy: f32, mid: (f32, f32), hit: (f32, f32), phase: GesturePhase },
    EmitMagnify { delta: f32, mid: (f32, f32), hit: (f32, f32), phase: GesturePhase },
    EmitRotate { degrees: f32, mid: (f32, f32), hit: (f32, f32), phase: GesturePhase },
}

#[inline]
pub(super) fn midpoint(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

#[inline]
pub(super) fn span(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Angle of the vector from `a` to `b` in degrees, positive counterclockwise in screen
/// coordinates (`+x` right, `+y` down).
#[inline]
pub(super) fn finger_angle_deg(a: (f32, f32), b: (f32, f32)) -> f32 {
    (-(b.1 - a.1)).atan2(b.0 - a.0).to_degrees()
}

/// Wraps an angular delta in degrees into `(-180.0, 180.0]`.
#[inline]
pub(super) fn wrap_deg(deg: f32) -> f32 {
    if !deg.is_finite() {
        return 0.0;
    }
    let r = (deg + 180.0).rem_euclid(360.0) - 180.0;
    if r == -180.0 { 180.0 } else { r }
}

/// Pure transition function for touchscreen contact state.
pub(super) fn step_touch(state: &mut TouchState, input: TouchInput) -> Effects<TouchEffect> {
    match (*state, input) {
        (TouchState::Idle, TouchInput::StartSingle(f)) => {
            *state = TouchState::Single(f);
            Effects::none()
        }
        (TouchState::Single(f), TouchInput::SecondDown { id, pos, targets })
            if f.id != id && f.mode.pinch_eligible() && targets.any() =>
        {
            let first = Contact { id: f.id, pos: f.pos };
            let second = Contact { id, pos };
            *state = TouchState::TwoFinger(TwoFinger::new(first, second, targets));
            let mut effects = Effects::none();
            effects.push(TouchEffect::CancelHold);
            effects.push(TouchEffect::EndScroll { scroll: f.mode.scroll(), velocity: 0.0 });
            effects.push(TouchEffect::CancelPointer);
            effects
        }
        (TouchState::PostPinch(c), TouchInput::SecondDown { id, pos, targets })
            if c.id != id && targets.any() =>
        {
            *state = TouchState::TwoFinger(TwoFinger::new(c, Contact { id, pos }, targets));
            Effects::none()
        }
        (TouchState::Single(mut f), TouchInput::Move { id, pos, time, slop }) if f.id == id => {
            let effects = step_single_move(&mut f, pos, time, slop);
            *state = TouchState::Single(f);
            effects
        }
        (TouchState::TwoFinger(mut tf), TouchInput::Move { id, pos, .. }) => {
            let effects = step_two_finger_move(&mut tf, id, pos);
            *state = TouchState::TwoFinger(tf);
            effects
        }
        (TouchState::PostPinch(mut c), TouchInput::Move { id, pos, .. }) => {
            let effects = step_post_pinch_move(&mut c, id, pos);
            *state = TouchState::PostPinch(c);
            effects
        }
        (TouchState::Single(f), TouchInput::End { id, pos, time, cancelled }) if f.id == id => {
            *state = TouchState::Idle;
            step_single_end(f, pos, time, cancelled)
        }
        (TouchState::TwoFinger(tf), TouchInput::End { id, pos, cancelled, .. }) => {
            if let Some((remaining, effects)) = step_two_finger_end(tf, id, pos, cancelled) {
                *state = TouchState::PostPinch(remaining);
                effects
            } else {
                Effects::none()
            }
        }
        (TouchState::PostPinch(c), TouchInput::End { id, .. }) if c.id == id => {
            *state = TouchState::Idle;
            Effects::none()
        }
        (TouchState::Single(mut f), TouchInput::HoldFired) if f.mode.hold_eligible() => {
            let (origin, scroll, was_window_drag) =
                (f.origin, f.mode.scroll(), f.mode == Mode::WindowDragPending);
            f.mode = Mode::Menu;
            *state = TouchState::Single(f);
            let mut effects = Effects::none();
            effects.push(TouchEffect::EndScroll { scroll, velocity: 0.0 });
            if was_window_drag {
                effects.push(TouchEffect::ClearTitlebar);
            }
            effects.push(TouchEffect::CommitHold { origin });
            effects
        }
        (TouchState::Single(mut f), TouchInput::CancelEdge) if matches!(f.mode, Mode::Edge { .. }) => {
            f.mode = Mode::Cancelled;
            *state = TouchState::Single(f);
            Effects::none()
        }
        _ => Effects::none(),
    }
}

fn step_single_move(
    f: &mut Finger,
    pos: (f32, f32),
    time: f64,
    slop: Option<SlopResolution>,
) -> Effects<TouchEffect> {
    f.pos = pos;
    let (dx, dy) = (pos.0 - f.origin.0, pos.1 - f.origin.1);
    let mut effects = Effects::none();
    if f.mode == Mode::WindowDragPending && dx.hypot(dy) > TOUCH_SLOP {
        f.mode = Mode::WindowDragging;
        effects.push(TouchEffect::StartWindowDrag);
        effects.push(TouchEffect::CancelHold);
    } else if let Mode::Pending { scroll, edge, .. } = f.mode
        && dx.hypot(dy) > TOUCH_SLOP
        && let Some((node, swipe_edge)) = super::edge_drag::moves_away_edge(dx, dy)
            .and_then(|e| edge[e.index()].map(|node| (node, e)))
    {
        f.mode = Mode::Edge { node, edge: swipe_edge };
        effects.push(TouchEffect::CancelHold);
        effects.push(TouchEffect::EndScroll { scroll, velocity: 0.0 });
        effects.push(TouchEffect::CancelPointer);
        effects.push(TouchEffect::EdgeDrag {
            node,
            edge: swipe_edge,
            phase: crate::Phase::Down,
            origin: f.origin,
            pos: f.origin,
            cancelled: false,
        });
    } else if let Mode::Pending { scroll, tap_focus, .. } = f.mode
        && dx.hypot(dy) > TOUCH_SLOP
    {
        let horizontal_bias = dx.abs() > dy.abs();
        let res = slop.unwrap_or(SlopResolution { press_drags: false, across: None });
        if res.press_drags {
            f.mode = Mode::Drag;
        } else {
            effects.push(TouchEffect::BeginPan {
                origin: f.origin,
                horizontal: horizontal_bias,
                vertical: scroll,
            });
            f.mode = match (horizontal_bias.then_some(res.across).flatten(), scroll) {
                (Some(id), _) => Mode::Pan { scroll: id, horizontal: true },
                (None, Some(id)) => Mode::Pan { scroll: id, horizontal: false },
                (None, None) => {
                    if tap_focus {
                        effects.push(TouchEffect::BeginTouchSelectionDrag { origin: f.origin });
                    }
                    Mode::Drag
                }
            };
        }
        effects.push(TouchEffect::CancelHold);
    }
    let along = if f.mode.horizontal() { dx } else { dy };
    let dt = time - f.at;
    if dt > 0.0 {
        f.velocity = (along - f.last) / dt as f32;
    }
    (f.last, f.at) = (along, time);
    match f.mode {
        Mode::Pan { scroll, .. } => effects.push(TouchEffect::PanMove { scroll, along }),
        Mode::Drag => effects.push(TouchEffect::PointerMove(pos)),
        Mode::Edge { node, edge } => effects.push(TouchEffect::EdgeDrag {
            node,
            edge,
            phase: crate::Phase::Move,
            origin: f.origin,
            pos,
            cancelled: false,
        }),
        Mode::Pending { .. } | Mode::WindowDragPending | Mode::WindowDragging | Mode::Cancelled | Mode::Menu => {}
    }
    effects
}

fn step_single_end(
    f: Finger,
    pos: (f32, f32),
    time: f64,
    cancelled: bool,
) -> Effects<TouchEffect> {
    let mut effects = Effects::none();
    effects.push(TouchEffect::CancelHold);
    let pan = matches!(f.mode, Mode::Pan { .. });
    let velocity = if pan && !cancelled && time - f.at < FLING_HOLD { f.velocity } else { 0.0 };
    effects.push(TouchEffect::EndScroll { scroll: f.mode.scroll(), velocity });
    if let Mode::Edge { node, edge } = f.mode {
        effects.push(TouchEffect::EdgeDrag {
            node,
            edge,
            phase: crate::Phase::Up,
            origin: f.origin,
            pos,
            cancelled,
        });
    }
    if cancelled || f.mode == Mode::WindowDragging {
        effects.push(TouchEffect::CancelPointer);
        return effects;
    }
    if f.mode == Mode::WindowDragPending {
        effects.push(TouchEffect::CommitWindowDragTap);
        effects.push(TouchEffect::ReleaseWithoutClick);
        return effects;
    }
    if matches!(f.mode, Mode::Menu | Mode::Edge { .. } | Mode::Cancelled) {
        effects.push(TouchEffect::ReleaseWithoutClick);
        return effects;
    }
    // A pan cancelled the press, so its release clicks nothing.
    let release_pos = if pan { f.origin } else { pos };
    if matches!(f.mode, Mode::Pending { tap_focus: true, .. }) {
        effects.push(TouchEffect::CommitTap(release_pos));
    }
    if f.mode == Mode::Drag {
        effects.push(TouchEffect::PointerMove(release_pos));
    }
    effects.push(TouchEffect::PointerUp(release_pos));
    effects.push(TouchEffect::PointerLeave);
    effects
}

/// Processes a move while two fingers are tracked. Ignores any third or further finger.
/// Emits events strictly in order: two-finger pan (`on_wheel`, `touch: true`), then pinch
/// (`on_magnify`), then rotation (`on_rotate`), so pan followed by zoom keeps both fingers
/// pinned to their world points.
fn step_two_finger_move(tf: &mut TwoFinger, id: u64, pos: (f32, f32)) -> Effects<TouchEffect> {
    if id == tf.first.id {
        tf.first.pos = pos;
    } else if id == tf.second.id {
        tf.second.pos = pos;
    } else {
        return Effects::none();
    }
    let next_mid = midpoint(tf.first.pos, tf.second.pos);
    if next_mid.0.is_finite() && next_mid.1.is_finite() {
        tf.mid = next_mid;
    }
    let raw_span = span(tf.first.pos, tf.second.pos);
    let mut effects = Effects::none();
    step_pan_channel(tf, &mut effects);
    let mut pinch_effects = Effects::none();
    let pinch_began = step_pinch_channel(tf, raw_span, &mut pinch_effects);
    if pinch_began && matches!(tf.pan, PanChannel::Armed) {
        // Matching Chromium `GestureProvider::SendImpl` on `kGesturePinchBegin`: begin pan at
        // the current midpoint with (0, 0) ahead of `Magnify(Began)` so subsequent moves track
        // both fingers 1:1 without waiting another 8 pt.
        tf.pan = PanChannel::Active { prev_mid: tf.mid };
        effects.push(TouchEffect::EmitTouchWheel {
            dx: 0.0,
            dy: 0.0,
            mid: tf.mid,
            hit: tf.initial_mid,
            phase: GesturePhase::Began,
        });
    }
    effects.extend(pinch_effects);
    step_rotate_channel(tf, raw_span, &mut effects);
    effects
}

fn step_pan_channel(tf: &mut TwoFinger, effects: &mut Effects<TouchEffect>) {
    match tf.pan {
        PanChannel::Disabled => {}
        PanChannel::Armed => {
            let (dx0, dy0) = (tf.mid.0 - tf.initial_mid.0, tf.mid.1 - tf.initial_mid.1);
            let dist = dx0.hypot(dy0);
            if !dist.is_finite() || dist <= PAN_SLOP {
                return;
            }
            let scale = (dist - PAN_SLOP) / dist;
            let (dx, dy) = (dx0 * scale, dy0 * scale);
            tf.pan = PanChannel::Active { prev_mid: tf.mid };
            effects.push(TouchEffect::EmitTouchWheel {
                dx: 0.0,
                dy: 0.0,
                mid: tf.mid,
                hit: tf.initial_mid,
                phase: GesturePhase::Began,
            });
            effects.push(TouchEffect::EmitTouchWheel {
                dx,
                dy,
                mid: tf.mid,
                hit: tf.mid,
                phase: GesturePhase::Moved,
            });
        }
        PanChannel::Active { ref mut prev_mid } => {
            let (dx, dy) = (tf.mid.0 - prev_mid.0, tf.mid.1 - prev_mid.1);
            *prev_mid = tf.mid;
            if (dx != 0.0 || dy != 0.0) && dx.is_finite() && dy.is_finite() {
                effects.push(TouchEffect::EmitTouchWheel {
                    dx,
                    dy,
                    mid: tf.mid,
                    hit: tf.mid,
                    phase: GesturePhase::Moved,
                });
            }
        }
    }
}

fn step_pinch_channel(tf: &mut TwoFinger, raw_span: f32, effects: &mut Effects<TouchEffect>) -> bool {
    if !raw_span.is_finite() {
        return false;
    }
    match tf.pinch {
        PinchChannel::Disabled => false,
        PinchChannel::Armed { initial_span } => {
            let initial = if initial_span.is_finite() { initial_span } else { raw_span };
            let diff = raw_span - initial;
            if diff.abs() <= SPAN_SLOP {
                if !initial_span.is_finite() {
                    tf.pinch = PinchChannel::Armed { initial_span: initial };
                }
                return false;
            }
            let boundary = if diff > 0.0 {
                (initial + SPAN_SLOP).max(MIN_SPAN)
            } else {
                (initial - SPAN_SLOP).max(MIN_SPAN)
            };
            let curr = raw_span.max(MIN_SPAN);
            let delta = curr / boundary - 1.0;
            let delta = if delta.is_finite() { delta } else { 0.0 };
            tf.pinch = PinchChannel::Active { prev_span: curr };
            effects.push(TouchEffect::EmitMagnify {
                delta: 0.0,
                mid: tf.mid,
                hit: tf.initial_mid,
                phase: GesturePhase::Began,
            });
            effects.push(TouchEffect::EmitMagnify {
                delta,
                mid: tf.mid,
                hit: tf.mid,
                phase: GesturePhase::Moved,
            });
            true
        }
        PinchChannel::Active { ref mut prev_span } => {
            let curr = raw_span.max(MIN_SPAN);
            let delta = curr / *prev_span - 1.0;
            let delta = if delta.is_finite() { delta } else { 0.0 };
            *prev_span = curr;
            effects.push(TouchEffect::EmitMagnify {
                delta,
                mid: tf.mid,
                hit: tf.mid,
                phase: GesturePhase::Moved,
            });
            false
        }
    }
}

fn step_rotate_channel(tf: &mut TwoFinger, raw_span: f32, effects: &mut Effects<TouchEffect>) {
    if !raw_span.is_finite() || raw_span < MIN_SPAN {
        return;
    }
    let angle = finger_angle_deg(tf.first.pos, tf.second.pos);
    if !angle.is_finite() {
        return;
    }
    match tf.rotate {
        RotateChannel::Disabled => {}
        RotateChannel::Armed { anchored, prev_angle, cumulative } => {
            if !anchored {
                tf.rotate = RotateChannel::Armed { anchored: true, prev_angle: angle, cumulative: 0.0 };
                return;
            }
            let next_cum = cumulative + wrap_deg(angle - prev_angle);
            if next_cum.abs() <= ROTATE_SLOP_DEG {
                tf.rotate = RotateChannel::Armed { anchored: true, prev_angle: angle, cumulative: next_cum };
                return;
            }
            let excess = if next_cum > 0.0 {
                next_cum - ROTATE_SLOP_DEG
            } else {
                next_cum + ROTATE_SLOP_DEG
            };
            tf.rotate = RotateChannel::Active { prev_angle: angle };
            effects.push(TouchEffect::EmitRotate {
                degrees: 0.0,
                mid: tf.mid,
                hit: tf.initial_mid,
                phase: GesturePhase::Began,
            });
            effects.push(TouchEffect::EmitRotate {
                degrees: excess,
                mid: tf.mid,
                hit: tf.mid,
                phase: GesturePhase::Moved,
            });
        }
        RotateChannel::Active { ref mut prev_angle } => {
            let degrees = wrap_deg(angle - *prev_angle);
            *prev_angle = angle;
            if degrees != 0.0 {
                effects.push(TouchEffect::EmitRotate {
                    degrees,
                    mid: tf.mid,
                    hit: tf.mid,
                    phase: GesturePhase::Moved,
                });
            }
        }
    }
}

fn step_two_finger_end(
    tf: TwoFinger,
    id: u64,
    pos: (f32, f32),
    cancelled: bool,
) -> Option<(Contact, Effects<TouchEffect>)> {
    let remaining = if id == tf.first.id {
        tf.second
    } else if id == tf.second.id {
        tf.first
    } else {
        return None;
    };
    let mid = if cancelled || !pos.0.is_finite() || !pos.1.is_finite() {
        tf.mid
    } else {
        midpoint(remaining.pos, pos)
    };
    let mut effects = Effects::none();
    if matches!(tf.pan, PanChannel::Active { .. }) {
        effects.push(TouchEffect::EmitTouchWheel {
            dx: 0.0,
            dy: 0.0,
            mid,
            hit: mid,
            phase: GesturePhase::Ended,
        });
    }
    if matches!(tf.pinch, PinchChannel::Active { .. }) {
        effects.push(TouchEffect::EmitMagnify {
            delta: 0.0,
            mid,
            hit: mid,
            phase: GesturePhase::Ended,
        });
    }
    if matches!(tf.rotate, RotateChannel::Active { .. }) {
        effects.push(TouchEffect::EmitRotate {
            degrees: 0.0,
            mid,
            hit: mid,
            phase: GesturePhase::Ended,
        });
    }
    Some((remaining, effects))
}

/// Keeps the remaining finger's live position current while quiescent after a pinch ends.
fn step_post_pinch_move(c: &mut Contact, id: u64, pos: (f32, f32)) -> Effects<TouchEffect> {
    if c.id == id && pos.0.is_finite() && pos.1.is_finite() {
        c.pos = pos;
    }
    Effects::none()
}
