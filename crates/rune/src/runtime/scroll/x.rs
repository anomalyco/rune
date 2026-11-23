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
/// A run of vertical deltas stays with the viewport it began on while each comes within this
/// many seconds of the last and the pointer within this many points of where it began, as
/// Chromium latches a wheel's scroll (`content/browser/renderer_host/input/
/// mouse_wheel_phase_handler.cc`: `kDefaultMouseWheelLatchingTransaction`, `kWheelLatchingSlopRegion`).
const RUN_IDLE: f64 = 0.5;
const RUN_SLOP: f32 = 10.0;

/// The wheel or trackpad gesture in progress, and who has it. Its phase carries when its last
/// delta came and, once the platform ended it, when that was: one fact, in one place.
#[derive(Debug, Default)]
pub(in crate::runtime) struct WheelLock {
    seq: Seq,
    /// The gesture deltas arriving now belong to: a new one whenever a gesture starts or the
    /// window loses the gesture.
    pub(in crate::runtime) gesture: Attempt,
    /// Who has the gesture's deltas: the arriving gesture's owner, or the previous gesture's
    /// while deltas of it still wait in the queue for `on_wheel` handlers.
    run: WheelRun,
    phase: WheelPhase,
    /// The vertical viewport the current run of deltas scrolls, once one has.
    vertical: Option<VerticalRun>,
    /// Whether the current run of deltas is a notched wheel's.
    kind: DeltaKind,
}

/// What a run of wheel deltas is while its deltas keep coming (within `RUN_IDLE`): a notched
/// wheel's whole lines, or a precision touchpad's or a free-spinning wheel's fractions. A run is
/// notched while every delta is whole, and precise from its first fraction to its end: a fast
/// swipe's delta that happens to be a whole line stays precise, rather than starting an ease the
/// next fraction throws away.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum DeltaKind {
    #[default]
    Idle,
    Notched { at: f64 },
    Precise { at: f64 },
}

impl DeltaKind {
    /// A delta at `now`, whole lines (`whole`) or not: what the run it belongs to is.
    fn step(&mut self, now: f64, whole: bool) -> bool {
        let notched = match *self {
            DeltaKind::Notched { at } | DeltaKind::Precise { at } if now - at > RUN_IDLE => whole,
            // A fraction ends a run of whole lines' easing: a touchpad swipe whose first delta was
            // exactly a line is precise from its first fraction on.
            DeltaKind::Notched { .. } => whole,
            DeltaKind::Precise { .. } => false,
            DeltaKind::Idle => whole,
        };
        *self = if notched { DeltaKind::Notched { at: now } } else { DeltaKind::Precise { at: now } };
        notched
    }
}

/// A run of vertical deltas and the viewport it keeps to: cc scrolls only the latched scroller,
/// so one that reaches its end mid-run takes the rest of the run, and the one around it moves
/// from the next run on. `down` is its first delta's direction; `moved`, whether that delta
/// moved the viewport: only a run whose first delta moved nothing ends when the wheel turns
/// (`ShouldBreakLatchingDueToDirectionChange`).
#[derive(Clone, Copy, Debug)]
struct VerticalRun {
    id: NodeId,
    at: f64,
    x: f32,
    y: f32,
    down: bool,
    moved: bool,
}

/// Who has the deltas still to run, keyed by the gesture each belongs to: the gesture arriving
/// now, and, until they run, the older gesture whose queued deltas already had an owner.
#[derive(Clone, Debug, PartialEq)]
enum WheelRun {
    /// The arriving gesture has `owner`: its deltas go to it, and its first delta picks it
    /// afresh.
    Arriving(WheelOwner),
    /// Deltas of `gesture` still wait in the queue: `owner` takes them. The gesture arriving
    /// now has `arriving` of its own, which it picks when one of its deltas scrolls before
    /// those queued ones run.
    Waiting { gesture: Attempt, owner: WheelOwner, arriving: WheelOwner },
}

impl Default for WheelRun {
    fn default() -> Self { WheelRun::Arriving(WheelOwner::default()) }
}

