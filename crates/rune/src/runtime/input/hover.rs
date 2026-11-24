//! Hover and press state: which elements the pointer is over, `on_hover_change` and its
//! delays, and the hover and press tracks hover styles and press scales animate.
use super::*;

/// Hover under a pointer that hasn't moved while layout carries content under it: a row
/// expanding and the rows below sliding, a growth stack, a flight, a glide the app asked for.
/// As on macOS, where tracking areas enter and exit as the mouse moves and re-sync once layout
/// settles, hover doesn't light each row that passes: it waits for the motion to settle or the
/// pointer to move. What leaves the tree or turns inert lets go at once, and presses are never
/// held.
#[derive(Default)]
pub(in crate::runtime) struct HoverHold {
    hold: Hold,
    /// Scratch for the hover tracks `update_interaction` would change.
    flips: Vec<Flip>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Hold {
    /// Hover follows the pointer, and moving layout may hold it.
    #[default]
    Free,
    /// The last pass held hover: moving layout carried an element whose hover would change.
    Held,
    /// The pointer moved, pressed or a hand scrolled while layout moved: hover follows the
    /// pointer until the motion settles.
    Released,
}

impl HoverHold {
    /// The last pass held hover.
    pub(in crate::runtime) fn held(&self) -> bool { self.hold == Hold::Held }

    /// Hover follows the pointer until layout settles.
    pub(in crate::runtime) fn released(&self) -> bool { self.hold == Hold::Released }

    /// Notes whether this pass held hover. Released, nothing holds it.
    fn set_held(&mut self, held: bool) {
        if self.hold != Hold::Released { self.hold = if held { Hold::Held } else { Hold::Free }; }
    }

    /// The hand moved: with layout `moving`, hover follows the pointer until it settles.
    fn release(&mut self, moving: bool) {
        if self.hold != Hold::Released { self.hold = if moving { Hold::Released } else { Hold::Free }; }
    }

    /// Layout settled: a released hold may hold again.
    fn settled(&mut self) {
        if self.hold == Hold::Released { self.hold = Hold::Free; }
    }
}

/// Node `i`'s hover track: where it would go, how, whether that changes it, and whether it
/// lands now whatever the hold (a leave from an element turned inert).
#[derive(Clone, Copy)]
struct Flip { i: usize, id: NodeId, on: bool, anim: Anim, changed: bool, now: bool }

/// What `hover_within` would become: the `on_hover_change` elements the pointer is inside,
/// and those of them entering.
type Within = (Vec<String>, Vec<usize>);

/// Viewport `s` moves content under a still pointer on its own at `now`: a glide, an app's
/// instant scrolls frame after frame (a glide it steps itself), or slack letting rows down.
fn scroll_carries(s: &ScrollState, now: f64) -> bool {
    s.glide().is_some() || s.mode.wheel().is_some() || s.x.carried() || s.hold_at(now) || s.slack.moving()
}

/// `inner` is `outer` or inside it.
fn holds(outer: &str, inner: &str) -> bool {
    inner == outer || inner.len() > outer.len() && inner.starts_with(outer) && inner.as_bytes()[outer.len()] == b'/'
}

impl capture::PointerState {
    /// Hover may wait on moving layout: the pointer is in the window, holds no press, and
    /// hasn't moved since the motion started.
    #[inline]
    pub(super) fn hover_may_hold(&self) -> bool {
        self.pos.is_some() && !self.press.is_held() && !self.hover_hold.released()
    }

    /// The pointer moved or pressed, or a hand scrolled: hover follows the pointer through the
    /// layout moving now, until it settles.
    #[inline]
    pub(super) fn release_hover_hold(&mut self, layout_moving: impl FnOnce() -> bool) {
        let moving = !self.hover_hold.released() && layout_moving();
        self.hover_hold.release(moving);
    }

