//! Horizontal scrolling for `El::scroll_x` and `El::scroll_xy` viewports: the axis a wheel or
//! trackpad gesture locks to, Shift with a mouse wheel, momentum, `Tx::scroll_into_view` and
//! `Tx::scroll_to_x` on x.
//!
//! A gesture moves one axis, picked by its first delta, as AppKit's predominant-axis scrolling
//! does: mostly horizontal goes to the innermost horizontal viewport under the pointer, else to
//! the innermost vertical one, and a diagonal never moves both. The gesture keeps that axis
//! wherever the pointer ends up, as AppKit latches a gesture to the view it began on: one that
//! began outside any horizontal viewport stays vertical when the content scrolls one under the
//! pointer. Without phases (a mouse wheel), only a pause or a delta wholly on the other axis
//! ends the lock, so Shift + wheel and then the plain wheel each scroll at once.
use super::*;
use crate::machine::{Attempt, Seq};

/// Seconds without a delta after which the next one starts a new gesture, where the platform
/// reports no phases (a mouse wheel, most trackpads off macOS).
const WHEEL_IDLE: f64 = 0.1;
/// Seconds without a delta after which a gesture the platform began counts as over, should its
/// end never come (the window lost the pointer or focus mid-gesture).
const PHASED_IDLE: f64 = 1.0;
/// A gesture that begins within this many seconds of one ending is its momentum (AppKit
/// reports momentum as a phase of its own), so it keeps the axis.
const MOMENTUM_GAP: f64 = 0.05;

/// The wheel or trackpad gesture in progress, and who has it. Its phase carries when its last
/// delta came and, once the platform ended it, when that was: one fact, in one place.
#[derive(Debug, Default)]
pub(super) struct WheelLock {
    seq: Seq,
    /// The gesture deltas arriving now belong to: a new one whenever a gesture starts or the
    /// window loses the gesture.
    pub(super) gesture: Attempt,
    /// The gesture `owner` belongs to, behind `gesture` while a delta of an earlier gesture
    /// still waits in the queue for `on_wheel` handlers.
    pub(super) run_gesture: Attempt,
    pub(super) owner: WheelOwner,
    phase: WheelPhase,
}

/// Who has the gesture `WheelLock::run_gesture`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum WheelOwner {
    /// Nobody yet: its first delta picks.
    #[default]
    Unpicked,
    /// The `on_wheel` element that took the gesture's first delta and hears the rest.
    Handler(String),
    /// Scrolling vertically.
    Vertical,
    /// Scrolling the horizontal viewport the gesture began on, latched through the gesture
    /// and its momentum.
    Horizontal(NodeId),
}

impl WheelOwner {
    /// Horizontal (`true`) or vertical, once a delta picked an axis.
    fn horizontal(&self) -> Option<bool> {
        match self {
            WheelOwner::Vertical => Some(false),
            WheelOwner::Horizontal(_) => Some(true),
            WheelOwner::Unpicked | WheelOwner::Handler(_) => None,
        }
    }
}

/// Whether the platform reports the gesture's phases, and when it last moved or ended.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WheelPhase {
    /// No phases (a mouse wheel): a pause or a delta wholly on the other axis ends it. `at` is
    /// when its last delta came, `None` before the first.
    Unphased { at: Option<f64> },
    /// The platform began it at `at`: it holds the axis until it ends, however slowly it moves.
    Phased { at: f64 },
    /// The momentum of a phased gesture that ended, last moving at `at`. One that begins as
    /// momentum ends is a new touch (a finger caught the coasting content), not more momentum.
    Momentum { at: f64 },
    /// The platform ended it at `at`. `from` is the phase it had then: a gesture that started
    /// unphased still picks its axis afresh, and one that ended as momentum never begins its
    /// own momentum.
    Ended { at: f64, from: GestureKind },
}

/// The phase a gesture had when the platform ended it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GestureKind {
    Unphased,
    Phased,
    Momentum,
}

impl Default for WheelPhase {
    fn default() -> Self { WheelPhase::Unphased { at: None } }
}

impl WheelPhase {
    /// The gesture is the momentum of one that ended, or ended as momentum.
    fn momentum(self) -> bool {
        matches!(self, WheelPhase::Momentum { .. } | WheelPhase::Ended { from: GestureKind::Momentum, .. })
    }

