//! A long paste's paragraphs that wait to be shaped (`text::Lazy`): shaped a few a frame, those
//! the field shows first, and all of those near what an input needs before it reads them.
use super::*;

/// How long a frame shapes waiting paragraphs.
const BUDGET: std::time::Duration = std::time::Duration::from_micros(1_500);

impl<S: 'static> Runtime<S> {
    /// Shapes waiting paragraphs of each field for up to [`BUDGET`], the rows it drew last
    /// first. True while some still wait, so another frame comes.
    pub(super) fn fill_text(&mut self) -> bool {
        self.fill_text_within(BUDGET, None)
    }

    /// Before an input that reads a field's layout (a caret key, a press, a drag): every
    /// waiting paragraph within a window's height of the rows each field drew, shaped now.
    pub(super) fn fill_text_near(&mut self) {
        self.fill_text_within(std::time::Duration::MAX, Some(1.0));
    }

    fn fill_text_within(&mut self, budget: std::time::Duration, reach: Option<f32>) -> bool {
        let mut more = false;
        for i in 0..self.nodes.len() {
            if self.nodes[i].input.is_none() { continue; }
            let Some(crate::text::TextLayout::Paragraphs(p)) = &self.nodes[i].text_layout else { continue };
            if !p.waiting() { continue; }
            let p = p.clone();
            let id = self.nodes[i].id;
            let rows = self.text_rows.get(&id).copied().unwrap_or((0.0, 0.0));
            let reach = reach.map_or(f32::INFINITY, |k| (rows.1 - rows.0).max(0.0) * k + 1.0);
            let Some((built, moved)) = self.text.fill_paragraphs(&p, budget, rows, reach) else { continue };
            more |= built.waiting();
            let n = &mut self.nodes[i];
            n.text_layout = Some(crate::text::TextLayout::Paragraphs(built));
            n.epoch = paint_keep::next_epoch();
            // A waiting height is exact; should one not be, the field lays out again.
            if moved {
                #[cfg(debug_assertions)]
                eprintln!("rune: text fill: a waiting paragraph of {} wasn't the height it waited at", self.nodes[i].path);
                self.dirty = true;
            }
        }
        if !more { self.text_rows.clear(); }
        more
    }
}
