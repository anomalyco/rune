//! Pointer selection in `text_input`: single-click caret placement, click-and-drag character
//! selection, double-click word selection (and word-by-word drag), triple-click line/paragraph
//! selection (and line-by-line drag), and Shift-click selection extension.
//!
//! Word and paragraph boundaries and drag extension share `selectable`'s `Unit`, `unit_at` and
//! `extend`.
use std::collections::VecDeque;

use super::selectable::{Unit, extend};
use super::*;

#[derive(Clone, Copy, Debug)]
enum Step {
    Down { clicks: u8, shift: bool, check_pill: bool },
    Drag,
    Cancelled,
}

#[derive(Clone, Copy)]
pub(in crate::runtime) struct Hit {
    pub(in crate::runtime) caret: usize,
    /// The side of `caret` the press was on, where two runs of different direction meet.
    side: parley::Affinity,
    cluster: usize,
}

/// Whether the runtime's held press belongs to a text field.
#[derive(Default)]
enum Capture {
    #[default]
    Free,
    /// An input-selection press holds the pointer. `dragging` is the field whose selection its
    /// moves extend, until focus leaves that field or it unmounts; the press stays the field's.
    Held { dragging: Option<String> },
}

#[derive(Default)]
pub(in crate::runtime) struct InputClick {
    /// The run of non-Shift presses on inputs.
    presses: press_run::PressRun,
    /// What the press that started the current selection selects by, and the unit range it
    /// began on, which a drag keeps whole. They outlive the press: drag steps it queued still
    /// read them after the release.
    unit: Unit,
    origin: (usize, usize),
    capture: Capture,
    /// Queued press/drag steps paired with `Action::Focus` entries in `Runtime::queue`.
    steps: VecDeque<(String, Step)>,
}

impl InputClick {
    pub(in crate::runtime) fn dragging_path(&self) -> Option<&str> {
        match &self.capture {
            Capture::Held { dragging } => dragging.as_deref(),
            Capture::Free => None,
        }
    }

    /// The runtime's held press is an input-selection press.
    pub(in crate::runtime) fn captured(&self) -> bool { matches!(self.capture, Capture::Held { .. }) }

    pub(in crate::runtime) fn clear(&mut self) {
        self.presses.clear();
        self.capture = Capture::Free;
        self.cancel_drag_steps();
    }

    pub(in crate::runtime) fn clear_last(&mut self) {
        self.presses.clear();
    }

    /// The field stops following the press: queued drag steps do nothing, and the press keeps
    /// the pointer until it releases.
    pub(in crate::runtime) fn drop_drag(&mut self) {
        if let Capture::Held { dragging } = &mut self.capture { *dragging = None; }
        self.cancel_drag_steps();
    }

    /// Queued drag steps stay, paired with their `Action::Focus` entries, but do nothing.
    fn cancel_drag_steps(&mut self) {
        for (_, step) in &mut self.steps {
            if matches!(step, Step::Drag) {
                *step = Step::Cancelled;
            }
        }
    }

    /// The press released: whether it was an input-selection press.
    pub(in crate::runtime) fn end_press(&mut self) -> bool {
        matches!(std::mem::take(&mut self.capture), Capture::Held { .. })
    }

    pub(in crate::runtime) fn on_focus_change(&mut self, focused: Option<&str>) {
        self.presses.keep_on(focused);
        if self.dragging_path().is_some_and(|p| Some(p) != focused) {
            self.drop_drag();
        }
    }
}

pub(in crate::runtime) fn hit(e: &crate::edit::TextEdit, l: TextFlow<'_>, x: f32, y: f32, s: f32, single: bool) -> Hit {
    let h = (l.height() - 1e-3).max(0.0);
    let ys = if single || x < 0.0 { (y * s).clamp(0.0, h) } else { (y * s).max(0.0) };
    let at = l.hit(x * s, ys);
    let caret = e.model_index(at.index);
    let cluster = at.before.map_or(caret, |b| e.model_index(b));
    Hit { caret, side: at.side, cluster }
}