    /// Removes a pruned node from hover tracking; returns true if its leave must still be queued.
    pub(super) fn prune_hover(&mut self, n: &Node) -> bool {
        let inside = self.hover_within.iter().position(|p| *p == n.path).map(|at| self.hover_within.remove(at)).is_some();
        let shown = self.hover_delays.forget(&n.path);
        let heard = if n.handlers.hover_delay == [0.0; 2] { inside } else { shown };
        let Some(f) = n.handlers.on_hover_change.clone().filter(|_| heard) else { return false };
        self.push_hover_gone(n.path.clone(), f);
        true
    }
}

impl RuntimeCore {
    /// Hover under the pointer at (x, y): what's hovered and what it's inside. Returns whether
    /// moving layout holds hover this pass, for the `update_interaction` that follows.
    pub(in crate::runtime) fn refresh_hover(&mut self, x: f32, y: f32) -> bool {
        if self.pointer.hover_hold.released() && !self.layout_moving() { self.pointer.hover_hold.settled(); }
        let hovered = (!self.pointer.left()).then(|| self.hover_target(x, y)).flatten();
        let within = self.within_next();
        let held = self.decide_hold(hovered, within.as_ref(), None);
        self.pointer.hover_hold.set_held(held);
        if let Some((inside, _)) = within { self.apply_within(inside, held); }
        self.apply_hovered(hovered, held);
        held
    }

    /// Queue `on_hover_change` for elements the pointer entered or left.
    pub(in crate::runtime) fn refresh_hover_within(&mut self) {
        let Some(within) = self.within_next() else { return };
        let hovered = (!self.pointer.left()).then_some(self.pointer.pos).flatten().filter(|_| self.pointer.hover_may_hold()).and_then(|(x, y)| self.hover_target(x, y));
        let held = self.decide_hold(hovered, Some(&within), None);
        self.pointer.hover_hold.set_held(held);
        self.apply_within(within.0, held);
    }

    fn apply_hovered(&mut self, mut hovered: Option<usize>, held: bool) {
        // What's hovered stays; one that left the tree or turned inert lets go now.
        if held { hovered = self.pointer.hovered.as_deref().and_then(|p| self.hover_live(p)); }
        if hovered.map(|i| self.tree.nodes[i].path.as_str()) == self.pointer.hovered.as_deref() { return; }
        self.pointer.hovered = hovered.map(|i| self.tree.nodes[i].path.clone());
        if let Some(path) = &self.pointer.hovered { self.frame.queue.push(Action::Hover(path.clone())); }
        // A row never takes focus from a text field (a search field above its results).
        let row = hovered.and_then(|i| self.highlight_row(i)).filter(|_| self.focused_input().is_none());
        match row {
            Some(i) if self.kbd.focus.path() != Some(&self.tree.nodes[i].path) || !self.kbd.focus.hovered() =>
                self.frame.queue.push(Action::HoverFocus(Some(self.tree.nodes[i].path.clone()))),
            None if self.kbd.focus.hovered() => self.frame.queue.push(Action::HoverFocus(None)),
            _ => {}
        }
    }

    /// None when no element tracks `on_hover_change` and none is waiting on a delay.
    fn within_next(&self) -> Option<Within> {
        if self.pointer.hover_within.is_empty() && self.pointer.hover_delays.is_empty()
            && !self.hit.hits().iter().any(|(id, _)| self.tree.node_index.get(id).is_some_and(|&i| self.tree.nodes[i].handlers.get().is_some_and(|h| h.on_hover_change.is_some())))
        {
            return None;
        }
        let mut inside: Vec<String> = Vec::new();
        let mut entering: Vec<usize> = Vec::new();
        if !self.pointer.left()
            && let Some((x, y)) = self.pointer.pos
            && let Some(top) = self.top_hover_hit(x, y)
        {
            for &(id, _) in self.hit.hits() {
                if let Some((i, n)) = self.tree.node_index.get(&id).map(|&i| (i, &self.tree.nodes[i]))
                    && self.allowed(i)
                    && n.handlers.get().is_some_and(|h| h.on_hover_change.is_some())
                    && self.node_holds(i, top)
                    && !inside.contains(&n.path)
                {
                    if !self.pointer.hover_within.contains(&n.path) { entering.push(i); }
                    inside.push(n.path.clone());
                }
            }
        }
        // A held press stays inside what holds it until it releases, unless that element opted
        // into tracking bounds (`El::hover_tracks_bounds`).
        if let Some(captured) = self.pointer.press.path() {
            for path in &self.pointer.hover_within {
                if !inside.contains(path) && holds(path, captured)
                    && self.tree.nodes.iter().enumerate().any(|(i, n)| n.path == *path && self.allowed(i) && !n.handlers.hover_tracks_bounds)
                {
                    inside.push(path.clone());
                }
            }
        }
        Some((inside, entering))
    }