impl WheelRun {
    /// Who has the gesture arriving now.
    fn arriving(&self) -> &WheelOwner {
        match self {
            WheelRun::Arriving(owner) => owner,
            WheelRun::Waiting { arriving, .. } => arriving,
        }
    }

    fn arriving_mut(&mut self) -> &mut WheelOwner {
        match self {
            WheelRun::Arriving(owner) => owner,
            WheelRun::Waiting { arriving, .. } => arriving,
        }
    }

    /// Who has `gesture`'s queued deltas, while the lock holds them.
    fn waiting(&self, gesture: Attempt) -> Option<&WheelOwner> {
        match self {
            WheelRun::Waiting { gesture: g, owner, .. } if *g == gesture => Some(owner),
            _ => None,
        }
    }

    fn waiting_mut(&mut self, gesture: Attempt) -> Option<&mut WheelOwner> {
        match self {
            WheelRun::Waiting { gesture: g, owner, .. } if *g == gesture => Some(owner),
            _ => None,
        }
    }
}

/// Who has the gesture `WheelLock` runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::runtime) enum WheelOwner {
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
    /// Whether a delta at `now` that is whole lines (`whole`) is a notch: the run it belongs to decides.
    pub(in crate::runtime) fn notched(&mut self, now: f64, whole: bool) -> bool { self.kind.step(now, whole) }

    /// Who has the gesture arriving now: deltas of older gestures don't reach it.
    pub(in crate::runtime) fn owner(&self) -> &WheelOwner { self.run.arriving() }

    /// Who has `gesture`'s deltas: the arriving gesture's own, or an older gesture's while its
    /// queued deltas still run.
    pub(in crate::runtime) fn owner_of(&self, gesture: Attempt) -> Option<&WheelOwner> {
        if gesture == self.gesture { Some(self.run.arriving()) } else { self.run.waiting(gesture) }
    }

    /// Who has `gesture`'s deltas becomes `owner`, giving a gesture the lock doesn't hold a
    /// fresh start of its own beside the arriving one's.
    pub(in crate::runtime) fn set_owner(&mut self, gesture: Attempt, owner: WheelOwner) {
        if gesture == self.gesture {
            *self.run.arriving_mut() = owner;
        } else {
            self.sync_gesture(gesture);
            if let Some(slot) = self.run.waiting_mut(gesture) { *slot = owner; }
        }
    }

    /// Whether the platform reports the phases of the gesture the lock runs.
    pub(in crate::runtime) fn phased(&self) -> bool { self.phase.phased() }

    /// A delta of `gesture` runs: an owner the lock already holds for that gesture takes it, and
    /// any other queued gesture starts afresh, beside the arriving gesture's own owner.
    pub(in crate::runtime) fn sync_gesture(&mut self, gesture: Attempt) {
        if gesture == self.gesture || self.run.waiting(gesture).is_some() { return; }
        let arriving = std::mem::take(self.run.arriving_mut());
        self.run = WheelRun::Waiting { gesture, owner: WheelOwner::Unpicked, arriving };
    }

    /// The gesture is the momentum of one that ended (`WheelEvent::momentum`).
    pub(in crate::runtime) fn momentum(&self) -> bool { self.phase.momentum() }

    /// The next gesture starts, unphased. Only an owner whose gesture still has a delta waiting
    /// for `on_wheel` handlers keeps its place: `pending` is the gesture the oldest queued delta
    /// belongs to, so one that has run its last delta gives its owner up.
    fn restart(&mut self, pending: Option<Attempt>) {
        let arriving = self.gesture;
        let mut run = std::mem::take(&mut self.run);
        self.gesture = self.seq.next();
        self.phase = WheelPhase::Unphased { at: None };
        self.run = match pending {
            None => WheelRun::Arriving(WheelOwner::Unpicked),
            Some(next) if next == arriving => {
                WheelRun::Waiting { gesture: next, owner: std::mem::take(run.arriving_mut()), arriving: WheelOwner::Unpicked }
            }
            Some(next) => WheelRun::Waiting {
                gesture: next,
                owner: run.waiting_mut(next).map(std::mem::take).unwrap_or_default(),
                arriving: WheelOwner::Unpicked,
            },
        };
    }

    /// The axis `gesture`'s owner keeps for a delta (dx, dy): horizontal (`true`) or vertical, or
    /// `None` when it picks afresh. `phased` is the delta's own gesture's policy: without phases,
    /// a delta wholly on the other axis is another wheel turning (Shift let go, a tilt, a second
    /// device).
    fn keeps_axis(&self, gesture: Attempt, dx: f32, dy: f32, phased: bool) -> Option<bool> {
        let h = self.owner_of(gesture)?.horizontal()?;
        (phased || if h { dx != 0.0 } else { dy != 0.0 }).then_some(h)
    }

    /// A delta picked afresh: the horizontal viewport under it (`across`), else vertical.
    /// Returns whether it scrolls horizontally.
    fn repick(&mut self, gesture: Attempt, across: Option<NodeId>) -> bool {
        self.set_owner(gesture, across.map_or(WheelOwner::Vertical, WheelOwner::Horizontal));
        across.is_some()
    }

    /// A delta arrived at `now`, on (dx, dy) points: a gesture idle too long, ended, or wholly
    /// on the other axis (Shift let go, a tilt, a second wheel), is over, and this delta starts
    /// the next.
    fn delta(&mut self, now: f64, dx: f32, dy: f32, pending: Option<Attempt>) {
        let phased = self.phase.phased();
        let idle = if phased { PHASED_IDLE } else { WHEEL_IDLE };
        let flipped = !phased && self.run.arriving().horizontal().is_some_and(|h| if h { dx == 0.0 } else { dy == 0.0 });
        let stale = self.phase.ended().is_some() || self.phase.at().is_none_or(|a| now - a > idle) || flipped;
        if stale { self.restart(pending); }
        self.phase = self.phase.moving(now);
    }

    /// The viewport a vertical delta `dy` at (x, y) at `now` keeps to, when it continues the run.
    pub(in crate::runtime) fn vertical_run(&self, now: f64, x: f32, y: f32, dy: f32) -> Option<NodeId> {
        let run = self.vertical?;
        let continues = now - run.at <= RUN_IDLE && (x - run.x).hypot(y - run.y) < RUN_SLOP && (run.moved || (dy < 0.0) == run.down);
        continues.then_some(run.id)
    }

    /// A vertical delta `dy` at (x, y) at `now` scrolled viewport `id` (`moved`: it moved it);
    /// `continued`, as part of the run, or else starting one.
    pub(in crate::runtime) fn ran_vertical(&mut self, id: NodeId, now: f64, x: f32, y: f32, dy: f32, moved: bool, continued: bool) {
        match &mut self.vertical {
            Some(run) if continued => run.at = now,
            _ => self.vertical = Some(VerticalRun { id, at: now, x, y, down: dy < 0.0, moved }),
        }
    }

    /// No viewport takes the run: the next delta starts afresh.
    pub(in crate::runtime) fn end_vertical(&mut self) { self.vertical = None; }

    /// The platform began a gesture at `now`: its first delta picks the axis afresh, unless it
    /// is the momentum of one that just ended.
    fn began(&mut self, now: f64, active: bool, queued: bool, pending: Option<Attempt>) {
        let momentum = !self.phase.momentum() && (active || queued) && self.phase.ended().is_some_and(|e| now - e <= MOMENTUM_GAP);
        // A new touch is a new run; its momentum keeps the one it has.
        if !momentum { self.restart(pending); self.vertical = None; }
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
    fn lost(&mut self, pending: Option<Attempt>) { self.restart(pending); self.vertical = None; }
}

