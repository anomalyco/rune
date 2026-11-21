//! Pointer press, drag, drag-out, cursor and wheel/fling input entry.
use super::*;

/// How far (points, per axis) a click may travel and still pair into a double click.
pub(in crate::runtime) const CLICK_SLOP: f32 = 4.0;

impl RuntimeCore {
    pub(crate) fn pointer_move(&mut self, app: &dyn ViewDriver, x: f32, y: f32) { self.move_pointer(app, x, y, true); }

    /// `drag_move` false keeps a held drag from getting a `Phase::Move`: a release at the
    /// point the drag last saw has nothing new to report, and its instant transaction would
    /// cut short whatever the press started (a tapped slider's knob mid-spring).
    pub(in crate::runtime) fn move_pointer(&mut self, app: &dyn ViewDriver, x: f32, y: f32, drag_move: bool) {
        let moved = self.pointer.pos != Some((x, y));
        if moved { self.pointer.hover_paused = false; self.release_hover_hold(); }
        self.pointer.pos = Some((x, y));
        self.autoscroll_moved(x, y);
        if self.in_viewport(x, y) { self.pointer.set_left(false); }
        for &(id, rect) in self.hit.hits() {
            if self.hit_contains(id, rect, x, y)
                && let Some(&i) = self.tree.node_index.get(&id)
                && self.tree.nodes[i].handlers.get().is_some_and(|h| h.on_pointer_move.is_some())
            {
                let point = self.pointer_event(i, x, y);
                self.frame.queue.push(Action::PointerMove(self.tree.nodes[i].path.clone(), point));
            }
        }
        self.refresh_hover(x, y);
        let Some(held) = self.pointer.press.held() else { return };
        let (path, d, at, button) = (held.path.clone(), held.drag, held.at, held.button);
        if held.drag_out.is_some_and(|threshold| crate::drag_out::past_threshold(at, (x, y), threshold)) && self.start_drag_out(app, path.clone()) {
            return;
        }
        if let Some(held) = self.pointer.press.held_mut() {
            held.moved |= !press_run::within_slop(at, (x, y));
        }
        self.selection_drag(x, y);
        self.edge_scroll_moved(y);
        if self.edit.input_click.captured() {
            self.input_drag(x, y, drag_move);
            return;
        }
        let Some(i) = self.live_index(&path) else { return };
        let (id, r, has_drag) = (self.tree.nodes[i].id, self.tree.nodes[i].target, self.drags_with(i, button));
        // A hit rect is where the element was drawn (after scrolling); `target` is its
        // unscrolled layout rect, kept only for elements that were never drawn. Only the
        // pointer moving decides whether it's still inside: a button that springs out from
        // under a still pointer stays pressed, as AppKit only re-tracks on a drag.
        if moved && self.pointer.press.held().is_some_and(|h| h.hit != capture::Hit::Cancelled) {
            let inside = match self.hit.has_node_hit(id, |id, r| self.hit_contains(id, r, x, y)) {
                Some(hit) => hit,
                None if self.host.native_overlay => false,
                None => contains(r, x, y),
            };
            if let Some(held) = self.pointer.press.held_mut() {
                held.hit = if inside { capture::Hit::Inside } else { capture::Hit::Outside };
            }
        }
        if has_drag && drag_move {
            let (dx, dy) = (x - at.0, y - at.1);
            let [lx, ly] = self.to_local(i, x, y);
            let d = Drag { phase: Phase::Move, x: lx, y: ly, dx, dy, window: [x, y], screen: self.screen_at(x, y), ..d };
            self.frame.queue.push(Action::Drag(path, d, button));
        }
    }

    /// Overlay observation updates coordinates without presenting unchanged hover pixels.
    pub(crate) fn observe_pointer(&mut self, app: &dyn ViewDriver, x: f32, y: f32) -> bool {
        if self.pointer.pos == Some((x, y)) { return false; }
        let visual_changed = self.tree.nodes.iter().enumerate().any(|(i, n)| {
            if n.vis.hover().a <= 0.0 && !n.vis.tracks_press() && n.handlers.get().is_none_or(|h| h.press_scale.is_none()) { return false; }
            let inside = |p: Option<(f32, f32)>| p.is_some_and(|(x, y)| self.node_contains(i, x, y));
            inside(self.pointer.pos) != inside(Some((x, y)))
        });
        // Reaching or leaving a help element starts or ends its tooltip's wait.
        let help_changed = self.help_target_changes(x, y);
        let (queued, capture_inside) = (self.frame.queue.len(), self.pointer.press_inside());
        self.pointer_move(app, x, y);
        visual_changed || help_changed || self.frame.queue.len() != queued || self.pointer.press_inside() != capture_inside
    }