    fn apply_within(&mut self, mut inside: Vec<String>, held: bool) {
        // Held, the pointer stays inside what it was until that leaves the tree or turns inert.
        if held { inside = self.pointer.hover_within.iter().filter(|p| self.hover_live(p).is_some()).cloned().collect(); }
        // An element on its way out keeps its delays: only a shown hover hears its leave.
        let delayed = |this: &Self, path: &str| this.tree.nodes.iter().find(|n| n.path == path).map(|n| n.handlers.hover_delay).filter(|d| *d != [0.0; 2]);
        for path in &self.pointer.hover_within {
            if !inside.contains(path) && delayed(self, path).is_none() { self.frame.queue.push(Action::HoverChange(path.clone(), false)); }
        }
        for path in &inside {
            if !self.pointer.hover_within.contains(path) && delayed(self, path).is_none() { self.frame.queue.push(Action::HoverChange(path.clone(), true)); }
        }
        self.pointer.hover_within = inside;
        self.settle_hover_delays();
    }

    /// Whether moving layout holds hover this pass, decided once before any of its three states
    /// change: the pointer may hold, and layout carries an element whose `hovered`,
    /// `hover_within` or hover track would change. `flips` are the track changes when known.
    fn decide_hold(&self, hovered: Option<usize>, within: Option<&Within>, flips: Option<&[Flip]>) -> bool {
        if !self.pointer.hover_may_hold() { return false; }
        let carried = |i: usize| self.layout_carries(i);
        let was = |p: &str| self.hover_live(p).is_some_and(carried);
        if hovered.map(|i| self.tree.nodes[i].path.as_str()) != self.pointer.hovered.as_deref()
            && (hovered.is_some_and(carried) || self.pointer.hovered.as_deref().is_some_and(was))
        {
            return true;
        }
        if let Some((inside, entering)) = within
            && (entering.iter().any(|&i| carried(i)) || self.pointer.hover_within.iter().any(|p| !inside.contains(p) && was(p)))
        {
            return true;
        }
        let held = |f: &Flip| f.changed && !f.now && carried(f.i);
        match flips {
            Some(flips) => flips.iter().any(held),
            None => {
                let top = (!self.pointer.left()).then_some(self.pointer.pos).flatten().and_then(|(x, y)| self.top_hover_hit(x, y));
                let (captured, alone, shared) = (self.pointer.captured_path(), self.hovered_alone_at(top), self.shared_hovered_at(top));
                (0..self.tree.nodes.len()).filter_map(|i| self.hover_flip(i, top, captured, alone, &shared)).any(|f| held(&f))
            }
        }
    }

    /// Node `n` leaves the tree: if it heard the pointer enter, its `on_hover_change` still
    /// hears the leave, though the handler goes with the node before the action runs. A
    /// delayed element heard the enter only once shown; a pending one just stops waiting.
    pub(in crate::runtime) fn hover_pruned(&mut self, n: &Node) {
        if !self.pointer.prune_hover(n) { return; }
        let queued = self.frame.queue.len();
        self.frame.queue.push(Action::HoverChange(n.path.clone(), false));
        self.frame.queue.observe_since(queued);
    }

    /// The pointer moved or pressed, or a hand scrolled: hover follows the pointer through the
    /// layout moving now, until it settles.
    pub(in crate::runtime) fn release_hover_hold(&mut self) {
        let moving = !self.pointer.hover_hold.released() && self.layout_moving();
        self.pointer.release_hover_hold(|| moving);
    }

