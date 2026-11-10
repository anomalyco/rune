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
    Down { clicks: u8, shift: bool },
    Drag,
    Cancelled,
}

#[derive(Clone, Copy)]
pub(super) struct Hit {
    pub(super) caret: usize,
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
pub(super) struct InputClick {
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
    pub(super) fn dragging_path(&self) -> Option<&str> {
        match &self.capture {
            Capture::Held { dragging } => dragging.as_deref(),
            Capture::Free => None,
        }
    }

    /// The runtime's held press is an input-selection press.
    pub(super) fn captured(&self) -> bool { matches!(self.capture, Capture::Held { .. }) }

    pub(super) fn clear(&mut self) {
        self.presses.clear();
        self.capture = Capture::Free;
        self.cancel_drag_steps();
    }

    pub(super) fn clear_last(&mut self) {
        self.presses.clear();
    }

    /// The field stops following the press: queued drag steps do nothing, and the press keeps
    /// the pointer until it releases.
    pub(super) fn drop_drag(&mut self) {
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
    pub(super) fn end_press(&mut self) -> bool {
        matches!(std::mem::take(&mut self.capture), Capture::Held { .. })
    }

    pub(super) fn on_focus_change(&mut self, focused: Option<&str>) {
        self.presses.keep_on(focused);
        if self.dragging_path().is_some_and(|p| Some(p) != focused) {
            self.drop_drag();
        }
    }
}

pub(super) fn hit(e: &crate::edit::TextEdit, l: &Layout<Ink>, x: f32, y: f32, s: f32, single: bool) -> Hit {
    let h = (l.height() - 1e-3).max(0.0);
    let ys = if single || x < 0.0 { (y * s).clamp(0.0, h) } else { (y * s).max(0.0) };
    let cur = crate::text::cursor_from_point(l, x * s, ys);
    let caret = e.model_index(cur.index());
    let cluster = if cur.affinity() == Affinity::Upstream
        && let Some(cl) = cur.index().checked_sub(1).and_then(|b| parley::Cluster::from_byte_index(l, b))
        && !cl.is_hard_line_break()
    {
        e.model_index(cl.text_range().start)
    } else {
        caret
    };
    Hit { caret, cluster }
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

impl<S: 'static> Runtime<S> {
    /// A press at (x, y) on text field `ji`, down at `down_at`; `second` when it's the second
    /// of a double press.
    pub(super) fn input_down(&mut self, ji: usize, x: f32, y: f32, down_at: f64, second: bool) {
        let now = self.now_clock();
        let shift = self.held_mods.shift;
        // Its text drew scaled or rotated with its box: map the press back to the rest layout.
        let [lx, ly] = self.to_local(ji, x, y);
        let jn = &self.nodes[ji];
        let path = jn.path.clone();
        let (ix, iy) = (lx - jn.inset[0], ly - jn.inset[1]);
        let r = jn.target;
        let clicks = if shift {
            self.input_click.presses.clear();
            1
        } else {
            self.input_click.presses.press(&path, now, (x, y), self.double_click_interval).min(3) as u8
        };
        self.input_click.capture = Capture::Held { dragging: Some(path.clone()) };
        self.input_click.steps.push_back((path.clone(), Step::Down { clicks, shift }));
        self.queue.push(Action::Focus(path.clone(), ix, iy));
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
            window: [x, y],
            screen: self.screen_at(x, y),
        };
        self.press.hold(capture::Held::new(path, d, crate::PointerButton::Primary, (x, y), down_at, second, None));
    }

    pub(super) fn input_drag(&mut self, x: f32, y: f32, drag_move: bool) {
        if !drag_move {
            return;
        }
        let Some(path) = self.input_click.dragging_path().map(str::to_owned) else { return };
        if self.press.held().is_some_and(|h| !press_run::within_slop(h.at, (x, y))) {
            self.input_click.presses.clear();
        }
        let Some(ji) = self.nodes.iter().enumerate().position(|(i, n)| n.path == path && !self.subtree_exiting(i) && n.input.is_some()) else {
            self.input_click.drop_drag();
            return;
        };
        let [lx, ly] = self.to_local(ji, x, y);
        let jn = &self.nodes[ji];
        let (ix, iy) = (lx - jn.inset[0], ly - jn.inset[1]);
        self.input_click.steps.push_back((path.clone(), Step::Drag));
        self.queue.push(Action::Focus(path, ix, iy));
    }

    pub(super) fn focus_input(&mut self, path: String, x: f32, y: f32, one: &mut Tx) -> bool {
        let step = if self.input_click.steps.front().is_some_and(|(p, _)| *p == path) {
            self.input_click.steps.pop_front().unwrap().1
        } else {
            Step::Down { clicks: 1, shift: false }
        };
        if matches!(step, Step::Cancelled) {
            return false;
        }
        let Some(access) = self.node(&path).and_then(|n| n.input.as_ref()?.access.clone()) else {
            self.input_click.drop_drag();
            return false;
        };
        let single = self.node(&path).and_then(|n| n.input.as_ref()).is_some_and(|i| i.single_line);
        let layout = self.node(&path).and_then(|n| n.text_layout.clone());
        let s = self.env.scale;
        let scroll = self.input_scroll.get(&path).copied().unwrap_or(0.0);
        match step {
            Step::Down { clicks, shift } => {
                self.set_focus(Some(path), false);
                let e = access(states!(self));
                e.set_single_line(single);
                e.break_history_group();
                e.clear_typing_style();
                if e.preedit().is_some() {
                    self.input_click.clear();
                    one.instant = true;
                    return true;
                }
                if let Some(l) = layout {
                    let h = hit(e, &l, x + scroll, y, s, single);
                    if shift {
                        let anchor = e.anchor;
                        self.input_click.unit = Unit::Char;
                        self.input_click.origin = (anchor, anchor);
                        e.set_selection(anchor, h.caret);
                    } else {
                        let unit = match clicks {
                            1 => Unit::Char,
                            2 => Unit::Word,
                            _ => Unit::Paragraph,
                        };
                        let origin = unit_at(e, h, unit);
                        self.input_click.unit = unit;
                        self.input_click.origin = origin;
                        e.set_selection(origin.0, origin.1);
                    }
                }
                one.instant = true;
                true
            }
            Step::Drag => {
                if self.focus.path() != Some(path.as_str()) {
                    self.input_click.drop_drag();
                    return false;
                }
                let e = access(states!(self));
                e.set_single_line(single);
                if e.preedit().is_some() {
                    self.input_click.clear();
                    return false;
                }
                let Some(l) = layout else { return false };
                let h = hit(e, &l, x + scroll, y, s, single);
                let at = unit_at(e, h, self.input_click.unit);
                let (anchor, focus) = extend(self.input_click.origin, at);
                let before = (e.anchor, e.focus);
                e.set_selection(anchor, focus);
                if (e.anchor, e.focus) == before {
                    return false;
                }
                e.break_history_group();
                one.instant = true;
                true
            }
            Step::Cancelled => false,
        }
    }
}

#[cfg(test)]
#[path = "../input_click_tests.rs"]
mod tests;
