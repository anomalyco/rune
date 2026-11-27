//! Routes the canvas input of `canvas_input.rs`: wheel deltas to `on_wheel` before scrolling,
//! pinch, rotation and smart magnify, extra mouse buttons to `on_press` and `on_drag`, and key
//! releases to `on_key_up`.
use super::*;
use crate::input::canvas_input::CanvasInput;
use crate::machine::Attempt;
use crate::runtime::scroll_x::WheelOwner;
use crate::{GesturePhase, MagnifyEvent, PointerButton, RotateEvent, WheelEvent};

/// A trackpad gesture or extra mouse button press at a window point, queued for the handlers under it.
#[derive(Clone, Copy, Debug)]
pub(in crate::runtime) enum Gesture {
    /// Handler-only two-finger touchscreen pan (`WheelEvent::touch == true`): never scrolls
    /// scroll views and never touches `WheelLock`.
    TouchWheel(WheelEvent),
    Magnify(MagnifyEvent),
    Rotate(RotateEvent),
    SmartMagnify(f32, f32),
    /// An extra mouse button (`Middle`, `Back`, `Forward`, `Other`) for [`El::on_button`]
    /// handlers; `released` records a same-frame middle release for deferred autoscroll fallback.
    Button { button: PointerButton, x: f32, y: f32, released: Option<(f32, f32)> },
}

/// Where a wheel delta's chain starts (`Runtime::chain_start`), when it was already found on the
/// way to scrolling, so one delta looks it up once.
#[derive(Clone, Copy, Debug)]
pub(crate) enum WheelStart {
    Unknown,
    At(Option<usize>),
}

/// Which phased canvas gesture channel owns a `GestureOwner` slot on `PointerState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhasedChannel {
    Magnify,
    Rotate,
    TouchWheel,
}

/// Effective phased-gesture state at queue time, taking already-queued actions on the same
/// channel into account ahead of `PointerState::gesture_owners`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueuedPhased {
    Idle,
    Active,
    Declined,
}

/// Points of pixel-precise Ctrl + wheel per unit of magnification, where a pinch arrives that way.
const PINCH_WHEEL: f32 = 200.0;
/// Zoom factor per vertical line-delta wheel notch with Ctrl held, matching Chromium's
/// `kTextSizeMultiplierRatio = 1.2` (`third_party/blink/common/page/page_zoom.cc`).
const PINCH_WHEEL_STEP: f32 = 1.2;

impl RuntimeCore {
    /// A wheel or trackpad scroll at window point (`ev.x`, `ev.y`), with its modifiers and
    /// phase. The innermost [`El::on_wheel`] handler there hears it first, bubbling through
    /// every ancestor past scroll views; what none takes scrolls as [`Self::wheel_xy`] does.
    pub fn wheel_event(&mut self, ev: WheelEvent) {
        let (x, y) = self.locked_point(ev.x, ev.y);
        let ev = WheelEvent { x, y, ..ev };
        if ev.touch {
            self.touch_wheel_event_at(ev, (x, y));
            return;
        }
        if !self.pointer.is_touch_input() {
            self.sync_primary_pointer(cx::PrimaryPointer::Hover);
        }
        if ev.phase == GesturePhase::Began { self.wheel_began(); }
        let moved = ev.dx != 0.0 || ev.dy != 0.0;
        if moved {
            self.dismiss_help();
            let now = self.now_clock();
            let pending = self.wheel_pending();
            let (dx, dy) = crate::runtime::scroll_x::shift_scroll_delta(ev);
            self.scroll.wheel_delta(now, dx, dy, ev.mods, pending);
        }
        // A notch or not is the run's to decide, not each delta's.
        let ev = if moved { WheelEvent { notch: self.scroll.wheel_notched(self.now_clock(), ev.notch), ..ev } } else { ev };
        if moved || ev.phase != GesturePhase::Moved {
            let ev = WheelEvent { momentum: ev.momentum || self.scroll.wheel_momentum(), ..ev };
            let gesture = self.scroll.wheel_gesture();
            let phased = self.scroll.wheel_phased();
            if !(moved && self.dismiss_modal_on_wheel(gesture, ev, phased))
                && let Some(start) = self.offer_wheel(gesture, ev)
            {
                self.wheel_scroll_for(gesture, ev, phased, start);
            }
        }
        if ev.phase == GesturePhase::Ended { self.wheel_ended(); }
    }