/// A wheel event's delta in points, positive moving content right and down. With Shift, a delta
/// with no horizontal part turns horizontal, as browsers and Windows and Linux apps do for a
/// mouse wheel. macOS turns it itself (`shift` is false there), and a delta that already has a
/// horizontal part was turned by the system or comes from a trackpad.
pub(crate) fn wheel_delta(delta: winit::event::MouseScrollDelta, scale: f32, shift: bool, line_points: f32) -> (f32, f32) {
    let (dx, dy) = match delta {
        winit::event::MouseScrollDelta::PixelDelta(p) => (p.x as f32 / scale, p.y as f32 / scale),
        winit::event::MouseScrollDelta::LineDelta(x, y) => (x * line_points, y * line_points),
    };
    if shift && dx == 0.0 { (dy, 0.0) } else { (dx, dy) }
}

// Chromium 152: ui/events/event.cc (kWheelDelta on Linux), ui/events/x/events_x_utils.cc
// (XI2 scroll increments), ui/events/cocoa/cocoa_event_utils.h (kScrollbarPixelsPerCocoaTick)
// and cc/input/scroll_utils.h (kPixelsPerLineStep for browser line units).
#[cfg(any(not(target_os = "windows"), test))]
pub(crate) const LINE_POINTS: f32 = if cfg!(target_os = "linux") { 120.0 } else { 40.0 };

