//! Focus is keyed retained state. Platform AX can read the same flags as the inspector.
use super::*;
use crate::edit::Key;

/// Groups whose items stack vertically: only Up and Down move between them.
const VERTICAL_ROLES: [&str; 4] = ["menu", "listbox", "list", "tree"];
/// Groups that move to the item whose label the user types.
const TYPE_SELECT_ROLES: [&str; 5] = ["menu", "listbox", "list", "tree", "tablist"];
/// Without Full Keyboard Access, Tab still visits lists inside these groups.
const LIST_ROLES: [&str; 4] = ["listbox", "list", "tree", "grid"];
/// Groups where Page Up and Page Down move by the items their scroll view shows; elsewhere the
/// keys page the scroll view itself.
const PAGED_ROLES: [&str; 2] = ["menu", "listbox"];
/// Tables and outlines stop at their ends (NSTableView); menus, popups and segments wrap.
const CLAMPED_ROLES: [&str; 3] = ["list", "tree", "grid"];
/// Roles whose pressable elements are Tab stops under `Cx::web_focus`, as HTML's buttons,
/// links and form controls are, and the items of trees and grids, which a reader moves through
/// by arrows once their group has focus (`El::focus_group` keeps the group one Tab stop).
/// Roles F6 cycles between, as a browser's and Windows' pane navigation does.
const LANDMARK_ROLES: [&str; 6] = ["banner", "navigation", "main", "complementary", "form", "search"];
const PRESSABLE_ROLES: [&str; 9] = ["button", "link", "tab", "checkbox", "switch", "radio", "combobox", "treeitem", "gridcell"];
/// The innermost open modal and the nodes input may reach while it shows: its subtree, the
/// popovers anchored inside it, and the window's caption buttons.
pub(in crate::runtime) struct ModalScope {
    index: usize,
    members: Vec<bool>,
}

/// Whether node `i` in `nodes` is not inert and has positive opacity along its ancestor chain.
fn node_visible(nodes: &[Node], i: usize) -> bool {
    let n = &nodes[i];
    !n.inert && n.vis.opacity > 0.0
        && std::iter::successors(n.parent, |&p| nodes[p].parent).all(|p| !nodes[p].inert && nodes[p].vis.opacity > 0.0)
}

/// Whether any ancestor of node `i` in `nodes` is hard-disabled (`El::disabled`, which sets
/// `focus.disabled`; `El::aria_disabled` leaves `focus.disabled` false).
pub(super) fn ancestor_disabled(nodes: &[Node], i: usize) -> bool {
    std::iter::successors(nodes[i].parent, |&p| nodes[p].parent)
        .any(|p| nodes[p].focus.disabled)
}

/// Whether node `i` in `nodes` has a non-zero laid-out box.
fn node_sized(nodes: &[Node], i: usize) -> bool {
    nodes[i].local.w > 0.0 && nodes[i].local.h > 0.0
}

/// Whether node `i` in `nodes` is visible and has a non-zero laid-out box.
fn node_shown(nodes: &[Node], i: usize) -> bool {
    node_sized(nodes, i) && node_visible(nodes, i)
}

/// Whether `ev` navigates (Tab, an arrow, Home, End, Page Up or Page Down, unmodified but for
/// Shift), which shows focus from then on, as CSS `:focus-visible` after keyboard navigation.
/// Other keys keep whatever showed: Space on a clicked button, Escape, or typing shows no ring.
/// Type-select shows it when it moves focus.
pub(in crate::runtime) fn navigates(ev: &KeyEvent) -> bool {
    !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo)
        && matches!(ev.key, Key::Tab | Key::Up | Key::Down | Key::Left | Key::Right | Key::Home | Key::End | Key::PageUp | Key::PageDown)
}

/// Pushes focus (and a secure flag) into an input's app-owned editor and mirrors it back.
fn sync_input_focus(input: &mut crate::el::InputSpec, states: crate::states::States<'_>, focused: bool) -> bool {
    let Some(access) = &input.access else {
        input.edit.focused = focused;
        return false;
    };
    let edit = access(states);
    let mut changed = false;
    edit.set_single_line(input.single_line);
    if input.edit.is_secure() && !edit.is_secure() {
        edit.set_secure(true);
        changed = true;
    }
    if let Some(revealed) = input.revealed_override
        && edit.is_revealed() != revealed
    {
        edit.set_revealed(revealed);
        changed = true;
    }
    if let Some(line_home_end) = input.line_home_end_override
        && edit.line_home_end() != line_home_end
    {
        edit.set_line_home_end(line_home_end);
        changed = true;
    }
    if edit.focused != focused || (!focused && edit.is_composing()) {
        edit.focused = focused;
        if !focused { edit.cancel_composition(); }
        edit.break_history_group();
        changed = true;
    }
    if edit.is_secure() {
        // Drop the previous copy whole so `TextEdit::drop` wipes its buffer instead of freeing
        // old bytes unwiped on growth or leaving them in spare capacity on shrink/clear.
        input.edit = edit.clone();
    } else {
        // Into the copy's own buffer: a long draft's field is copied without a new allocation, and
        // without wiping the old one.
        let mut text = std::mem::take(&mut input.edit.text);
        let kept = std::mem::take(&mut edit.text);
        input.edit = edit.clone();
        edit.text = kept;
        text.clone_from(&edit.text);
        input.edit.text = text;
    }
    changed
}

/// Whether node `i` in `nodes` is a landmark F6 cycles between.
fn is_landmark(nodes: &[Node], i: usize) -> bool {
    let n = &nodes[i];
    LANDMARK_ROLES.contains(&n.role) || (n.role == "region" && n.label.as_deref().is_some_and(|l| !l.trim().is_empty()))
}

/// Node `j` is node `i` or inside it.
fn path_inside(nodes: &[Node], j: usize, i: usize) -> bool {
    let (p, root) = (nodes[j].path.as_str(), nodes[i].path.as_str());
    p == root || root.is_empty() || (p.len() > root.len() && p.starts_with(root) && p.as_bytes()[root.len()] == b'/')
}

/// The innermost landmark around node `i`, for F6 to come back to what had focus in it.
fn landmark_of(nodes: &[Node], mut i: usize) -> Option<usize> {
    loop {
        if is_landmark(nodes, i) { return Some(i); }
        i = nodes[i].parent?;
    }
}

impl ModalScope {
    fn build(nodes: &[Node], is_exiting: impl Fn(usize) -> bool, is_unmounted: impl Fn(usize) -> bool) -> Option<Self> {
        let index = nodes.iter().enumerate().rev()
            .find(|(i, n)| n.handlers.get().is_some_and(|h| h.modal.is_some()) && !is_exiting(*i) && !is_unmounted(*i)).map(|(i, _)| i)?;
        // The caption buttons are the window's, not the view's: a modal in the view leaves them
        // to the pointer and assistive technology, as the native ones are. So are a Linux
        // frame's resize edges (`titlebar::linux`).
        let caption = |n: &Node| matches!(n.key.as_deref(), Some(crate::titlebar::BUTTONS_KEY | crate::titlebar::EDGES_KEY))
            && n.parent.is_some_and(|p| nodes[p].key.as_deref() == Some(crate::titlebar::FRAME_KEY));
        let mut members: Vec<bool> = nodes.iter().enumerate().map(|(i, n)| i == index || caption(n)).collect();
        // Portals are root siblings. Follow their anchor ownership, including chained
        // popovers, rather than requiring their layout parent to be the modal.
        loop {
            let mut changed = false;
            for (i, n) in nodes.iter().enumerate() {
                if members[i] || is_exiting(i) || is_unmounted(i) { continue; }
                let parent_inside = n.parent.is_some_and(|p| members[p]);
                let anchor_inside = n.above.as_ref().is_some_and(|a| nodes.iter().enumerate()
                    .any(|(j, anchor)| anchor.key.as_deref() == Some(&a.key) && members[j]));
                if parent_inside || anchor_inside { members[i] = true; changed = true; }
            }
            if !changed { break; }
        }
        Some(Self { index, members })
    }
}

