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
pub(super) enum Gesture {
    Magnify(MagnifyEvent),
    Rotate(RotateEvent),
    SmartMagnify(f32, f32),
}

/// Points of Ctrl + wheel per unit of magnification, where a pinch arrives that way.
const PINCH_WHEEL: f32 = 200.0;

impl<S: 'static> Runtime<S> {
    /// A wheel or trackpad scroll at window point (`ev.x`, `ev.y`), with its modifiers and
    /// phase. The innermost [`El::on_wheel`] handler there hears it first, up to the nearest
    /// scroll view; what none takes scrolls as [`Self::wheel_xy`] does.
    pub fn wheel_event(&mut self, ev: WheelEvent) {
        if ev.phase == GesturePhase::Began { self.wheel_began(); }
        let moved = ev.dx != 0.0 || ev.dy != 0.0;
        if moved {
            self.dismiss_help();
            self.wheel_delta_arrived(ev.dx, ev.dy);
        }
        if moved || ev.phase != GesturePhase::Moved {
            let ev = WheelEvent { momentum: self.wheel_lock.momentum(), ..ev };
            let gesture = self.wheel_lock.gesture;
            let phased = self.wheel_lock.phased();
            if !self.offer_wheel(gesture, ev) { self.wheel_scroll_for(gesture, ev.x, ev.y, ev.dx, ev.dy, phased); }
        }
        if ev.phase == GesturePhase::Ended { self.wheel_ended(); }
    }

    /// Queues `ev` for the `on_wheel` handlers it may go to; false leaves it to scrolling now.
    fn offer_wheel(&mut self, gesture: Attempt, ev: WheelEvent) -> bool {
        let lock = &self.wheel_lock;
        let same_gesture = lock.owner_is_arriving();
        // A delta queued this frame may yet be taken: later ones wait their turn behind it.
        let pending = self.wheel_queued(Some(gesture));
        let (scrolls, handled) = match lock.owner() {
            WheelOwner::Vertical | WheelOwner::Horizontal(_) => (same_gesture, false),
            WheelOwner::Handler(_) => (false, same_gesture),
            WheelOwner::Unpicked => (false, false),
        };
        if scrolls && !pending { return false; }
        let chain = if handled { Vec::new() } else { self.gesture_chain(ev.x, ev.y, |g| g.wheel.is_some(), true) };
        if chain.is_empty() && !handled && !pending { return false; }
        let phased = lock.phased();
        self.queue.push(Action::Wheel { gesture, chain, ev, phased });
        true
    }

    /// A queued wheel delta: the handler holding its gesture hears it, else each handler
    /// under the pointer until one takes it, else it scrolls. Each queued delta keeps its own
    /// gesture's turn and axis policy, so one queued before a newer gesture began still reaches
    /// the owner its own gesture picked.
    pub(super) fn run_wheel(&mut self, gesture: Attempt, chain: Vec<String>, ev: WheelEvent, phased: bool, tx: &mut Tx) -> bool {
        self.wheel_lock.sync_gesture(gesture);
        let mut taken = None;
        if let WheelOwner::Handler(path) = self.wheel_lock.owner().clone() {
            taken = self.call_wheel(&path, ev, tx);
            if taken.is_none() { self.wheel_lock.set_owner(WheelOwner::Unpicked); }
        } else if self.wheel_lock.owner() == &WheelOwner::Unpicked {
            for path in chain {
                if self.call_wheel(&path, ev, tx) == Some(true) {
                    self.wheel_lock.set_owner(WheelOwner::Handler(path));
                    taken = Some(true);
                    break;
                }
            }
        }
        if taken.is_none() {
            self.wheel_scroll_for(gesture, ev.x, ev.y, ev.dx, ev.dy, phased);
        }
        taken.unwrap_or(false)
    }

    /// Element `path`'s `on_wheel` on `ev`, in its own points; None when it's gone.
    fn call_wheel(&mut self, path: &str, ev: WheelEvent, tx: &mut Tx) -> Option<bool> {
        let (i, f) = self.gesture_handler(path, |g| g.wheel.clone())?;
        let [x, y] = self.to_local(i, ev.x, ev.y);
        Some(Tx::took(f(states!(self), WheelEvent { x, y, ..ev }, tx), tx))
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
        if !chain.is_empty() { self.queue.push(Action::Gesture(chain, gesture)); }
    }

    pub(super) fn run_gesture(&mut self, chain: Vec<String>, gesture: Gesture, tx: &mut Tx) -> bool {
        for path in chain {
            let taken = match gesture {
                Gesture::Magnify(ev) => self.gesture_handler(&path, |g| g.magnify.clone()).map(|(i, f)| {
                    let [x, y] = self.to_local(i, ev.x, ev.y);
                    Tx::took(f(states!(self), MagnifyEvent { x, y, ..ev }, tx), tx)
                }),
                Gesture::Rotate(ev) => self.gesture_handler(&path, |g| g.rotate.clone()).map(|(i, f)| {
                    let [x, y] = self.to_local(i, ev.x, ev.y);
                    Tx::took(f(states!(self), RotateEvent { x, y, ..ev }, tx), tx)
                }),
                Gesture::SmartMagnify(x, y) => self.gesture_handler(&path, |g| g.smart_magnify.clone()).map(|(i, f)| {
                    let [x, y] = self.to_local(i, x, y);
                    Tx::took(f(states!(self), x, y, tx), tx)
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
        let hit = self.hits.iter().rev().find(|&&(id, r)| self.hit_contains(id, r, x, y)).and_then(|(id, _)| self.node_index.get(id).copied());
        // A scroll view without handlers of its own isn't a hit: one inside the hit wins.
        let scroll = (0..self.nodes.len()).rev().find(|&i| self.nodes[i].scroll.is_some() && !self.subtree_exiting(i)
            && self.allowed(i) && self.node_contains(i, x, y));
        let inside = |s: usize, h: usize| std::iter::successors(self.nodes[s].parent, |&p| self.nodes[p].parent).any(|p| p == h);
        let start = match (hit, scroll) {
            (Some(h), Some(s)) if inside(s, h) => Some(s),
            (None, s) => s,
            (h, _) => h,
        };
        let mut chain = Vec::new();
        let mut at = start;
        while let Some(i) = at {
            if self.subtree_exiting(i) || !self.allowed(i) { break; }
            let n = &self.nodes[i];
            if n.handlers.gestures.as_deref().is_some_and(&has) { chain.push(n.path.clone()); }
            if stop_at_scroll && n.scroll.is_some() { break; }
            at = n.parent;
        }
        chain
    }

    /// Element `path` (not exiting or inert) and the handler `pick` takes from its canvas input.
    fn gesture_handler<F>(&self, path: &str, pick: impl Fn(&CanvasInput) -> Option<F>) -> Option<(usize, F)> {
        let i = self.nodes.iter().enumerate().position(|(i, n)| n.path == path && !self.subtree_exiting(i) && self.allowed(i))?;
        Some((i, pick(self.nodes[i].handlers.gestures.as_deref()?)?))
    }

    /// A mouse button went down at (x, y). The primary and secondary buttons do what
    /// [`Self::pointer_down`] and [`Self::secondary_down`] do. Another presses the innermost
    /// element there that takes it: one that opted in ([`El::pointer_buttons`]), whose `on_drag`
    /// and `on_press` follow as for the primary button; for the middle button, also a link of a
    /// text with `on_link`, or an element with [`El::on_middle_press`], each clicked when the
    /// button comes up over it.
    pub fn button_down(&mut self, x: f32, y: f32, button: PointerButton) {
        match button {
            PointerButton::Primary => return self.pointer_down(x, y),
            PointerButton::Secondary => return self.secondary_down(x, y),
            _ => {}
        }
        if self.autoscroll_press(button) { self.pointer = Some((x, y)); return; }
        if self.press.is_held() { return; }
        self.pointer = Some((x, y));
        self.dismiss_help();
        let hit = self.hits.iter().rev().find(|&&(id, r)| self.hit_contains(id, r, x, y)).and_then(|(id, _)| self.node_index.get(id).copied());
        let middle = button == PointerButton::Middle;
        let target = std::iter::successors(hit, |&i| self.nodes[i].parent)
            .take_while(|&i| !self.subtree_exiting(i) && self.allowed(i))
            .find_map(|i| {
                let h = &self.nodes[i].handlers;
                if self.takes_button(i, button) && (h.on_press.is_some() || h.on_drag.is_some()) { return Some((i, None)); }
                if !middle { return None; }
                if let Some(k) = self.middle_link_at(i, x, y) { return Some((i, Some(k))); }
                h.on_middle_press.is_some().then_some((i, None))
            });
        let Some((i, link)) = target else {
            if middle { self.autoscroll_down(x, y); }
            return;
        };
        let n = &self.nodes[i];
        let r = n.target;
        let [lx, ly] = self.to_local(i, x, y);
        let d = Drag { phase: Phase::Down, x: lx, y: ly, w: r[2] - r[0], h: r[3] - r[1], start_x: lx, start_y: ly, dx: 0.0, dy: 0.0, data: 0.0, clicks: 1, cancelled: false, window: [x, y], screen: self.screen_at(x, y) };
        let path = n.path.clone();
        if self.drags_with(i, button) { self.queue.push(Action::Drag(path.clone(), d, button)); }
        self.press_clicks.clear();
        let now = self.now_clock();
        self.press.hold(capture::Held::new(path, d, button, (x, y), now, false, None).on_link(link));
    }

    /// Element `i` opted in to `button` ([`El::pointer_buttons`]); every element takes the
    /// primary.
    pub(super) fn takes_button(&self, i: usize, button: PointerButton) -> bool {
        button == PointerButton::Primary || self.nodes[i].handlers.gestures.as_ref().is_some_and(|g| g.buttons & button.bit() != 0)
    }

    /// Element `i`'s `on_drag` hears a press of `button`.
    pub(super) fn drags_with(&self, i: usize, button: PointerButton) -> bool {
        let n = &self.nodes[i];
        n.handlers.on_drag.is_some() && !self.drags_out(n) && self.takes_button(i, button)
    }

    /// The link of text `i` under (x, y) that a middle click opens: one its `on_link` hears.
    fn middle_link_at(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        self.nodes[i].text.as_ref()?.cold.on_link.as_ref()?;
        self.link_at(i, x, y)
    }

    /// A mouse button came up at (x, y): the release of [`Self::button_down`].
    pub fn button_up(&mut self, x: f32, y: f32, button: PointerButton) {
        if !matches!(button, PointerButton::Primary | PointerButton::Secondary) && self.autoscroll_release(button) { self.pointer = Some((x, y)); return; }
        match button {
            PointerButton::Primary => self.pointer_up(x, y),
            PointerButton::Secondary => self.secondary_up(x, y),
            _ if self.press.held().is_some_and(|h| h.button == button) => self.release_capture(x, y),
            _ => {}
        }
    }

    /// A key came up, for the [`El::on_key_up`] handlers: true when one is there to hear it,
    /// and a frame is needed. A host with physical keys passes the release's logical key to
    /// `physical_then` instead, so the focused `on_physical_input` hears it first.
    pub fn key_up(&mut self, ev: KeyEvent) -> bool {
        if !self.wants_key_up() { return false; }
        if self.queue.is_empty() && !self.dirty && self.eat_ime_key_up() { return false; }
        self.queue.push(Action::KeyUp(ev));
        true
    }

    /// Some reachable element has an `on_wheel` handler.
    pub(crate) fn hears_wheel(&self) -> bool {
        self.nodes.iter().enumerate().any(|(i, n)| n.handlers.get().is_some_and(|h| h.gestures.as_ref().is_some_and(|g| g.wheel.is_some())) && self.allowed(i))
    }

    /// Some element hears key releases, so a host maps them to logical keys.
    pub(crate) fn wants_key_up(&self) -> bool {
        self.has_key_up || !self.queue.is_empty() || self.dirty
    }

    /// Records a key press for [`Self::route_key_up`]: whether it belongs to an open IME
    /// composition, or which focused element should still hear its release if focus moves.
    pub(super) fn note_key_down(&mut self, repeat: bool) {
        let composing = self.composing();
        let focus = &self.focus;
        self.key_releases.down(composing, repeat, || focus.owned());
    }

    /// Suppresses key releases that belong to an IME composition (while preedit is open, or
    /// the release of the key that confirmed or cancelled it).
    pub(super) fn eat_ime_key_up(&mut self) -> bool {
        let composing = self.composing();
        self.key_releases.eat(composing)
    }

    /// A key release goes to the focused element and up through its ancestors, else to every
    /// element with a handler (deepest first), inside the open modal if there is one. If focus moved since the key went down, the element that had
    /// focus on the press (and its ancestors) hears the release first.
    pub(super) fn route_key_up(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.eat_ime_key_up() { return false; }
        tx.focused = self.focused_key();
        let pressed = self.key_releases.route().and_then(|p| self.live_index(&p));
        let focused = self.focus.path().and_then(|p| self.live_index(p));
        let secure = [pressed, focused].into_iter().flatten()
            .any(|i| self.nodes[i].input.as_ref().is_some_and(|input| input.edit.is_secure()));
        let mut chain = Vec::new();
        for start in [pressed, focused].into_iter().flatten() {
            let mut at = Some(start);
            while let Some(i) = at {
                if !self.allowed(i) { break; }
                if !chain.contains(&i) { chain.push(i); }
                at = self.nodes[i].parent;
            }
        }
        // Nothing is focused, or focus rests on the open modal itself (where it lands on open):
        // every element that hears releases is offered this one, the deepest first, confined to
        // the modal, not only a handler on the root.
        let at_rest = chain.is_empty() || (self.active_modal().is_some() && focused == self.active_modal() && pressed.is_none_or(|p| Some(p) == focused));
        if at_rest {
            let rest: Vec<usize> = (0..self.nodes.len()).rev().filter(|&i| !chain.contains(&i) && self.allowed(i) && self.nodes[i].focus.key_up.is_some()).collect();
            chain.extend(rest);
        } else if pressed.is_some() && focused.is_some() && pressed != focused {
            // Keep shared ancestors (such as the root) after both focused elements.
            let mut shared = Vec::new();
            let mut at = focused;
            while let Some(i) = at {
                shared.push(i);
                at = self.nodes[i].parent;
            }
            chain.sort_by_key(|i| shared.iter().position(|s| s == i).map_or(0, |p| p + 1));
        }
        for i in chain {
            if let Some(f) = self.nodes[i].focus.key_up.clone() && Tx::took(f(states!(self), ev, tx), tx) {
                if secure { log_event!(self, "key-up [redacted]") } else { log_event!(self, "key-up {:?}", ev.key) }
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Env, Mods, PhysicalInput, PhysicalKeyCode, el};

    #[derive(Default)]
    struct State { ups: Vec<crate::Key>, swallow: bool, zoom: Vec<f32> }

    fn view(_: &State, _: &mut crate::Cx<State>) -> El {
        el().w_full().h_full()
            .on_key_up(|s: &mut State, e, _| { s.ups.push(e.key.clone()); true })
            .on_magnify(|s: &mut State, e, _| { s.zoom.push(e.delta); true })
            .child(el().key("field").size(40.0, 40.0).focusable(true).on_physical_input(|s: &mut State, _, _| s.swallow))
    }

    #[test]
    fn a_physical_release_carries_its_logical_key_to_on_key_up_after_the_focused_handler() {
        let mut rt = Runtime::new(State::default(), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.wants_key_up());
        let release = PhysicalInput::Key { key: PhysicalKeyCode::Space, mods: Mods::default(), pressed: false, repeat: false };
        let space = KeyEvent { key: crate::Key::Text(" ".into()), mods: Mods::default() };
        assert!(rt.physical_with_fallback(release, Some(space.clone())));
        rt.frame(0.0);
        assert_eq!(rt.state.ups, [space.key.clone()]);

        // A focused `on_physical_input` that takes the release keeps it from `on_key_up`.
        let (x, y) = rt.find("field").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.state.swallow = true;
        rt.physical_with_fallback(release, Some(space.clone()));
        rt.frame(0.0);
        assert_eq!(rt.state.ups.len(), 1);
        rt.state.swallow = false;
        rt.physical_with_fallback(release, Some(space));
        rt.frame(0.0);
        assert_eq!(rt.state.ups.len(), 2);
    }

    #[derive(Default)]
    struct Keys { log: Vec<&'static str> }

    fn stage(s: &Keys, _: &mut crate::Cx<Keys>) -> El {
        let _ = s;
        el().w_full().h_full()
            .child(el().key("stage").size(40.0, 40.0).on_key_up(|s: &mut Keys, _, _| { s.log.push("stage"); true }))
            .child(el().key("other").size(40.0, 40.0).focusable(true))
    }

    fn up() -> KeyEvent { KeyEvent { key: crate::Key::Text("a".into()), mods: Mods::default() } }

    #[test]
    fn an_unfocused_release_reaches_a_nested_handler() {
        let mut rt = Runtime::new(Keys::default(), stage, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.wants_key_up());
        rt.key_up(up());
        rt.frame(0.0);
        assert_eq!(rt.state.log, ["stage"]);
    }

    fn pair(_: &Keys, _: &mut crate::Cx<Keys>) -> El {
        el().w_full().h_full()
            .child(el().key("a").size(40.0, 40.0).focusable(true).on_key_up(|s: &mut Keys, _, _| { s.log.push("a"); true }))
            .child(el().key("b").size(40.0, 40.0).focusable(true).on_key_up(|s: &mut Keys, _, _| { s.log.push("b"); true }))
    }

    #[test]
    fn a_release_goes_to_the_element_focused_at_the_press_even_after_focus_moves() {
        let mut rt = Runtime::new(Keys::default(), pair, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let click = |rt: &mut Runtime<Keys>, k: &str| { let (x, y) = rt.find(k).unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0); };
        click(&mut rt, "a");
        rt.key(up());
        rt.frame(0.0);
        click(&mut rt, "b");
        rt.key_up(up());
        rt.frame(0.0);
        assert_eq!(rt.state.log, ["a"]);
    }

    fn modal_view(_: &Keys, _: &mut crate::Cx<Keys>) -> El {
        el().w_full().h_full()
            .on_key_up(|s: &mut Keys, _, _| { s.log.push("root"); true })
            .child(el().key("under").size(40.0, 40.0).on_key_up(|s: &mut Keys, _, _| { s.log.push("under"); true }))
            .child(el().key("sheet").absolute(50.0, 50.0).size(40.0, 40.0).modal(true, |_: &mut Keys| {})
                .child(el().key("inside").size(10.0, 10.0).on_key_up(|s: &mut Keys, _, _| { s.log.push("inside"); true })))
    }

    #[test]
    fn an_unfocused_release_in_a_modal_stays_inside_it() {
        let mut rt = Runtime::new(Keys::default(), modal_view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.key_up(up());
        rt.frame(0.0);
        assert_eq!(rt.state.log, ["inside"]);
    }

    #[test]
    fn a_ctrl_wheel_pinches_only_where_a_magnify_handler_is() {
        let mut rt = Runtime::new(State::default(), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.pinch_wheel(50.0, 50.0, -20.0));
        rt.frame(0.0);
        assert_eq!(rt.state.zoom, [20.0 / PINCH_WHEEL]);
        assert!(!rt.pinch_wheel(500.0, 50.0, -20.0), "off the window nothing takes it");
    }

    #[derive(Default)]
    struct Wheeled { heard: Vec<f32> }

    fn wheeling(_: &Wheeled, _: &mut crate::Cx<Wheeled>) -> El {
        el().w_full().h_full()
            .child(el().key("list").w_full().h(300.0).scroll(crate::ScrollSpec { stick_bottom: false })
                .child(el().w_full().h(1000.0)
                    .child(el().key("takes").absolute(100.0, 40.0).size(40.0, 40.0)
                        .on_wheel(|s: &mut Wheeled, e: crate::WheelEvent, _: &mut Tx| { s.heard.push(e.x); true }))))
    }

    /// Deltas queued before the platform began the next gesture finish with the owner the
    /// gesture picked: the one outside the handler still reaches it, rather than scrolling the
    /// viewport under the pointer.
    #[test]
    fn deltas_queued_before_a_new_gesture_began_finish_with_the_gesture_s_owner() {
        let mut rt = Runtime::new(Wheeled::default(), wheeling, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(110.0, 50.0));
        rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(290.0, 50.0));
        rt.wheel_began();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.heard, [10.0, 190.0], "both deltas in the handler's own points");
        let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
        assert_eq!(rt.scrolls[&id].offset, 0.0, "the whole gesture stays with the handler");
    }

    /// A delta queued over the handler, before it has the gesture, still reaches it when a newer
    /// gesture scrolls at once outside every handler: the newer one's own delta scrolls once.
    #[test]
    fn a_queued_delta_picks_its_handler_before_a_newer_gesture_s_scroll() {
        let mut rt = Runtime::new(Wheeled::default(), wheeling, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(110.0, 50.0));
        rt.wheel_began();
        rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(290.0, 50.0));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.heard, [10.0], "the queued delta reaches the handler it was offered to");
        let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
        assert_eq!(rt.scrolls[&id].offset, 10.0, "only the newer gesture's own delta scrolled");
    }

    #[derive(Default)]
    struct Two { heard: Vec<&'static str> }

    fn two_handlers(_: &Two, _: &mut crate::Cx<Two>) -> El {
        el().w_full().h_full()
            .child(el().key("list").w_full().h(300.0).scroll(crate::ScrollSpec { stick_bottom: false })
                .child(el().w_full().h(1000.0)
                    .child(el().key("a").absolute(20.0, 40.0).size(40.0, 40.0)
                        .on_wheel(|s: &mut Two, _: crate::WheelEvent, _: &mut Tx| { s.heard.push("a"); true }))
                    .child(el().key("b").absolute(200.0, 40.0).size(40.0, 40.0)
                        .on_wheel(|s: &mut Two, _: crate::WheelEvent, _: &mut Tx| { s.heard.push("b"); true }))))
    }

    /// With a delta queued for a handler under one gesture and another under the next, each
    /// reaches its own gesture's handler, however many gestures began before the frame.
    #[test]
    fn a_delta_queued_for_each_of_two_gestures_reaches_its_own_handler() {
        let mut rt = Runtime::new(Two::default(), two_handlers, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(30.0, 50.0));
        rt.wheel_began();
        rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(210.0, 50.0));
        rt.wheel_began();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.heard, ["a", "b"], "each delta goes to the handler its own gesture found");
    }
}