#[cfg(target_os = "windows")]
impl RuntimeCore {
    #[allow(unsafe_code)]
    pub(crate) fn windows_wheel_points(&self, delta: winit::event::MouseScrollDelta, shift: bool) -> f32 {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES};
        let winit::event::MouseScrollDelta::LineDelta(x, y) = delta else { return 0.0 };
        let horizontal = x != 0.0;
        let (setting, default) = if horizontal { (SPI_GETWHEELSCROLLCHARS, 1u32) } else { (SPI_GETWHEELSCROLLLINES, 3u32) };
        let mut count = default;
        // SAFETY: both SPI queries write one UINT. Read per event so Settings changes apply immediately.
        if unsafe { SystemParametersInfoW(setting, 0, (&raw mut count).cast(), 0) } == 0 { count = default; }
        // Chromium 152: ui/events/blink/web_input_event_builders_win.cc, WebMouseWheelEventBuilder.
        const POINTS_PER_SCROLL_LINE: f32 = 100.0 / 3.0;
        if !horizontal && count == u32::MAX {
            // WHEEL_PAGESCROLL: cc/input/input_handler.cc, ResolveScrollGranularityToPixels,
            // uses the smaller of the scroller and the window. Clamp after applying the ticks.
            const PAGE_FRACTION: f32 = 0.875;
            let viewport = self.pointer().and_then(|(x, y)| self.scroll_at_axis(x, y, shift))
                .and_then(|id| self.tree.node_index.get(&id)).map(|&i| self.tree.nodes[i].local);
            let (window, scroller) = if shift { (self.viewport.0, viewport.map(|r| r.w)) } else { (self.viewport.1, viewport.map(|r| r.h)) };
            let page = scroller.unwrap_or(window).min(window) * self.zoom() * PAGE_FRACTION;
            if y == 0.0 { 0.0 } else { page.max(1.0 / y.abs()) }
        } else { count as f32 * POINTS_PER_SCROLL_LINE }
    }
}

/// Whether Shift turns the wheel on this platform (macOS does it before the app sees the event).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) const SHIFT_TURNS_WHEEL: bool = !cfg!(target_os = "macos");

impl ScrollState {
    /// The offset, velocity and limit on one axis.
    pub(in crate::runtime) fn axis(&mut self, horizontal: bool) -> (&mut f64, f64) {
        if horizontal { (&mut self.x.offset, self.x.max) } else { (&mut self.offset, self.max) }
    }
}

impl requests::ScrollTables {
    /// A delta arrived: a gesture idle too long, or ended and not followed by its momentum,
    /// is over, and this delta starts the next. Without phases, a delta wholly on the other
    /// axis (Shift let go, a tilt, a second wheel) also starts a new gesture.
    #[inline]
    pub(in crate::runtime) fn wheel_delta(&mut self, now: f64, dx: f32, dy: f32, pending: Option<Attempt>) {
        self.drop_holds();
        self.wheel_lock.delta(now, dx, dy, pending);
    }