impl focus_machine::KeyboardState {
    #[inline]
    pub(in crate::runtime) fn active_modal(&self) -> Option<usize> {
        self.modal_scope.as_ref().map(|m| m.index)
    }

    /// Node `i` is inside the open modal's scope, or no modal is open.
    #[inline]
    pub(in crate::runtime) fn modal_member(&self, i: usize) -> bool {
        self.modal_scope.as_ref().is_none_or(|m| m.members.get(i).copied().unwrap_or(false))
    }

    /// Input may reach node `i`: it's outside any `El::inert` subtree and not behind a modal.
    #[inline]
    pub(in crate::runtime) fn allowed(&self, nodes: &[Node], i: usize) -> bool {
        !nodes.get(i).is_some_and(|n| n.inert) && self.modal_member(i)
    }

    #[inline]
    pub(super) fn set_modal_scope(&mut self, scope: Option<ModalScope>) {
        self.modal_scope = scope;
    }

    pub(super) fn record_unmounted_focused(
        &mut self,
        old_nodes: &[Node],
        new_nodes: &[Node],
        virtual_inputs: &HashMap<String, Box<InputSpec>>,
    ) {
        for old in old_nodes {
            if let Some(input) = &old.input
                && input.edit.focused
                && !new_nodes.iter().any(|n| n.path == old.path || n.input.as_ref().is_some_and(|i| i.edit.same_model(&input.edit)))
                && !virtual_inputs.values().any(|i| i.edit.same_model(&input.edit))
                && !self.unmounted_focused.iter().any(|w| input.edit.matches_history(w))
            {
                self.unmounted_focused.push(input.edit.weak_history());
            }
        }
    }

    #[inline]
    pub(super) fn was_unmounted_focused(&self, edit: &crate::edit::TextEdit) -> bool {
        self.unmounted_focused.iter().any(|w| edit.matches_history(w))
    }

    pub(super) fn prune_unmounted_focused(
        &mut self,
        new_nodes: &[Node],
        virtual_inputs: &HashMap<String, Box<InputSpec>>,
    ) {
        self.unmounted_focused.retain(|w| {
            w.strong_count() > 0
                && !new_nodes.iter().any(|n| n.input.as_ref().is_some_and(|i| i.edit.matches_history(w)))
                && !virtual_inputs.values().any(|i| i.edit.matches_history(w))
        });
    }

    /// Type-select: the typed prefix picks the first item whose label starts with it. A
    /// repeated letter cycles the items starting with it. The prefix resets after a pause.
    /// When `pos` is `None` (focus is on the group container itself), search starts at index 0.
    pub(super) fn type_select(&mut self, nodes: &[Node], items: &[usize], pos: Option<usize>, typed: &str, now: f64) -> Option<usize> {
        let buffer = self.type_select_typed(typed, now).to_owned();
        let label = |j: usize| nodes[items[j]].label.as_deref().unwrap_or("").to_lowercase();
        let first = buffer.chars().next()?;
        let cycle = buffer.chars().all(|c| c == first);
        let order = (0..items.len()).map(|k| match pos {
            Some(p) if cycle => (p + 1 + k) % items.len(),
            _ => k,
        });
        let prefix = if cycle { first.to_string() } else { buffer.clone() };
        order.into_iter().find(|&j| label(j).starts_with(&prefix))
    }
}

