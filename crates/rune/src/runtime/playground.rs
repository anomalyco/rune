//! The runtime's side of [`App::playground`](crate::App::playground): its keys, its requests,
//! and what a new stage leaves behind.

use super::*;
use crate::playground::{FILTER, Moved, Nav, Region, Request};

impl RuntimeCore {
    /// The node the app's view returned: the first, or with the playground attached, the one
    /// above its bar or on its stage. Root handlers (window focus, paste, file drops) are there.
    #[inline]
    pub(super) fn app_root(&self) -> usize {
        self.tree.app_root()
    }

    #[inline]
    pub(super) fn find_app_root(&mut self) {
        self.tree.find_app_root();
    }

    /// Moves a Tx's playground requests to the runtime, for the next commit to apply before its
    /// view.
    pub(super) fn latch_playground(&mut self, app: &mut dyn AppDriver, tx: &mut Tx) {
        let requests = std::mem::take(&mut tx.playground);
        if requests.is_empty() { return; }
        // The playground swaps whole views: no fades between them.
        tx.commit = tx.commit.instant();
        let Some(playground) = app.playground_mut() else { return };
        playground.request(requests);
        self.commit.dirty = true;
    }

    /// Applies queued playground requests. Focus leaves the old stage first, while the states
    /// its fields were built for are still there; a replay keeps it in the side column.
    pub(super) fn apply_playground(&mut self, app: &mut dyn AppDriver) {
        self.sync_region(app);
        let (has_pending, swaps, replays_only, was_open) = {
            let Some(playground) = app.playground() else { return };
            (playground.has_pending(), playground.pending_swaps_stage(), playground.pending_replays_only(), playground.is_open())
        };
        if !has_pending { return; }
        let keep = replays_only && self.focused_index().is_some_and(|i| self.tree.under(i, |k| k == "playground-side"));
        if swaps && !keep && self.kbd.focus.path().is_some() { self.set_focus(app, None, false); }
        let clock = self.clock;
        if !app.playground_apply(clock) { return; }
        self.commit.clear_memo();
        if swaps { self.leave_stage(app, was_open); }
        // A new stage state gets its first poll on the next frame, as an app's does.
        if app.playground().is_some_and(|p| p.polls()) { self.schedule_wakes([std::time::Duration::ZERO]); }
    }

    /// The old stage's elements, and the pointer's hold on them, go with its state: their
    /// handlers were built for it. Its exits end now, nothing it held hears a release or a
    /// hover ending, and the time scale it set goes back to the app's.
    fn leave_stage(&mut self, app: &mut dyn AppDriver, was_open: bool) {
        let exiting: Vec<_> = self.tree.nodes.iter().map(|n| n.id).filter(|&id| self.is_exiting(id)).collect();
        for id in exiting { self.scene.unmount(id); }
        self.drop_drag_out_request();
        self.select_step(selectable::phase::Event::Release);
        self.edit.clear_input_click();
        self.pointer.clear_for_stage();
        self.host.clear_titlebar();
        let Some(playground) = app.playground_mut() else { return };
        if was_open { self.time_scale = playground.app_time_scale(); } else { playground.set_app_time_scale(self.time_scale); }
    }

    /// Whether node `i` or an ancestor has a key `is` takes.
    #[inline]
    pub(super) fn under(&self, i: usize, is: impl Fn(&str) -> bool) -> bool {
        self.tree.under(i, is)
    }

