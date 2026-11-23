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
    SetValue(zeroize::Zeroizing<String>),
    Replace(zeroize::Zeroizing<String>),
    /// Anchor and focus, in the field's display bytes.
    Select(usize, usize),
    ScrollIntoView,
    /// Pages (or, with `false`, lines) down and right.
    Scroll { x: i32, y: i32, pages: bool },
    ScrollTo(f32, f32),
}

impl std::fmt::Debug for A11yAct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Focus(link) => f.debug_tuple("Focus").field(link).finish(),
            Self::Step(delta) => f.debug_tuple("Step").field(delta).finish(),
            Self::SetValue(_) => f.debug_tuple("SetValue").field(&"[redacted]").finish(),
            Self::Replace(_) => f.debug_tuple("Replace").field(&"[redacted]").finish(),
            Self::Select(a, b) => f.debug_tuple("Select").field(a).field(b).finish(),
            Self::ScrollIntoView => f.write_str("ScrollIntoView"),
            Self::Scroll { x, y, pages } => f.debug_struct("Scroll").field("x", x).field("y", y).field("pages", pages).finish(),
            Self::ScrollTo(x, y) => f.debug_tuple("ScrollTo").field(x).field(y).finish(),
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
        let pressable = handlers.is_some_and(|h| h.on_press.is_some());
        let secondary = handlers.is_some_and(|h| h.on_secondary_press.is_some());
        let (input, editable) = (n.input.is_some(), n.input.as_ref().is_some_and(|i| i.access.is_some()));
        let (expanded, steps, scrolls) = (n.a11y.expanded, n.focus.step.is_some(), n.scroll.is_some());
        let focusable = self.can_focus(i);
        match (request.action, link) {
            (Ak::Click, Some(k)) => self.queue_a11y_link(path, k),
            (Ak::ShowContextMenu, Some(k)) => {
                let Some((_, r)) = self.link_boxes_for(i, k, self.drawn_box(i)).into_iter().next() else { return false };
                let ev = self.pointer_event(i, r[0], r[3]);
                self.queue_a11y_link_secondary(path, k, ev);
            }
            (Ak::Focus, Some(k)) if focusable => self.queue_a11y(path, A11yAct::Focus(Some(k))),
            (Ak::Click, None) if pressable => self.queue_a11y_press(path),
            (Ak::Click, None) if input => self.queue_a11y(path, A11yAct::Focus(None)),
            (Ak::Focus, None) if focusable => self.queue_a11y(path, A11yAct::Focus(None)),
            (Ak::Expand | Ak::Collapse, None) if pressable && expanded == Some(request.action == Ak::Collapse) => {
                self.queue_a11y_press(path);
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
            // A slider with a stated step takes a number as that many steps from its value.
            (Ak::SetValue, None) if steps => {
                let Some(ActionData::NumericValue(target)) = request.data else { return false };
                if !target.is_finite() { return false; }
                let Some(delta) = n.a11y.more.as_deref().and_then(|m| m.range).and_then(|r| {
                    let step = r.usable_step()?;
                    let target = target.clamp(r.min, r.max);
                    Some(((target - r.value) / step).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
                }) else { return false };
                if delta == 0 { return false; }
                self.queue_a11y(path, A11yAct::Step(delta));
            }
            (Ak::ReplaceSelectedText, None) if editable => match &request.data {
                Some(ActionData::Value(v)) => self.queue_a11y(path, A11yAct::Replace(zeroize::Zeroizing::new(v.to_string()))),
                _ => return false,
            },
            (Ak::SetTextSelection, None) if editable && !n.is_secure() => {
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
            _ => return false,
        }
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
            A11yAct::SetValue(value) => self.a11y_edit(app, path, tx, move |e| {
                let len = e.text.len();
                e.set_selection(0, len);
                e.paste(&value)
            }),
            A11yAct::Replace(value) => self.a11y_edit(app, path, tx, move |e| e.paste(&value)),
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