    /// A button holds the pointer: the OS keeps sending its moves to this window, and a leave
    /// waits for the release.
    #[inline]
    pub(crate) fn pointer_captured(&self) -> bool { self.pointer.captured() }

    /// The host's window content top-left in desktop pixels; `None` where it has none.
    pub(crate) fn set_window_pixels(&mut self, pixels: Option<(i32, i32)>) {
        self.host.window_pixels = pixels;
    }

    /// (x, y) window points in desktop pixels (`Drag::screen_point`).
    pub(in crate::runtime) fn screen_at(&self, x: f32, y: f32) -> Option<(i32, i32)> {
        let s = self.env.scale;
        self.host.window_pixels.map(|(px, py)| (px + (x * s).round() as i32, py + (y * s).round() as i32))
    }

    /// The host moved its window by (dx, dy) points under a still pointer. Keep the pointer and
    /// a held press's origin in window points, so a drag that moves its own window still reports
    /// pointer travel in `Drag::dx`/`dy`.
    #[inline]
    pub(crate) fn window_moved(&mut self, dx: f32, dy: f32) {
        self.pointer.window_moved(dx, dy);
    }

    pub(crate) fn cancel_pointer(&mut self) {
        // differs from dev: a cancelled press cannot pair with a click after refocusing.
        self.host.titlebar.clear();
        self.pointer.autoscroll_cancel();
        self.cancel_drag();
        self.drop_drag_out_request();
        self.edit.input_click.clear();
        self.drop_press();
        self.pointer.clear_secondary();
        self.pointer.clear_tint();
        self.pointer_leave();
        self.drag_files_leave();
    }

    /// End a held drag without a drop: its handler hears `Phase::Up` with [`Drag::cancelled`]
    /// and the release that follows does nothing. A press selecting text lets go where the
    /// selection stands, without a click. False when no drag is held.
    pub(in crate::runtime) fn cancel_drag(&mut self) -> bool {
        if self.cancel_drag_at(self.pointer.pos) { return true; }
        if !self.edit.selectable.phase.dragging() || !self.pointer.press.is_held() || self.edit.input_click.captured() { return false; }
        self.drop_press();
        true
    }

    /// Lets go of the held press without a click: a cancel, a drag-out taking the pointer, a
    /// modal opening. Every press that ends this way, and every release, ends the selection
    /// drag it was making, and with it the list's edge scroll. Returns the press.
    pub(in crate::runtime) fn drop_press(&mut self) -> Option<Box<capture::Held>> {
        self.select_step(selectable::phase::Event::Release);
        self.pointer.press.drop_held()
    }

    /// [`Self::cancel_drag`], its last report at `point`, or where the press began.
    fn cancel_drag_at(&mut self, point: Option<(f32, f32)>) -> bool {
        if self.edit.input_click.captured() { return false; }
        let Some(held) = self.pointer.press.held() else { return false };
        let Some(i) = self.live_index(&held.path) else { return false };
        if !self.drags_with(i, held.button) { return false; }
        let at = held.at;
        let (x, y) = point.unwrap_or(at);
        let [lx, ly] = self.to_local(i, x, y);
        let (dx, dy) = (x - at.0, y - at.1);
        let screen = self.screen_at(x, y);
        let Some(held) = self.drop_press() else { return false };
        let d = Drag { phase: Phase::Up, x: lx, y: ly, dx, dy, cancelled: true, window: [x, y], screen, ..held.drag };
        self.frame.queue.push(Action::Drag(held.path, d, held.button));
        self.pointer.end_press_repeat();
        true
    }

    /// The button is up, so the held press can no longer start a drag-out; it stays a press.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[inline]
    pub(crate) fn disarm_drag_out(&mut self) {
        self.pointer.disarm_drag_out();
    }