    /// Keys nothing else took: Escape gives the keyboard back to the list from the stage, else
    /// goes Back.
    pub(super) fn playground_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
        let plain = !ev.mods.cmd && !ev.mods.alt && !ev.mods.ctrl && !ev.mods.shift;
        if ev.key != crate::Key::Escape || !plain || !app.playground().is_some_and(|p| p.is_open()) { return false; }
        self.sync_region(app);
        if app.playground().is_some_and(|p| p.region() != Region::List) { self.to_list(app); } else { one.close_playground(); }
        true
    }

    /// The playground's own keys, ahead of the stage's `on_key` and focus traversal: its
    /// shortcut, and while it's open, the keys of the column that has the keyboard (see
    /// [`Region`]). A focused text field keeps its keys, and the stage all of its own.
    pub(super) fn playground_nav(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
        let Some(shortcut) = app.playground().map(|p| p.is_shortcut(ev)) else { return false };
        if shortcut {
            if !one.key_repeat { one.toggle_playground(); }
            return true;
        }
        // Ahead of fields too: route_key doesn't offer it again.
        if app.playground_typing() && app.has_on_key() && Tx::took(app.on_key(ev, one), one) { return true; }
        if !app.playground().is_some_and(|p| p.is_open()) { return false; }
        self.sync_region(app);
        if self.focused_input().is_some() { return self.filter_key(app, ev, one); }
        if self.on_side_keys() {
            if matches!(ev.key, crate::Key::Escape | crate::Key::Tab) {
                self.to_list(app);
                return true;
            }
            if app.playground_mut().is_some_and(|p| p.side_key(ev, one)) { return true; }
        }
        let nav = match app.playground() {
            Some(playground) => playground.nav(ev, playground.region()),
            None => return false,
        };
        let side = self.playground_side();
        let at = self.kbd.focus.path().filter(|_| !self.kbd.focus.hovered()).and_then(|f| side.iter().position(|p| p == f));
        match nav {
            Nav::Pass => false,
            Nav::Stay => true,
            Nav::Request(request) => {
                let trigger = matches!(request, Request::Trigger(_) | Request::Replay);
                if !(trigger && one.key_repeat) { one.playground.push(request); }
                true
            }
            Nav::IntoSide => {
                if let Some(first) = side.first() { self.set_focus(app, Some(first.clone()), true); }
                self.give_region(app, Region::Side);
                true
            }
            Nav::IntoStage => {
                self.set_focus(app, None, true);
                self.give_region(app, Region::Stage);
                true
            }
            Nav::Filter => {
                let Some(path) = self.tree.key_path(FILTER) else { return false };
                self.set_focus(app, Some(path), true);
                true
            }
            Nav::OutOfSide => {
                self.to_list(app);
                true
            }
            Nav::SideStep(_) if side.is_empty() => true,
            Nav::SideStep(by) => {
                let n = side.len() as isize;
                let to = match at {
                    Some(at) => (at as isize + by).rem_euclid(n),
                    None if by > 0 => 0,
                    None => n - 1,
                };
                let path = side[to as usize].clone();
                self.set_focus(app, Some(path.clone()), true);
                one.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, path));
                true
            }
        }
    }

    /// Keys in a focused text field. In the list's filter, ↓ goes down into the matches, onto
    /// the first; Return goes to the list on the first match, if the selection isn't one; and
    /// Esc empties it, then goes to the list. Every other key edits it, and every other field
    /// keeps all its keys.
    fn filter_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
        use crate::Key::*;
        let plain = !ev.mods.alt && !ev.mods.ctrl && !ev.mods.cmd && !ev.mods.shift;
        if self.focused_index().and_then(|i| self.tree.nodes[i].key.as_deref()) != Some(FILTER) || !plain { return false; }
        let Some(playground) = app.playground_mut() else { return false };
        let to = match ev.key {
            Escape if playground.clear_filter() => {
                self.commit.dirty = true;
                return true;
            }
            Down => playground.first_shown(),
            Enter => playground.first_match(),
            Escape => None,
            _ => return false,
        };
        if let Some(i) = to { one.playground.push(Request::Select(i)); }
        self.set_focus(app, None, true);
        true
    }

    /// The list takes the keyboard back: focus leaves the stage or the side column.
    fn to_list(&mut self, app: &mut dyn AppDriver) {
        self.set_focus(app, None, true);
        self.give_region(app, Region::List);
    }

    /// Gives `region` the keyboard, redrawing the columns' focus.
    fn give_region(&mut self, app: &mut dyn AppDriver, region: Region) {
        let focus = self.kbd.focus.owned();
        let Some(playground) = app.playground_mut() else { return };
        if playground.moved(Moved::Give { region, seen: focus }) { self.commit.dirty = true; }
    }

    /// A press in the open playground gives its column the keyboard.
    pub(super) fn playground_pointer_down(&mut self, app: &mut dyn AppDriver, x: f32) {
        let width = self.viewport.0;
        let Some(region) = app.playground().filter(|p| p.is_open()).map(|p| p.region_at(x, width)) else { return };
        self.give_region(app, region);
    }

    /// Focus that moved into a column since the last look gives that column the keyboard: a
    /// field focused in the stage, Tab into the side column. Focus a hover moved (a menu-like
    /// row under the pointer) is passed over, and so is focus leaving: the column keeps it.
    fn sync_region(&mut self, app: &mut dyn AppDriver) {
        if app.playground().is_none_or(|p| p.saw_focus(self.kbd.focus.path())) { return; }
        let at = if self.kbd.focus.hovered() { None } else { self.focused_index() };
        let region = at.map(|i| {
            if self.tree.under(i, |k| k == "playground-side") {
                Region::Side
            } else if self.tree.under(i, crate::playground::is_stage) {
                Region::Stage
            } else {
                Region::List
            }
        });
        let focus = self.kbd.focus.owned();
        let Some(playground) = app.playground_mut() else { return };
        if playground.moved(Moved::Focus { focus, region }) { self.commit.dirty = true; }
    }

    /// Polls the open stage, then queues the keys its steps pressed ([`Play::press`]) behind
    /// the input already waiting, so they route on this frame as a person's keys would.
    ///
    /// [`Play::press`]: crate::playground::Play::press
    pub(super) fn poll_playground(&mut self, app: &mut dyn AppDriver, clock: f64, one: &mut Tx) -> bool {
        let Some(playground) = app.playground_mut() else { return false };
        let ran = playground.poll_stage(clock, one);
        let pressed = playground.take_pressed();
        for ev in pressed {
            self.trace_event(format_args!("press {:?}", ev.key));
            self.queue_key(ev, true);
        }
        ran
    }

    /// Routes a key a step pressed as [`Runtime::route_key`] routes a person's, with the stage
    /// holding the keyboard. Focus outside the stage (the list's filter, a side control) is set
    /// aside for it, and the keyboard goes back where the person left it.
    pub(super) fn route_stage_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
        let Some(region) = app.playground().filter(|p| p.is_open()).map(|p| p.region()) else { return self.route_key(app, ev, false, one) };
        let aside = self.focused_index().filter(|&i| !self.tree.under(i, crate::playground::is_stage)).and_then(|_| self.kbd.focus.set_aside());
        if let Some(p) = app.playground_mut() {
            p.moved(Moved::Hold(Region::Stage));
            p.set_pressing(true);
        }
        let took = self.route_key(app, ev, false, one);
        if let Some(p) = app.playground_mut() { p.set_pressing(false); }
        // Focus goes back only if the key left it where it was: otherwise what it focused stays,
        // and the next look at focus gives that column the keyboard.
        let back = aside.filter(|_| self.kbd.focus.path().is_none());
        let path = back.as_ref().and_then(|aside| match aside {
            focus_machine::Focus::On { path, .. } => Some(path.clone()),
            _ => None,
        });
        let Some(p) = app.playground_mut() else { return took };
        p.moved(Moved::Resume { region, back: path });
        if let Some(aside) = back {
            self.kbd.focus.put_back(aside);
            // A key that blurred it reported the slot off; it's back.
            self.sync_side_focus(app);
        }
        took
    }

    /// The keyboard is on the open story's side controls that take keys (`Story::side_keys`),
    /// whose slot is focusable only then.
    fn on_side_keys(&self) -> bool {
        !self.kbd.focus.hovered() && self.focused_index().is_some_and(|i| self.tree.nodes[i].key.as_deref().is_some_and(|k| k.starts_with("playground-slot-")))
    }

    /// Tells the keyboard whether the side controls that take keys have focus, so their view
    /// can mark what the keys move; a change rebuilds the view.
    pub(super) fn sync_side_focus(&mut self, app: &mut dyn AppDriver) {
        let slot = self.on_side_keys();
        if app.playground_mut().is_some_and(|p| p.moved(Moved::Slot(slot))) { self.commit.dirty = true; }
    }

    /// The paths of the focusable controls in the playground's side column, in order.
    fn playground_side(&self) -> Vec<String> {
        (0..self.tree.nodes.len()).filter(|&i| self.can_focus(i) && self.tree.under(i, |k| k == "playground-side")).map(|i| self.tree.nodes[i].path.clone()).collect()
    }
}
