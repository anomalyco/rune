//! What assistive technology asks of an element (AccessKit action requests), run through the
//! handlers a pointer or keyboard user reaches: a click presses it, an increment steps it as
//! an arrow key does, a new value goes through the field's editor and its change handler.

use accesskit::{Action as Ak, ActionData, ActionRequest, ScrollUnit};

use super::text;
use super::tree::{ROOT, split_id, sub_link};
use crate::el::Tx;
use crate::runtime::event_log::log_event;
use crate::runtime::{AppDriver, RuntimeCore};

/// An action request resolved against the tree it named, run at the next frame.
pub(crate) enum A11yAct {
    /// Focus the element, or one of its text's links.
    Focus(Option<usize>),
    Step(i32),
    SetNumericValue(f64),
    SetValue(zeroize::Zeroizing<String>),
    Replace(zeroize::Zeroizing<String>),
    /// Anchor and focus, in the field's display bytes.
    Select(usize, usize),
    ScrollIntoView,
    /// Pages (or, with `false`, lines) down and right.
    Scroll { x: i32, y: i32, pages: bool },
    ScrollTo(f32, f32),
    /// Delegate an unhandled element action to the nearest ancestor `El::on_a11y_action`.
    Delegate { target_key: String, action: Ak },
}

impl std::fmt::Debug for A11yAct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Focus(link) => f.debug_tuple("Focus").field(link).finish(),
            Self::Step(delta) => f.debug_tuple("Step").field(delta).finish(),
            Self::SetNumericValue(value) => f.debug_tuple("SetNumericValue").field(value).finish(),
            Self::SetValue(_) => f.debug_tuple("SetValue").field(&"[redacted]").finish(),
            Self::Replace(_) => f.debug_tuple("Replace").field(&"[redacted]").finish(),
            Self::Select(a, b) => f.debug_tuple("Select").field(a).field(b).finish(),
            Self::ScrollIntoView => f.write_str("ScrollIntoView"),
            Self::Scroll { x, y, pages } => f.debug_struct("Scroll").field("x", x).field("y", y).field("pages", pages).finish(),
            Self::ScrollTo(x, y) => f.debug_tuple("ScrollTo").field(x).field(y).finish(),
            Self::Delegate { target_key, action } => f.debug_struct("Delegate").field("target_key", target_key).field("action", action).finish(),
        }
    }
}

/// One scroll "item" in points: about a line of body text.
const LINE: f32 = 40.0;