    /// The held press crossed the drag threshold on a `drag_out` element: ask the host for an
    /// OS drag and hand it the pointer, so nothing stays pressed or hovered after the drop.
    /// False when the element offers nothing this time and the press stays a press.
    pub(in crate::runtime) fn start_drag_out(&mut self, app: &dyn ViewDriver, path: String) -> bool {
        let Some(held) = self.pointer.press.held_mut() else { return false };
        held.drag_out = None;
        let press = held.at;
        let Some(i) = self.live_index(&path) else { return false };
        let Some(f) = self.tree.nodes[i].handlers.drag_out.clone() else { return false };
        let Some(item) = f(app.states_ref()).filter(|item| !item.is_empty()) else { return false };
        let n = &self.tree.nodes[i];
        let r = self.paint.drawn.get(i).copied().unwrap_or([n.target[0], n.target[1], n.target[2] - n.target[0], n.target[3] - n.target[1]]);
        let clip = self.screen_clip_of_parent(n.parent).map(|v| v / self.env.scale);
        let (x0, y0) = (r[0].max(clip[0]), r[1].max(clip[1]));
        let rect = [x0, y0, ((r[0] + r[2]).min(clip[2]) - x0).max(0.0), ((r[1] + r[3]).min(clip[3]) - y0).max(0.0)];
        log_event!(self, "drag-out {path}");
        self.drop_drag_out_request();
        self.frame.queue.push(Action::DragStart(path.clone()));
        self.pointer.drag_out_request = Some(crate::drag_out::DragOutStart { path, item, rect, bounds: r, press });
        self.drop_press();
        self.pointer_leave();
        true
    }

    /// A request no host took (headless tools, a cancelled press) ends as `Cancelled`, queued
    /// behind its start, so `on_drag_start` is always followed by `on_drag_end`.
    pub(in crate::runtime) fn drop_drag_out_request(&mut self) {
        if let Some(start) = self.pointer.drag_out_request.take() {
            self.frame.queue.push(Action::DragEnd(start.path, crate::DragOutcome::Cancelled));
        }
    }

    /// A release outside the window, or after the pointer left during the press, ends the hover.
    pub(in crate::runtime) fn leave_after_release(&mut self, left: bool) {
        if self.pointer_captured() { return; }
        if left || self.pointer.pos.is_some_and(|(x, y)| !self.in_viewport(x, y)) {
            self.pointer_leave();
        }
    }

    pub(crate) fn pointer_down(&mut self, app: &mut dyn AppDriver, x: f32, y: f32) {
        self.press_down(app, x, y, false);
    }