    /// Whether the platform reports its phases: it has begun it, or it ended one that had.
    fn phased(self) -> bool {
        !matches!(self, WheelPhase::Unphased { .. } | WheelPhase::Ended { from: GestureKind::Unphased, .. })
    }

    /// When its last delta came.
    fn at(self) -> Option<f64> {
        match self {
            WheelPhase::Unphased { at } => at,
            WheelPhase::Phased { at } | WheelPhase::Momentum { at } | WheelPhase::Ended { at, .. } => Some(at),
        }
    }

    /// When the platform ended it, while that is what happened last.
    fn ended(self) -> Option<f64> {
        match self {
            WheelPhase::Ended { at, .. } => Some(at),
            _ => None,
        }
    }

    /// The same live phase, with its last delta at `now`.
    fn moving(self, now: f64) -> Self {
        match self {
            WheelPhase::Unphased { .. } => WheelPhase::Unphased { at: Some(now) },
            WheelPhase::Phased { .. } => WheelPhase::Phased { at: now },
            WheelPhase::Momentum { .. } => WheelPhase::Momentum { at: now },
            WheelPhase::Ended { .. } => self,
        }
    }
}

impl WheelLock {
    pub(super) fn sync_gesture(&mut self, gesture: Attempt) {
        if self.run_gesture != gesture {
            self.run_gesture = gesture;
            self.owner = WheelOwner::Unpicked;
        }
    }

    /// The gesture is the momentum of one that ended (`WheelEvent::momentum`).
    pub(super) fn momentum(&self) -> bool { self.phase.momentum() }

    /// The next gesture starts, unphased. Who has it starts afresh too, unless a delta of an
    /// earlier gesture still waits for `on_wheel` handlers (`queued`): `run_wheel` catches up
    /// once it has run.
    fn restart(&mut self, queued: bool) {
        self.gesture = self.seq.next();
        self.phase = WheelPhase::Unphased { at: None };
        if !queued {
            self.run_gesture = self.gesture;
            self.owner = WheelOwner::Unpicked;
        }
    }

    /// The axis the gesture's owner keeps for a delta (dx, dy): horizontal (`true`) or
    /// vertical, or `None` when it picks afresh. Without phases, a delta wholly on the other
    /// axis is another wheel turning (Shift let go, a tilt, a second device).
    fn keeps_axis(&self, dx: f32, dy: f32) -> Option<bool> {
        let h = self.owner.horizontal()?;
        (self.phase.phased() || if h { dx != 0.0 } else { dy != 0.0 }).then_some(h)
    }

    /// A delta picked afresh: the horizontal viewport under it (`across`), else vertical.
    /// Returns whether it scrolls horizontally.
    fn repick(&mut self, across: Option<NodeId>) -> bool {
        self.owner = across.map_or(WheelOwner::Vertical, WheelOwner::Horizontal);
        across.is_some()
    }

    /// A delta arrived at `now`, on (dx, dy) points: a gesture idle too long, ended, or wholly
    /// on the other axis (Shift let go, a tilt, a second wheel), is over, and this delta starts
    /// the next.
    fn delta(&mut self, now: f64, dx: f32, dy: f32, queued: bool) {
        let phased = self.phase.phased();
        let idle = if phased { PHASED_IDLE } else { WHEEL_IDLE };
        let flipped = !phased && self.run_gesture == self.gesture && self.owner.horizontal().is_some_and(|h| if h { dx == 0.0 } else { dy == 0.0 });
        let stale = self.phase.ended().is_some() || self.phase.at().is_none_or(|a| now - a > idle) || flipped;
        if stale { self.restart(queued); }
        self.phase = self.phase.moving(now);
    }

    /// The platform began a gesture at `now`: its first delta picks the axis afresh, unless it
    /// is the momentum of one that just ended.
    fn began(&mut self, now: f64, active: bool, queued: bool, any_queued: bool) {
        let momentum = !self.phase.momentum() && (active || queued) && self.phase.ended().is_some_and(|e| now - e <= MOMENTUM_GAP);
        if !momentum { self.restart(any_queued); }
        self.phase = if momentum { WheelPhase::Momentum { at: now } } else { WheelPhase::Phased { at: now } };
    }