fn unit_at(e: &crate::edit::TextEdit, h: Hit, unit: Unit) -> (usize, usize) {
    if unit == Unit::Char {
        let to = e
            .pills()
            .find_map(|(r, _)| (r.start < h.caret && h.caret < r.end).then(|| if h.caret - r.start < r.end - h.caret { r.start } else { r.end }))
            .unwrap_or(h.caret);
        return (to, to);
    }
    if e.is_secure() || (unit == Unit::Paragraph && e.is_single_line()) {
        return (0, e.text.len());
    }
    let pill = e.pills().find_map(|(r, _)| r.contains(&h.cluster).then_some((r.start, r.end)));
    if unit == Unit::Word
        && let Some(p) = pill
    {
        return p;
    }
    let (mut a, mut b) = super::selectable::unit_at(&e.text, h.cluster, unit);
    if let Some((ps, pe)) = pill {
        (a, b) = (a.min(ps), b.max(pe));
    }
    for (r, _) in e.pills() {
        if r.start < a && a < r.end {
            a = r.start;
        }
        if r.start < b && b < r.end {
            b = r.end;
        }
    }
    (a, b)
}

impl RuntimeCore {
    /// Window point `(x, y)` in text field `i`'s layout points, using the same `text_x` origin and
    /// single-line horizontal scroll rule as paint (`links::origin`).
    pub(in crate::runtime) fn input_text_point(&self, i: usize, x: f32, y: f32) -> Option<[f32; 2]> {
        let n = &self.tree.nodes[i];
        let (inp, spec, laid) = (n.input.as_deref()?, n.text.as_deref()?, n.text_layout.as_ref()?);
        let flow = laid.view();
        let s = self.env.scale;
        let valid = self.tree.built && self.paint.drawn.len() == self.tree.nodes.len();
        let (rect, ux, uy) = if valid { self.unturned_box_and_point(i, x, y) } else { (n.target, x, y) };
        let z = if valid { self.paint.drawn_scale.get(i).copied().unwrap_or(1.0).max(1e-4) } else { 1.0 };
        let [mut ox, oy] = links::origin(n, spec, flow, rect, s, z);
        if inp.single_line {
            ox -= self.edit.single_line_scroll(&n.path) * z;
        }
        Some([(ux - ox) / z, (uy - oy) / z])
    }

    /// Whether dragging from `from` to `to` in text field `i` selects a non-empty range.
    pub(in crate::runtime) fn input_drag_selects(&self, i: usize, from: (f32, f32), to: (f32, f32)) -> bool {
        if from == to {
            return false;
        }
        let n = &self.tree.nodes[i];
        let (Some(inp), Some(laid)) = (n.input.as_deref(), n.text_layout.as_ref()) else { return false };
        let (Some([x0, y0]), Some([x1, y1])) = (self.input_text_point(i, from.0, from.1), self.input_text_point(i, to.0, to.1)) else { return false };
        let flow = laid.view();
        let s = self.env.scale;
        let a = unit_at(&inp.edit, hit(&inp.edit, flow, x0, y0, s, inp.single_line), Unit::Char);
        let b = unit_at(&inp.edit, hit(&inp.edit, flow, x1, y1, s, inp.single_line), Unit::Char);
        a != b
    }

    /// Whether text field node `i` shows `Cursor::Pointer` at window point `(x, y)` (over an
    /// `on_link` link or an `on_pill_press` pill chip when not dragging a selection).
    pub(in crate::runtime) fn input_pointer_cursor(&self, i: usize, x: f32, y: f32) -> bool {
        let Some(inp) = self.tree.nodes[i].input.as_deref() else { return false };
        let input_link = self.tree.nodes[i].text.as_ref().is_some_and(|t| t.cold.on_link.is_some())
            && (!self.edit.input_click.captured()
                || self.pointer.press_held().is_some_and(|h| !h.moved && h.link.is_some()))
            && self.link_at(i, x, y).is_some();
        if input_link {
            return true;
        }
        inp.on_pill_press.is_some()
            && !self.tree.nodes[i].a11y.disabled
            && !self.kbd.held_mods.shift
            && (self.edit.input_click.dragging_path().is_none()
                || self.pointer.press_held().is_some_and(|h| !h.moved))
            && self.input_pill_at(i, x, y).is_some()
    }