    /// A primary press at (x, y); `touch`: a finger's, which never runs `on_press` on the way
    /// down (`El::press_on_down` is a mouse's).
    pub(in crate::runtime) fn press_down(&mut self, app: &mut dyn AppDriver, x: f32, y: f32, touch: bool) {
        // A press ends a middle-click autoscroll, and does nothing else.
        if self.pointer.autoscroll_press(crate::PointerButton::Primary) { self.pointer.pos = Some((x, y)); return; }
        if self.pointer.press.held().is_some_and(|h| h.button != crate::PointerButton::Primary) {
            self.pointer.pos = Some((x, y));
            return;
        }
        self.drop_drag_out_request();
        // A host that didn't act on the last press's request mustn't get it with this one.
        self.host.titlebar.action = None;
        self.pointer.pos = Some((x, y));
        // A press is the hand's: hover follows the pointer through moving layout.
        self.release_hover_hold();
        self.kbd.focus.step(focus_machine::FocusEvent::Pointed);
        self.dismiss_help();
        // A press cancels the waits to report an enter; the next move after it starts them again.
        self.pointer.press_hover_delays();
        self.pointer.hover_paused = true;
        let target = self.primary_target(x, y);
        if target.and_then(|t| self.textbox_input(t)).is_none() {
            self.edit.input_click.clear();
        }
        if let Some(i) = self.active_modal()
            && target.is_none_or(|t| !self.allowed(t))
        {
            if self.tree.nodes[i].handlers.modal.as_ref().is_some_and(|m| m.outside) {
                self.frame.queue.push(Action::Dismiss(self.tree.nodes[i].path.clone()));
            }
            return;
        }
        self.playground_pointer_down(app, x);
        self.press_selection(target, x, y);
        // Inside a draggable row, dragging drags the row; clicks still select words and lines.
        if target.is_some_and(|t| self.tree.nodes[t].handlers.on_drag.is_some() || self.tree.nodes[t].handlers.drag_out.is_some()) {
            self.select_step(selectable::phase::Event::Release);
        }
        let Some(ti) = target else {
            let now = self.now_clock();
            self.host.titlebar.press_empty(now, x, y, self.double_click_interval);
            if self.kbd.focus.path().is_some() {
                self.frame.queue.push(Action::Blur);
            }
            return;
        };
        let link = self.link_at(ti, x, y);
        let on_link = link.is_some();
        // Any press breaks a pending pair unless it's the second press of it.
        let pending = self.pointer.press.take_first();
        let now = self.now_clock();
        if self.bare_drag_region(ti) {
            self.host.titlebar.press_region(now, x, y, self.double_click_interval);
            if self.kbd.focus.path().is_some() {
                self.frame.queue.push(Action::Blur);
            }
            return;
        }
        let n = &self.tree.nodes[ti];
        let clicks = self.pointer.press_click(&n.path, now, (x, y), self.double_click_interval);
        let second = n.handlers.on_double_press.is_some() && pending.is_some_and(|first| {
            first.path == n.path && press_run::follows(first.at, first.point, now, (x, y), self.double_click_interval)
        });
        if let Some(ji) = self.textbox_input(ti) {
            self.input_down(ji, x, y, now, second);
            return;
        }
        // Clicking a link leaves focus where it is (a composer keeps typing). Selectable text
        // off its links isn't a control: the press blurs, as a click on plain content does.
        // Inside a control that owns its keyboard (`El::on_key`), a click on any part of it
        // focuses it, as a click on a table view's row focuses the table.
        // A `Cx::web_focus` control that keeps the field's focus on a press (`preserve_focus_on_press`)
        // doesn't take it, as a button whose mousedown is prevented.
        // So does a click on plain content in a focusable scroll region, as a web page focuses a
        // `tabIndex` scroller (a `ScrollView`) clicked where nothing inside takes focus; and a
        // press on a layer that hands focus below it (`El::press_focuses_below`) focuses the one
        // under the pointer.
        if n.focus.below && !on_link {
            match self.scroll_region_below(ti, x, y) {
                Some(r) => self.frame.queue.push(Action::FocusControl(self.tree.nodes[r].path.clone())),
                None if self.kbd.focus.path().is_some() => self.frame.queue.push(Action::Blur),
                None => {}
            }
        }
        let takes_focus = n.focus.enabled || (self.takes_focus(n) && !self.press_keeps_focus(ti));
        let keyed = (!takes_focus && !self.is_selectable(ti)).then(|| std::iter::successors(n.parent, |&p| self.tree.nodes[p].parent)
            .take_while(|&p| self.allowed(p))
            .find(|&p| (self.tree.nodes[p].focus.key.is_some() || self.focuses_on_click(p)) && self.can_focus(p))).flatten();
        let in_textbox = std::iter::successors(n.parent, |&p| self.tree.nodes[p].parent).any(|p| self.textbox_input(p).is_some());
        let (preserve_focus, keeps) = (self.press_keeps_focus(ti), self.press_keeps_selection(ti));
        let below = n.focus.below;
        if !below && !on_link && !keeps && takes_focus && !self.is_selectable(ti) {
            self.frame.queue.push(Action::FocusControl(n.path.clone()));
        } else if !below && !on_link && !keeps && let Some(p) = keyed {
            self.frame.queue.push(Action::FocusControl(self.tree.nodes[p].path.clone()));
        } else if !below && !on_link && !in_textbox && !preserve_focus {
            // Tab goes on from here, as from a browser's click on plain content.
            if self.commit.defaults.web_focus { self.kbd.focus.step(focus_machine::FocusEvent::StartAt(n.path.clone())); }
            if self.kbd.focus.path().is_some() { self.frame.queue.push(Action::Blur); }
        }
        let r = n.target;
        let [lx, ly] = self.to_local(ti, x, y);
        let d = Drag {
            phase: Phase::Down,
            x: lx,
            y: ly,
            w: r[2] - r[0],
            h: r[3] - r[1],
            start_x: lx,
            start_y: ly,
            dx: 0.0,
            dy: 0.0,
            data: 0.0,
            clicks,
            cancelled: false,
            window: [x, y],
            screen: self.screen_at(x, y),
        };
        let path = n.path.clone();
        let drag_out = self.drags_out(n).then(|| n.handlers.drag_threshold.unwrap_or(crate::drag_out::DRAG_THRESHOLD));
        if n.handlers.on_drag.is_some() && drag_out.is_none() {
            self.frame.queue.push(Action::Drag(path.clone(), d, crate::PointerButton::Primary));
        }
        let on_down = !touch && n.handlers.press_on_down && n.handlers.on_press.is_some();
        if on_down {
            self.frame.queue.push(Action::Press(path.clone(), crate::PointerButton::Primary, false));
        }
        let mut held = capture::Held::new(path, d, crate::PointerButton::Primary, (x, y), now, second, drag_out).on_link(link).pressed_on_down(on_down);
        held.touch = touch;
        self.pointer.press.hold(held);
    }

    pub(crate) fn pointer_up(&mut self, app: &dyn ViewDriver, x: f32, y: f32) {
        if self.pointer.autoscroll_release(crate::PointerButton::Primary) { self.pointer.pos = Some((x, y)); return; }
        #[cfg(target_os = "linux")]
        self.primary_released();
        if self.pointer.press.held().is_some_and(|h| h.button != crate::PointerButton::Primary) {
            self.pointer.pos = Some((x, y));
            return;
        }
        self.release_capture(app, x, y);
    }

