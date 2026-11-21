//! A long paste's paragraphs that wait to be shaped (`text::Lazy`): shaped a few a frame, those
//! the field shows first, and all of those near what an input needs before it reads them.
use super::*;

/// How long a frame shapes waiting paragraphs.
const BUDGET: std::time::Duration = std::time::Duration::from_micros(1_500);

/// Visible vertical row range (in unscaled layout device pixels) drawn by `paint_content` and `fill_visible_by_line`.
pub(crate) fn visible_text_rows(spec: &crate::el::TextSpec, rest_y: f32, shown: [f32; 2], tk: f32, raster: f32, s: f32) -> std::ops::Range<f32> {
    let em = spec.spans.iter().filter_map(|sp| sp.size).fold(spec.style.size.max(spec.style.line_px()), f32::max);
    let pad = 2.0 * em * s + 2.0 / raster.max(1e-4);
    if tk > 1e-4 { (shown[0] - rest_y) / tk - pad..(shown[1] - rest_y) / tk + pad } else { f32::NEG_INFINITY..f32::INFINITY }
}

impl types::TreeState {
    /// Whether any painted text still has unshaped paragraphs needing a background fill frame:
    /// every waiting paragraph for an input field, or those within a viewport's height of the
    /// painted clip for a read-only `by_line` block (whose off-screen lines stay compact runs).
    pub(super) fn text_fill_due(&self, text_rows: &HashMap<NodeId, (f32, f32)>) -> bool {
        text_rows.iter().any(|(&id, &(r0, r1))| {
            let Some(n) = self.node_by_id(id) else { return false };
            let Some(crate::text::TextLayout::Paragraphs(p)) = &n.text_layout else { return false };
            if !p.waiting() { return false; }
            if n.input.is_some() { return true; }
            let reach = (r1 - r0).max(0.0) + 1.0;
            p.paragraphs.iter().any(|q| !q.shaped && q.y + q.height >= r0 - reach && q.y <= r1 + reach)
        })
    }
}

impl RuntimeCore {
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

    #[inline]
    pub(super) fn text_fill_due(&self) -> bool {
        self.tree.text_fill_due(&self.paint.text_rows)
    }

    /// Before painting a `by_line` node (`input.is_none()`): shapes any waiting lines inside its
    /// visible clip with no time budget, so a jump, scrollbar drag, `End` or restored session
    /// never paints interior lines blank for a frame; `fill_text` keeps its budget for the margin.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fill_visible_by_line(
        &mut self,
        i: usize,
        r: [f32; 4],
        exact_off: [f32; 2],
        sc: f32,
        cover: Option<(f32, [f32; 2])>,
        scale_motion: ScaleMotion,
        rest: f32,
        gliding: bool,
        shown: [f32; 2],
    ) {
        if self.tree.nodes[i].input.is_some() { return; }
        let (Some(spec), Some(crate::text::TextLayout::Paragraphs(p))) = (&self.tree.nodes[i].text, &self.tree.nodes[i].text_layout) else { return };
        if !p.waiting() { return; }
        let s = self.env.scale;
        let tk = if cover.is_some() { 1.0 } else { sc };
        let zk = match cover { Some((k, _)) => k, None => sc };
        let raster = if cover.is_some() { 1.0 } else { scale_motion.raster(sc, rest) };
        let still = scale_motion == ScaleMotion::Still;
        let y_top = match cover { Some((_, c)) => r[1] + c[1] * s, None => r[1] }
            + if still && (sc - 1.0).abs() < 1e-5 { exact_off[1] } else { 0.0 }
            + self.tree.nodes[i].inset[1] * zk * s;
        let rest_y = y_top - if gliding && cover.is_none() && still { r[1] - r[1].round() } else { 0.0 };
        let rows = visible_text_rows(spec, rest_y, shown, tk, raster, s);
        let rows = (rows.start, rows.end);
        if !p.paragraphs.iter().any(|q| !q.shaped && q.y + q.height >= rows.0 && q.y <= rows.1) { return; }
        let p = p.clone();
        if let Some((built, moved)) = self.text.fill_paragraphs(&p, std::time::Duration::MAX, rows, 0.0) {
            self.tree.nodes[i].text_layout = Some(crate::text::TextLayout::Paragraphs(built));
            if moved { self.commit.dirty = true; }
        }
    }

    fn fill_text_within(&mut self, budget: std::time::Duration, reach: Option<f32>) -> bool {
        let mut more = false;
        for i in 0..self.tree.nodes.len() {
            let is_input = self.tree.nodes[i].input.is_some();
            let Some(crate::text::TextLayout::Paragraphs(p)) = &self.tree.nodes[i].text_layout else { continue };
            if !p.waiting() { continue; }
            let id = self.tree.nodes[i].id;
            let rows = match (is_input, self.paint.text_rows.get(&id).copied()) {
                (true, r) => r.unwrap_or((0.0, 0.0)),
                (false, Some(r)) => r,
                (false, None) => continue,
            };
            let p = p.clone();
            let eff_reach = match (is_input, reach) {
                (true, None) => f32::INFINITY,
                (_, r) => (rows.1 - rows.0).max(0.0) * r.unwrap_or(1.0) + 1.0,
            };
            let Some((built, moved)) = self.text.fill_paragraphs(&p, budget, rows, eff_reach) else { continue };
            if is_input {
                more |= built.waiting();
            } else {
                more |= built.paragraphs.iter().any(|q| !q.shaped && q.y + q.height >= rows.0 - eff_reach && q.y <= rows.1 + eff_reach);
            }
            let n = &mut self.tree.nodes[i];
            n.text_layout = Some(crate::text::TextLayout::Paragraphs(built));
            n.epoch = paint_keep::next_epoch();
            // A waiting height is exact; should one not be, the field lays out again.
            if moved {
                #[cfg(debug_assertions)]
                eprintln!("rune: text fill: a waiting paragraph of {} wasn't the height it waited at", self.tree.nodes[i].path);
                self.commit.dirty = true;
            }
        }
        let tree = &self.tree;
        self.paint.text_rows.retain(|&id, _| {
            tree.node_by_id(id)
                .and_then(|n| n.text_layout.as_ref())
                .is_some_and(|l| matches!(l, crate::text::TextLayout::Paragraphs(p) if p.waiting()))
        });
        more
    }
}