    /// Dismisses an active modal opted into [`crate::El::modal_wheel_outside`] on an outside,
    /// non-momentum wheel and queues [`Action::WheelRepost`] so the delta routes to what lies
    /// beneath once the dismissal commits and rebuilds hit-testing.
    pub(super) fn dismiss_modal_on_wheel(&mut self, gesture: Attempt, ev: WheelEvent, phased: bool) -> bool {
        if ev.momentum { return false; }
        let owner_allows = matches!(self.scroll.wheel_owner(), WheelOwner::Unpicked)
            || (!phased && matches!(self.scroll.wheel_owner(), WheelOwner::Vertical | WheelOwner::Horizontal(_)));
        if !owner_allows { return false; }
        let Some(i) = self.active_modal() else { return false };
        if self.point_in_modal(i, ev.x, ev.y) { return false; }
        if !self.tree.nodes[i].handlers.modal.as_ref().is_some_and(|m| m.wheel_outside && m.dismiss.is_some()) {
            return false;
        }
        if self.scroll.wheel_owner() != &WheelOwner::Unpicked {
            self.scroll.set_wheel_owner(gesture, WheelOwner::Unpicked);
            self.scroll.end_wheel_vertical();
        }
        let modal = self.tree.nodes[i].path.clone();
        let outer = self.outer_modal_path(i);
        if !self.frame.queue.iter().any(|a| matches!(a, Action::Dismiss(p) if *p == modal)) {
            self.frame.queue.push(Action::Dismiss(modal.clone()));
        }
        self.frame.queue.push(Action::WheelRepost(action::WheelRepost { modal, outer, gesture, ev, phased }));
        true
    }

    /// Same-frame outside-wheel repost: runs after `Action::Dismiss` commits so `ModalScope` and
    /// hit-testing are freshly rebuilt for the post-dismiss tree.
    pub(super) fn run_wheel_repost(&mut self, app: &mut dyn AppDriver, repost: action::WheelRepost, one: &mut Tx) -> bool {
        if !self.repost_allowed(&repost.modal, repost.outer.as_deref()) { return false; }
        if !self.tree.built {
            self.scene.present();
            self.begin_exit_mask();
            self.build();
            self.end_exit_mask();
        }
        self.run_wheel(app, repost.gesture, None, repost.ev, repost.phased, one)
    }

    /// Queues `ev` for the `on_wheel` handlers it may go to (None), or leaves it to scrolling now,
    /// with where its chain starts when that was already found on the way.
    fn offer_wheel(&mut self, gesture: Attempt, ev: WheelEvent) -> Option<WheelStart> {
        // A delta queued this frame may yet be taken: later ones wait their turn behind it.
        let pending = self.wheel_queued(Some(gesture));
        let (scrolls, handled) = match self.scroll.wheel_owner() {
            WheelOwner::Vertical | WheelOwner::Horizontal(_) => (true, false),
            WheelOwner::Handler(_) => (false, true),
            WheelOwner::Unpicked => (false, false),
        };
        if scrolls && !pending { return Some(WheelStart::Unknown); }
        let start = if handled { WheelStart::Unknown } else { WheelStart::At(self.chain_start(ev.x, ev.y)) };
        let chain = match start { WheelStart::At(s) => Some(self.gesture_chain_from(s, |g| g.wheel.is_some())), WheelStart::Unknown => None };
        if chain.as_ref().is_some_and(Vec::is_empty) && !handled && !pending { return Some(start); }
        let phased = self.scroll.wheel_phased();
        self.frame.queue.push(Action::Wheel { gesture, chain, ev, phased });
        None
    }

    /// A queued wheel delta: the handler holding its gesture hears it, else each handler
    /// under the pointer until one takes it, else it scrolls. If the owning handler declines
    /// (returns false) or goes away, the delta bubbles to outer handlers and then scrolls as
    /// usual. Each queued delta keeps its own gesture's turn and axis policy, so one queued
    /// before a newer gesture began still reaches the owner its own gesture picked.
    pub(in crate::runtime) fn run_wheel(&mut self, app: &mut dyn AppDriver, gesture: Attempt, chain: Option<Vec<String>>, ev: WheelEvent, phased: bool, tx: &mut Tx) -> bool {
        self.scroll.sync_wheel_gesture(gesture);
        let mut taken = false;
        match self.scroll.wheel_owner_of(gesture).cloned() {
            Some(WheelOwner::Handler(path)) => {
                match self.call_wheel(app, &path, ev, tx) {
                    Some(true) => taken = true,
                    result => {
                        self.scroll.set_wheel_owner(gesture, WheelOwner::Unpicked);
                        let rest = if result.is_some() {
                            let parent = self.live_index(&path).and_then(|i| self.tree.nodes[i].parent);
                            self.gesture_chain_from(parent, |g| g.wheel.is_some())
                        } else {
                            self.gesture_chain(ev.x, ev.y, |g| g.wheel.is_some())
                        };
                        for next in rest {
                            if self.call_wheel(app, &next, ev, tx) == Some(true) {
                                self.scroll.set_wheel_owner(gesture, WheelOwner::Handler(next));
                                taken = true;
                                break;
                            }
                        }
                    }
                }
            }
            Some(WheelOwner::Unpicked) => {
                let chain = chain.unwrap_or_else(|| self.gesture_chain(ev.x, ev.y, |g| g.wheel.is_some()));
                for path in chain {
                    if self.call_wheel(app, &path, ev, tx) == Some(true) {
                        self.scroll.set_wheel_owner(gesture, WheelOwner::Handler(path));
                        taken = true;
                        break;
                    }
                }
            }
            _ => {}
        }
        if !taken {
            // Queued from an earlier frame: what lies under the pointer may have moved since.
            self.wheel_scroll_for(gesture, ev, phased, WheelStart::Unknown);
        }
        taken
    }