    /// Whether `k`'s `prop::FRAME` spring moves `child` (or `k` itself, when `child` is `None`).
    /// `Rect` animates as four edge lanes `[left, top, right, bottom]`. An ancestor that only
    /// resizes while its origin stays fixed moves no normal child; a child with `ride_trailing`
    /// rides the ancestor's trailing (right) edge instead of its leading (left) one, and a
    /// scaled or rotated ancestor's pivot moves with its size.
    fn frame_carries(&self, k: usize, child: Option<usize>) -> bool {
        let store = self.scene.store();
        let n = &self.tree.nodes[k];
        let id = n.id;
        if !store.is_animating(id, prop::FRAME) { return false; }
        let Some(c) = child else { return true };
        let scaled_or_turned = (n.static_scale - 1.0).abs() > 1e-5
            || (n.scale_track && self.scene.get(id, P_SCALE).is_some_and(|s| (s - 1.0).abs() > 1e-5))
            || (n.handlers.get().is_some_and(|h| h.press_scale.is_some()) && self.scene.get(id, P_PRESS_SCALE).is_some_and(|s| (s - 1.0).abs() > 1e-5))
            || self.scene.presented(id).is_some_and(|p| (p.transform.scale.x - 1.0).abs() > 1e-5 || (p.transform.scale.y - 1.0).abs() > 1e-5 || p.transform.rotate != 0.0);
        if scaled_or_turned { return true; }
        let x_lane = if self.tree.nodes[c].vis_flags.ride_trailing() { 2 } else { 0 };
        store.is_lane_animating(id, prop::FRAME, x_lane) || store.is_lane_animating(id, prop::FRAME, 1)
    }

    /// Element `i` is carried by moving layout: its own frame spring, an ancestor's frame spring
    /// moving the edge this subtree rides (origin, or trailing edge under `El::ride_trailing`),
    /// an offset or flight spring on it or an ancestor, a growth stack moving its rows, or a
    /// viewport gliding where the app sent it, stepped by the app frame by frame, or letting rows
    /// down. A hand's scroll and live transforms aren't layout motion. Asked only of elements
    /// whose hover would change, so a frame at rest walks nothing.
    fn layout_carries(&self, i: usize) -> bool {
        let store = self.scene.store();
        let mut at = Some(i);
        let mut child = None;
        while let Some(k) = at {
            let id = self.tree.nodes[k].id;
            if self.frame_carries(k, child) || store.is_animating(id, prop::OFFSET) || store.is_animating(id, prop::TRAVEL)
                || self.motion.stacks.get(&id).is_some_and(|s| s.moving) || self.motion.grows.get(&id).is_some_and(|g| g.moving())
                || self.scroll.scrolls.get(&id).is_some_and(|s| scroll_carries(s, self.clock))
            {
                return true;
            }
            child = Some(k);
            at = self.tree.nodes[k].parent;
        }
        false
    }

    /// Any layout moving at all: the running stacks, growths, glides, flights and spring
    /// tracks (started this frame or before), not the tree. Asked only when a hand moves and
    /// while hover follows it.
    fn layout_moving(&self) -> bool {
        let layout = [prop::FRAME.id, prop::OFFSET.id, prop::TRAVEL.id];
        !self.motion.flying.is_empty()
            || self.motion.stacks.values().any(|s| s.moving)
            || self.motion.grows.values().any(|g| g.moving())
            || self.scroll.scrolls.values().any(|s| scroll_carries(s, self.clock))
            || self.scene.store().active().any(|(_, p)| layout.contains(&p))
    }

    /// While moving layout holds hover, the hovered element, as the cursor and tooltips see
    /// it: what the pointer was over, not what slid under it.
    pub(crate) fn held_hover(&self) -> Option<Option<usize>> {
        (self.pointer.hover_hold.held() && !self.pointer.captured())
            .then(|| self.pointer.hovered.as_deref().and_then(|p| self.hover_live(p)))
    }

    /// The element at `path`, while it's in the tree, not leaving it, and not inert.
    pub(crate) fn hover_live(&self, path: &str) -> Option<usize> {
        (0..self.tree.nodes.len())
            .find(|&i| self.tree.nodes[i].path == path && !self.subtree_exiting(i) && !self.subtree_unmounted(i))
            .filter(|&i| self.allowed(i))
    }

    /// Whether the pointer hovers node `i`, given the topmost hit target `top` under it:
    /// follows the hit-tested element and its ancestors (as CSS `:hover` does), so an
    /// overhanging hit child keeps its group lit and another subtree's hit blocks elements
    /// underneath. When `top` is `None` or a strict ancestor of `i`, `i`'s own box decides.
    fn pointer_hovers(&self, i: usize, top: Option<usize>) -> bool {
        if self.pointer.left() { return false; }
        let Some((x, y)) = self.pointer.pos else { return false };
        match top {
            Some(t) if self.node_holds(i, t) => true,
            Some(t) => self.node_contains(i, x, y) && self.node_holds(t, i),
            None => self.node_contains(i, x, y),
        }
    }

