//! Text a view keeps painted as selected after the selection has gone ([`Cx::mark_texts`]), as a
//! web page paints a CSS custom highlight styled like its `::selection`: a quote's source in a
//! transcript while its comment is written, say. Each mark names a text by path and a range of
//! its bytes, with the bytes it held then; it paints only while the text still holds them there,
//! so a text that changed under it never lights up something else.
use super::*;
use std::ops::Range;

/// A range of an [`El::selectable`](crate::El::selectable) text to paint as selected
/// ([`Cx::mark_texts`]): the text's path ([`ElementInfo::path`](crate::ElementInfo)), the
/// bytes of its content, and what they held when marked. Take one from a selection with
/// [`SelectedText::mark`](crate::SelectedText::mark).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextMark {
    path: String,
    range: Range<usize>,
    text: String,
}

impl TextMark {
    /// Bytes `range` of the text at `path`, which hold `text`.
    pub fn new(path: impl Into<String>, range: Range<usize>, text: impl Into<String>) -> TextMark {
        TextMark { path: path.into(), range, text: text.into() }
    }
    /// The text's path.
    pub fn path(&self) -> &str { &self.path }
    /// The marked bytes of the text's content.
    pub fn range(&self) -> Range<usize> { self.range.clone() }
    /// What the marked bytes held.
    pub fn text(&self) -> &str { &self.text }
}

impl<S> Cx<S> {
    /// Paints `marks` as the selection paints, in each text's own selection colour
    /// ([`El::selection_color`](crate::El::selection_color)), while the view keeps declaring
    /// them: one square box per line of each mark, a line tall and as wide as its glyphs, with
    /// no newline's width at a line's end, as a CSS custom highlight's background is drawn over
    /// a text's line boxes. A mark whose text has gone, isn't mounted, or no longer holds the
    /// marked bytes there paints nothing. Set them on every run of the view; a later call adds
    /// to the earlier ones.
    pub fn mark_texts(&mut self, marks: impl IntoIterator<Item = TextMark>) {
        let mut all = self.defaults.marks.to_vec();
        all.extend(marks);
        self.defaults.marks = all.into();
    }
}

impl<S: 'static> Runtime<S> {
    /// The marks the view declared, as (node, start, end) for each mounted text that still
    /// holds its marked bytes, in node order.
    pub(super) fn mark_ranges(&self) -> Vec<(usize, usize, usize)> {
        if self.defaults.marks.is_empty() { return Vec::new(); }
        let mut by_path: HashMap<&str, Vec<&TextMark>> = HashMap::default();
        for m in self.defaults.marks.iter() { by_path.entry(m.path.as_str()).or_default().push(m); }
        let mut out = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            let Some(marks) = by_path.get(n.path.as_str()) else { continue };
            if self.subtree_exiting(i) { continue; }
            let content = self.content_of(i);
            for m in marks {
                if !m.range.is_empty() && content.get(m.range.clone()) == Some(m.text.as_str()) {
                    out.push((i, m.range.start, m.range.end));
                }
            }
        }
        out
    }
}

/// Pushes the boxes of mark `a..b` of `layout`'s `content`, drawn at `origin` (device px) `k`
/// times its size: square, one per line, the line box tall, each line's run of glyphs wide
/// without the newline that may end it.
/// `laid` maps content bytes to the layout's (an elided text's differ).
#[allow(clippy::too_many_arguments)]
pub(super) fn push_mark(out: &mut Vec<Instance>, layout: &Layout<Ink>, content: &str, (a, b): (usize, usize), laid: impl Fn(usize, usize) -> (usize, usize),
    origin: [f32; 2], k: f32, fill: [f32; 4], clip: [f32; 4], fade: [f32; 4]) {
    let Some(marked) = content.get(a..b) else { return };
    let mut at = a;
    for line in marked.split_inclusive('\n') {
        let (from, to) = laid(at, at + line.trim_end_matches(['\n', '\r']).len());
        for (bb, _) in crate::text::range_boxes(layout, from, to, true) {
            let rect = [origin[0] + bb.x0 as f32 * k, origin[1] + bb.y0 as f32 * k, (bb.x1 - bb.x0) as f32 * k, (bb.y1 - bb.y0) as f32 * k];
            out.push(rect_inst(rect, fill, 0.0, clip, fade));
        }
        at += line.len();
    }
}