    /// Element `path`'s `on_wheel` on `ev`, in its own points; None when it's gone.
    fn call_wheel(&mut self, app: &mut dyn AppDriver, path: &str, ev: WheelEvent, tx: &mut Tx) -> Option<bool> {
        let (i, f) = self.gesture_handler(path, |g| g.wheel.clone())?;
        let [x, y] = self.to_local(i, ev.x, ev.y);
        Some(Tx::took(f(app.states(), WheelEvent { x, y, ..ev }, tx), tx))
    }

    /// Handler-only two-finger touchscreen pan (`WheelEvent::touch == true`): locked to its
    /// accepting `Began` `on_wheel` handler until `Ended`, never falling through to scroll views
    /// or mutating `WheelLock`.
    pub(in crate::runtime) fn touch_wheel_event_at(&mut self, ev: WheelEvent, hit: (f32, f32)) {
        let ev = WheelEvent { touch: true, ..ev };
        self.offer_phased_gesture(
            PhasedChannel::TouchWheel,
            ev.phase,
            (ev.x, ev.y),
            hit,
            |g| g.wheel.is_some(),
            Gesture::TouchWheel(ev),
        );
    }

    /// A trackpad or touchscreen pinch at window point (`ev.x`, `ev.y`) for the
    /// [`El::on_magnify`] handlers there, innermost first. Phased non-wheel pinches
    /// (`Began` / `Moved` / `Ended`) lock the accepting `Began` handler until `Ended`.
    pub fn magnify_event(&mut self, ev: MagnifyEvent) {
        self.magnify_event_at(ev, (ev.x, ev.y));
    }

    /// Like [`Self::magnify_event`], resolving `Began`'s candidate chain at `hit` while
    /// delivering `(ev.x, ev.y)` in the owning handler's local coordinates.
    pub(in crate::runtime) fn magnify_event_at(&mut self, ev: MagnifyEvent, hit: (f32, f32)) {
        if ev.wheel {
            self.offer_gesture(ev.x, ev.y, |g| g.magnify.is_some(), Gesture::Magnify(ev));
            return;
        }
        self.offer_phased_gesture(
            PhasedChannel::Magnify,
            ev.phase,
            (ev.x, ev.y),
            hit,
            |g| g.magnify.is_some(),
            Gesture::Magnify(ev),
        );
    }

    /// A two-finger trackpad or touchscreen rotation at window point (`ev.x`, `ev.y`) for the
    /// [`El::on_rotate`] handlers there, innermost first. Phased rotations (`Began` / `Moved` /
    /// `Ended`) lock the accepting `Began` handler until `Ended`.
    pub fn rotate_event(&mut self, ev: RotateEvent) {
        self.rotate_event_at(ev, (ev.x, ev.y));
    }

    /// Like [`Self::rotate_event`], resolving `Began`'s candidate chain at `hit` while
    /// delivering `(ev.x, ev.y)` in the owning handler's local coordinates.
    pub(in crate::runtime) fn rotate_event_at(&mut self, ev: RotateEvent, hit: (f32, f32)) {
        self.offer_phased_gesture(
            PhasedChannel::Rotate,
            ev.phase,
            (ev.x, ev.y),
            hit,
            |g| g.rotate.is_some(),
            Gesture::Rotate(ev),
        );
    }

    fn gesture_owner(&self, channel: PhasedChannel) -> &capture::GestureOwner {
        match channel {
            PhasedChannel::Magnify => &self.pointer.gesture_owners.magnify,
            PhasedChannel::Rotate => &self.pointer.gesture_owners.rotate,
            PhasedChannel::TouchWheel => &self.pointer.gesture_owners.touch_wheel,
        }
    }

    fn gesture_owner_mut(&mut self, channel: PhasedChannel) -> &mut capture::GestureOwner {
        match channel {
            PhasedChannel::Magnify => &mut self.pointer.gesture_owners.magnify,
            PhasedChannel::Rotate => &mut self.pointer.gesture_owners.rotate,
            PhasedChannel::TouchWheel => &mut self.pointer.gesture_owners.touch_wheel,
        }
    }