    /// The innermost `hover_alone` element under the pointer: the elements around it show no
    /// hover.
    fn hovered_alone_at(&self, top: Option<usize>) -> Option<usize> {
        self.pointer.pos?;
        (0..self.tree.nodes.len())
            .filter(|&i| self.tree.nodes[i].vis.hover_alone && !self.subtree_exiting(i) && self.allowed(i) && self.pointer_hovers(i, top))
            .max_by_key(|&i| self.tree.nodes[i].path.len())
    }

    #[cfg(test)]
    fn hovered_alone(&self) -> Option<usize> {
        self.hovered_alone_at(self.pointer.pos.and_then(|(x, y)| self.top_hover_hit(x, y)))
    }

    /// Keyboard focus shows on node `i` or inside it (CSS `:has(:focus-visible)`): a hover group
    /// lights for it as for the pointer, so the controls it reveals (a row's actions, a tab's
    /// close button) show when Tab reaches them rather than taking focus unseen.
    fn keyboard_within(&self, i: usize) -> bool {
        self.kbd.focus.keyboard() && self.focus_within(i)
    }

    /// The `hover_group_shared` keys of the elements under the pointer or holding keyboard focus.
    fn shared_hovered_at(&self, top: Option<usize>) -> Vec<Rc<str>> {
        (0..self.tree.nodes.len())
            .filter_map(|i| {
                let key = self.tree.nodes[i].vis.hover_share.as_ref()?;
                (!self.subtree_exiting(i) && self.allowed(i) && (self.pointer_hovers(i, top) || self.keyboard_within(i))).then(|| key.clone())
            })
            .collect()
    }

    #[cfg(test)]
    fn shared_hovered(&self) -> Vec<Rc<str>> {
        self.shared_hovered_at(self.pointer.pos.and_then(|(x, y)| self.top_hover_hit(x, y)))
    }

    /// Node `i`'s hover track, if it has one and isn't leaving. `top`: the topmost hit under
    /// the pointer. `alone`: the `hover_alone` element under the pointer, which turns off the
    /// tracks of the elements around it. `shared`: the `hover_group_shared` keys the pointer
    /// is over.
    fn hover_flip(&self, i: usize, top: Option<usize>, captured: Option<&str>, alone: Option<usize>, shared: &[Rc<str>]) -> Option<Flip> {
        let n = &self.tree.nodes[i];
        if !(n.vis.tracks_hover_paint() || n.vis_flags.hover_group()) || self.subtree_exiting(i) { return None; }
        let inside = self.pointer_hovers(i, top)
            || n.vis.hover_share.as_ref().is_some_and(|k| shared.contains(k))
            || (n.vis_flags.hover_group() && self.keyboard_within(i));
        let capture_inside = if self.pointer.pressing() { self.pointer.press_inside() } else { inside };
        let held = captured == Some(n.path.as_str()) && capture_inside;
        let allowed = self.allowed(i);
        // A held press stays hovered while it's inside, though a press scale shrank the drawn
        // rect away from the pointer. A group stays hovered while a descendant holds the press.
        let shadowed = alone.is_some_and(|a| a != i && holds(&n.path, &self.tree.nodes[a].path));
        let on = allowed && (inside || held) && !shadowed && captured.is_none_or(|p| holds(&n.path, p));
        // Like a native sidebar or menu: the highlight lands with the pointer and fades
        // briefly as it leaves.
        let anim = match n.hover_motion() {
            Some((enter, exit)) => if on { enter } else { exit },
            None if on => Anim::INSTANT,
            None => rune_motion::Curve::tween(0.1, rune_motion::Easing::EASE_OUT).into(),
        };
        let changed = self.scene.store().target(n.id, P_HOVER).unwrap_or(0.0) != on as u8 as f32;
        Some(Flip { i, id: n.id, on, anim, changed, now: !on && !allowed })
    }

