//! A text field's visual lines in window points (`Runtime::text_line`, `Tx::text_line`), so an
//! app can align a gutter, per-line notes or rules with the field's own lines.
use super::*;

/// One visual line of a `text_input`, as [`Runtime::text_line`] and
/// [`Tx::text_line`](crate::Tx::text_line) read it from the last layout. Bytes index the
/// field's [`TextEdit`](crate::TextEdit) text.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextLine {
    /// The line's box `[x, y, w, h]` in window points at its laid-out place, after scrolling:
    /// `y` and `h` are the line's, `x` and `w` span the field's laid-out text.
    pub rect: [f32; 4],
    /// The line's first byte.
    pub start: usize,
    /// The previous visual line's first byte; `None` on the first line.
    pub prev: Option<usize>,
    /// The next visual line's first byte; `None` on the last line.
    pub next: Option<usize>,
}

/// Where an IME preedit sits in the drawn text: the model range it replaces and its length.
fn splice(e: &crate::edit::TextEdit) -> Option<(usize, usize, usize)> {
    let (p, _) = e.preedit()?;
    let (a, b) = e.range();
    Some((a, b, p.len()))
}

/// Model byte `byte` in the drawn (display) text: a byte the preedit replaces, and the byte it
/// sits at, land where the preedit starts.
fn to_display(e: &crate::edit::TextEdit, byte: usize) -> usize {
    let byte = byte.min(e.text.len());
    match splice(e) {
        Some((a, _, _)) if byte <= a => byte,
        Some((a, b, _)) if byte < b => a,
        Some((a, b, len)) => byte - (b - a) + len,
        None => byte,
    }
}

/// Drawn (display) byte `byte` in the model text: a byte inside the preedit lands where it starts.
fn to_model(e: &crate::edit::TextEdit, byte: usize) -> usize {
    match splice(e) {
        Some((a, b, len)) if byte >= a + len => (byte - len + (b - a)).min(e.text.len()),
        Some((a, _, _)) if byte > a => a,
        _ => byte.min(e.text.len()),
    }
}

/// Whether the visual line starting at display byte `start` begins a model line. A line that
/// starts inside the preedit or just past it continues the one above, so the visual lines a
/// composition covers read as one line and model line starts strictly increase.
fn heads(e: &crate::edit::TextEdit, start: usize) -> bool {
    splice(e).is_none_or(|(a, _, len)| !(a < start && start <= a + len))
}

/// The first display byte of the visual line above (`!down`) or below (`down`) the one
/// starting at display byte `start`.
fn step(flow: TextFlow<'_>, len: usize, start: usize, down: bool) -> Option<usize> {
    if !down && start == 0 { return None; }
    let (b, _) = flow.move_line(start, down, None)?;
    let b = flow.line_edge(b.min(len), false).min(len);
    (if down { b > start } else { b < start }).then_some(b)
}

impl RuntimeCore {
    /// The visual line holding byte `byte` of the text field named `key` (a key or key path, as
    /// [`Runtime::find`] resolves it), focused or not, from the last layout. `None` when `key`
    /// is missing, exiting, not a text field, or secure. While an IME composes, the visual lines
    /// the composition covers read as one line.
    pub fn text_line(&self, key: &str, byte: usize) -> Option<TextLine> {
        let i = self.live_named(None, key)?;
        let n = &self.tree.nodes[i];
        if n.is_secure() { return None; }
        let (inp, spec, laid) = (n.input.as_deref()?, n.text.as_deref()?, n.text_layout.as_ref()?);
        let e = &inp.edit;
        let flow = laid.view();
        let s = self.env.scale;
        let len = spec.content.len();
        let at = spec.content.floor_char_boundary(to_display(e, byte).min(len));
        let mut start = flow.line_edge(at, false).min(len);
        while !heads(e, start) && let Some(up) = step(flow, len, start, false) {
            start = up;
        }
        let mut last = start;
        let next = loop {
            match step(flow, len, last, true) {
                Some(b) if heads(e, b) => break Some(b),
                Some(b) => last = b,
                None => break None,
            }
        };
        let mut prev = step(flow, len, start, false);
        while let Some(p) = prev.filter(|&p| !heads(e, p)) {
            prev = step(flow, len, p, false);
        }
        let [x, y] = self.field_text_origin(&self.tree.nodes, i, inp, spec, laid);
        let line_px = spec.style.line_px();
        let (top_y0, _) = flow.line_bounds(start, parley::Affinity::Downstream);
        let (bot_y0, bot_y1) = flow.line_bounds(last, parley::Affinity::Downstream);
        let y0 = top_y0 / s;
        let y1 = if bot_y1 > bot_y0 { bot_y1 / s } else { bot_y0 / s + line_px };
        Some(TextLine {
            rect: [x, y + y0, text_width(flow.full_width()) / s, (y1 - y0).max(0.0)],
            start: to_model(e, start),
            prev: prev.map(|b| to_model(e, b)),
            next: next.map(|b| to_model(e, b)),
        })
    }
}

#[cfg(test)]
#[path = "field_lines_tests.rs"]
mod tests;