    /// The platform ended the gesture at `now`, keeping the phase it had.
    fn ended_at(&mut self, now: f64) {
        let from = match self.phase {
            WheelPhase::Unphased { .. } => GestureKind::Unphased,
            WheelPhase::Phased { .. } => GestureKind::Phased,
            WheelPhase::Momentum { .. } => GestureKind::Momentum,
            WheelPhase::Ended { from, .. } => from,
        };
        self.phase = WheelPhase::Ended { at: now, from };
    }

    /// The window lost the pointer or focus: a gesture in progress won't report its end here.
    fn lost(&mut self, queued: bool) { self.restart(queued); }
}

/// A wheel event's delta in points, positive moving content right and down. With Shift, a delta
/// with no horizontal part turns horizontal, as browsers and Windows and Linux apps do for a
/// mouse wheel. macOS turns it itself (`shift` is false there), and a delta that already has a
/// horizontal part was turned by the system or comes from a trackpad.
pub(crate) fn wheel_delta(delta: winit::event::MouseScrollDelta, scale: f32, shift: bool) -> (f32, f32) {
    let (dx, dy) = match delta {
        winit::event::MouseScrollDelta::PixelDelta(p) => (p.x as f32 / scale, p.y as f32 / scale),
        winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
    };
    if shift && dx == 0.0 { (dy, 0.0) } else { (dx, dy) }
}

/// Whether Shift turns the wheel on this platform (macOS does it before the app sees the event).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) const SHIFT_TURNS_WHEEL: bool = !cfg!(target_os = "macos");

impl ScrollState {
    /// The offset, velocity and limit on one axis.
    pub(super) fn axis(&mut self, horizontal: bool) -> (&mut f64, f64) {
        if horizontal { (&mut self.x.offset, self.x.max) } else { (&mut self.offset, self.max) }
    }
}