impl RuntimeCore {
    /// Queues what `request` asks for its frame, if it names an element that can do it. Returns
    /// whether it queued anything (the host then draws a frame).
    pub(crate) fn a11y_action(&mut self, request: &ActionRequest) -> bool {
        if request.target_node == ROOT {
            return false;
        }
        let (id, sub) = split_id(request.target_node);
        let Some(i) = self.tree.nodes.iter().position(|n| n.id == id) else { return false };
        if !self.allowed(i) || self.subtree_exiting(i) { return false; }
        let n = &self.tree.nodes[i];
        let path = n.path.clone();
        let link = sub_link(sub);
        let handlers = n.handlers.get();
        let enabled = !n.a11y.disabled;
        let pressable = enabled && handlers.is_some_and(|h| h.on_press.is_some());
        let secondary = enabled && handlers.is_some_and(|h| h.on_secondary_press.is_some());
        let read_only = self.is_node_read_only(i);
        let (input, has_access) = (
            enabled && n.input.is_some(),
            enabled && n.input.as_ref().is_some_and(|inp| inp.access.is_some()),
        );
        let editable = has_access && !read_only;
        let (expanded, steps, sets_value, scrolls) = (
            n.a11y.expanded,
            enabled && n.focus.step.is_some(),
            enabled && n.focus.set_value.is_some(),
            enabled && n.scroll.is_some(),
        );
        let focusable = self.can_focus(i);
        match (request.action, link) {
            (Ak::Click, Some(k)) if enabled => self.queue_a11y_link(path, k),
            (Ak::ShowContextMenu, Some(k)) if enabled => {
                let Some((_, r)) = self.link_boxes_for(i, k, self.drawn_box(i)).into_iter().next() else { return false };
                let ev = self.pointer_event(i, r[0], r[3]);
                self.queue_a11y_link_secondary(path, k, ev);
            }
            (Ak::Focus, Some(k)) if focusable => self.queue_a11y(path, A11yAct::Focus(Some(k))),
            (Ak::Click, None) if pressable => self.queue_a11y_press(path),
            (Ak::Click, None) if input => self.queue_a11y(path, A11yAct::Focus(None)),
            (Ak::Focus, None) if focusable => self.queue_a11y(path, A11yAct::Focus(None)),
            (Ak::Expand | Ak::Collapse, None) if pressable && expanded.is_some() => {
                if expanded == Some(request.action == Ak::Collapse) {
                    self.queue_a11y_press(path);
                } else {
                    return false;
                }
            }
            (Ak::ShowContextMenu, None) if secondary => {
                let r = self.drawn_box(i);
                let ev = self.pointer_event(i, r[0], r[3]);
                self.queue_a11y_secondary_press(path, ev);
            }
            (Ak::Increment, None) if steps => self.queue_a11y(path, A11yAct::Step(1)),
            (Ak::Decrement, None) if steps => self.queue_a11y(path, A11yAct::Step(-1)),
            (Ak::SetValue, None) if editable && matches!(request.data, Some(ActionData::Value(_))) => {
                let Some(ActionData::Value(v)) = &request.data else { return false };
                self.queue_a11y(path, A11yAct::SetValue(zeroize::Zeroizing::new(v.to_string())));
            }
            // A range control with `on_set_value` takes the target value directly, clamped to its range.
            (Ak::SetValue, None) if sets_value => {
                let Some(ActionData::NumericValue(target)) = request.data else { return false };
                if !target.is_finite() { return false; }
                let clamped = match n.a11y.more.as_deref().and_then(|m| m.range) {
                    Some(r) => {
                        let Some((min, max)) = r.usable_bounds() else { return false };
                        target.clamp(min, max)
                    }
                    None => target,
                };
                self.queue_a11y(path, A11yAct::SetNumericValue(clamped));
            }
            // A slider with only `on_step` and a stated step takes a number as that many steps from
            // its last built value; any earlier queued steps on the same element are replaced so
            // repeated SetValue requests before a frame do not add up.
            (Ak::SetValue, None) if steps => {
                let Some(ActionData::NumericValue(target)) = request.data else { return false };
                if !target.is_finite() { return false; }
                let Some(delta) = n.a11y.more.as_deref().and_then(|m| m.range).and_then(|r| {
                    let step = r.usable_step()?;
                    let target = target.clamp(r.min, r.max);
                    Some(((target - r.value) / step).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
                }) else { return false };
                let had_steps = self.clear_a11y_steps(&path);
                if delta == 0 { return had_steps; }
                self.queue_a11y(path, A11yAct::Step(delta));
            }
            (Ak::ReplaceSelectedText, None) if editable => match &request.data {
                Some(ActionData::Value(v)) => self.queue_a11y(path, A11yAct::Replace(zeroize::Zeroizing::new(v.to_string()))),
                _ => return false,
            },
            (Ak::SetValue | Ak::ReplaceSelectedText, None) if has_access && read_only => return false,
            (Ak::SetTextSelection, None) if has_access && !n.is_secure() => {
                let Some(ActionData::SetTextSelection(sel)) = &request.data else { return false };
                // A field past its runs: bytes of its value, on the field itself (`a11y_text_nodes`).
                if let Some(input) = n.input.as_deref() && split_id(sel.anchor.node) == (id, 0) && split_id(sel.focus.node) == (id, 0) {
                    let at = |b: usize| input.edit.display_index(input.edit.text.floor_char_boundary(b.min(input.edit.text.len())));
                    let (a, b) = (at(sel.anchor.character_index), at(sel.focus.character_index));
                    self.queue_a11y(path, A11yAct::Select(a, b));
                    return true;
                }
                let Some((_, lines)) = self.a11y_lines(i) else { return false };
                let byte = |p: accesskit::TextPosition| {
                    let (pid, line) = split_id(p.node);
                    if pid != id || line == 0 { return None; }
                    text::byte_at(&lines, line as usize - 1, p.character_index)
                };
                let (Some(a), Some(b)) = (byte(sel.anchor), byte(sel.focus)) else { return false };
                self.queue_a11y(path, A11yAct::Select(a, b));
            }
            (Ak::ScrollIntoView, _) => self.queue_a11y(path, A11yAct::ScrollIntoView),
            (Ak::ScrollUp | Ak::ScrollDown | Ak::ScrollLeft | Ak::ScrollRight, None) if scrolls => {
                let pages = !matches!(request.data, Some(ActionData::ScrollUnit(ScrollUnit::Item)));
                let (x, y) = match request.action {
                    Ak::ScrollUp => (0, -1),
                    Ak::ScrollDown => (0, 1),
                    Ak::ScrollLeft => (-1, 0),
                    _ => (1, 0),
                };
                self.queue_a11y(path, A11yAct::Scroll { x, y, pages });
            }
            (Ak::SetScrollOffset, None) if scrolls => match request.data {
                Some(ActionData::SetScrollOffset(p)) => self.queue_a11y(path, A11yAct::ScrollTo(p.x as f32, p.y as f32)),
                _ => return false,
            },
            _ if sub == 0 => return self.queue_a11y_delegate(i, request.action),
            _ => return false,
        }
        true
    }

    /// Nearest live, enabled, allowed strict ancestor of `i` with `El::on_a11y_action`.
    pub(crate) fn a11y_action_ancestor(&self, i: usize) -> Option<usize> {
        std::iter::successors(self.tree.nodes[i].parent, |&j| self.tree.nodes[j].parent)
            .take_while(|&j| self.allowed(j) && !self.subtree_exiting(j))
            .find(|&j| {
                let n = &self.tree.nodes[j];
                !n.a11y.disabled && n.handlers.get().is_some_and(|h| h.on_a11y_action.is_some())
            })
    }

    /// Queues a delegated accessibility action for node `i` if it is enabled, allowed, not
    /// exiting, and has a live enabled ancestor with `El::on_a11y_action`.
    pub(crate) fn queue_a11y_delegate(&mut self, i: usize, action: Ak) -> bool {
        if !self.allowed(i) || self.subtree_exiting(i) {
            return false;
        }
        let n = &self.tree.nodes[i];
        if n.a11y.disabled || self.a11y_action_ancestor(i).is_none() {
            return false;
        }
        let path = n.path.clone();
        let target_key = n.key.clone().unwrap_or_else(|| path.clone());
        self.queue_a11y(path, A11yAct::Delegate { target_key, action });
        true
    }

    /// Asks what an assistive client would of the element (or inline link) whose key is `key`,
    /// for tests: it runs at the next frame, through the same handlers pointer and keyboard
    /// input reach. False when nothing keyed so supports `action`.
    pub fn accessibility_action(&mut self, key: &str, action: accesskit::Action, data: Option<ActionData>) -> bool {
        let tree = self.accessibility_tree();
        let Some(target_node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).map(|(id, _)| *id) else { return false };
        self.a11y_action(&ActionRequest { action, target_tree: accesskit::TreeId::ROOT, target_node, data })
    }

    /// Runs a queued accessibility action in its frame.
    pub(crate) fn run_a11y(&mut self, app: &mut dyn AppDriver, path: String, act: A11yAct, tx: &mut Tx) -> bool {
        match act {
            A11yAct::Focus(link) => {
                log_event!(self, "a11y focus {path}");
                self.set_focus(app, Some(path), true);
                if let Some(k) = link { self.set_link_focus(k); }
                true
            }
            A11yAct::Step(delta) => {
                let Some(f) = self.node(&path).and_then(|n| n.focus.step.clone()) else { return false };
                log_event!(self, "a11y step {delta} {path}");
                f(app.states(), delta, tx);
                true
            }
            A11yAct::SetNumericValue(value) => {
                let Some(f) = self.node(&path).and_then(|n| n.focus.set_value.clone()) else { return false };
                log_event!(self, "a11y set_value {value} {path}");
                f(app.states(), value, tx);
                true
            }
            A11yAct::SetValue(value) => {
                if self.node_index(&path).is_some_and(|i| self.is_node_read_only(i)) { return false; }
                self.a11y_edit(app, path, tx, move |e| {
                    let len = e.text.len();
                    e.set_selection(0, len);
                    e.paste(&value)
                })
            }
            A11yAct::Replace(value) => {
                if self.node_index(&path).is_some_and(|i| self.is_node_read_only(i)) { return false; }
                self.a11y_edit(app, path, tx, move |e| e.paste(&value))
            }
            A11yAct::Select(a, b) => self.a11y_edit(app, path, tx, move |e| {
                let (a, b) = (e.model_index(a), e.model_index(b));
                e.set_selection(a, b);
                crate::edit::EditOutcome::default()
            }),
            A11yAct::ScrollIntoView => {
                tx.scroll_into_view(path);
                true
            }
            A11yAct::Scroll { x, y, pages } => {
                let Some(i) = self.tree.nodes.iter().position(|n| n.path == path) else { return false };
                let Some(st) = self.scroll.scrolls.get(&self.tree.nodes[i].id) else { return false };
                if pages && x == 0 {
                    tx.scroll_page(path, y);
                } else {
                    let r = self.paint.drawn[i];
                    let (dx, dy) = if pages { (r[2] * 0.9, r[3] * 0.9) } else { (LINE, LINE) };
                    let nx = (st.x.offset + st.x.toward(-f64::from(dx) * f64::from(x))) as f32;
                    let ny = st.offset as f32 + dy * y as f32;
                    tx.scroll_to_instant(path, nx, ny);
                }
                true
            }
            A11yAct::ScrollTo(x, y) => {
                tx.scroll_to_instant(path, x, y);
                true
            }
            A11yAct::Delegate { target_key, action } => {
                let Some(i) = self.node_index(&path) else { return false };
                if !self.allowed(i) || self.tree.nodes[i].a11y.disabled { return false; }
                let handlers: Vec<_> = std::iter::successors(self.tree.nodes[i].parent, |&j| self.tree.nodes[j].parent)
                    .take_while(|&j| self.allowed(j) && !self.subtree_exiting(j))
                    .filter_map(|j| {
                        let n = &self.tree.nodes[j];
                        (!n.a11y.disabled).then(|| n.handlers.get().and_then(|h| h.on_a11y_action.clone())).flatten()
                    })
                    .collect();
                if handlers.is_empty() { return false; }
                log_event!(self, "a11y delegate {action:?} {target_key}");
                handlers.into_iter().any(|f| Tx::took(f(app.states(), &target_key, action, tx), tx))
            }
        }
    }

    /// Focuses field `path` (as a reader focuses it first) and edits it through its editor, so
    /// its change and submit handlers run as for typing.
    fn a11y_edit(&mut self, app: &mut dyn AppDriver, path: String, tx: &mut Tx, f: impl FnOnce(&mut crate::edit::TextEdit) -> crate::edit::EditOutcome) -> bool {
        if self.kbd.focus.path() != Some(path.as_str()) { self.set_focus(app, Some(path.clone()), true); }
        log_event!(self, "a11y edit {path}");
        self.with_focused(app, f, tx)
    }
}
