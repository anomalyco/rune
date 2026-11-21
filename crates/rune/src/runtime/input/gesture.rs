//! Routes the canvas input of `canvas_input.rs`: wheel deltas to `on_wheel` before scrolling,
//! pinch, rotation and smart magnify, extra mouse buttons to `on_press` and `on_drag`, and key
//! releases to `on_key_up`.
use super::*;
use crate::input::canvas_input::CanvasInput;
use crate::machine::Attempt;
use crate::runtime::scroll_x::WheelOwner;
use crate::{GesturePhase, MagnifyEvent, PointerButton, RotateEvent, WheelEvent};

/// A trackpad gesture at a window point, queued for the handlers under it.
#[derive(Clone, Copy, Debug)]
pub(in crate::runtime) enum Gesture {
    Magnify(MagnifyEvent),
    Rotate(RotateEvent),
    SmartMagnify(f32, f32),
}

/// Where a wheel delta's chain starts (`Runtime::chain_start`), when it was already found on the
/// way to scrolling, so one delta looks it up once.
#[derive(Clone, Copy, Debug)]
pub(crate) enum WheelStart {
    Unknown,
    At(Option<usize>),
}

/// Points of Ctrl + wheel per unit of magnification, where a pinch arrives that way.
const PINCH_WHEEL: f32 = 200.0;

impl RuntimeCore {
    /// A wheel or trackpad scroll at window point (`ev.x`, `ev.y`), with its modifiers and
    /// phase. The innermost [`El::on_wheel`] handler there hears it first, up to the nearest
    /// scroll view; what none takes scrolls as [`Self::wheel_xy`] does.
    pub fn wheel_event(&mut self, ev: WheelEvent) {
        if ev.phase == GesturePhase::Began { self.wheel_began(); }
        let moved = ev.dx != 0.0 || ev.dy != 0.0;
        if moved {
            self.dismiss_help();
            let now = self.now_clock();
            let pending = self.wheel_pending();
            self.scroll.wheel_delta(now, ev.dx, ev.dy, pending);
        }
        // A notch or not is the run's to decide, not each delta's.
        let ev = if moved { WheelEvent { notch: self.scroll.wheel_notched(self.now_clock(), ev.notch), ..ev } } else { ev };
        if moved || ev.phase != GesturePhase::Moved {
            let ev = WheelEvent { momentum: self.scroll.wheel_momentum(), ..ev };
            let gesture = self.scroll.wheel_gesture();
            let phased = self.scroll.wheel_phased();
            if let Some(start) = self.offer_wheel(gesture, ev) { self.wheel_scroll_for(gesture, ev, phased, start); }
        }
        if ev.phase == GesturePhase::Ended { self.wheel_ended(); }
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
        let chain = match start { WheelStart::At(s) => self.gesture_chain_from(s, |g| g.wheel.is_some(), true), WheelStart::Unknown => Vec::new() };
        if chain.is_empty() && !handled && !pending { return Some(start); }
        let phased = self.scroll.wheel_phased();
        self.frame.queue.push(Action::Wheel { gesture, chain, ev, phased });
        None
    }

    /// A queued wheel delta: the handler holding its gesture hears it, else each handler
    /// under the pointer until one takes it, else it scrolls. Each queued delta keeps its own
    /// gesture's turn and axis policy, so one queued before a newer gesture began still reaches
    /// the owner its own gesture picked.
    pub(in crate::runtime) fn run_wheel(&mut self, app: &mut dyn AppDriver, gesture: Attempt, chain: Vec<String>, ev: WheelEvent, phased: bool, tx: &mut Tx) -> bool {
        self.scroll.sync_wheel_gesture(gesture);
        let mut taken = None;
        match self.scroll.wheel_owner_of(gesture).cloned() {
            Some(WheelOwner::Handler(path)) => {
                taken = self.call_wheel(app, &path, ev, tx);
                if taken.is_none() { self.scroll.set_wheel_owner(gesture, WheelOwner::Unpicked); }
            }
            Some(WheelOwner::Unpicked) => {
                for path in chain {
                    if self.call_wheel(app, &path, ev, tx) == Some(true) {
                        self.scroll.set_wheel_owner(gesture, WheelOwner::Handler(path));
                        taken = Some(true);
                        break;
                    }
                }
            }
            _ => {}
        }
        if taken.is_none() {
            // Queued from an earlier frame: what lies under the pointer may have moved since.
            self.wheel_scroll_for(gesture, ev, phased, WheelStart::Unknown);
        }
        taken.unwrap_or(false)
    }

    /// Element `path`'s `on_wheel` on `ev`, in its own points; None when it's gone.
    fn call_wheel(&mut self, app: &mut dyn AppDriver, path: &str, ev: WheelEvent, tx: &mut Tx) -> Option<bool> {
        let (i, f) = self.gesture_handler(path, |g| g.wheel.clone())?;
        let [x, y] = self.to_local(i, ev.x, ev.y);
        Some(Tx::took(f(app.states(), WheelEvent { x, y, ..ev }, tx), tx))
    }

    /// A trackpad pinch at window point (`ev.x`, `ev.y`) for the [`El::on_magnify`] handlers
    /// there, innermost first.
    pub fn magnify_event(&mut self, ev: MagnifyEvent) {
        self.offer_gesture(ev.x, ev.y, |g| g.magnify.is_some(), Gesture::Magnify(ev));
    }