impl<S: 'static> Runtime<S> {
    /// A wheel or trackpad delta in points on both axes (positive moves content right and
    /// down). The gesture's first delta picks the axis and the rest of the gesture keeps it:
    /// mostly horizontal scrolls the innermost [`El::scroll_x`] or [`El::scroll_xy`] viewport
    /// under (x, y), anything else the innermost vertical one as [`Self::wheel`] does. Call
    /// [`Self::wheel_began`] and [`Self::wheel_ended`] where the platform reports phases;
    /// without them a pause of 0.1 s ends a gesture. An [`El::on_wheel`] handler under the
    /// pointer hears it first; [`Self::wheel_event`] also passes modifiers and phases.
    pub fn wheel_xy(&mut self, x: f32, y: f32, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 { return; }
        self.wheel_event(crate::WheelEvent::new(dx, dy).at(x, y));
    }

    /// A delta arrived: a gesture idle too long, or ended and not followed by its momentum,
    /// is over, and this delta starts the next. Without phases, a delta wholly on the other
    /// axis (Shift let go, a tilt, a second wheel) also starts a new gesture.
    pub(super) fn wheel_delta_arrived(&mut self, dx: f32, dy: f32) {
        self.drop_scroll_holds();
        let now = self.now_clock();
        let queued = self.wheel_queued(None);
        self.wheel_lock.delta(now, dx, dy, queued);
    }

    /// Whether a wheel delta waits in the queue for `on_wheel` handlers: one of gesture `of`,
    /// or of any gesture.
    pub(super) fn wheel_queued(&self, of: Option<Attempt>) -> bool {
        self.queue.iter().any(|a| matches!(a, Action::Wheel(g, ..) if of.is_none_or(|of| *g == of)))
    }

    /// Scrolls by a delta no `on_wheel` handler took, on the axis the gesture locked to.
    pub(super) fn wheel_scroll_for(&mut self, gesture: Attempt, x: f32, y: f32, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 { return; }
        self.wheel_lock.sync_gesture(gesture);
        // Without phases, a delta wholly on the other axis is another wheel turning (Shift let
        // go, a tilt, a second device), so it picks afresh rather than being dropped.
        let horizontal = match self.wheel_lock.keeps_axis(dx, dy) {
            Some(h) => h,
            None => {
                let scroller = (dx.abs() > dy.abs()).then(|| self.scroll_at_axis(x, y, true)).flatten();
                self.wheel_lock.repick(scroller)
            }
        };
        if horizontal { if dx != 0.0 { self.wheel_x(x, y, dx); } } else if dy != 0.0 { self.wheel(x, y, dy); }
    }

    /// The platform began a trackpad gesture (winit's `TouchPhase::Started` on a wheel event):
    /// its first delta picks the axis afresh, unless it is the momentum of one that just ended.
    pub fn wheel_began(&mut self) {
        let now = self.now_clock();
        let gesture = self.wheel_lock.gesture;
        // A delta queued for `on_wheel` handlers hasn't picked who has the gesture yet.
        let queued = self.wheel_queued(Some(gesture));
        let any_queued = self.wheel_queued(None);
        let active = self.wheel_lock.run_gesture == gesture && self.wheel_lock.owner != WheelOwner::Unpicked;
        self.wheel_lock.began(now, active, queued, any_queued);
    }

    /// The platform ended a trackpad gesture (`TouchPhase::Ended` or `Cancelled`).
    pub fn wheel_ended(&mut self) {
        let now = self.now_clock();
        self.wheel_lock.ended_at(now);
    }

    /// The window lost the pointer or focus: a gesture in progress won't report its end here.
    pub(super) fn wheel_lost(&mut self) {
        let queued = self.wheel_queued(None);
        self.wheel_lock.lost(queued);
    }

    /// Scrolls the horizontal viewport latched for the gesture (or the innermost one at (x, y))
    /// by `dx` points, 1:1.
    fn wheel_x(&mut self, x: f32, y: f32, dx: f32) {
        self.release_hover_hold();
        self.dismiss_help();
        let latched = match self.wheel_lock.owner { WheelOwner::Horizontal(id) => Some(id), _ => None }.filter(|id| {
            self.node_index.get(id).is_some_and(|&i| {
                self.nodes[i].scroll.is_some() && self.nodes[i].scroll_axes.x() && !self.subtree_exiting(i) && self.allowed(i)
            })
        });
        let Some(id) = latched.or_else(|| self.scroll_at_axis(x, y, true)) else { return };
        let Some(st) = self.scrolls.get_mut(&id) else { return };
        let before = st.x.offset;
        st.x.offset = (st.x.offset + st.x.toward(f64::from(dx))).clamp(0.0, st.x.max);
        st.x.v = 0.0;
        st.x.glide = None;
        st.moved();
        log_event!(self, "wheel-x {dx:.1}");
        if st.x.offset != before { self.scroll_cancels_press(id); }
    }

    /// Releases a scroll with velocity (vx, vy) in points per second (positive moves content
    /// right and down), which the runtime decays, on the axis [`Self::wheel_xy`] would pick
    /// for it. [`Self::fling`] is the vertical one.
    pub fn fling_xy(&mut self, x: f32, y: f32, vx: f32, vy: f32) {
        if vx.abs() <= vy.abs() || self.scroll_at_axis(x, y, true).is_none() {
            self.fling(x, y, vy);
            return;
        }
        self.release_hover_hold();
        self.dismiss_help();
        let Some(id) = self.scroll_at_axis(x, y, true) else { return };
        if let Some(st) = self.scrolls.get_mut(&id) {
            st.x.v = st.x.toward(f64::from(vx)) as f32;
            st.x.glide = None;
            log_event!(self, "fling-x {vx:.0}");
            if vx != 0.0 { self.scroll_cancels_press(id); }
        }
    }

    /// How far horizontal viewport `i`'s content reaches past its end edge (its right, or its
    /// left right to left), negative while it fits.
    fn scroll_overflow_x(&self, i: usize) -> f64 {
        let view = self.nodes[i].target;
        let reach = |c: usize| match self.nodes[i].flow {
            crate::el::Flow::Ltr => self.nodes[c].target[2] - view[0],
            crate::el::Flow::Rtl => view[2] - self.nodes[c].target[0],
        };
        let content_w = self.scroll_children(i).map(|c| f64::from(reach(c))).reduce(f64::max).unwrap_or(0.0);
        content_w - f64::from(view[2] - view[0])
    }

    /// Steps viewport `i`'s horizontal limit and momentum, as `step_scroll` does the vertical.
    pub(super) fn step_scroll_x(&mut self, i: usize, dt: f32) -> bool {
        let id = self.nodes[i].id;
        let overflow = self.scroll_overflow_x(i);
        if let Some(active) = self.step_touch_scroll(id, overflow, dt, true) { return active; }
        let Some(st) = self.scrolls.get_mut(&id) else { return false };
        st.x.max = overflow.max(0.0);
        let mut moving = false;
        if st.x.glide.is_some() {
            moving = scroll_offset::glide_axis(st.glide_spring(), &mut st.x.offset, &mut st.x.glide, st.x.aim.min(st.x.max), dt);
        } else if st.x.v.abs() > 1.0 {
            let decay = (-SCROLL_DECAY * dt).exp();
            st.x.offset += f64::from(st.x.v * (1.0 - decay) / SCROLL_DECAY);
            st.x.v *= decay;
            if st.x.offset <= 0.0 || st.x.offset >= st.x.max { st.x.v = 0.0; }
            moving = true;
        } else {
            st.x.v = 0.0;
        }
        st.x.offset = st.x.offset.clamp(0.0, st.x.max);
        moving
    }

    /// `Tx::scroll_into_view` on x: scrolls the nearest horizontal viewport around row `key`
    /// just far enough to show it, leaving its `El::scroll_margin`.
    pub(super) fn reveal_row_x(&mut self, scope: Option<&str>, key: &str) {
        let target = self.nodes.iter().position(|n| {
            n.named(key) && n.parent.is_none_or(|p| super::in_scope(&self.nodes[p].path, scope))
        }).map(|i| (i, self.nodes[i].target[0], self.nodes[i].local.w)).or_else(|| {
            let (i, [x0, _, x1, _]) = self.highlight_target_in(&self.nodes, scope, key)?;
            Some((i, x0, (x1 - x0).max(0.0)))
        });
        let Some((i, target_x, target_w)) = target else { return };
        let Some(v) = std::iter::successors(self.nodes[i].parent, |&p| self.nodes[p].parent)
            .find(|&p| self.nodes[p].scroll.is_some() && self.nodes[p].scroll_axes.x()) else { return };
        let view = &self.nodes[v];
        // From the viewport's start edge: its right, right to left.
        let start = match view.flow {
            crate::el::Flow::Ltr => target_x - view.target[0],
            crate::el::Flow::Rtl => view.target[2] - (target_x + target_w),
        };
        let (left, width, viewport) = (f64::from(start), f64::from(target_w), f64::from(view.local.w));
        let (margin, id) = (f64::from(view.scroll_edges.margin), view.id);
        // A vertical reveal may have just mounted rows (virtual ones) that widened the content.
        let max = self.scroll_overflow_x(v).max(0.0);
        let Some(st) = self.scrolls.get_mut(&id) else { return };
        st.x.max = max;
        let offset = scroll_edge::reveal(st.x.offset, left, width, viewport, margin).clamp(0.0, max);
        if offset == st.x.offset { return; }
        st.x.offset = offset;
        st.x.v = 0.0;
        st.x.glide = None;
        st.moved();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta::{LineDelta, PixelDelta};

    #[test]
    fn a_gesture_begun_as_one_ends_is_its_momentum_once() {
        use crate::{GesturePhase, WheelEvent};
        fn view(_: &(), _: &mut Cx<()>) -> El {
            crate::col().w_full().h_full().child(crate::col().w_full().h(300.0)
                .scroll(crate::ScrollSpec { stick_bottom: false }).child(crate::el().w_full().h(2000.0)))
        }
        let mut rt = Runtime::new((), view, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        let wheel = |rt: &mut Runtime<()>, phase| rt.wheel_event(WheelEvent::new(0.0, -10.0).at(100.0, 100.0).phase(phase));
        wheel(&mut rt, GesturePhase::Began);
        let first = rt.wheel_lock.gesture;
        assert_eq!((rt.wheel_lock.owner.clone(), rt.wheel_lock.momentum()), (WheelOwner::Vertical, false));
        wheel(&mut rt, GesturePhase::Ended);
        wheel(&mut rt, GesturePhase::Began);
        assert!(rt.wheel_lock.momentum(), "began right as one ended: its momentum");
        assert_eq!((rt.wheel_lock.gesture, rt.wheel_lock.owner.clone()), (first, WheelOwner::Vertical), "keeping the gesture");
        wheel(&mut rt, GesturePhase::Ended);
        wheel(&mut rt, GesturePhase::Began);
        assert!(!rt.wheel_lock.momentum(), "a finger caught the momentum: a new gesture");
        let second = rt.wheel_lock.gesture;
        assert_ne!(second, first);

        rt.wheel_lost();
        assert_ne!(rt.wheel_lock.gesture, second);
        assert_eq!((rt.wheel_lock.owner.clone(), rt.wheel_lock.momentum()), (WheelOwner::Unpicked, false));
        rt.wheel_xy(100.0, 100.0, 0.0, -10.0);
        let wheel_gesture = rt.wheel_lock.gesture;
        rt.wheel_xy(100.0, 100.0, -10.0, 0.0);
        assert_ne!(rt.wheel_lock.gesture, wheel_gesture, "without phases, a delta wholly on the other axis starts another");
    }

    #[test]
    fn the_phase_carries_when_the_gesture_last_moved_and_when_it_ended() {
        let mut lock = WheelLock::default();
        lock.delta(1.0, 0.0, -10.0, false);
        assert_eq!(lock.phase, WheelPhase::Unphased { at: Some(1.0) }, "a delta starts it, timed");
        lock.delta(1.05, 0.0, -10.0, false);
        assert_eq!(lock.phase, WheelPhase::Unphased { at: Some(1.05) }, "a pause under the idle keeps the gesture");
        let gesture = lock.gesture;
        lock.delta(1.05 + WHEEL_IDLE + 0.01, 0.0, -10.0, false);
        assert_ne!(lock.gesture, gesture, "a longer pause starts another");
        assert_eq!(lock.phase, WheelPhase::Unphased { at: Some(1.05 + WHEEL_IDLE + 0.01) }, "timed from the new delta");
        lock.owner = WheelOwner::Vertical;
        lock.ended_at(2.0);
        assert_eq!(lock.phase, WheelPhase::Ended { at: 2.0, from: GestureKind::Unphased }, "an unphased end keeps its phase");
        let ended = lock.gesture;
        lock.delta(2.001, 0.0, -10.0, false);
        assert_ne!(lock.gesture, ended, "a delta after the end starts another gesture at once");
        lock.lost(false);
        assert_eq!(lock.phase, WheelPhase::Unphased { at: None }, "the loss forgets when it last moved");
    }

    #[test]
    fn an_ended_gestures_momentum_has_the_gap_its_end_time_measures() {
        let mut lock = WheelLock::default();
        lock.delta(0.0, 0.0, -10.0, false);
        lock.owner = WheelOwner::Vertical;
        lock.ended_at(1.0);
        let gesture = lock.gesture;
        let soon = 1.0 + MOMENTUM_GAP * 0.5;
        lock.began(soon, true, false, false);
        assert_eq!(lock.phase, WheelPhase::Momentum { at: soon }, "within the gap: its momentum");
        assert_eq!(lock.gesture, gesture, "keeping the gesture");
        assert!(lock.momentum());
        lock.ended_at(2.0);
        assert_eq!(lock.phase, WheelPhase::Ended { at: 2.0, from: GestureKind::Momentum });
        lock.began(2.0 + MOMENTUM_GAP * 0.5, true, false, false);
        assert!(matches!(lock.phase, WheelPhase::Phased { .. }), "one that ended as momentum is a new touch");
        assert!(!lock.momentum());

        let mut lock = WheelLock::default();
        lock.delta(0.0, 0.0, -10.0, false);
        lock.owner = WheelOwner::Vertical;
        lock.ended_at(1.0);
        let gesture = lock.gesture;
        lock.began(1.0 + MOMENTUM_GAP + 0.01, true, false, false);
        assert_ne!(lock.gesture, gesture, "after the gap it is its own gesture");
    }

    /// The queued deltas of an unphased gesture that ended before the frame drained them still
    /// pick each axis as they arrive: the end doesn't make the gesture phased.
    #[test]
    fn a_queued_cross_axis_delta_after_an_unphased_end_picks_its_axis() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            crate::col().size(200.0, 200.0)
                .child(crate::col().key("xy").size(200.0, 200.0).scroll_xy()
                    .on_wheel(|_: &mut (), _: crate::WheelEvent, _: &mut Tx| false)
                    .child(crate::el().key("content").w(1000.0).h(1000.0)))
        }
        use crate::WheelEvent;
        let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.wheel_event(WheelEvent::new(-10.0, 0.0).at(100.0, 100.0));
        rt.wheel_event(WheelEvent::new(0.0, -20.0).at(100.0, 100.0));
        rt.wheel_ended();
        rt.frame(1.0 / 60.0);
        let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("xy")).unwrap().id;
        assert_eq!((rt.scrolls[&id].x.offset, rt.scrolls[&id].offset), (10.0, 20.0), "each queued delta scrolls the axis it picks");
    }

    /// origin/dev's wheel lock, transcribed from this file: when its last delta came (`at`),
    /// whether the gesture was phased and momentum, and when the platform ended it (`ended`),
    /// beside the owner, in place of the phase that carries them.
    #[derive(Default)]
    struct DevLock {
        gesture: u32,
        run_gesture: u32,
        owner: DevOwner,
        at: Option<f64>,
        phased: bool,
        momentum: bool,
        ended: Option<f64>,
    }

    #[derive(Clone, Copy, Default, PartialEq, Debug)]
    enum DevOwner {
        #[default]
        Unpicked,
        Vertical,
        Horizontal,
    }

    impl DevLock {
        fn restart(&mut self, queued: bool) {
            self.gesture += 1;
            self.phased = false;
            self.momentum = false;
            self.ended = None;
            if !queued {
                self.run_gesture = self.gesture;
                self.owner = DevOwner::Unpicked;
            }
        }

        fn delta(&mut self, now: f64, dx: f32, dy: f32, queued: bool) {
            let idle = if self.phased { PHASED_IDLE } else { WHEEL_IDLE };
            let flipped = !self.phased && self.run_gesture == self.gesture && match self.owner {
                DevOwner::Horizontal => dx == 0.0,
                DevOwner::Vertical => dy == 0.0,
                DevOwner::Unpicked => false,
            };
            let stale = self.ended.is_some() || self.at.is_none_or(|a| now - a > idle) || flipped;
            if stale { self.restart(queued); }
            self.at = Some(now);
        }

        fn began(&mut self, now: f64, active: bool, queued: bool, any_queued: bool) {
            let momentum = !self.momentum && (active || queued) && self.ended.is_some_and(|e| now - e <= MOMENTUM_GAP);
            if !momentum { self.restart(any_queued); }
            self.phased = true;
            self.momentum = momentum;
            self.ended = None;
            self.at = Some(now);
        }

        fn lost(&mut self, queued: bool) {
            self.restart(queued);
            self.at = None;
        }

        fn horizontal(&self) -> Option<bool> {
            match self.owner {
                DevOwner::Unpicked => None,
                DevOwner::Vertical => Some(false),
                DevOwner::Horizontal => Some(true),
            }
        }

        fn scroll(&mut self, dx: f32, dy: f32, across: bool) -> bool {
            match self.horizontal() {
                Some(h) if self.phased || if h { dx != 0.0 } else { dy != 0.0 } => h,
                _ => {
                    self.owner = if across { DevOwner::Horizontal } else { DevOwner::Vertical };
                    across
                }
            }
        }
    }

    fn owner_kind(owner: &WheelOwner) -> Option<DevOwner> {
        match owner {
            WheelOwner::Unpicked => Some(DevOwner::Unpicked),
            WheelOwner::Vertical => Some(DevOwner::Vertical),
            WheelOwner::Horizontal(_) => Some(DevOwner::Horizontal),
            // A handler owns the gesture only after `run_wheel` took a delta, which this walk
            // never drives.
            WheelOwner::Handler(_) => None,
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum WAct {
        Delta { dx: f32, dy: f32, gap: f64, queued: bool },
        Began { gap: f64, queued: bool },
        Ended { gap: f64 },
        Lost { queued: bool },
        Scroll { dx: f32, dy: f32, across: bool },
    }

    /// Drives every sequence through the lock and through dev's fields, one event at a time,
    /// comparing the phase's facts, the owner and the axis each delta scrolls after every event.
    fn wheel_walk(acts: &[WAct]) {
        let (mut lock, mut dev, mut now) = (WheelLock::default(), DevLock::default(), 0.0);
        for (i, act) in acts.iter().enumerate() {
            let before = lock.gesture;
            let dev_before = dev.gesture;
            match act {
                WAct::Delta { dx, dy, gap, queued } => {
                    now += gap;
                    lock.delta(now, *dx, *dy, *queued);
                    dev.delta(now, *dx, *dy, *queued);
                }
                WAct::Began { gap, queued } => {
                    now += gap;
                    let active = lock.run_gesture == lock.gesture && lock.owner != WheelOwner::Unpicked;
                    lock.began(now, active, *queued, *queued);
                    let dev_active = dev.run_gesture == dev.gesture && dev.owner != DevOwner::Unpicked;
                    dev.began(now, dev_active, *queued, *queued);
                }
                WAct::Ended { gap } => {
                    now += gap;
                    lock.ended_at(now);
                    dev.ended = Some(now);
                }
                WAct::Lost { queued } => {
                    lock.lost(*queued);
                    dev.lost(*queued);
                }
                WAct::Scroll { dx, dy, across } => {
                    let h = match lock.keeps_axis(*dx, *dy) {
                        Some(h) => h,
                        None => lock.repick(across.then_some(NodeId(1))),
                    };
                    assert_eq!(h, dev.scroll(*dx, *dy, *across), "{acts:?}, event {i}: axis");
                }
            }
            let at = format!("{acts:?}, event {i} ({act:?})");
            assert_eq!((lock.gesture != before), (dev.gesture != dev_before), "gesture: {at}");
            assert_eq!(lock.phase.phased(), dev.phased, "phased: {at}");
            assert_eq!(lock.phase.momentum(), dev.momentum, "momentum: {at}");
            // An ended phase keeps its end time where dev kept a separate `ended`: the last
            // delta's time is no longer read, so only a live phase's is compared.
            if dev.ended.is_none() {
                assert_eq!(lock.phase.at(), dev.at, "at: {at}");
            }
            assert_eq!(lock.phase.ended(), dev.ended, "ended: {at}");
            assert_eq!(owner_kind(&lock.owner), Some(dev.owner), "owner: {at}");
        }
    }

    /// Every order of deltas, platform phases, losses and the deltas that later scroll, up to
    /// five long, including an unphased gesture that ends with a cross-axis delta queued.
    #[test]
    fn the_phase_matches_devs_fields_over_every_order() {
        let acts = [
            WAct::Delta { dx: -10.0, dy: 0.0, gap: 0.0, queued: false },
            WAct::Delta { dx: -10.0, dy: 0.0, gap: 0.0, queued: true },
            WAct::Delta { dx: 0.0, dy: -20.0, gap: 0.0, queued: false },
            WAct::Delta { dx: 0.0, dy: -20.0, gap: 0.0, queued: true },
            WAct::Delta { dx: -10.0, dy: 0.0, gap: 0.2, queued: false },
            WAct::Ended { gap: 0.0 },
            WAct::Ended { gap: 0.2 },
            WAct::Began { gap: 0.02, queued: false },
            WAct::Began { gap: 0.02, queued: true },
            WAct::Lost { queued: false },
            WAct::Lost { queued: true },
            WAct::Scroll { dx: -10.0, dy: 0.0, across: true },
            WAct::Scroll { dx: 0.0, dy: -20.0, across: true },
            WAct::Scroll { dx: 0.0, dy: -20.0, across: false },
        ];
        for len in 0..=5 {
            wheel_walk_all(&acts, len, &mut Vec::new());
        }
    }

    fn wheel_walk_all(acts: &[WAct], len: usize, prefix: &mut Vec<WAct>) {
        if prefix.len() == len {
            wheel_walk(prefix);
            return;
        }
        for &act in acts {
            prefix.push(act);
            wheel_walk_all(acts, len, prefix);
            prefix.pop();
        }
    }

    #[test]
    fn shift_turns_only_a_purely_vertical_wheel() {
        assert_eq!(wheel_delta(LineDelta(0.0, -1.0), 2.0, false), (0.0, -40.0));
        assert_eq!(wheel_delta(LineDelta(0.0, -1.0), 2.0, true), (-40.0, 0.0), "Shift + mouse wheel");
        assert_eq!(wheel_delta(LineDelta(1.0, 0.0), 2.0, false), (40.0, 0.0), "a horizontal wheel (WM_MOUSEHWHEEL, X11 buttons 6/7)");
        let trackpad = PixelDelta(PhysicalPosition::new(-8.0, 20.0));
        assert_eq!(wheel_delta(trackpad, 2.0, true), (-4.0, 10.0), "a delta with a horizontal part stays as it is");
        assert!(SHIFT_TURNS_WHEEL != cfg!(target_os = "macos"));
    }
}