    /// A press at (x, y) on text field `ji`, down at `down_at`; `second` when it's the second
    /// of a double press; `link` when the press landed on a link in `ji`'s own text; `direct`
    /// when the press landed directly on `ji` rather than an outer `role="textbox"` wrapper.
    pub(in crate::runtime) fn input_down(&mut self, ji: usize, (x, y): (f32, f32), down_at: f64, second: bool, link: Option<usize>, direct: bool) {
        let now = self.now_clock();
        let shift = self.kbd.held_mods.shift;
        // Its text drew scaled or rotated with its box: map the press back to the rest layout.
        let [lx, ly] = self.to_local(ji, x, y);
        let jn = &self.tree.nodes[ji];
        let path = jn.path.clone();
        let r = jn.target;
        let clicks = if shift {
            self.edit.input_click.presses.clear();
            1
        } else {
            self.edit.input_click.presses.press(&path, now, (x, y), self.double_click_interval).min(3) as u8
        };
        let link = link.filter(|_| !shift && clicks == 1);
        let check_pill = direct && !shift;
        self.edit.input_click.capture = Capture::Held { dragging: Some(path.clone()) };
        self.edit.input_click.steps.push_back((path.clone(), Step::Down { clicks, shift, check_pill }));
        self.frame.queue.push(Action::Focus(path.clone(), x, y));
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
            clicks: u32::from(clicks),
            cancelled: false,
            rtl: jn.flow == crate::el::Flow::Rtl,
            window: [x, y],
            screen: self.screen_at(x, y),
        };
        self.pointer.hold_press(
            capture::Held::new(path, d, crate::PointerButton::Primary, (x, y), down_at, second, None).on_link(link),
        );
    }

    /// Focuses text field `ji` with its caret at (x, y), as a single click does, but holds no
    /// press: nothing drags from it (Linux's middle-click paste, `primary_selection.rs`).
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    pub(in crate::runtime) fn place_caret(&mut self, ji: usize, x: f32, y: f32) {
        let path = self.tree.nodes[ji].path.clone();
        self.edit.input_click.presses.clear();
        self.edit.input_click.steps.push_back((path.clone(), Step::Down { clicks: 1, shift: false, check_pill: false }));
        self.frame.queue.push(Action::Focus(path, x, y));
    }

    pub(in crate::runtime) fn input_drag(&mut self, x: f32, y: f32, drag_move: bool) {
        if !drag_move {
            return;
        }
        let Some(path) = self.edit.input_click.dragging_path().map(str::to_owned) else { return };
        if self.pointer.press_held().is_some_and(|h| !press_run::within_slop(h.at, (x, y))) {
            self.edit.input_click.presses.clear();
        }
        if !self.tree.nodes.iter().enumerate().any(|(i, n)| n.path == path && !self.subtree_exiting(i) && n.input.is_some()) {
            self.edit.drop_input_drag();
            return;
        }
        self.edit.input_click.steps.push_back((path.clone(), Step::Drag));
        self.frame.queue.push(Action::Focus(path, x, y));
    }

    pub(in crate::runtime) fn focus_input(&mut self, app: &mut dyn AppDriver, path: String, x: f32, y: f32, one: &mut Tx) -> bool {
        let step = if self.edit.input_click.steps.front().is_some_and(|(p, _)| *p == path) {
            self.edit.input_click.steps.pop_front().unwrap().1
        } else {
            Step::Down { clicks: 1, shift: false, check_pill: false }
        };
        if matches!(step, Step::Cancelled) {
            return false;
        }
        let Some(i) = self.tree.nodes.iter().enumerate()
            .find(|&(i, n)| n.path == path && !self.subtree_exiting(i) && !self.subtree_unmounted(i))
            .map(|(i, _)| i)
        else {
            self.edit.drop_input_drag();
            return false;
        };
        if self.node_disabled(i) {
            self.edit.drop_input_drag();
            if matches!(step, Step::Down { .. }) && self.can_focus(i) {
                self.set_focus(app, Some(path), false);
                one.commit = one.commit.instant();
                return true;
            }
            return false;
        }
        if let Step::Down { shift: false, check_pill: true, .. } = step
            && self.allowed(i)
            && let Some(f) = self.tree.nodes[i].input.as_deref().and_then(|inp| inp.on_pill_press.clone())
            && let Some(pill_id) = self.input_pill_at(i, x, y)
            && f(app.states(), &pill_id, crate::PointerPress { x, y }, one)
        {
            log_event!(self, "pill press {path}");
            self.edit.input_click.presses.clear();
            self.edit.drop_input_drag();
            return true;
        }
        let selection_change = self.tree.nodes[i].input.as_ref().and_then(|inp| inp.selection_change.clone());
        let Some(access) = self.tree.nodes[i].input.as_ref().and_then(|inp| inp.access.clone()) else {
            self.edit.drop_input_drag();
            return false;
        };
        let single = self.tree.nodes[i].input.as_ref().is_some_and(|inp| inp.single_line);
        let layout = self.tree.nodes[i].text_layout.clone();
        let point = self.input_text_point(i, x, y);
        let s = self.env.scale;
        match step {
            Step::Down { clicks, shift, .. } => {
                self.set_focus(app, Some(path), false);
                // A press mid-composition writes it into the text first, as a browser's field
                // does, then puts the caret where it landed: the layout it hits already shows
                // the composition in place.
                self.finish_field_composition(app, one);
                let changed_sel = {
                    let e = access(app.states());
                    e.set_single_line(single);
                    e.break_history_group();
                    e.clear_typing_style();
                    if e.preedit().is_some() {
                        self.edit.clear_input_click();
                        one.commit = one.commit.instant();
                        return true;
                    }
                    let before = (e.anchor, e.focus);
                    if let (Some(l), Some([tx, ty])) = (layout, point) {
                        let h = hit(e, l.view(), tx, ty, s, single);
                        if shift {
                            let anchor = e.anchor;
                            self.edit.input_click.unit = Unit::Char;
                            self.edit.input_click.origin = (anchor, anchor);
                            e.set_selection(anchor, h.caret);
                        } else {
                            let unit = match clicks {
                                1 => Unit::Char,
                                2 => Unit::Word,
                                _ => Unit::Paragraph,
                            };
                            let origin = unit_at(e, h, unit);
                            // Not after a pill: it is a word of its own.
                            let origin = if unit == Unit::Word && e.pills().all(|(r, _)| (r.start, r.end) != origin) { super::selectable::with_trailing_space(&e.text, origin) } else { origin };
                            self.edit.input_click.unit = unit;
                            self.edit.input_click.origin = origin;
                            e.set_selection(origin.0, origin.1);
                            // A caret put down where two runs meet stays on the side pressed.
                            if origin == (h.caret, h.caret) { e.affinity = h.side; }
                        }
                    }
                    (e.anchor, e.focus) != before
                };
                if changed_sel && let Some(selection_change) = selection_change {
                    selection_change(app.states(), one);
                }
                one.commit = one.commit.instant();
                true
            }
            Step::Drag => {
                if self.kbd.focus.path() != Some(path.as_str()) {
                    self.edit.drop_input_drag();
                    return false;
                }
                let changed_sel = {
                    let e = access(app.states());
                    e.set_single_line(single);
                    if e.preedit().is_some() {
                        self.edit.clear_input_click();
                        return false;
                    }
                    let (Some(l), Some([tx, ty])) = (layout, point) else { return false };
                    let h = hit(e, l.view(), tx, ty, s, single);
                    let at = unit_at(e, h, self.edit.input_click.unit);
                    let (anchor, focus) = extend(self.edit.input_click.origin, at);
                    let before = (e.anchor, e.focus);
                    e.set_selection(anchor, focus);
                    if (e.anchor, e.focus) == before {
                        return false;
                    }
                    e.break_history_group();
                    true
                };
                if changed_sel && let Some(selection_change) = selection_change {
                    selection_change(app.states(), one);
                }
                one.commit = one.commit.instant();
                true
            }
            Step::Cancelled => false,
        }
    }
}

#[cfg(test)]
#[path = "../input_click_tests.rs"]
mod tests;