    /// Whether node `i` shows pressed right now (primary capture, secondary capture, or tint).
    fn press_down_node(&self, i: usize, captured: Option<&str>) -> bool {
        let n = &self.tree.nodes[i];
        let capture_inside = if self.pointer.pressing() {
            self.pointer.press_inside()
        } else if self.pointer.secondary_held() {
            !self.pointer.left() && self.pointer.pos.is_some_and(|(x, y)| self.node_contains(i, x, y))
        } else {
            false
        };
        // The middle or secondary button shows pressed what it went down on while it's over
        // it, as the primary does.
        let tinted = !self.pointer.left() && self.pointer.tint_path() == Some(n.path.as_str()) && self.pointer.pos.is_some_and(|(x, y)| self.node_contains(i, x, y));
        self.allowed(i) && ((captured == Some(n.path.as_str()) && capture_inside) || tinted)
    }

    /// Whether `update_interaction` would retarget any hover, press or press-scale track from
    /// the current pointer state.
    pub(in crate::runtime) fn interaction_changes(&self) -> bool {
        if !self.tree.nodes.iter().map(|n| n.id).eq(self.tree.built_ids.iter().copied()) {
            return false;
        }
        let top = (!self.pointer.left()).then_some(self.pointer.pos).flatten().and_then(|(x, y)| self.top_hover_hit(x, y));
        let captured = self.pointer.captured_path();
        let alone = self.hovered_alone_at(top);
        let shared = self.shared_hovered_at(top);
        for i in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let press_scale = n.handlers.get().and_then(|h| h.press_scale);
            let press_offset = n.handlers.get().and_then(|h| h.press_offset);
            if !n.vis.tracks_hover_paint() && !n.vis_flags.hover_group() && !n.vis.tracks_press() && press_scale.is_none() && press_offset.is_none() { continue; }
            if self.subtree_exiting(i) { continue; }
            if self.hover_flip(i, top, captured, alone, &shared).is_some_and(|f| f.changed) { return true; }
            if !n.vis.tracks_press() && press_scale.is_none() && press_offset.is_none() { continue; }
            let down = self.press_down_node(i, captured);
            let id = n.id;
            if n.vis.tracks_press() && self.scene.store().target(id, P_PRESS).unwrap_or(0.0) != down as u8 as f32 {
                return true;
            }
            if let Some(k) = press_scale {
                let want = if down && !self.reduced_motion { k } else { 1.0 };
                if self.scene.store().target(id, P_PRESS_SCALE).unwrap_or(1.0) != want {
                    return true;
                }
            }
            if let Some(off) = press_offset {
                let want = if down && !self.reduced_motion { off } else { [0.0; 2] };
                if self.scene.store().target(id, P_PRESS_OX).unwrap_or(0.0) != want[0]
                    || self.scene.store().target(id, P_PRESS_OY).unwrap_or(0.0) != want[1]
                {
                    return true;
                }
            }
        }
        false
    }