    /// The platform lost the release of a held press, so nothing was dropped: a held drag ends
    /// cancelled at (x, y), and any other press is released there.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn pointer_release_lost(&mut self, app: &dyn ViewDriver, x: f32, y: f32) {
        let held = self.pointer.press.held().filter(|h| h.button == crate::PointerButton::Primary)
            .map(|h| (h.left, h.moved || !press_run::within_slop(h.at, (x, y))));
        if let Some((left, moved)) = held && self.cancel_drag_at(Some((x, y))) {
            // As a normal release: a press that dragged ends the run of clicks.
            if moved { self.pointer.clear_press_clicks(); }
            self.move_pointer(app, x, y, false);
            self.leave_after_release(left);
        } else {
            self.pointer_up(app, x, y);
        }
    }

    /// What input has queued for handlers, emptied: each drag event, `None` for a press.
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn take_queued_pointer(&mut self) -> Vec<Option<Drag>> {
        let heard = self.frame.queue.iter().filter_map(|action| match action {
            Action::Drag(_, drag, _) => Some(Some(*drag)),
            Action::Press(..) => Some(None),
            _ => None,
        }).collect();
        self.frame.queue.retain(|_| false);
        heard
    }

    #[cfg(all(test, target_os = "macos"))]
    #[inline]
    pub(crate) fn end_click_run(&mut self) {
        self.pointer.end_click_run();
    }

    pub(in crate::runtime) fn release_capture(&mut self, app: &dyn ViewDriver, x: f32, y: f32) {
        // Hosts release at the last position they saw, which a leave doesn't move.
        let left = self.pointer.press.held().is_some_and(|h| h.left);
        self.release(app, x, y);
        self.leave_after_release(left);
    }

    pub(in crate::runtime) fn release(&mut self, app: &dyn ViewDriver, x: f32, y: f32) {
        // A release never starts a drag-out: the system session needs the button held.
        if let Some(held) = self.pointer.press.held_mut() { held.drag_out = None; }
        let moved = self.pointer.pos != Some((x, y));
        self.move_pointer(app, x, y, moved);
        // A drag that selected text is a selection, not a press.
        let selected = self.selected_by_drag();
        self.select_step(selectable::phase::Event::Release);
        let input_capture = self.edit.input_click.end_press();
        self.pointer.end_press_repeat();
        let now = self.now_clock();
        let interval = self.double_click_interval;
        let Some(h) = self.pointer.press.held() else { return };
        // Only clicks pair: a press that dragged or was held past the interval is neither half.
        let first = (!input_capture && !h.second && h.inside() && !h.moved && now - h.down_at <= interval
            && h.button == crate::PointerButton::Primary
            && self.live_index(&h.path).is_some_and(|i| self.tree.nodes[i].handlers.on_double_press.is_some()))
            .then(|| capture::FirstClick { path: h.path.clone(), at: h.down_at, point: h.at });
        let Some(held) = self.pointer.press.release(first) else { return };
        let inside = held.inside();
        let capture::Held { path, drag: d, button, at, down_at, moved, second: double, link, pressed_on_down, .. } = *held;
        self.refresh_hover_within();
        if moved { self.pointer.clear_press_clicks(); }
        if input_capture { return; }
        let Some(i) = self.live_index(&path) else { return };
        let n = &self.tree.nodes[i];
        // A button the element didn't opt in to is the middle button's click on a link or on
        // `on_middle_press`: neither its `on_press` nor its `on_drag` hears it.
        let opted = self.takes_button(i, button);
        let (has_drag, has_press, has_double) = (self.drags_with(i, button), opted && n.handlers.on_press.is_some(), n.handlers.on_double_press.is_some());
        let middle_press = !opted && link.is_none() && n.handlers.on_middle_press.is_some();
        if has_drag {
            let (dx, dy) = (x - at.0, y - at.1);
            let [lx, ly] = self.to_local(i, x, y);
            self.frame.queue.push(Action::Drag(path.clone(), Drag { phase: Phase::Up, x: lx, y: ly, dx, dy, window: [x, y], screen: self.screen_at(x, y), ..d }, button));
        }
        if let Some(k) = link.filter(|_| inside) { self.link_up(&path, k, x, y, button); }
        if has_press && inside && !selected && !pressed_on_down {
            self.frame.queue.push(Action::Press(path.clone(), button, held.touch));
        }
        if middle_press && inside {
            self.frame.queue.push(Action::MiddlePress(path.clone()));
        }
        let click = !moved && now - down_at <= interval;
        if has_double && inside && click && button == crate::PointerButton::Primary && double {
            self.frame.queue.push(Action::DoublePress(path));
        }
    }

    pub fn pointer_leave(&mut self) {
        self.wheel_lost();
        if !self.pointer.captured() {
            self.pointer.pos = None;
            self.pointer.hovered = None;
            self.refresh_hover_within();
        } else {
            self.pointer.set_left(true);
        }
    }

    /// A press on an element is held (the hand drives whatever follows it).
    #[inline]
    pub fn pressing(&self) -> bool {
        self.pointer.pressing()
    }
    #[inline]
    pub fn pointer(&self) -> Option<(f32, f32)> {
        self.pointer.pos()
    }

    /// Native overlay regions from the most recent frame, in logical window points (x0,y0,x1,y1).
    #[inline]
    pub fn hit_regions(&self) -> &[[f32; 4]] { self.hit.overlay_hits() }

    /// The pointer shape the shell should show: the pressed element's while a gesture holds
    /// the pointer, else the one under it in the current frame (the held hover while moving
    /// layout holds it), from the nearest ancestor that sets one. The arrow while the pointer is outside the window.
    pub fn cursor(&self) -> crate::el::Cursor {
        let Some((x, y)) = self.pointer.pos.filter(|&(x, y)| self.in_viewport(x, y)) else { return crate::el::Cursor::Default };
        if let Some(pan) = self.autoscroll_cursor() { return pan; }
        // An element on its way out has no cursor: `Cursor::Hidden` ends when it's removed.
        // Selectable text shows its I-beam even where its presses go to a row around it.
        let target = match self.pointer.press.path() {
            Some(_) if self.edit.selectable.phase.dragging() => self.selection_holder(),
            Some(p) => self.live_index(p).filter(|&i| self.allowed(i)),
            // Moving layout holds hover: the cursor stays with what the pointer was over.
            None => self.held_hover().flatten().or_else(|| self.selectable_at(x, y).or_else(|| self.primary_target(x, y))),
        };
        let Some(mut i) = target else {
            return crate::el::Cursor::Default;
        };
        if self.tree.nodes[i].handlers.cursor.is_none() && (self.tree.nodes[i].input.is_some() || self.tree.nodes[i].role == "textbox") {
            return crate::el::Cursor::Text;
        }
        if self.is_selectable(i) && !self.edit.selectable.phase.dragging() && self.link_at(i, x, y).is_some() {
            return crate::el::Cursor::Pointer;
        }
        loop {
            if let Some(c) = self.tree.nodes[i].handlers.cursor {
                return c;
            }
            let Some(parent) = self.tree.nodes[i].parent else { return crate::el::Cursor::Default };
            i = parent;
        }
    }

    /// Whether node `i` is a live, reachable scroll viewport on the horizontal or vertical axis.
    pub(in crate::runtime) fn is_scroll_viewport(&self, i: usize, horizontal: bool) -> bool {
        let n = &self.tree.nodes[i];
        n.scroll.is_some() && n.scroll_axes.has(horizontal) && !self.subtree_exiting(i) && self.allowed(i)
    }

    /// Walks up from `start` through ancestors to the first live, reachable scroll viewport on
    /// `horizontal` whose scroll state satisfies `has_room`.
    ///
    /// An element that names the viewport it scrolls (`El::wheel_scrolls`, an overlay scroll bar's
    /// track beside the viewport it follows) hands the wheel to that viewport first, then chains on
    /// up from it as a wheel over the viewport itself does. One hand-off, so it can't loop.
    fn scroll_in_chain(&self, mut at: Option<usize>, horizontal: bool, has_room: impl Fn(&ScrollState) -> bool) -> Option<NodeId> {
        let mut handed = false;
        while let Some(i) = at {
            if self.is_scroll_viewport(i, horizontal)
                && self.scroll.scrolls.get(&self.tree.nodes[i].id).is_some_and(&has_room)
            {
                return Some(self.tree.nodes[i].id);
            }
            let hand = (!handed).then(|| self.tree.nodes[i].wheel_scrolls.as_deref()).flatten()
                .and_then(|key| self.wheel_scroll_viewport(i, key)).filter(|&j| j != i);
            handed |= hand.is_some();
            at = hand.or_else(|| self.tree.nodes[i].parent);
        }
        None
    }

    /// The element keyed `key` that a wheel over element `from` naming it (`El::wheel_scrolls`)
    /// scrolls: `key` looked up inside `from` itself, then inside each of its ancestors, innermost
    /// first, so a wheel over a bar finds the viewport beside it rather than the first element
    /// keyed alike elsewhere (two panes, or every wide markdown table, can key their viewport
    /// `scroll`). The first scope that holds one decides: a match on its way out or inert hands
    /// the wheel nowhere rather than to another pane's. Named as `Tx::scroll_into_view` names one.
    fn wheel_scroll_viewport(&self, from: usize, key: &str) -> Option<usize> {
        let mut at = Some(from);
        while let Some(a) = at {
            let scope = &self.tree.nodes[a].path;
            if let Some(i) = self.tree.nodes.iter().position(|n| n.named(key) && within(&n.path, scope)) {
                return (!self.subtree_exiting(i) && self.allowed(i)).then_some(i);
            }
            at = self.tree.nodes[a].parent;
        }
        None
    }

    /// The innermost vertical scroll viewport at (x, y).
    pub(in crate::runtime) fn scroll_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.scroll_at_axis(x, y, false)
    }

    /// The innermost scroll viewport at (x, y) that moves on the horizontal axis or the vertical,
    /// walking up from [`Self::chain_start`] so raised/anchored overlays occlude viewports beneath.
    /// One whose content fits on that axis is passed over for the one around it, as Chromium
    /// skips a scroller with nothing to scroll: a list of sections, each its own viewport as
    /// tall as its rows, scrolls from over any row.
    pub(in crate::runtime) fn scroll_at_axis(&self, x: f32, y: f32, horizontal: bool) -> Option<NodeId> {
        self.scroll_at_axis_from(x, y, horizontal, crate::runtime::gesture_dispatch::WheelStart::Unknown)
    }

    /// [`Self::scroll_at_axis`], with where its chain starts when the caller already found it.
    pub(in crate::runtime) fn scroll_at_axis_from(&self, x: f32, y: f32, horizontal: bool, start: crate::runtime::gesture_dispatch::WheelStart) -> Option<NodeId> {
        let at = match start { crate::runtime::gesture_dispatch::WheelStart::At(s) => s, crate::runtime::gesture_dispatch::WheelStart::Unknown => self.chain_start(x, y) };
        self.scroll_in_chain(at, horizontal, |st| if horizontal { st.x.max } else { st.max } > 0.0)
    }

    /// The vertical viewport a delta `dy` at (x, y) starts a run on: from where a gesture there
    /// starts (`Self::chain_start`: the topmost hit, or a scroll view inside it) up its ancestors,
    /// the first that can still move that way, as Chromium chains a wheel's scroll. One at its end
    /// passes the run to the one around it, so a short region inside a page never traps the wheel;
    /// a viewport that only lies beneath (the timeline under an open menu, with or without a list
    /// of its own, or under a raised panel) never takes it. A wheel's ease counts from where it heads.
    pub(in crate::runtime) fn wheel_target(&self, x: f32, y: f32, dy: f32, start: crate::runtime::gesture_dispatch::WheelStart) -> Option<NodeId> {
        let at = match start { crate::runtime::gesture_dispatch::WheelStart::At(s) => s, crate::runtime::gesture_dispatch::WheelStart::Unknown => self.chain_start(x, y) };
        self.scroll_in_chain(at, false, |st| {
            let cur = st.mode.wheel().map_or(st.offset, |c| c.target());
            if dy > 0.0 { cur > 0.0 } else { cur < st.max }
        })
    }

    /// Wheel or trackpad delta in points (positive `dy` moves content down, like a browser's
    /// negative deltaY). Applied 1:1: the platform supplies its own momentum phases. A run of
    /// deltas keeps to the viewport it began on (`WheelLock::vertical_run`), even at its end.
    pub fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        self.wheel_from(x, y, dy, crate::runtime::gesture_dispatch::WheelStart::Unknown);
    }

    /// [`Self::wheel`], with where its chain starts when the caller already found it.
    pub(in crate::runtime) fn wheel_from(&mut self, x: f32, y: f32, dy: f32, start: crate::runtime::gesture_dispatch::WheelStart) {
        self.release_hover_hold();
        self.drop_scroll_holds();
        self.dismiss_help();
        let now = self.now_clock();
        let Some((id, held)) = self.wheel_run(now, x, y, dy, start) else { return };
        let stick = self.tree.node_index.get(&id).and_then(|&i| self.tree.nodes[i].scroll).is_some_and(|s| s.stick_bottom);
        let Some(st) = self.scroll.scrolls.get_mut(&id) else { return };
        let before = st.offset;
        st.offset = (st.offset - f64::from(dy)).clamp(0.0, st.max);
        st.moved();
        st.land(stick);
        log_event!(self, "wheel {dy:.1}");
        let moved = st.offset != before;
        if moved { self.pointer.scroll_cancels_press(&self.tree.nodes, id); }
        self.scroll.ran_wheel_vertical(id, now, x, y, dy, moved, held);
    }

    /// The vertical viewport a delta `dy` at (x, y) at `now` scrolls, and whether it continues the
    /// run (`WheelLock::vertical_run`) rather than starting one (`Self::wheel_target`). None ends
    /// the run.
    fn wheel_run(&mut self, now: f64, x: f32, y: f32, dy: f32, start: crate::runtime::gesture_dispatch::WheelStart) -> Option<(NodeId, bool)> {
        let held = self.scroll.wheel_vertical_run(now, x, y, dy).filter(|id| {
            self.tree.node_index.get(id).is_some_and(|&i| self.is_scroll_viewport(i, false))
        });
        let picked = held.map(|id| (id, true)).or_else(|| self.wheel_target(x, y, dy, start).map(|id| (id, false)));
        if picked.is_none() { self.scroll.end_wheel_vertical(); }
        picked
    }

    /// Whether whole notches of a mouse wheel ease rather than jump: the app hasn't turned it off
    /// (`Runtime::set_wheel_glide`), and motion isn't reduced or settled.
    #[inline]
    pub(in crate::runtime) fn wheel_glides(&self) -> bool { self.scroll.wheel_glides(self.reduced_motion, self.settled) }

    /// Whole notches of a mouse wheel, `dy` points (positive moves content down): the offset
    /// eases to where they send it as Chromium's does (`runtime/scroll/wheel.rs`), a notch that
    /// comes while it moves sending it on from where it is. A run of notches keeps to the viewport
    /// it began on, as `Self::wheel`'s deltas do. With reduced motion, it jumps.
    pub(in crate::runtime) fn wheel_notch(&mut self, x: f32, y: f32, dy: f32, start: crate::runtime::gesture_dispatch::WheelStart) {
        if !self.wheel_glides() {
            self.wheel_from(x, y, dy, start);
            return;
        }
        self.release_hover_hold();
        self.drop_scroll_holds();
        self.dismiss_help();
        let now = self.now_clock();
        let Some((id, held)) = self.wheel_run(now, x, y, dy, start) else { return };
        // The ease's time follows the distance in device pixels, as Chromium measures it.
        let scale = f64::from(self.env.scale * self.zoom());
        let stick = self.tree.node_index.get(&id).and_then(|&i| self.tree.nodes[i].scroll).is_some_and(|s| s.stick_bottom);
        let Some(st) = self.scroll.scrolls.get_mut(&id) else { return };
        let easing = st.mode.wheel().is_some();
        let mut curve = st.mode.wheel().unwrap_or_else(|| scroll_wheel::WheelCurve::still(st.offset));
        curve.follow(st.offset);
        let to = (curve.target() - f64::from(dy)).clamp(0.0, st.max);
        let moved = curve.retarget(to, scale);
        curve.aimed_at_end(to >= st.max);
        if moved {
            st.mode = st.mode.to_wheel(curve);
            st.moved();
            log_event!(self, "wheel-notch {dy:.1}");
            self.pointer.scroll_cancels_press(&self.tree.nodes, id);
        } else if easing {
            // Sent back to where it is (a flick back before the next frame): it stays put, as
            // Chromium's does, rather than finishing the ease the notch before started.
            st.land(stick);
        }
        self.scroll.ran_wheel_vertical(id, now, x, y, dy, moved, held);
    }

    /// Releases a scroll with `velocity` (points per second, positive moves content down);
    /// the runtime decays it (for platforms without OS momentum and headless scripts).
    pub fn fling(&mut self, x: f32, y: f32, velocity: f32) {
        self.release_hover_hold();
        self.drop_scroll_holds();
        self.dismiss_help();
        let Some(id) = self.scroll_at(x, y) else { return };
        if let Some(st) = self.scroll.scrolls.get_mut(&id) {
            st.mode = st.mode.to_momentum(-velocity);
            log_event!(self, "fling {velocity:.0}");
            if velocity != 0.0 { self.pointer.scroll_cancels_press(&self.tree.nodes, id); }
        }
    }

    pub fn has_pending_input(&self) -> bool {
        !self.frame.queue.is_empty()
    }
}

/// Whether `path` is the element at `scope` or one inside it, by the paths' own segments (so
/// `area/list` isn't inside `area/listx`).
fn within(path: &str, scope: &str) -> bool {
    path.strip_prefix(scope).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}