    fn offer_phased_gesture(
        &mut self,
        channel: PhasedChannel,
        phase: GesturePhase,
        pos: (f32, f32),
        hit: (f32, f32),
        has: impl Fn(&CanvasInput) -> bool,
        gesture: Gesture,
    ) {
        let (queued_state, has_queued_phased) = self.queued_phased_state(channel);
        match phase {
            GesturePhase::Began => {
                let chain = self.gesture_chain(hit.0, hit.1, has);
                if !chain.is_empty() || has_queued_phased {
                    self.frame.queue.push(Action::Gesture(chain, gesture));
                } else {
                    *self.gesture_owner_mut(channel) = capture::GestureOwner::Declined;
                }
            }
            GesturePhase::Moved => match queued_state {
                QueuedPhased::Active => self.frame.queue.push(Action::Gesture(Vec::new(), gesture)),
                QueuedPhased::Declined => {}
                QueuedPhased::Idle => self.offer_gesture(pos.0, pos.1, has, gesture),
            },
            GesturePhase::Ended => match queued_state {
                QueuedPhased::Active => self.frame.queue.push(Action::Gesture(Vec::new(), gesture)),
                QueuedPhased::Declined => {
                    if has_queued_phased {
                        self.frame.queue.push(Action::Gesture(Vec::new(), gesture));
                    } else {
                        *self.gesture_owner_mut(channel) = capture::GestureOwner::Idle;
                    }
                }
                QueuedPhased::Idle => self.offer_gesture(pos.0, pos.1, has, gesture),
            },
        }
    }

    fn queued_phased_state(&self, channel: PhasedChannel) -> (QueuedPhased, bool) {
        let mut has_queued_phased = false;
        let mut latest = None;
        for action in self.frame.queue.iter() {
            let Some((chain, phase)) = (match (channel, action) {
                (PhasedChannel::Magnify, Action::Gesture(c, Gesture::Magnify(q))) if !q.wheel => Some((c, q.phase)),
                (PhasedChannel::Rotate, Action::Gesture(c, Gesture::Rotate(q))) => Some((c, q.phase)),
                (PhasedChannel::TouchWheel, Action::Gesture(c, Gesture::TouchWheel(q))) => Some((c, q.phase)),
                _ => None,
            }) else {
                continue;
            };
            has_queued_phased = true;
            match phase {
                GesturePhase::Began => {
                    latest = Some(if chain.is_empty() {
                        QueuedPhased::Declined
                    } else {
                        QueuedPhased::Active
                    });
                }
                GesturePhase::Ended => latest = Some(QueuedPhased::Idle),
                GesturePhase::Moved => {}
            }
        }
        let state = latest.unwrap_or(match self.gesture_owner(channel) {
            capture::GestureOwner::Idle => QueuedPhased::Idle,
            capture::GestureOwner::Owned(_) => QueuedPhased::Active,
            capture::GestureOwner::Declined => QueuedPhased::Declined,
        });
        (state, has_queued_phased)
    }

    /// A two-finger double tap at window point (x, y) for the [`El::on_smart_magnify`]
    /// handlers there, innermost first.
    pub fn smart_magnify(&mut self, x: f32, y: f32) {
        self.offer_gesture(x, y, |g| g.smart_magnify.is_some(), Gesture::SmartMagnify(x, y));
    }

    /// A Ctrl + wheel at (x, y), which is how a trackpad pinch reaches Windows, Linux and web
    /// apps, and how Chromium hands any page Ctrl + wheel: a pinch (`MagnifyEvent::wheel`) when
    /// an `on_magnify` handler is there (true), else nothing. For line-delta wheels (`ticks`),
    /// scales by `1.2^ticks - 1.0` (+20% per forward notch, independent of OS scroll-lines
    /// settings, matching Chromium's tick-based `WebContentsImpl::HandleWheelEvent` and
    /// `kTextSizeMultiplierRatio`); for pixel-precise deltas (`None`), scales by `dy / PINCH_WHEEL`.
    /// Positive `ticks` or `dy` moves content down, so the wheel turned away from the user (or the
    /// fingers spread) zooms in, as in a browser.
    pub(crate) fn pinch_wheel(&mut self, x: f32, y: f32, dy: f32, ticks: Option<f32>) -> bool {
        let delta = match ticks {
            Some(t) if t != 0.0 => PINCH_WHEEL_STEP.powf(t) - 1.0,
            Some(_) => return false,
            None if dy != 0.0 => dy / PINCH_WHEEL,
            None => return false,
        };
        if self.gesture_chain(x, y, |g| g.magnify.is_some()).is_empty() { return false; }
        self.magnify_event(MagnifyEvent::new(delta).at(x, y).wheel(true));
        true
    }

    fn offer_gesture(&mut self, x: f32, y: f32, has: impl Fn(&CanvasInput) -> bool, gesture: Gesture) {
        let chain = self.gesture_chain(x, y, has);
        if !chain.is_empty() { self.frame.queue.push(Action::Gesture(chain, gesture)); }
    }

    fn call_magnify(&mut self, app: &mut dyn AppDriver, path: &str, ev: MagnifyEvent, tx: &mut Tx) -> Option<bool> {
        let (i, f) = self.gesture_handler(path, |g| g.magnify.clone())?;
        let [x, y] = self.to_local(i, ev.x, ev.y);
        Some(Tx::took(f(app.states(), MagnifyEvent { x, y, ..ev }, tx), tx))
    }