    #[inline]
    pub(super) fn wheel_began_at(&mut self, now: f64, queued: bool, pending: Option<Attempt>) {
        let active = self.wheel_lock.owner() != &WheelOwner::Unpicked;
        self.wheel_lock.began(now, active, queued, pending);
    }

    #[inline]
    pub(super) fn wheel_ended_at(&mut self, now: f64) {
        self.wheel_lock.ended_at(now);
    }

    #[inline]
    pub(super) fn wheel_lost(&mut self, pending: Option<Attempt>) {
        self.wheel_lock.lost(pending);
    }

    /// Steps viewport `id`'s horizontal limit and momentum once `overflow` is known.
    pub(super) fn step_x_state(&mut self, id: NodeId, overflow: f64, dt: f32, catch_up: bool, scale: f64, land_wheel: bool) -> bool {
        let Some(st) = self.scrolls.get_mut(&id) else { return false };
        st.x.max = overflow.max(0.0);
        let mut moving = false;
        match st.x.motion {
            XMotion::Glide { v, aim } => {
                let mut glide = Some(v);
                moving = scroll_offset::glide_axis(st.glide_spring(), &mut st.x.offset, &mut glide, aim.min(st.x.max), dt);
                st.x.motion = glide.map_or(XMotion::Rest, |v| XMotion::Glide { v, aim });
            }
            XMotion::Momentum(v) if v.abs() > 1.0 => {
                let decay = (-SCROLL_DECAY * dt).exp();
                st.x.offset += f64::from(v * (1.0 - decay) / SCROLL_DECAY);
                let edge = st.x.offset <= 0.0 || st.x.offset >= st.x.max;
                st.x.motion = if edge { XMotion::Rest } else { XMotion::Momentum(v * decay) };
                moving = true;
            }
            XMotion::Momentum(_) => st.x.motion = XMotion::Rest,
            XMotion::Wheel(mut curve) => {
                curve.follow(st.x.offset);
                if curve.target() > st.x.max { curve.retarget(st.x.max, scale); }
                if land_wheel { curve.advance(f64::INFINITY, false) } else { curve.advance(f64::from(dt), catch_up) }
                st.x.offset = scroll_wheel::on_grid_or_end(curve.value(), st.x.max, scale);
                curve.applied(st.x.offset);
                st.moved();
                if curve.done() { st.x.motion = XMotion::Rest; } else { st.x.motion = XMotion::Wheel(curve); moving = true; }
            }
            XMotion::Rest => {}
        }
        st.x.offset = st.x.offset.clamp(0.0, st.x.max);
        moving
    }
}

impl RuntimeCore {
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

    /// The gesture the oldest wheel delta still waiting for `on_wheel` handlers belongs to: the
    /// only owner a new gesture can hand over to, since the queue runs in order.
    pub(in crate::runtime) fn wheel_pending(&self) -> Option<Attempt> {
        self.frame.queue.iter().find_map(|a| match a {
            Action::Wheel { gesture, .. } => Some(*gesture),
            _ => None,
        })
    }

    /// Whether a wheel delta waits in the queue for `on_wheel` handlers: one of gesture `of`,
    /// or of any gesture.
    pub(in crate::runtime) fn wheel_queued(&self, of: Option<Attempt>) -> bool {
        self.frame.queue.iter().any(|a| matches!(a, Action::Wheel { gesture: g, .. } if of.is_none_or(|of| *g == of)))
    }