    /// A two-finger rotation at window point (`ev.x`, `ev.y`) for the [`El::on_rotate`]
    /// handlers there, innermost first.
    pub fn rotate_event(&mut self, ev: RotateEvent) {
        self.offer_gesture(ev.x, ev.y, |g| g.rotate.is_some(), Gesture::Rotate(ev));
    }

    /// A two-finger double tap at window point (x, y) for the [`El::on_smart_magnify`]
    /// handlers there, innermost first.
    pub fn smart_magnify(&mut self, x: f32, y: f32) {
        self.offer_gesture(x, y, |g| g.smart_magnify.is_some(), Gesture::SmartMagnify(x, y));
    }

    /// A Ctrl + wheel `dy` at (x, y), which is how a trackpad pinch reaches Windows, Linux and
    /// web apps: a pinch when an `on_magnify` handler is there (true), else nothing.
    pub(crate) fn pinch_wheel(&mut self, x: f32, y: f32, dy: f32) -> bool {
        if dy == 0.0 || self.gesture_chain(x, y, |g| g.magnify.is_some(), false).is_empty() { return false; }
        self.magnify_event(MagnifyEvent::new(-dy / PINCH_WHEEL).at(x, y));
        true
    }

    fn offer_gesture(&mut self, x: f32, y: f32, has: impl Fn(&CanvasInput) -> bool, gesture: Gesture) {
        let chain = self.gesture_chain(x, y, has, false);
        if !chain.is_empty() { self.frame.queue.push(Action::Gesture(chain, gesture)); }
    }

    pub(in crate::runtime) fn run_gesture(&mut self, app: &mut dyn AppDriver, chain: Vec<String>, gesture: Gesture, tx: &mut Tx) -> bool {
        for path in chain {
            let taken = match gesture {
                Gesture::Magnify(ev) => self.gesture_handler(&path, |g| g.magnify.clone()).map(|(i, f)| {
                    let [x, y] = self.to_local(i, ev.x, ev.y);
                    Tx::took(f(app.states(), MagnifyEvent { x, y, ..ev }, tx), tx)
                }),
                Gesture::Rotate(ev) => self.gesture_handler(&path, |g| g.rotate.clone()).map(|(i, f)| {
                    let [x, y] = self.to_local(i, ev.x, ev.y);
                    Tx::took(f(app.states(), RotateEvent { x, y, ..ev }, tx), tx)
                }),
                Gesture::SmartMagnify(x, y) => self.gesture_handler(&path, |g| g.smart_magnify.clone()).map(|(i, f)| {
                    let [x, y] = self.to_local(i, x, y);
                    Tx::took(f(app.states(), x, y, tx), tx)
                }),
            };
            if taken == Some(true) { return true; }
        }
        false
    }

    /// The elements under window point (x, y) with a handler `has` picks, innermost first: up
    /// from the topmost hit, and from a scroll view there inside it. With `stop_at_scroll`, the
    /// walk ends at the first scroll view, which scrolls what its handlers leave.
    fn gesture_chain(&self, x: f32, y: f32, has: impl Fn(&CanvasInput) -> bool, stop_at_scroll: bool) -> Vec<String> {
        self.gesture_chain_from(self.chain_start(x, y), has, stop_at_scroll)
    }

    /// [`Self::gesture_chain`] from a start already found.
    fn gesture_chain_from(&self, start: Option<usize>, has: impl Fn(&CanvasInput) -> bool, stop_at_scroll: bool) -> Vec<String> {
        let mut chain = Vec::new();
        let mut at = start;
        while let Some(i) = at {
            if self.subtree_exiting(i) || !self.allowed(i) { break; }
            let n = &self.tree.nodes[i];
            if n.handlers.gestures.as_deref().is_some_and(&has) { chain.push(n.path.clone()); }
            if stop_at_scroll && n.scroll.is_some() { break; }
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
        match button {
            PointerButton::Primary => return self.pointer_down(app, x, y),
            PointerButton::Secondary => return self.secondary_down(x, y),
            _ => {}
        }
        if self.pointer.autoscroll_press(button) { self.pointer.pos = Some((x, y)); return; }
        if self.pointer.press.is_held() { return; }
        self.pointer.pos = Some((x, y));
        if button == PointerButton::Middle { self.tint_down(x, y, button); }
        // On Linux a middle click in a text field pastes the primary selection there.
        #[cfg(target_os = "linux")]
        if button == PointerButton::Middle && self.primary_paste_down(x, y) { return; }
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
            if middle { self.autoscroll_down(x, y); }
            return;
        };
        let n = &self.tree.nodes[i];
        let r = n.target;
        let [lx, ly] = self.to_local(i, x, y);
        let d = Drag { phase: Phase::Down, x: lx, y: ly, w: r[2] - r[0], h: r[3] - r[1], start_x: lx, start_y: ly, dx: 0.0, dy: 0.0, data: 0.0, clicks: 1, cancelled: false, window: [x, y], screen: self.screen_at(x, y) };
        let path = n.path.clone();
        if self.drags_with(i, button) { self.frame.queue.push(Action::Drag(path.clone(), d, button)); }
        self.pointer.clear_press_clicks();
        let now = self.now_clock();
        self.pointer.press.hold(capture::Held::new(path, d, button, (x, y), now, false, None).on_link(link));
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
        self.pointer.release_tint(button);
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