impl RuntimeCore {
    #[inline]
    pub(crate) fn active_modal(&self) -> Option<usize> {
        self.kbd.active_modal()
    }
    /// Node `i` is inside the open modal's scope, or no modal is open.
    #[inline]
    pub(crate) fn modal_member(&self, i: usize) -> bool {
        self.kbd.modal_member(i)
    }
    pub(in crate::runtime) fn refresh_modal_index(&mut self) {
        let scope = ModalScope::build(&self.tree.nodes, |i| self.subtree_exiting(i), |i| self.subtree_unmounted(i));
        self.kbd.set_modal_scope(scope);
    }
    /// Input may reach node `i`: it's outside any `El::inert` subtree and not behind a modal.
    #[inline]
    pub(crate) fn allowed(&self, i: usize) -> bool {
        self.kbd.allowed(&self.tree.nodes, i)
    }
    /// `El::focusable`, or under `Cx::web_focus` a pressable control that didn't opt out.
    pub(crate) fn takes_focus(&self, n: &Node) -> bool {
        if n.focus.disabled || n.focus.opt_out { return false; }
        n.focus.enabled || (self.commit.defaults.web_focus && n.handlers.on_press.is_some() && PRESSABLE_ROLES.contains(&n.role))
    }
    /// Whether node `i` in `nodes` can take keyboard focus itself or is an editable text field
    /// that has not opted out (`El::disabled`, `El::focusable(false)`) and is not inside a
    /// hard-disabled ancestor (`El::disabled`).
    fn can_take_focus_in(&self, nodes: &[Node], i: usize) -> bool {
        if ancestor_disabled(nodes, i) { return false; }
        let n = &nodes[i];
        if n.focus.disabled || n.focus.opt_out { return false; }
        self.takes_focus(n) || n.input.as_ref().is_some_and(|s| s.access.is_some())
    }
    pub(crate) fn can_focus(&self, i: usize) -> bool {
        self.can_take_focus_in(&self.tree.nodes, i) && self.reachable(i)
    }
    /// Whether `i` can take focus once any modal covering it closes (same as [`Self::can_focus`]
    /// without the active modal's membership check).
    pub(super) fn can_focus_unblocked(&self, i: usize) -> bool {
        self.can_take_focus_in(&self.tree.nodes, i) && !self.tree.nodes[i].inert && !self.subtree_exiting(i) && node_shown(&self.tree.nodes, i)
    }
    /// Shown, sized, not exiting and not behind a modal.
    pub(super) fn reachable(&self, i: usize) -> bool {
        self.allowed(i) && !self.subtree_exiting(i) && node_shown(&self.tree.nodes, i)
    }
    /// Whether Return goes to focused element `i` rather than the default button: a focused
    /// link, a `focus_highlight` row, an element opted in with [`El::press_on_return`], or an
    /// item of a menu, list, tree or grid group. Under `Cx::web_focus` a focused button presses
    /// on Return too, as an HTML one does.
    fn takes_return(&self, i: usize) -> bool {
        let n = &self.tree.nodes[i];
        self.edit.link_focused(n) || n.focus.highlight.is_some()
            || ((n.focus.press_on_return || self.commit.defaults.web_focus) && n.handlers.on_press.is_some() && self.takes_focus(n))
            || self.focus_group(i).is_some_and(|g| LIST_ROLES.contains(&self.tree.nodes[g].role) || VERTICAL_ROLES.contains(&self.tree.nodes[g].role))
    }
    /// Return with no modifiers presses the window's default button (`El::default_button`)
    /// unless the focused element takes Return itself. Text fields decide in `with_focused`.
    pub(in crate::runtime) fn default_button_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if ev.key != Key::Enter || ev.mods != crate::Mods::default() { return false; }
        if self.focused_input().is_some() || self.focused_index().is_some_and(|i| self.takes_return(i)) { return false; }
        self.press_default_button(app, tx)
    }
    /// Presses the default button, if one is in reach; a held Return presses it once.
    pub(in crate::runtime) fn press_default_button(&mut self, app: &mut dyn AppDriver, tx: &mut Tx) -> bool {
        let Some(i) = (0..self.tree.nodes.len()).rev()
            .find(|&i| self.tree.nodes[i].focus.default && self.tree.nodes[i].handlers.on_press.is_some() && self.reachable(i))
        else { return false };
        if tx.key_repeat { return true; }
        let f = self.tree.nodes[i].handlers.on_press.clone().unwrap();
        log_event!(self, "default {}", self.tree.nodes[i].path);
        f(app.states(), tx);
        true
    }
    /// In `nodes` (a tree being committed), whether node `j` is a Tab stop: in the Tab order
    /// (`El::tab_index` not negative) and not an item of a roving focus group, whatever it can take
    /// now (a control just disabled still was one).
    fn tab_stop_in(&self, nodes: &[Node], j: usize) -> bool {
        if nodes[j].focus.order < 0 { return false; }
        let mut p = nodes[j].parent;
        while let Some(i) = p {
            if nodes[i].focus.group { return false; }
            if self.takes_focus(&nodes[i]) { return true; }
            p = nodes[i].parent;
        }
        true
    }
    /// In `nodes`, the focus group `j` is an item of, as [`Self::focus_group`] finds it in the
    /// committed tree.
    fn group_in(&self, nodes: &[Node], j: usize) -> Option<usize> {
        let mut p = nodes[j].parent;
        while let Some(i) = p {
            if nodes[i].focus.group { return Some(i); }
            if self.takes_focus(&nodes[i]) { return None; }
            p = nodes[i].parent;
        }
        None
    }
    /// In `nodes`, the nearest roving focus group around `j`: the one it is an item of, or the one
    /// whose item holds it (a tab's Close button is in its tab strip's group).
    fn roving_home_in(&self, nodes: &[Node], j: usize) -> Option<usize> {
        let mut p = nodes[j].parent;
        while let Some(i) = p {
            if nodes[i].focus.group { return Some(i); }
            p = nodes[i].parent;
        }
        None
    }
    /// The focus group `i` is an item of. A control inside an item (a tab's close button) is
    /// no item: it's a Tab stop of its own, as an HTML button inside a roving `tablist` is.
    pub(crate) fn focus_group(&self, i: usize) -> Option<usize> {
        let mut p = self.tree.nodes[i].parent;
        while let Some(i) = p {
            if self.tree.nodes[i].focus.group { return Some(i); }
            if self.takes_focus(&self.tree.nodes[i]) { return None; }
            p = self.tree.nodes[i].parent;
        }
        None
    }
    /// The `focus_highlight` row that `i` (a pointer target) belongs to.
    pub(in crate::runtime) fn highlight_row(&self, mut i: usize) -> Option<usize> {
        loop {
            let n = &self.tree.nodes[i];
            if self.takes_focus(n) { return (n.focus.highlight.is_some() && self.can_focus(i)).then_some(i); }
            i = n.parent?;
        }
    }
    #[inline]
    pub(in crate::runtime) fn highlight_visible(&self, i: usize) -> bool {
        self.kbd.highlight_visible(&self.tree.nodes, i)
    }
    /// Focus is on node `i` or inside it (CSS `:focus-within`), however it got there.
    #[inline]
    pub(in crate::runtime) fn focus_within(&self, i: usize) -> bool {
        self.kbd.focus_within(&self.tree.nodes, i)
    }
    pub(in crate::runtime) fn focus_ring_visible(&self, mut i: usize) -> bool {
        // `Cx::force_focus_rings`: the ring of whichever control owns node `i`.
        if self.commit.defaults.force_focus_rings {
            loop {
                if self.can_focus(i) { return true; }
                let Some(p) = self.tree.nodes[i].parent else { return false };
                i = p;
            }
        }
        if !self.kbd.focus.keyboard() { return false; }
        loop {
            if self.kbd.focus.is(&self.tree.nodes[i].path) { return true; }
            // A nested focusable owns its ring, never inherit another control's focus.
            if self.takes_focus(&self.tree.nodes[i]) { return false; }
            let Some(p) = self.tree.nodes[i].parent else { return false };
            i = p;
        }
    }
    /// The focused element that draws the default ring, if any (see `El::no_focus_ring`).
    pub(in crate::runtime) fn default_ring_owner(&self) -> Option<usize> {
        if !self.kbd.focus.keyboard() { return None; }
        let i = self.focused_index()?;
        let n = &self.tree.nodes[i];
        // A focused link shows its focus with its own hover look.
        // A modal that holds focus itself (`Cx::web_focus`) shows none, as a web dialog doesn't.
        if n.input.is_some() || n.focus.highlight.is_some() || self.edit.link_focused(n) || !self.can_focus(i) { return None; }
        if self.focus_group(i).is_some_and(|g| LIST_ROLES.contains(&self.tree.nodes[g].role) || VERTICAL_ROLES.contains(&self.tree.nodes[g].role)) {
            return None;
        }
        let own = self.tree.nodes.iter().enumerate()
            .any(|(j, m)| (m.focus.ring.is_some() || m.focus.ring_off) && self.focus_ring_visible(j));
        (!own).then_some(i)
    }
    /// Under `Cx::force_focus_rings`, whether node `i` draws the default ring: a control that
    /// can take focus and would draw it when focused (see [`Self::default_ring_owner`]), with no
    /// ring of its own on it or on a part of it.
    pub(in crate::runtime) fn forced_default_ring(&self, i: usize) -> bool {
        if !self.commit.defaults.force_focus_rings || !self.can_focus(i) { return false; }
        let n = &self.tree.nodes[i];
        if n.input.is_some() || n.focus.highlight.is_some() || links::link_count(n) > 0 { return false; }
        if self.focus_group(i).is_some_and(|g| LIST_ROLES.contains(&self.tree.nodes[g].role) || VERTICAL_ROLES.contains(&self.tree.nodes[g].role)) {
            return false;
        }
        // A part's own ring belongs to its nearest focusable ancestor-or-self.
        !self.tree.nodes.iter().enumerate().any(|(j, m)| (m.focus.ring.is_some() || m.focus.ring_off) && self.owner_of(j) == Some(i))
    }
    /// The nearest node at or above `j` that can take focus.
    fn owner_of(&self, mut j: usize) -> Option<usize> {
        loop {
            if self.can_focus(j) { return Some(j); }
            j = self.tree.nodes[j].parent?;
        }
    }
    /// The view's `Cx::focus_ring`, else AppKit's: 3 points just outside the border box.
    pub(in crate::runtime) fn default_focus_ring(&self) -> crate::el::Outline {
        self.commit.defaults.focus_ring
            .unwrap_or(crate::el::Outline { width: 3.0, offset: 0.0, color: self.host.system_accent.focus(self.env.system_dark) })
    }
    pub(crate) fn set_focus(&mut self, app: &mut dyn AppDriver, path: Option<String>, keyboard: bool) -> bool {
        let (changed, cb_changed) = self.set_focus_notifying(app, path, keyboard);
        changed || cb_changed
    }
    pub(in crate::runtime) fn set_focus_notifying(&mut self, app: &mut dyn AppDriver, path: Option<String>, keyboard: bool) -> (bool, bool) {
        let old_path = self.kbd.focus.owned();
        let moved = old_path.as_deref() != path.as_deref();
        let changed = self.set_focus_entry(app, path, keyboard, false, false);
        let mut cb_changed = false;
        if moved && let Some((dirty, mut cb_tx)) = self.notify_focus_change(app, old_path.as_deref(), None) {
            cb_changed = dirty;
            if dirty { self.commit.dirty = true; }
            self.finish_focus_tx(app, &mut cb_tx);
        }
        (changed, cb_changed)
    }
    fn finish_focus_tx(&mut self, app: &mut dyn AppDriver, cb_tx: &mut Tx) {
        let wakes = std::mem::take(&mut cb_tx.wakes);
        self.latch_requests(app, cb_tx);
        if !wakes.is_empty() { self.schedule_wakes(wakes); }
        merge(&mut self.frame.pending, std::mem::take(cb_tx));
    }
    /// Delivers `El::on_focus_change` and `El::on_focus_within_change` for a transition from
    /// `old_path` to the current `self.kbd.focus.path()`, leaves before enters, innermost first.
    fn notify_focus_change(&mut self, app: &mut dyn AppDriver, old_path: Option<&str>, old_nodes: Option<&[Node]>) -> Option<(bool, Tx)> {
        let new_path = self.kbd.focus.owned();
        let new_ref = new_path.as_deref();
        let calls: Vec<(crate::el::WindowFocusFn, bool)> = {
            let find = |path: &str, within: bool, allow_old: bool| -> Option<crate::el::WindowFocusFn> {
                let pick = |n: &Node| {
                    let h = n.handlers.get()?;
                    if within { h.on_focus_within_change.clone() } else { h.on_focus_change.clone() }
                };
                self.tree.nodes.iter().enumerate()
                    .find(|&(i, n)| n.path == path && (allow_old || old_nodes.is_some() || !self.subtree_exiting(i)))
                    .and_then(|(_, n)| pick(n))
                    .or_else(|| allow_old.then(|| old_nodes?.iter().find(|n| !n.is_moved_stub() && n.path == path).and_then(pick)).flatten())
            };
            let mut out = Vec::new();
            if let Some(old) = old_path {
                if let Some(f) = find(old, false, true) { out.push((f, false)); }
                for anc in ancestor_paths(old) {
                    if new_ref.is_some_and(|n| path_holds(anc, n)) { break; }
                    if let Some(f) = find(anc, true, true) { out.push((f, false)); }
                }
            }
            if let Some(new) = new_ref {
                if let Some(f) = find(new, false, false) { out.push((f, true)); }
                for anc in ancestor_paths(new) {
                    if old_path.is_some_and(|o| path_holds(anc, o)) { break; }
                    if let Some(f) = find(anc, true, false) { out.push((f, true)); }
                }
            }
            out
        };
        if calls.is_empty() { return None; }
        let mut out = Tx::default();
        let mut dirty = false;
        for (f, focused) in calls {
            let mut one = self.transaction(app);
            f(app.states(), focused, &mut one);
            if std::mem::take(&mut one.live_only) {
                one.commit = Commit::Animate;
                one.motion = None;
                one.follow_edge = false;
                one.color_motion = None;
            } else {
                dirty = true;
            }
            merge(&mut out, one);
        }
        Some((dirty, out))
    }
    fn set_focus_entry(&mut self, app: &mut dyn AppDriver, path: Option<String>, keyboard: bool, keep_entry: bool, sync_all: bool) -> bool {
        self.edit.on_input_focus_change(path.as_deref());
        let moved = self.kbd.focus.path() != path.as_deref();
        // A field left mid-composition keeps what it showed, as a browser's does (Tab the input
        // method let through, a click on another control, `Tx::focus`).
        if moved && self.focused_input().is_some() && self.composing(app) {
            let mut tx = self.transaction(app);
            self.finish_field_composition(app, &mut tx);
            self.latch_requests(app, &mut tx);
            merge(&mut self.frame.pending, tx);
        }
        // Tabbing away or into a field takes the selection's keyboard; a hovered row doesn't.
        if moved
            && let Some(p) = path.as_deref()
            && self.text_selection().is_some_and(|(held, _)| !path_holds(p, held))
            && (keyboard || self.node(p).is_some_and(|n| n.input.is_some()) || self.commit.virtual_inputs.contains_key(p))
        {
            self.clear_text_selection();
        }
        let mut changed = moved && self.physical_focus_lost(app);
        let prev = self.kbd.focus.path();
        let next = path.as_deref();
        let needs_sync = |p: &str, input: &crate::el::InputSpec, focused: bool| {
            prev == Some(p)
                || next == Some(p)
                || (sync_all
                    && (input.edit.focused != focused
                        || (!focused && input.edit.is_composing())
                        || input.edit.is_secure()
                        || input.revealed_override.is_some()
                        || input.line_home_end_override.is_some()))
        };
        let mut reveal_caret = false;
        for n in &mut self.tree.nodes {
            if let Some(input) = &mut n.input {
                let focused = next == Some(n.path.as_str());
                if moved && focused && input.reveal_caret_on_focus {
                    reveal_caret = true;
                }
                if needs_sync(n.path.as_str(), input, focused) {
                    changed |= sync_input_focus(input, app.states(), focused);
                }
            }
        }
        for (p, input) in &mut self.commit.virtual_inputs {
            let focused = next == Some(p.as_str());
            if moved && focused && input.reveal_caret_on_focus {
                reveal_caret = true;
            }
            if needs_sync(p.as_str(), input, focused) {
                changed |= sync_input_focus(input, app.states(), focused);
            }
        }
        if reveal_caret {
            self.edit.caret_follow = true;
        }
        let shown = (self.kbd.focus.keyboard(), self.kbd.focus.shown());
        for effect in self.kbd.focus.step(focus_machine::FocusEvent::Set { path, keyboard, keep_entry }) {
            match effect {
                focus_machine::FocusEffect::Moved => {
                    self.ime_area = None;
                    self.edit.ime_reset = true;
                    self.edit.blur_links();
                }
            }
        }
        if let Some((mark, at)) = self.focused_index().and_then(|f| landmark_of(&self.tree.nodes, f).map(|m| (m, f))) {
            let (mark, at) = (self.tree.nodes[mark].path.clone(), self.tree.nodes[at].path.clone());
            self.kbd.focus.remember_scope(mark, at);
        }
        self.sync_side_focus(app);
        if moved {
            self.cancel_pending_caret();
            if let Some(fi) = self.focused_index() && self.is_caret_browsing_node(&self.tree.nodes[fi]) {
                self.enter_caret_browsing_focus(fi);
            }
        }
        // A hover group keyboard focus enters or leaves hovers or rests now (`El::hover_group`).
        // Focus that stayed where it was, shown as it was (every commit's focus pass, most of
        // the time), moves no group: the frame's own interaction pass sees the rest.
        let regrouped = moved || shown != (self.kbd.focus.keyboard(), self.kbd.focus.shown());
        if regrouped && self.tree.nodes.iter().any(|n| n.vis_flags.hover_group()) {
            changed |= self.update_interaction(None);
        }
        changed
    }
    pub(in crate::runtime) fn sync_requested_focus(&mut self, app: &mut dyn AppDriver, nodes: &mut Vec<Node>) -> bool {
        // A `Tx::focus_when_shown` still owed stands until the user's own input or another ask,
        // whatever focus did meanwhile (a closing palette leaves it nowhere, then on the prompt).
        let owed = self.kbd.focus_owed.take().map(|(key, ring)| (key, ring, crate::el::FocusAsk::WhenShown));
        let request = self.kbd.focus_request.take();
        let blurred = matches!(request, Some(focus_machine::FocusRequest::Blur));
        let asked = match request {
            Some(focus_machine::FocusRequest::Focus(key, ring, ask)) => Some((key, ring, ask)),
            Some(focus_machine::FocusRequest::Blur) | None => owed,
        };
        if blurred { self.kbd.focus_before_commit = None; }
        let waits = asked.as_ref().filter(|(.., ask)| *ask == crate::el::FocusAsk::WhenShown).map(|(key, ring, _)| (key.clone(), *ring));
        self.kbd.record_unmounted_focused(&self.tree.nodes, nodes, &self.commit.virtual_inputs);
        let within = |path: &str, root: &str| {
            path == root || (path.len() > root.len() && path.starts_with(root) && path.as_bytes()[root.len()] == b'/')
        };
        let shown = |idx: usize| node_shown(nodes, idx);
        let first_link = |j: usize| {
            (nodes[j].input.is_none() && links::link_count(&nodes[j]) > 0).then_some(0)
        };
        let link_target = |key: &str| {
            (0..nodes.len()).find_map(|j| {
                if !shown(j) || nodes[j].input.is_some() || !self.can_take_focus_in(nodes, j) { return None; }
                let k = nodes[j].text.as_ref()?.cold.links.iter().position(|l| l.id == key)?;
                Some((j, k))
            })
        };
        let restore = (!blurred).then(|| self.tree.nodes.iter().enumerate().rev().find_map(|(i, old)| {
            let key = old.focus.return_to.as_deref()?;
            if self.subtree_exiting(i) || nodes.iter().any(|n| n.path == old.path) { return None; }
            let focused = self.kbd.focus.now_or_blurred()?;
            let has_children = self.tree.nodes.iter().any(|m| m.parent == Some(i));
            let scope = if !has_children && !self.takes_focus(old) && old.input.is_none()
                && old.local.w == 0.0 && old.local.h == 0.0 {
                let p = self.tree.nodes.get(old.parent?)?;
                if p.is_moved_stub() || p.path.is_empty() {
                    nodes.get(*self.tree.node_index.get(&p.id)?)?.path.as_str()
                } else {
                    p.path.as_str()
                }
            } else {
                old.path.as_str()
            };
            if !within(focused, scope) { return None; }
            let by_key = nodes.iter().find(|n| n.key.as_deref() == Some(key)).and_then(|anchor| {
                let candidates = (0..nodes.len()).filter(|&j| within(&nodes[j].path, &anchor.path) && shown(j)
                    && self.can_take_focus_in(nodes, j));
                candidates.clone().find(|&j| nodes[j].focus.selected)
                    .or_else(|| candidates.into_iter().next())
                    .map(|j| (j, first_link(j)))
            });
            let (j, link) = by_key.or_else(|| link_target(key).map(|(j, k)| (j, Some(k))))?;
            Some((nodes[j].path.clone(), link))
        })).flatten();
        // `Tx::focus`: the first shown element with the key, or its first focusable child,
        // else a shown text's `Link` with that id.
        let asked = asked.and_then(|(key, ring, ask)| {
            let by_key = (0..nodes.len()).find(|&j| nodes[j].key.as_deref() == Some(key.as_str()) && shown(j))
                .and_then(|anchor| {
                    (anchor..nodes.len()).take_while(|&j| within(&nodes[j].path, &nodes[anchor].path))
                        .find(|&j| shown(j) && self.can_take_focus_in(nodes, j))
                        .map(|j| (j, first_link(j)))
                });
            let (j, link) = by_key.or_else(|| link_target(key.as_str()).map(|(j, k)| (j, Some(k))))?;
            Some((nodes[j].path.clone(), link, ring, ask))
        });
        let input_focused = self.kbd.focus.path().is_some_and(|p| {
            nodes.iter().enumerate().any(|(j, n)| n.path == p && !n.inert && !n.focus.opt_out && !ancestor_disabled(nodes, j) && n.input.as_ref().is_some_and(|i| i.edit.focused))
                || self.commit.virtual_inputs.get(p).is_some_and(|i| i.edit.focused)
        });
        let was_focused = |edit: &crate::edit::TextEdit, path: &str| {
            self.tree.nodes.iter().any(|old| {
                old.input.as_ref().is_some_and(|i| i.edit.focused && (old.path == path || i.edit.same_model(edit)))
            }) || (input_focused && self.kbd.was_unmounted_focused(edit))
        };
        let virtual_req = || self.commit.virtual_inputs.iter()
            .filter(|(_, i)| i.edit.focused)
            .map(|(p, i)| (p.as_str(), &i.edit));
        // A newly requested text field wins; a focused control otherwise retains focus. Nothing
        // inside an `El::inert` subtree or disabled by `El::disabled` takes or keeps it.
        let requested = (0..nodes.len())
            .filter(|&j| shown(j) && !nodes[j].focus.opt_out && !ancestor_disabled(nodes, j) && nodes[j].input.as_ref().is_some_and(|i| i.edit.focused))
            .map(|j| (nodes[j].path.as_str(), &nodes[j].input.as_ref().unwrap().edit));
        // Hash iteration must not decide between simultaneous offscreen requests. Select
        // only the winning path rather than allocating and sorting every focused field.
        let newly_requested = |p: &str, edit: &crate::edit::TextEdit| {
            !self.kbd.focus.is(p) && !was_focused(edit, p)
        };
        let next = requested.clone().find(|&(p, edit)| newly_requested(p, edit))
            .or_else(|| virtual_req().filter(|&(p, edit)| newly_requested(p, edit)).min_by_key(|&(p, _)| p))
            .map(|(p, _)| p.to_owned())
            .or_else(|| asked.as_ref().map(|(p, ..)| p.clone()))
            .or_else(|| restore.as_ref().map(|(p, _)| p.clone()))
            .or_else(|| (!blurred).then(|| self.kbd.focus.owned().filter(|p| (0..nodes.len()).any(|j| &nodes[j].path == p
                && (shown(j) || (node_visible(nodes, j) && nodes[j].input.is_some()))
                && !ancestor_disabled(nodes, j)
                && (self.takes_focus(&nodes[j]) || (!nodes[j].focus.opt_out && nodes[j].input.as_ref().is_some_and(|i| i.edit.focused))))
                || self.commit.virtual_inputs.get(p).is_some_and(|i| i.edit.focused))).flatten())
            .or_else(|| (!blurred).then(|| requested.into_iter().next().or_else(|| virtual_req().min_by_key(|&(p, _)| p)).map(|(p, _)| p.to_owned())).flatten())
            .or_else(|| (!blurred).then(|| self.focus_fallback(nodes)).flatten());
        // State may have deleted a row. Only the new view's accessors are valid now.
        // Use the same focus path as input events, then restore the old presentation
        // tree so apply can retain its exit visuals.
        let explicit = asked.as_ref().filter(|(p, ..)| next.as_deref() == Some(p.as_str()));
        let restored = explicit.is_none().then(|| restore.as_ref().filter(|(p, _)| next.as_deref() == Some(p.as_str()))).flatten();
        let target_link = explicit.map(|(_, link, ..)| *link).or_else(|| restored.map(|(_, link)| *link));
        let keep_entry = explicit.is_none() && next.as_deref() == self.kbd.focus.path();
        let keyboard = explicit.map_or_else(|| self.kbd.focus.keyboard(), |(_, _, ring, _)| *ring);
        self.kbd.set_focus_returned(matches!(explicit, Some((_, _, _, crate::el::FocusAsk::Return))));
        if asked.is_none() && let Some(owed) = waits { self.kbd.focus_owed = Some(owed); }
        let old_path = self.kbd.focus.owned();
        let moved = old_path.as_deref() != next.as_deref();
        let was_built = std::mem::replace(&mut self.tree.built, false);
        std::mem::swap(&mut self.tree.nodes, nodes);
        self.nodes_replaced();
        let mut changed = self.set_focus_entry(app, next, keyboard, keep_entry, true);
        if let Some(link) = target_link {
            match link {
                Some(k) => self.edit.set_link_focus(k),
                None => self.edit.blur_links(),
            }
        } else if let Some(i) = self.focused_index()
            && self.tree.nodes[i].input.is_none()
            && !self.edit.link_focused(&self.tree.nodes[i])
        {
            self.edit.enter_links(&self.tree.nodes[i], false);
        }
        let cb = if moved { self.notify_focus_change(app, old_path.as_deref(), Some(nodes)) } else { None };
        std::mem::swap(&mut self.tree.nodes, nodes);
        self.tree.built = was_built;
        self.nodes_replaced();
        if let Some((dirty, mut cb_tx)) = cb {
            changed |= dirty;
            self.finish_focus_tx(app, &mut cb_tx);
        }
        if blurred && self.kbd.focus.path().is_none() {
            // An explicit blur is not the temporary pointer-down blur of a scrim, and nothing
            // hands focus on from it: only a control the user blurred and then left does.
            self.kbd.focus.step(focus_machine::FocusEvent::Dropped);
            self.kbd.focus.step(focus_machine::FocusEvent::Settled);
        }
        if self.edit.input_click.dragging_path().is_some_and(|p| !nodes.iter().any(|n| n.path == p && !n.inert && n.input.is_some())) {
            self.edit.drop_input_drag();
        }
        if !self.pointer.press.is_held() {
            self.kbd.focus.step(focus_machine::FocusEvent::Settled);
        }
        self.kbd.prune_unmounted_focused(nodes, &self.commit.virtual_inputs);
        changed
    }
    /// Where focus goes when the element holding it leaves the tree or can't hold it anymore (a
    /// deleted row, a closed tab, a decided prompt, a control disabled while focused, a pane closed
    /// under it) and nothing else asked for it. It and [`Self::fallback_in_new`] (a region kept
    /// from an earlier commit, or a control rebuilt disabled) share one rule, [`rescue`], looking
    /// first inside its nearest container that survived and then further out:
    /// - **It was a Tab stop:** the next Tab stop after it in that container, else the one before,
    ///   else the container itself when it is one, then the same in the next container out; only
    ///   when no container out to the root has a Tab stop left does focus go to any control. A
    ///   keyboard user lands where Tab would go, as OpenCode Desktop's focus rescue does
    ///   (opencode#64909), not on a control Tab never reaches.
    /// - **Before either:** focus that was in a roving group (on an item, or on a control inside one,
    ///   a tab's Close button) goes to another of that group's items, the next, else the previous:
    ///   the next tab, not the next tab's Close button.
    /// - **It wasn't** (an item of a roving group, a control out of the Tab order): the next control
    ///   after it that can take focus, else the one before, else the container, as a native list
    ///   hands its selection to the neighbour.
    ///
    /// Without a rescue focus fell to nothing: a screen reader went back to the window and Tab
    /// started over at its top. A modal's own focus is left to the modal machine, which returns
    /// it where it was.
    fn focus_fallback(&self, nodes: &[Node]) -> Option<String> {
        // A field that Escape blurred and that then closed (a comment box cancelled, gone once
        // its exit ends) counts as focused: the keyboard was on it, and nothing took focus since.
        let gone = self.kbd.focus.path().or_else(|| self.kbd.focus.now_or_blurred().or(self.kbd.focus.tab_start()).filter(|_| self.kbd.focus.keyboard()))?;
        // `Tx::blur` says nothing has focus: it hands nothing on, unlike Escape or a deleted row.
        if self.kbd.focus.path().is_none() && !self.kbd.focus.handoff() { return None; }
        let Some(at) = self.tree.nodes.iter().position(|n| n.path == gone) else {
            // An offscreen field had no layout node. Only a moved node lost its path, or one whose
            // rebuilt replacement took it over: a control rebuilt disabled (Send as its prompt
            // sends) still has to hand focus on, which `fallback_in_new` does from the new tree.
            let old = self.tree.nodes.get(*self.tree.node_index.get(self.tree.path_ids.get(gone)?)?)?;
            let disabled_here = nodes.iter().enumerate().any(|(j, n)| n.path == gone && !self.can_take_focus_in(nodes, j));
            if !old.is_moved_stub() && !disabled_here { return None; }
            return self.fallback_in_new(gone, nodes);
        };
        // The control that held it is still in the tree, even when a model cleared its flag or a
        // commit hid it: nothing takes its keyboard. One that can't hold focus any more (disabled
        // while focused, as a Send button is while its prompt sends) hands it on as if it had left,
        // as OpenCode Desktop's focus rescue does (opencode#64909); focus never falls to the window.
        if nodes.iter().enumerate().any(|(j, n)| n.path == gone && self.can_take_focus_in(nodes, j)) { return None; }
        if self.kbd.focus.path().is_none() && self.tree.nodes[at].input.is_none() { return None; }
        if self.active_modal().is_some() && self.modal_member(at) { return None; }
        let new: rustc_hash::FxHashMap<&str, usize> = nodes.iter().enumerate().map(|(j, n)| (n.path.as_str(), j)).collect();
        // Building the new tree hands an old node's path and id to the node that replaces it and
        // leaves it without a path, so a control still here can look absent. Resolve those by id.
        let at_new = |i: usize| -> Option<usize> {
            let n = &self.tree.nodes[i];
            if !n.path.is_empty() { return new.get(n.path.as_str()).copied(); }
            self.tree.node_index.get(&n.id).copied()
                .filter(|&j| nodes.get(j).is_some_and(|m| m.id == n.id))
        };
        // A node built this commit may have no box yet, and is judged by the size its old node
        // had; one laid out keeps its own box, so a collapsed or hidden control takes no focus.
        let unlaid = |j: usize| nodes[j].local.w <= 0.0 && nodes[j].local.h <= 0.0;
        let survivor = |i: usize| at_new(i)
            .filter(|&j| (node_shown(nodes, j) || (unlaid(j) && node_sized(&self.tree.nodes, i) && node_visible(nodes, j)))
                && self.can_take_focus_in(nodes, j));
        let within = |j: usize, s: usize| {
            // Compare in the new tree where both ends are in it; a node that left keeps its old
            // path, so the scan still reaches the siblings after it.
            let (p, root) = match (at_new(j), at_new(s)) {
                (Some(p), Some(r)) => (nodes[p].path.as_str(), nodes[r].path.as_str()),
                _ => (self.tree.nodes[j].path.as_str(), self.tree.nodes[s].path.as_str()),
            };
            root.is_empty() || (p.len() > root.len() && p.starts_with(root) && p.as_bytes()[root.len()] == b'/')
        };
        // Each surviving container, nearest first, with its descendants' span in the old tree.
        let mut scopes = Vec::new();
        let mut scope = self.tree.nodes[at].parent;
        while let Some(s) = scope {
            if at_new(s).is_some() {
                scopes.push((s, (s + 1..self.tree.nodes.len()).find(|&j| !within(j, s)).unwrap_or(self.tree.nodes.len())));
            }
            scope = self.tree.nodes[s].parent;
        }
        // The container itself counts after its children, when it takes focus (a list row whose
        // comment box closed under it).
        // Focus that was in a roving group (on an item, or on a control inside one: a tab's Close
        // button) goes first to another item of that group, where the keys move inside it (the
        // next tab, not the next tab's Close button).
        let home = self.roving_home_in(&self.tree.nodes, at).and_then(at_new);
        let item = |i: usize| survivor(i).filter(|&j| home.is_some() && self.group_in(nodes, j) == home);
        let tab = |i: usize| survivor(i).filter(|&j| self.tab_stop_in(nodes, j));
        let lost_tab_stop = self.tree.nodes[at].focus.order >= 0 && self.focus_group(at).is_none();
        rescue(lost_tab_stop, at, &scopes, &item, &tab, &survivor).map(|j| nodes[j].path.clone())
    }
    /// [`Self::focus_fallback`] when the old tree holds the part that had focus only as a moved
    /// stub (a region kept from an earlier commit, a virtual row): the nearest container of it in
    /// the new tree takes focus when it can (a diff line whose comment box closed), else its first
    /// control.
    /// A control still here that can't take focus any more (disabled while focused, inside a
    /// region kept from an earlier commit) hands it on as [`Self::focus_fallback`] does: the next
    /// control after it in its nearest container that has one, else the one before, else that
    /// container.
    fn fallback_in_new(&self, gone: &str, nodes: &[Node]) -> Option<String> {
        if self.active_modal().is_some() { return None; }
        let can = |j: usize| node_shown(nodes, j) && self.can_take_focus_in(nodes, j);
        if let Some(at) = nodes.iter().position(|n| n.path == gone) {
            if self.can_take_focus_in(nodes, at) { return None; }
            let mut scopes = Vec::new();
            let mut p = gone;
            while let Some(k) = p.rfind('/') {
                p = &p[..k];
                let Some(c) = nodes.iter().position(|n| n.path == p) else { continue };
                let inside = |m: usize| nodes[m].path.len() > p.len() && nodes[m].path.starts_with(p) && nodes[m].path.as_bytes()[p.len()] == b'/';
                scopes.push((c, (c + 1..nodes.len()).find(|&m| !inside(m)).unwrap_or(nodes.len())));
            }
            let any = |m: usize| can(m).then_some(m);
            let home = self.roving_home_in(nodes, at);
            let item = |m: usize| (can(m) && home.is_some() && self.group_in(nodes, m) == home).then_some(m);
            let tab = |m: usize| (can(m) && self.tab_stop_in(nodes, m)).then_some(m);
            return rescue(self.tab_stop_in(nodes, at), at, &scopes, &item, &tab, &any).map(|m| nodes[m].path.clone());
        }
        let mut p = gone;
        while let Some(k) = p.rfind('/') {
            p = &p[..k];
            let Some(j) = nodes.iter().position(|n| n.path == p) else { continue };
            let inside = |m: usize| nodes[m].path.len() > p.len() && nodes[m].path.starts_with(p) && nodes[m].path.as_bytes()[p.len()] == b'/';
            return std::iter::once(j).chain((j + 1..nodes.len()).take_while(|&m| inside(m))).find(|&m| can(m)).map(|m| nodes[m].path.clone());
        }
        None
    }
    pub(in crate::runtime) fn tab_order(&self) -> Vec<usize> {
        let all: Vec<_> = (0..self.tree.nodes.len()).filter(|&i| self.can_focus(i) && self.tree.nodes[i].focus.order >= 0).collect();
        let mut order: Vec<_> = all.iter().copied().filter(|&i| {
            let Some(g) = self.focus_group(i) else { return true; };
            let chosen = all.iter().copied().filter(|&j| self.focus_group(j) == Some(g))
                .find(|&j| self.kbd.focus.is(&self.tree.nodes[j].path))
                .or_else(|| all.iter().copied().find(|&j| self.focus_group(j) == Some(g) && self.tree.nodes[j].focus.selected))
                .or_else(|| all.iter().copied().find(|&j| self.focus_group(j) == Some(g)));
            chosen == Some(i)
        }).collect();
        order.sort_by_key(|&i| { let n = self.tree.nodes[i].focus.order; (if n > 0 { n } else { i32::MAX }, i) });
        order
    }
    /// F6 (Shift+F6 back): focus moves to the next landmark (`banner`, `navigation`, `main`,
    /// `complementary`, `form`, `search`, a labelled `region`) that has a Tab stop, onto the
    /// control focused last inside it, else its first Tab stop, as browsers and Windows apps
    /// cycle panes. Inside a modal it does nothing: the modal is the only pane.
    fn cycle_landmark(&mut self, app: &mut dyn AppDriver, back: bool, tx: &mut Tx) -> bool {
        if self.active_modal().is_some() { return false; }
        let order = self.tab_order();
        let marks: Vec<(usize, usize)> = (0..self.tree.nodes.len())
            .filter(|&i| is_landmark(&self.tree.nodes, i) && self.allowed(i) && !self.subtree_exiting(i) && self.tree.nodes[i].local.w > 0.0 && self.tree.nodes[i].local.h > 0.0)
            .filter_map(|i| order.iter().copied().find(|&j| path_inside(&self.tree.nodes, j, i)).map(|first| (i, first)))
            .collect();
        if marks.is_empty() { return false; }
        let here = self.focused_index().and_then(|f| marks.iter().rposition(|&(m, _)| path_inside(&self.tree.nodes, f, m)));
        let next = match here {
            Some(k) if back => (k + marks.len() - 1) % marks.len(),
            Some(k) => (k + 1) % marks.len(),
            None if back => marks.len() - 1,
            None => 0,
        };
        let (mark, first) = marks[next];
        let remembered = self.kbd.focus.scope_memory(&self.tree.nodes[mark].path)
            .and_then(|p| order.iter().copied().find(|&j| self.tree.nodes[j].path == p));
        let target = remembered.unwrap_or(first);
        let path = self.tree.nodes[target].path.clone();
        let reveal_caret = self.tree.nodes[target].input.as_ref().is_some_and(|i| i.reveal_caret_on_focus);
        self.set_focus(app, Some(path.clone()), true);
        if !reveal_caret {
            tx.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, path));
        }
        true
    }
    /// `shown`: the focus highlight was visible before this key arrived.
    pub(in crate::runtime) fn focus_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, shown: bool, tx: &mut Tx) -> bool {
        // WAI-ARIA Select-Only Combobox: Alt+Down opens a focused combobox popup.
        if ev.mods.alt && !(ev.mods.cmd || ev.mods.ctrl || ev.mods.logo || ev.mods.shift) && ev.key == Key::Down {
            let Some(i) = self.focused_index().filter(|&i| self.can_focus(i)) else { return false; };
            if self.tree.nodes[i].role == "combobox" && self.tree.nodes[i].input.is_none() {
                if tx.key_repeat { return true; }
                self.kbd.focus.step(focus_machine::FocusEvent::Navigated);
                if let Some(f) = self.tree.nodes[i].handlers.on_press.clone() { f(app.states(), tx); }
                return true;
            }
            return false;
        }
        if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo { return false; }
        // Activations (dismiss, press, open) fire once per press. Their repeats are consumed so
        // they reach nothing else; navigation below keeps repeating.
        let repeat = tx.key_repeat;
        if ev.key == Key::Escape && let Some(i) = self.active_modal() {
            if repeat { return true; }
            if let Some(f) = self.tree.nodes[i].handlers.modal.as_ref().and_then(|m| m.dismiss.clone()) {
                self.set_focus(app, None, self.kbd.focus.keyboard());
                f(app.states(), tx);
            }
            return true;
        }
        if ev.key == Key::Function(6) {
            return self.cycle_landmark(app, ev.mods.shift, tx);
        }
        if ev.key == Key::Tab {
            if self.focused_index().is_some_and(|i| self.edit.link_tab(&self.tree.nodes[i], ev.mods.shift)) { return true; }
            let mut order = self.tab_order();
            if !self.kbd.tab_visits_all() {
                // A control that owns its keyboard (`El::on_key`) is a list of its own.
                order.retain(|&i| self.tree.nodes[i].input.is_some() || self.tree.nodes[i].focus.key.is_some()
                    || self.is_caret_browsing_node(&self.tree.nodes[i])
                    || self.focus_group(i).is_some_and(|g| LIST_ROLES.contains(&self.tree.nodes[g].role)));
            }
            if order.is_empty() { return self.active_modal().is_some(); }
            let current = order.iter().position(|&i| self.kbd.focus.is(&self.tree.nodes[i].path));
            // With nothing focused, Tab goes on from where focus was or the pointer pressed last;
            // from an element outside the Tab order (`tab_index(-1)`, a region a request focused),
            // it goes on from that element's place, as a browser's does.
            let from = self.kbd.focus.tab_start().filter(|_| self.commit.defaults.web_focus).and_then(|p| self.tree.nodes.iter().position(|n| n.path == p))
                .or_else(|| self.focused_index().filter(|_| current.is_none()));
            let next = match (current, from) {
                (Some(i), _) if ev.mods.shift => (i + order.len() - 1) % order.len(),
                (Some(i), _) => (i + 1) % order.len(),
                (None, Some(k)) if ev.mods.shift => order.iter().rposition(|&j| j < k).unwrap_or(order.len() - 1),
                (None, Some(k)) => order.iter().position(|&j| j > k).unwrap_or(0),
                (None, None) if ev.mods.shift => order.len() - 1,
                (None, None) => 0,
            };
            let target = order[next];
            let path = self.tree.nodes[target].path.clone();
            let reveal_caret = self.tree.nodes[target].input.as_ref().is_some_and(|i| i.reveal_caret_on_focus);
            self.set_focus(app, Some(path.clone()), true);
            if !reveal_caret {
                tx.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, path));
            }
            self.edit.enter_links(&self.tree.nodes[target], ev.mods.shift);
            return true;
        }
        let Some(i) = self.focused_index() else { return false; };
        if !self.can_focus(i) { return false; }
        // Past Tab, a Shift-modified key navigates and presses nothing: it goes on to the app.
        // Typed text still type-selects (Shift-A is "A").
        if ev.mods.shift && !matches!(&ev.key, Key::Text(t) if !t.trim().is_empty()) { return false; }
        // A slider's keys come before its group's arrows. On a text field (a spin button),
        // Left and Right stay with the caret.
        if !self.node_disabled(i) && let Some(step) = self.tree.nodes[i].focus.step.clone() {
            // A right-to-left slider runs from its right: Left raises it.
            let rtl = self.tree.nodes[i].flow == crate::el::Flow::Rtl;
            let field = self.tree.nodes[i].input.is_some();
            let delta = match ev.key {
                Key::Left if !field => Some(if rtl { 1 } else { -1 }),
                Key::Right if !field => Some(if rtl { -1 } else { 1 }),
                Key::Down => Some(-1), Key::Up => Some(1),
                Key::PageDown => Some(-10), Key::PageUp => Some(10),
                _ => None,
            };
            if let Some(delta) = delta { step(app.states(), delta, tx); return true; }
            if matches!(ev.key, Key::Home | Key::End) {
                let Some(f) = self.tree.nodes[i].focus.step_limit.clone() else { return false };
                f(app.states(), ev.key == Key::End, tx);
                return true;
            }
        }
        // A focused field inside a list row (an inline rename) keeps its letters and arrows.
        // When a focusable `focus_group` container holds focus itself, navigation and typeahead
        // enter its items directly (WAI-ARIA Menu and Listbox container focus).
        if self.tree.nodes[i].input.is_none()
            && !self.is_caret_browsing_node(&self.tree.nodes[i])
            && let Some(g) = self.focus_group(i).or_else(|| self.tree.nodes[i].focus.group.then_some(i))
        {
            let items: Vec<_> = (0..self.tree.nodes.len()).filter(|&j| self.can_focus(j) && self.focus_group(j) == Some(g)).collect();
            if !items.is_empty() {
                let vertical = VERTICAL_ROLES.contains(&self.tree.nodes[g].role);
                // A right-to-left row's first item is at its right: Left goes on to the next.
                let (back, ahead) = if self.tree.nodes[g].flow == crate::el::Flow::Rtl { (Key::Right, Key::Left) } else { (Key::Left, Key::Right) };
                let backward = matches!(ev.key, Key::Up) || (!vertical && ev.key == back);
                let forward = matches!(ev.key, Key::Down) || (!vertical && ev.key == ahead);
                let pos = items.iter().position(|&j| j == i);
                let page = matches!(ev.key, Key::PageUp | Key::PageDown) && PAGED_ROLES.contains(&self.tree.nodes[g].role);
                let next = if backward || forward || page || matches!(ev.key, Key::Home | Key::End) {
                    self.kbd.reset_type_select();
                    Some(match (pos, &ev.key) {
                        (_, Key::Home) => 0,
                        (_, Key::End) => items.len() - 1,
                        (None, Key::PageUp) => 0,
                        (None, Key::PageDown) => items.len() - 1,
                        (None, _) if forward => 0,
                        (None, _) => items.len() - 1,
                        (Some(pos), Key::PageUp | Key::PageDown) => self.page_item(&items, pos, ev.key == Key::PageDown),
                        // A highlight the user cannot see yet appears where it is (a popup's checked
                        // item, else the first) before arrows move it; Up from nothing is the last.
                        (Some(pos), _) if self.tree.nodes[i].focus.highlight.is_some() && !shown =>
                            if backward && !self.tree.nodes[i].focus.selected { items.len() - 1 } else { pos },
                        (Some(pos), _) if CLAMPED_ROLES.contains(&self.tree.nodes[g].role) =>
                            if backward { pos.saturating_sub(1) } else { (pos + 1).min(items.len() - 1) },
                        (Some(pos), _) if backward => (pos + items.len() - 1) % items.len(),
                        (Some(pos), _) => (pos + 1) % items.len(),
                    })
                } else if let Key::Text(t) = &ev.key && TYPE_SELECT_ROLES.contains(&self.tree.nodes[g].role) && !t.trim().is_empty() {
                    let now = self.now_clock();
                    self.kbd.type_select(&self.tree.nodes, &items, pos, t, now)
                } else { None };
                if let Some(next) = next {
                    let n = &self.tree.nodes[items[next]];
                    let follows = n.role == "radio" || n.role == "tab" || self.tree.nodes[g].focus.follows;
                    let action = follows.then(|| n.handlers.on_press.clone()).flatten();
                    let path = n.path.clone();
                    self.set_focus(app, Some(path.clone()), true);
                    tx.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, path));
                    if let Some(f) = action { f(app.states(), tx); }
                    return true;
                }
            }
        } else if self.tree.nodes[i].role == "combobox" && self.tree.nodes[i].input.is_none() && matches!(ev.key, Key::Up | Key::Down) {
            // A popup button opens on its arrows as well as Space (NSPopUpButton).
            if repeat { return true; }
            if let Some(f) = self.tree.nodes[i].handlers.on_press.clone() { f(app.states(), tx); }
            return true;
        }
        // Space presses the focused control. Return presses only rows and links; for a button
        // it goes on to the default button (`default_button_key`, which ran first) or `on_key`.
        let space = ev.key == Key::Text(" ".into()) && !self.scrolls_on_space(i);
        if self.tree.nodes[i].input.is_none() && (space || (ev.key == Key::Enter && self.takes_return(i))) {
            // A row the user cannot see highlighted (the pointer left it) chooses nothing.
            if self.tree.nodes[i].focus.highlight.is_some() && !shown { self.kbd.focus.step(focus_machine::FocusEvent::Pointed); return true; }
            if repeat { return true; }
            if self.link_key(app, i, tx) { return true; }
            if let Some(f) = self.tree.nodes[i].handlers.on_press.clone() { f(app.states(), tx); }
            return true;
        }
        false
    }
    /// The item Page Down (or Up) moves to from `items[pos]`: the farthest one still inside a
    /// viewport's height of it, as a list's highlight pages in AppKit, at least one item on and
    /// stopping at the ends. Without a scroll view around it, the last (or first).
    fn page_item(&self, items: &[usize], pos: usize, down: bool) -> usize {
        let at = &self.tree.nodes[items[pos]];
        let page = std::iter::successors(at.parent, |&p| self.tree.nodes[p].parent)
            .find(|&p| self.tree.nodes[p].scroll.is_some()).map_or(f32::INFINITY, |v| self.tree.nodes[v].local.h);
        let within = |j: &usize| {
            let n = &self.tree.nodes[items[*j]];
            if down { n.target[3] - at.target[1] <= page + 0.5 } else { at.target[3] - n.target[1] <= page + 0.5 }
        };
        let far = if down { (pos..items.len()).take_while(within).last() } else { (0..=pos).rev().take_while(within).last() };
        match far.filter(|&j| j != pos) {
            Some(j) => j,
            None if down => (pos + 1).min(items.len() - 1),
            None => pos.saturating_sub(1),
        }
    }

    /// In a group whose selection follows focus, focus follows selection too: when the app
    /// selects another item (a shortcut, say), focus in that group moves to it.
    pub(in crate::runtime) fn sync_follows(&mut self, app: &mut dyn AppDriver) -> bool {
        let Some(i) = self.focused_index() else { return false };
        if self.tree.nodes[i].input.is_some() || self.tree.nodes[i].focus.selected { return false; }
        let Some(g) = self.focus_group(i).filter(|&g| self.tree.nodes[g].focus.follows) else { return false };
        let Some(j) = (0..self.tree.nodes.len())
            .find(|&j| self.tree.nodes[j].focus.selected && self.focus_group(j) == Some(g) && self.can_focus(j)) else { return false };
        let path = self.tree.nodes[j].path.clone();
        self.set_focus_notifying(app, Some(path), self.kbd.focus.keyboard()).1
    }
}