    /// Retargets the hover, press and press-scale tracks. `held` is the hold `refresh_hover`
    /// just decided; without it (a commit's pass) this decides it.
    pub(in crate::runtime) fn update_interaction(&mut self, held: Option<bool>) -> bool {
        // Hit tests read last frame's drawn rects and clips by index. When a commit moved the
        // indices (a commit's focus pass runs before the frame is built again), they belong to
        // other elements until the build re-evaluates from fresh geometry.
        if !self.tree.nodes.iter().map(|n| n.id).eq(self.tree.built_ids.iter().copied()) {
            return false;
        }
        let mut changed = false;
        let mut flips = std::mem::take(&mut self.pointer.hover_hold.flips);
        let top = (!self.pointer.left()).then_some(self.pointer.pos).flatten().and_then(|(x, y)| self.top_hover_hit(x, y));
        let captured = self.pointer.captured_path();
        let alone = self.hovered_alone_at(top);
        let shared = self.shared_hovered_at(top);
        for i in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let press_scale = n.handlers.get().and_then(|h| h.press_scale);
            let press_offset = n.handlers.get().and_then(|h| h.press_offset);
            if !n.vis.tracks_hover_paint() && !n.vis_flags.hover_group() && !n.vis.tracks_press() && press_scale.is_none() && press_offset.is_none() { continue; }
            if self.subtree_exiting(i) { continue; }
            match self.hover_flip(i, top, captured, alone, &shared) {
                Some(f) if f.changed => flips.push(f),
                // Default hover animations inherit velocity and have no completion token.
                // An unchanged target preserves its trajectory in the store already; avoid
                // dirtying the scene node for that no-op. Custom animations may carry an
                // explicit velocity, decay or token and must still reach the store.
                Some(f) if n.hover_motion().is_some() => {
                    if !self.scene.store().has(f.id, P_HOVER) { self.scene.set(f.id, P_HOVER, 0.0); }
                    self.scene.animate(f.id, P_HOVER, f.on as u8 as f32, f.anim);
                }
                Some(_) => {}
                None => {}
            }
            if !n.vis.tracks_press() && press_scale.is_none() && press_offset.is_none() { continue; }
            let down = self.press_down_node(i, captured);
            let pressed = down && n.vis.tracks_press();
            let press_motion = n.vis.press_motion.as_deref().copied();
            let id = n.id;
            changed |= self.scene.store().target(id, P_PRESS).unwrap_or(0.0) != pressed as u8 as f32;
            if paint_tracks::ready_from_rest(&mut self.scene, id, P_PRESS, 0.0, pressed as u8 as f32) {
                let anim = match press_motion {
                    Some((d, u)) => if pressed { d } else { u },
                    None => if pressed { crate::tok::motion::INSTANT.into() } else { crate::tok::motion::SNAPPY.into() },
                };
                self.scene.animate(id, P_PRESS, pressed as u8 as f32, anim);
            }
            if let Some(k) = press_scale {
                // Down follows the hand; the release springs back with a little life.
                let down = down && !self.reduced_motion;
                let want = if down { k } else { 1.0 };
                changed |= self.scene.store().target(id, P_PRESS_SCALE).unwrap_or(1.0) != want;
                if paint_tracks::ready_from_rest(&mut self.scene, id, P_PRESS_SCALE, 1.0, want) {
                    let anim = match press_motion {
                        Some((d, u)) => if down { d } else { u },
                        None => if down { crate::tok::motion::INSTANT.into() } else { crate::tok::motion::POP.into() },
                    };
                    self.scene.animate(id, P_PRESS_SCALE, want, anim);
                }
            }
            if let Some(off) = press_offset {
                let down = down && !self.reduced_motion;
                let want = if down { off } else { [0.0; 2] };
                let anim = match press_motion {
                    Some((d, u)) => if down { d } else { u },
                    None => if down { crate::tok::motion::INSTANT.into() } else { crate::tok::motion::POP.into() },
                };
                for (prop, target) in [(P_PRESS_OX, want[0]), (P_PRESS_OY, want[1])] {
                    changed |= self.scene.store().target(id, prop).unwrap_or(0.0) != target;
                    if paint_tracks::ready_from_rest(&mut self.scene, id, prop, 0.0, target) {
                        self.scene.animate(id, prop, target, anim);
                    }
                }
            }
        }
        let held = held.unwrap_or_else(|| {
            let within = self.within_next();
            let hovered = (!self.pointer.left()).then_some(self.pointer.pos).flatten().filter(|_| self.pointer.hover_may_hold()).and_then(|(x, y)| self.hover_target(x, y));
            self.decide_hold(hovered, within.as_ref(), Some(&flips))
        });
        // A leave from an element turned inert lands now; the rest wait while layout holds hover.
        // An element that drew nothing (a held hover whose row a glide carried out of view)
        // lands it rather than fading where nobody sees.
        let mut unseen: Option<Vec<bool>> = None;
        for f in flips.drain(..) {
            if held && !f.now { continue; }
            if paint_tracks::ready_from_rest(&mut self.scene, f.id, P_HOVER, 0.0, f.on as u8 as f32) {
                if unseen.get_or_insert_with(|| self.drew_nothing()).get(f.i).copied().unwrap_or(false) {
                    self.scene.set(f.id, P_HOVER, f.on as u8 as f32);
                } else {
                    self.scene.animate(f.id, P_HOVER, f.on as u8 as f32, f.anim);
                }
            }
            changed = true;
        }
        self.pointer.hover_hold.flips = flips;
        self.pointer.hover_hold.set_held(held);
        changed
    }
}

#[cfg(test)]
#[path = "../hover_tests.rs"]
mod tests;