    /// Scrolls by a delta no `on_wheel` handler took, on the axis the gesture locked to.
    /// `phased` is that delta's own gesture's policy, which a delta queued before a newer
    /// gesture began keeps.
    pub(in crate::runtime) fn wheel_scroll_for(&mut self, gesture: Attempt, ev: crate::WheelEvent, phased: bool, start: crate::runtime::gesture_dispatch::WheelStart) {
        let (x, y, dx, dy) = (ev.x, ev.y, ev.dx, ev.dy);
        if dx == 0.0 && dy == 0.0 { return; }
        self.scroll.wheel_lock.sync_gesture(gesture);
        // Without phases, a delta wholly on the other axis is another wheel turning (Shift let
        // go, a tilt, a second device), so it picks afresh rather than being dropped.
        let horizontal = match self.scroll.wheel_lock.keeps_axis(gesture, dx, dy, phased) {
            Some(h) => h,
            None => {
                let scroller = (dx.abs() > dy.abs()).then(|| self.scroll_at_axis_from(x, y, true, start)).flatten();
                self.scroll.wheel_lock.repick(gesture, scroller)
            }
        };
        if horizontal {
            if dx != 0.0 { self.wheel_x(gesture, x, y, dx, ev.notch, start); }
        } else if dy != 0.0 {
            if ev.notch { self.wheel_notch(x, y, dy, start) } else { self.wheel_from(x, y, dy, start) }
        }
    }

    /// The platform began a trackpad gesture (winit's `TouchPhase::Started` on a wheel event):
    /// its first delta picks the axis afresh, unless it is the momentum of one that just ended.
    pub fn wheel_began(&mut self) {
        let now = self.now_clock();
        let gesture = self.scroll.wheel_lock.gesture;
        // A delta queued for `on_wheel` handlers hasn't picked who has the gesture yet.
        let queued = self.wheel_queued(Some(gesture));
        let pending = self.wheel_pending();
        self.scroll.wheel_began_at(now, queued, pending);
    }

    /// The platform ended a trackpad gesture (`TouchPhase::Ended` or `Cancelled`).
    pub fn wheel_ended(&mut self) {
        let now = self.now_clock();
        self.scroll.wheel_ended_at(now);
    }

    /// The window lost the pointer or focus: a gesture in progress won't report its end here.
    pub(in crate::runtime) fn wheel_lost(&mut self) {
        let pending = self.wheel_pending();
        self.scroll.wheel_lost(pending);
    }

    /// Scrolls the horizontal viewport latched for `gesture` (or the innermost one at (x, y)) by
    /// `dx` points: 1:1, or for whole notches of a wheel eased there as the vertical axis does
    /// (`Self::wheel_notch`).
    fn wheel_x(&mut self, gesture: Attempt, x: f32, y: f32, dx: f32, notch: bool, start: crate::runtime::gesture_dispatch::WheelStart) {
        self.release_hover_hold();
        self.dismiss_help();
        let latched = match self.scroll.wheel_lock.owner_of(gesture) { Some(WheelOwner::Horizontal(id)) => Some(*id), _ => None }.filter(|id| {
            self.tree.node_index.get(id).is_some_and(|&i| self.is_scroll_viewport(i, true))
        });
        let Some(id) = latched.or_else(|| self.scroll_at_axis_from(x, y, true, start)) else { return };
        let glides = notch && self.wheel_glides();
        let scale = f64::from(self.env.scale * self.zoom());
        let Some(st) = self.scroll.scrolls.get_mut(&id) else { return };
        let before = st.x.offset;
        if glides {
            let easing = matches!(st.x.motion, XMotion::Wheel(_));
            let mut curve = match st.x.motion { XMotion::Wheel(c) => c, _ => scroll_wheel::WheelCurve::still(st.x.offset) };
            curve.follow(st.x.offset);
            let to = (curve.target() + st.x.toward(f64::from(dx))).clamp(0.0, st.x.max);
            if !curve.retarget(to, scale) {
                // Sent back to where it is: it stays put.
                if easing { st.x.motion = XMotion::Rest; }
                return;
            }
            st.x.motion = XMotion::Wheel(curve);
            st.moved();
            log_event!(self, "wheel-x-notch {dx:.1}");
            self.pointer.scroll_cancels_press(&self.tree.nodes, id);
            return;
        }
        st.x.offset = (st.x.offset + st.x.toward(f64::from(dx))).clamp(0.0, st.x.max);
        st.x.motion = XMotion::Rest;
        st.moved();
        log_event!(self, "wheel-x {dx:.1}");
        if st.x.offset != before { self.pointer.scroll_cancels_press(&self.tree.nodes, id); }
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
        if let Some(st) = self.scroll.scrolls.get_mut(&id) {
            st.x.motion = XMotion::Momentum(st.x.toward(f64::from(vx)) as f32);
            log_event!(self, "fling-x {vx:.0}");
            if vx != 0.0 { self.pointer.scroll_cancels_press(&self.tree.nodes, id); }
        }
    }