    fn call_rotate(&mut self, app: &mut dyn AppDriver, path: &str, ev: RotateEvent, tx: &mut Tx) -> Option<bool> {
        let (i, f) = self.gesture_handler(path, |g| g.rotate.clone())?;
        let [x, y] = self.to_local(i, ev.x, ev.y);
        Some(Tx::took(f(app.states(), RotateEvent { x, y, ..ev }, tx), tx))
    }

    fn dispatch_phased_chain(
        &mut self,
        app: &mut dyn AppDriver,
        chain: &[String],
        tx: &mut Tx,
        call: &impl Fn(&mut Self, &mut dyn AppDriver, &str, &mut Tx) -> Option<bool>,
    ) -> Option<String> {
        for path in chain {
            if call(self, app, path, tx) == Some(true) {
                return Some(path.clone());
            }
        }
        None
    }

    fn run_phased_gesture(
        &mut self,
        app: &mut dyn AppDriver,
        channel: PhasedChannel,
        phase: GesturePhase,
        chain: &[String],
        tx: &mut Tx,
        call: impl Fn(&mut Self, &mut dyn AppDriver, &str, &mut Tx) -> Option<bool>,
    ) -> bool {
        match phase {
            GesturePhase::Began => {
                if let Some(owner) = self.dispatch_phased_chain(app, chain, tx, &call) {
                    *self.gesture_owner_mut(channel) = capture::GestureOwner::Owned(owner);
                    true
                } else {
                    *self.gesture_owner_mut(channel) = capture::GestureOwner::Declined;
                    false
                }
            }
            GesturePhase::Moved => match self.gesture_owner(channel).clone() {
                capture::GestureOwner::Owned(path) => {
                    if call(self, app, &path, tx) == Some(true) {
                        true
                    } else {
                        *self.gesture_owner_mut(channel) = capture::GestureOwner::Declined;
                        false
                    }
                }
                capture::GestureOwner::Declined => false,
                capture::GestureOwner::Idle => self.dispatch_phased_chain(app, chain, tx, &call).is_some(),
            },
            GesturePhase::Ended => match std::mem::take(self.gesture_owner_mut(channel)) {
                capture::GestureOwner::Owned(path) => call(self, app, &path, tx) == Some(true),
                capture::GestureOwner::Declined => false,
                capture::GestureOwner::Idle => self.dispatch_phased_chain(app, chain, tx, &call).is_some(),
            },
        }
    }

    pub(in crate::runtime) fn run_gesture(&mut self, app: &mut dyn AppDriver, chain: Vec<String>, gesture: Gesture, tx: &mut Tx) -> bool {
        match gesture {
            Gesture::TouchWheel(ev) => {
                return self.run_phased_gesture(app, PhasedChannel::TouchWheel, ev.phase, &chain, tx, |rt, app, path, tx| {
                    rt.call_wheel(app, path, ev, tx)
                });
            }
            Gesture::Magnify(ev) if ev.wheel => {
                return self.dispatch_phased_chain(app, &chain, tx, &|rt, app, path, tx| {
                    rt.call_magnify(app, path, ev, tx)
                }).is_some();
            }
            Gesture::Magnify(ev) => {
                return self.run_phased_gesture(app, PhasedChannel::Magnify, ev.phase, &chain, tx, |rt, app, path, tx| {
                    rt.call_magnify(app, path, ev, tx)
                });
            }
            Gesture::Rotate(ev) => {
                return self.run_phased_gesture(app, PhasedChannel::Rotate, ev.phase, &chain, tx, |rt, app, path, tx| {
                    rt.call_rotate(app, path, ev, tx)
                });
            }
            Gesture::SmartMagnify(..) | Gesture::Button { .. } => {}
        }
        if let Gesture::Button { button, x, y, released } = gesture {
            tx.button = button;
            for path in chain {
                let taken = self.gesture_handler(&path, |g| g.button.clone()).map(|(_, f)| {
                    Tx::took(f(app.states(), button, tx), tx)
                });
                if taken == Some(true) { return true; }
            }
            if button == PointerButton::Middle && self.pointer.pos.is_some() && self.autoscroll_down(x, y) {
                if let Some((ux, uy)) = released {
                    self.autoscroll_moved(ux, uy);
                    self.pointer.autoscroll_release(PointerButton::Middle);
                } else if let Some((px, py)) = self.pointer.pos && (px, py) != (x, y) {
                    self.autoscroll_moved(px, py);
                }
            }
            return false;
        }
        for path in chain {
            let taken = match gesture {
                Gesture::SmartMagnify(x, y) => self.gesture_handler(&path, |g| g.smart_magnify.clone()).map(|(i, f)| {
                    let [x, y] = self.to_local(i, x, y);
                    Tx::took(f(app.states(), x, y, tx), tx)
                }),
                Gesture::TouchWheel(_) | Gesture::Magnify(_) | Gesture::Rotate(_) | Gesture::Button { .. } => unreachable!(),
            };
            if taken == Some(true) { return true; }
        }
        false
    }