/// `descendant` is `ancestor` or inside it along `/`-separated path segments.
fn path_holds(ancestor: &str, descendant: &str) -> bool {
    !ancestor.is_empty()
        && descendant.strip_prefix(ancestor).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Yields `path` and each of its `/`-separated ancestor paths, innermost to outermost.
fn ancestor_paths(mut path: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if path.is_empty() { return None; }
        let cur = path;
        path = match path.rfind('/') {
            Some(slash) => &path[..slash],
            None => "",
        };
        Some(cur)
    })
}

/// The rescue's pick for [`RuntimeCore::focus_fallback`] and [`RuntimeCore::fallback_in_new`]:
/// `scopes` are the containers that survived, nearest first, each with the end of its
/// descendants' span (indices in document order; `at` lost focus). In each container, the next
/// control after `at` and then the one before, then the container itself. A Tab stop that lost
/// focus is handed only to Tab stops (`tab`) while any container out to the root has one; only
/// then to any control (`any`). Anything else hands focus to any control. Focus that was in a
/// roving group goes first to another of that group's items (`item`), whatever it was.
fn rescue(lost_tab_stop: bool, at: usize, scopes: &[(usize, usize)], item: &dyn Fn(usize) -> Option<usize>, tab: &dyn Fn(usize) -> Option<usize>, any: &dyn Fn(usize) -> Option<usize>) -> Option<usize> {
    let pass = |ok: &dyn Fn(usize) -> Option<usize>| scopes.iter().find_map(|&(s, end)| {
        (at + 1..end).find_map(ok).or_else(|| (s + 1..at).rev().find_map(ok)).or_else(|| ok(s))
    });
    pass(item).or_else(|| if lost_tab_stop { pass(tab).or_else(|| pass(any)) } else { pass(any) })
}

#[cfg(test)]
#[path = "focus_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "focus_key_tests.rs"]
mod key_tests;

#[cfg(test)]
#[path = "focus_change_tests.rs"]
mod change_tests;