    /// How far horizontal viewport `i`'s content reaches past its end edge (its right, or its
    /// left right to left), negative while it fits.
    fn scroll_overflow_x(&self, i: usize) -> f64 {
        let view = self.tree.nodes[i].target;
        let reach = |c: usize| match self.tree.nodes[i].flow {
            crate::el::Flow::Ltr => self.tree.nodes[c].target[2] - view[0],
            crate::el::Flow::Rtl => view[2] - self.tree.nodes[c].target[0],
        };
        let content_w = self.scroll_children(i).map(|c| f64::from(reach(c))).reduce(f64::max).unwrap_or(0.0);
        content_w - f64::from(view[2] - view[0])
    }

    /// Steps viewport `i`'s horizontal limit and momentum, as `step_scroll` does the vertical.
    pub(in crate::runtime) fn step_scroll_x(&mut self, i: usize, dt: f32, catch_up: bool) -> bool {
        let id = self.tree.nodes[i].id;
        let overflow = self.scroll_overflow_x(i);
        if let Some(active) = self.step_touch_scroll(id, overflow, dt, true) { return active; }
        let scale = f64::from(self.env.scale * self.zoom());
        let land_wheel = !self.wheel_glides();
        self.scroll.step_x_state(id, overflow, dt, catch_up, scale, land_wheel)
    }

    /// `Tx::scroll_into_view` on x: scrolls the nearest horizontal viewport around row `key`
    /// (within `scope`, when given) just far enough to show it, leaving its `El::scroll_margin`.
    pub(in crate::runtime) fn reveal_row_x(&mut self, scope: Option<&str>, key: &str) {
        let target = self.live_named(scope, key)
            .map(|i| (i, self.tree.nodes[i].target[0], self.tree.nodes[i].local.w))
            .or_else(|| {
                let (i, [x0, _, x1, _]) = self.highlight_target_in(&self.tree.nodes, scope, key)?;
                (!self.subtree_exiting(i)).then_some((i, x0, (x1 - x0).max(0.0)))
            });
        let Some((i, target_x, target_w)) = target else { return };
        let Some(v) = std::iter::successors(self.tree.nodes[i].parent, |&p| self.tree.nodes[p].parent)
            .take_while(|&p| in_scope(&self.tree.nodes[p].path, scope))
            .find(|&p| self.tree.nodes[p].scroll.is_some() && self.tree.nodes[p].scroll_axes.x()) else { return };
        let view = &self.tree.nodes[v];
        // From the viewport's start edge: its right, right to left.
        let start = match view.flow {
            crate::el::Flow::Ltr => target_x - view.target[0],
            crate::el::Flow::Rtl => view.target[2] - (target_x + target_w),
        };
        let (left, width, viewport) = (f64::from(start), f64::from(target_w), f64::from(view.local.w));
        let (margin, id) = (f64::from(view.scroll_edges.margin), view.id);
        // A vertical reveal may have just mounted rows (virtual ones) that widened the content.
        let max = self.scroll_overflow_x(v).max(0.0);
        let Some(st) = self.scroll.scrolls.get_mut(&id) else { return };
        st.x.max = max;
        let offset = scroll_edge::reveal(st.x.offset, left, width, viewport, margin).clamp(0.0, max);
        if offset == st.x.offset { return; }
        st.x.offset = offset;
        st.x.motion = XMotion::Rest;
        st.moved();
    }
}

#[cfg(test)]
#[path = "x_tests.rs"]
mod tests;
