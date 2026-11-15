//! Waiting paragraphs of a large field are filled where paint clips it. A caret or input can
//! need a real line before paint; smaller fields and flight layouts still fill progressively.
use super::*;

/// How long a frame shapes waiting paragraphs.
const BUDGET: std::time::Duration = std::time::Duration::from_micros(1_500);

impl<S: 'static> Runtime<S> {
    /// Resolve waiting field lines using *this* draw's clip and text origin. A scroll or a
    /// caret jump can have moved the band since the last paint; last frame's rows won't do.
    pub(super) fn fill_drawn_field(&mut self, i: usize, b: &paint_content::LeafBox) {
        let n = &self.nodes[i];
        let Some(crate::text::TextLayout::Paragraphs(p)) = &n.text_layout else { return };
        if !p.fills_visible() { return; }
        let Some(spec) = &n.text else { return };
        let s = self.env.scale;
        let (k, top) = b.cover.map_or((b.sc, b.r[1]), |(k, c)| (k, b.r[1] + c[1] * s));
        let tk = if b.cover.is_some() { 1.0 } else { b.sc };
        if tk <= 1e-4 { return; }
        let off = if b.gliding && b.cover.is_none() && b.scale_motion == ScaleMotion::Still { b.r[1] - b.r[1].round() } else { 0.0 };
        let origin = top + n.inset[1] * k * s - off;
        let raster = if b.cover.is_some() { 1.0 } else { b.scale_motion.raster(b.sc, b.rest) };
        let pad = 2.0 * spec.style.size.max(spec.style.line_px()) * s + 2.0 / raster.max(1e-4);
        let rows = ((b.shown[0] - origin) / tk - pad, (b.shown[1] - origin) / tk + pad);
        if let Some((filled, moved)) = self.text.fill_paragraphs(p, std::time::Duration::MAX, rows, 0.0) {
            let n = &mut self.nodes[i];
            n.text_layout = Some(crate::text::TextLayout::Paragraphs(filled));
            n.epoch = paint_keep::next_epoch();
            self.dirty |= moved;
        }
    }

    /// Shapes waiting paragraphs of each field for up to [`BUDGET`], the rows it drew last
    /// first. True while some still wait, so another frame comes.
    pub(super) fn fill_text(&mut self) -> bool {
        // A viewport-only field may wait indefinitely. Don't keep old fields' bands alive
        // beside it after a remount, an exit or a replacement with a short document.
        self.text_rows.retain(|id, _| self.node_index.get(id).and_then(|&i| self.nodes.get(i))
            .is_some_and(|n| matches!(&n.text_layout, Some(crate::text::TextLayout::Paragraphs(p)) if p.waiting())));
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
            // Documents larger than AccessKit's TextRun limit have no per-line AT nodes.
            // Preserve progressive fill for other consumers that need an entire flight's layout.
            if reach.is_none() && p.fills_visible() && self.flights.is_empty() && self.nodes[i].matched.is_none() { continue; }
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
        more
    }
}