    /// Whether an allowed, non-exiting element under window point `(x, y)` handles pinch
    /// magnification (`El::on_magnify` / `El::on_magnify_any`) or two-finger rotation
    /// (`El::on_rotate`). Used on Safari/WebKit so `gesturestart` / `gesturechange` only
    /// prevents native browser pinch-zoom when a Rune element actually claims the gesture.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn has_magnify_or_rotate(&self, x: f32, y: f32) -> bool {
        let mut at = self.chain_start(x, y);
        while let Some(i) = at {
            if self.subtree_exiting(i) || !self.allowed(i) { break; }
            if self.tree.nodes[i].handlers.gestures.as_deref()
                .is_some_and(|g| g.magnify.is_some() || g.rotate.is_some())
            {
                return true;
            }
            at = self.tree.nodes[i].parent;
        }
        false
    }

    /// The elements under window point (x, y) with a handler `has` picks, innermost first,
    /// walking every ancestor up to the root (or open modal) past scroll views.
    pub(super) fn gesture_chain(&self, x: f32, y: f32, has: impl Fn(&CanvasInput) -> bool) -> Vec<String> {
        self.gesture_chain_from(self.chain_start(x, y), has)
    }

    /// [`Self::gesture_chain`] from a start already found.
    fn gesture_chain_from(&self, start: Option<usize>, has: impl Fn(&CanvasInput) -> bool) -> Vec<String> {
        let mut chain = Vec::new();
        let mut at = start;
        while let Some(i) = at {
            if self.subtree_exiting(i) || !self.allowed(i) { break; }
            let n = &self.tree.nodes[i];
            if n.handlers.gestures.as_deref().is_some_and(&has) { chain.push(n.path.clone()); }
            at = n.parent;
        }
        chain
    }

    /// Where a wheel or gesture at window point (x, y) starts walking up the tree: the topmost of
    /// what takes input, the scroll views and the painted surfaces there (`wheel_hits`, in paint
    /// order; a row inside a list paints after the list, so it wins). Its ancestors are what
    /// Chromium chains a scroll through: a viewport that only lies beneath (the timeline under an
    /// open menu, a popover or a raised panel) is never one of them.
    pub(in crate::runtime) fn chain_start(&self, x: f32, y: f32) -> Option<usize> {
        self.hit.find_wheel_hit(|id, r| {
            if !self.hit_contains(id, r, x, y) { return None; }
            let i = *self.tree.node_index.get(&id)?;
            (!self.subtree_exiting(i) && self.allowed(i)).then_some(i)
        })
    }

    /// Element `path` (not exiting or inert) and the handler `pick` takes from its canvas input.
    fn gesture_handler<F>(&self, path: &str, pick: impl Fn(&CanvasInput) -> Option<F>) -> Option<(usize, F)> {
        let i = self.tree.nodes.iter().enumerate().position(|(i, n)| n.path == path && !self.subtree_exiting(i) && self.allowed(i))?;
        Some((i, pick(self.tree.nodes[i].handlers.gestures.as_deref()?)?))
    }

    /// A mouse button went down at (x, y). The primary and secondary buttons do what
    /// [`Self::pointer_down`] and [`Self::secondary_down`] do. Another presses the innermost
    /// element there that takes it: one that opted in ([`El::pointer_buttons`]), whose `on_drag`
    /// and `on_press` follow as for the primary button; for the middle button, also a link of a
    /// text with `on_link`, or an element with [`El::on_middle_press`], each clicked when the
    /// button comes up over it.
    pub(crate) fn button_down(&mut self, app: &mut dyn AppDriver, x: f32, y: f32, button: PointerButton) {
        self.button_down_with_primary(app, x, y, button, cfg!(target_os = "linux"));
    }

    /// Queues passive [`El::on_any_press`] observers for a button-down that starts a new
    /// gesture, in reverse tree order (topmost live, allowed, non-exiting observer first), ahead
    /// of that press's normal target actions. Windows without an observer pay one cached boolean
    /// check.
    pub(in crate::runtime) fn queue_any_press(&mut self, button: PointerButton, x: f32, y: f32) {
        if !self.tree.has_any_press { return; }
        for i in (0..self.tree.nodes.len()).rev() {
            if self.tree.nodes[i].handlers.get().is_some_and(|h| h.on_any_press.is_some())
                && self.allowed(i)
                && !self.subtree_exiting(i)
                && !self.subtree_unmounted(i)
            {
                let point = self.pointer_event(i, x, y);
                let path = self.tree.nodes[i].path.clone();
                self.frame.queue.push(Action::AnyPress(path, button, point));
            }
        }
    }

    pub(super) fn button_down_with_primary(&mut self, app: &mut dyn AppDriver, x: f32, y: f32, button: PointerButton, primary_paste: bool) {
        match button {
            PointerButton::Primary => return self.pointer_down(app, x, y),
            PointerButton::Secondary => return self.secondary_down(x, y),
            _ => {}
        }
        if !self.pointer.is_touch_input() {
            self.sync_primary_pointer(cx::PrimaryPointer::Hover);
        }
        let (x, y) = self.locked_point(x, y);
        let starts_gesture = !self.pointer.any_button_down() && !self.pointer.captured();
        self.pointer.note_button_down(button);
        if self.pointer.autoscroll_press(button) { self.pointer.pos = Some((x, y)); return; }
        if self.pointer.press.is_held() { return; }
        self.pointer.pos = Some((x, y));
        if starts_gesture { self.queue_any_press(button, x, y); }
        if button == PointerButton::Middle { self.tint_down(x, y, button); }
        // On Linux a middle click in a text field pastes the primary selection there.
        #[cfg(any(target_os = "linux", test))]
        if primary_paste && button == PointerButton::Middle && self.primary_paste_down(x, y) { return; }
        #[cfg(not(any(target_os = "linux", test)))]
        let _ = primary_paste;
        self.dismiss_help();
        let hit = self.hit.top_hit(|id, r| self.hit_contains(id, r, x, y)).and_then(|id| self.tree.node_index.get(&id).copied());
        let middle = button == PointerButton::Middle;
        let target = std::iter::successors(hit, |&i| self.tree.nodes[i].parent)
            .take_while(|&i| !self.subtree_exiting(i) && self.allowed(i))
            .find_map(|i| {
                let h = &self.tree.nodes[i].handlers;
                if self.takes_button(i, button) && (h.on_press.is_some() || h.on_drag.is_some()) { return Some((i, None)); }
                if !middle { return None; }
                if let Some(k) = self.middle_link_at(i, x, y) { return Some((i, Some(k))); }
                h.on_middle_press.is_some().then_some((i, None))
            });
        let Some((i, link)) = target else {
            let chain = self.gesture_chain(x, y, |g| g.button.is_some());
            if !chain.is_empty() {
                self.frame.queue.push(Action::Gesture(chain, Gesture::Button { button, x, y, released: None }));
            } else if middle {
                self.autoscroll_down(x, y);
            }
            return;
        };
        let n = &self.tree.nodes[i];
        let r = n.target;
        let [lx, ly] = self.to_local(i, x, y);
        let d = Drag { phase: Phase::Down, x: lx, y: ly, w: r[2] - r[0], h: r[3] - r[1], start_x: lx, start_y: ly, dx: 0.0, dy: 0.0, data: 0.0, clicks: 1, cancelled: false, rtl: n.flow == crate::el::Flow::Rtl, window: [x, y], screen: self.screen_at(x, y), edge: None };
        let path = n.path.clone();
        let drag_handler = self.drags_with(i, button).then(|| n.handlers.on_drag.clone()).flatten();
        if drag_handler.is_some() { self.frame.queue.push(Action::Drag(path.clone(), d, button)); }
        self.pointer.clear_press_clicks();
        let now = self.now_clock();
        self.pointer.press.hold(capture::Held::new(path, d, button, (x, y), now, false, None).on_link(link).drag_handler(drag_handler));
    }

    /// Element `i` opted in to `button` ([`El::pointer_buttons`]); every element takes the
    /// primary.
    pub(in crate::runtime) fn takes_button(&self, i: usize, button: PointerButton) -> bool {
        button == PointerButton::Primary || self.tree.nodes[i].handlers.gestures.as_ref().is_some_and(|g| g.buttons & button.bit() != 0)
    }

    /// Element `i`'s `on_drag` hears a press of `button`.
    pub(in crate::runtime) fn drags_with(&self, i: usize, button: PointerButton) -> bool {
        let n = &self.tree.nodes[i];
        n.handlers.on_drag.is_some() && !self.drags_out(n) && self.takes_button(i, button)
    }

    /// The link of text `i` under (x, y) that a middle click opens: one its `on_link` hears.
    fn middle_link_at(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        self.tree.nodes[i].text.as_ref()?.cold.on_link.as_ref()?;
        self.link_at(i, x, y)
    }

    /// A mouse button came up at (x, y): the release of [`Self::button_down`].
    pub(crate) fn button_up(&mut self, app: &dyn ViewDriver, x: f32, y: f32, button: PointerButton) {
        let (x, y) = self.locked_point(x, y);
        self.pointer.note_button_up(button);
        self.pointer.release_tint(button);
        if button == PointerButton::Middle { self.frame.queue.note_middle_button_up(x, y); }
        if !matches!(button, PointerButton::Primary | PointerButton::Secondary) && self.pointer.autoscroll_release(button) { self.pointer.pos = Some((x, y)); return; }
        match button {
            PointerButton::Primary => self.pointer_up(app, x, y),
            PointerButton::Secondary => self.secondary_up(app, x, y),
            _ if self.pointer.press.held().is_some_and(|h| h.button == button) => self.release_capture(app, x, y),
            _ => {}
        }
    }

    /// A key came up, for the [`El::on_key_up`] handlers: true when one is there to hear it,
    /// and a frame is needed. A host with physical keys passes the release's logical key to
    /// `physical_then` instead, so the focused `on_physical_input` hears it first.
    pub(crate) fn key_up(&mut self, app: &mut dyn AppDriver, ev: KeyEvent) -> bool {
        if matches!(ev.key, crate::Key::Media(_)) && !self.kbd.media_keys { return false; }
        if !self.wants_key_up() { return false; }
        if self.frame.queue.is_empty() && !self.commit.dirty && self.eat_ime_key_up(app) { return false; }
        self.frame.queue.push(Action::KeyUp(ev));
        true
    }

    /// Some reachable element has an `on_wheel` handler.
    pub(crate) fn hears_wheel(&self) -> bool {
        self.tree.nodes.iter().enumerate().any(|(i, n)| n.handlers.get().is_some_and(|h| h.gestures.as_ref().is_some_and(|g| g.wheel.is_some())) && self.allowed(i))
    }

    /// Some element hears key releases, so a host maps them to logical keys.
    pub(crate) fn wants_key_up(&self) -> bool {
        self.tree.has_key_up || !self.frame.queue.is_empty() || self.commit.dirty
    }

    /// Records a key press for [`Self::route_key_up`]: whether it belongs to an open IME
    /// composition, or which focused element should still hear its release if focus moves.
    pub(in crate::runtime) fn note_key_down(&mut self, app: &mut dyn AppDriver, repeat: bool) {
        let composing = self.composing(app);
        self.kbd.note_key_down(composing, repeat);
    }

    /// Suppresses key releases that belong to an IME composition (while preedit is open, or
    /// the release of the key that confirmed or cancelled it).
    pub(in crate::runtime) fn eat_ime_key_up(&mut self, app: &mut dyn AppDriver) -> bool {
        let composing = self.composing(app);
        self.kbd.eat_ime_key_up(composing)
    }

    /// A key release goes to the focused element and up through its ancestors, else to every
    /// element with a handler (deepest first), inside the open modal if there is one. If focus moved since the key went down, the element that had
    /// focus on the press (and its ancestors) hears the release first.
    pub(in crate::runtime) fn route_key_up(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if matches!(ev.key, crate::Key::Media(_)) && !self.kbd.media_keys { return false; }
        if self.eat_ime_key_up(app) { return false; }
        tx.focused = self.focused_key();
        let pressed = self.kbd.route_key_release().and_then(|p| self.live_index(&p));
        let focused = self.kbd.focus.path().and_then(|p| self.live_index(p));
        let secure = [pressed, focused].into_iter().flatten()
            .any(|i| self.tree.nodes[i].input.as_ref().is_some_and(|input| input.edit.is_secure()));
        let mut chain = Vec::new();
        for start in [pressed, focused].into_iter().flatten() {
            let mut at = Some(start);
            while let Some(i) = at {
                if !self.allowed(i) { break; }
                if !chain.contains(&i) { chain.push(i); }
                at = self.tree.nodes[i].parent;
            }
        }
        // Nothing is focused, or focus rests on the open modal itself (where it lands on open):
        // every element that hears releases is offered this one, the deepest first, confined to
        // the modal, not only a handler on the root.
        let at_rest = chain.is_empty() || (self.active_modal().is_some() && focused == self.active_modal() && pressed.is_none_or(|p| Some(p) == focused));
        if at_rest {
            let rest: Vec<usize> = (0..self.tree.nodes.len()).rev().filter(|&i| !chain.contains(&i) && self.allowed(i) && self.tree.nodes[i].focus.key_up.is_some()).collect();
            chain.extend(rest);
        } else if pressed.is_some() && focused.is_some() && pressed != focused {
            // Keep shared ancestors (such as the root) after both focused elements.
            let mut shared = Vec::new();
            let mut at = focused;
            while let Some(i) = at {
                shared.push(i);
                at = self.tree.nodes[i].parent;
            }
            chain.sort_by_key(|i| shared.iter().position(|s| s == i).map_or(0, |p| p + 1));
        }
        for i in chain {
            if let Some(f) = self.tree.nodes[i].focus.key_up.clone() && Tx::took(f(app.states(), ev, tx), tx) {
                if secure { log_event!(self, "key-up [redacted]") } else { log_event!(self, "key-up {:?}", ev.key) }
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
#[path = "gesture_routing_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "gesture_tests.rs"]
mod gesture_tests;

#[cfg(test)]
#[path = "modal_wheel_tests.rs"]
mod modal_wheel_tests;
