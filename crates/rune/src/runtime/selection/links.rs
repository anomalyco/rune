//! Pressable spans in text (`El::link`). Each link's line boxes come from the shaped layout,
//! so only its glyphs take the pointer. The text element holds focus for its links and
//! `LinkState::focus` says which one; links are reported to tools as their own elements.
use super::*;
use crate::text::LinkPaint;

/// The focused text element's focused link. A press's link is the press's
/// (`capture::Held::link`, `capture::Secondary::Held`).
#[derive(Default)]
pub(in crate::runtime) struct LinkState {
    focus: Option<usize>,
}

/// Where node `n`'s text starts inside its border box `rect` (points, x0 y0 x1 y1), its text
/// drawn `k` times its size.
pub(crate) fn origin(n: &Node, spec: &TextSpec, flow: TextFlow<'_>, rect: [f32; 4], s: f32, k: f32) -> [f32; 2] {
    let x = text_x(spec, rect[0] + n.inset[0] * k, rect[2] - n.inset[2] * k, text_width(flow.full_width() * k) / s, flow.flow());
    let y = text_y(spec, rect[1] + n.inset[1] * k, rect[3] - n.inset[3] * k, text_height(spec, flow, s) * k / s);
    [x, y]
}

/// Link bytes `a..b` of node `n`'s text as bytes of its drawn layout, which truncation may
/// have cut (empty when the cut hides the link).
fn drawn(n: &Node, a: u32, b: u32) -> (u32, u32) {
    let (a, b) = n.elided.map_or((a as usize, b as usize), |e| e.mark(a as usize, b as usize, true));
    (a as u32, b as u32)
}

/// The byte length of node `n`'s drawn text.
fn layout_len(n: &Node, spec: &TextSpec) -> usize {
    n.elided.map_or(spec.content.len(), |e| e.drawn_len(spec.content.len()))
}

/// What a pointer position `(x, y)` hits on a laid-out text flow: enough to decide which
/// drawn link byte ranges `a..b` could produce a line box containing `(x, y)` without calling
/// `range_boxes` on every link.
enum LinkHitCandidate<'a> {
    /// A shaped byte on `piece` (a visual cluster's start byte or an explicit newline byte).
    ShapedByte(crate::text::Piece<'a>, usize),
    /// A plain line's shaped range `start..end` on `piece` when `(x, y)` is inside the line's
    /// whole-line box (`range_boxes`'s `plain && a <= lr.start && lr.end <= b` fast path).
    WholeLine(crate::text::Piece<'a>, usize, usize),
    /// A paragraph-break newline byte `piece.end` between paragraphs in `TextFlow::Paragraphs`.
    ParaNewline(usize),
}

impl LinkHitCandidate<'_> {
    #[inline]
    fn matches(&self, a: usize, b: usize) -> bool {
        match *self {
            Self::ShapedByte(piece, at) => piece.clip(a, b).is_some_and(|(sa, sb)| sa <= at && at < sb),
            Self::WholeLine(piece, ls, le) => piece.clip(a, b).is_some_and(|(sa, sb)| sa <= ls && le <= sb),
            Self::ParaNewline(nl) => a <= nl && nl < b,
        }
    }
}

/// Collects the shaped-byte, whole-line and paragraph-break candidates on `flow` whose
/// `range_boxes(..., false)` output can contain window point `(x, y)`.
fn link_hit_candidates<'a>(
    flow: TextFlow<'a>,
    spec: &TextSpec,
    origin: [f32; 2],
    px: f32,
    s: f32,
    x: f32,
    y: f32,
) -> Vec<LinkHitCandidate<'a>> {
    let [ox, oy] = origin;
    let eps = (ox.abs().max(x.abs()) * 1e-5).max(1e-3);
    // `range_boxes` merges cluster intervals separated by <= 0.5 device px, so a point inside a
    // merged box is always within 0.5 device px of at least one constituent interval.
    let near_x = |x0: f64, x1: f64| -> bool {
        x >= ox + (x0 - 0.5) as f32 / px - eps && x <= ox + (x1 + 0.5) as f32 / px + eps
    };
    let para_count = match flow {
        TextFlow::Single(_) => 1,
        TextFlow::Paragraphs(pl) => pl.paragraphs.len(),
    };
    let py = (y - oy) * px;
    let margin = (spec.style.line_px() * s * 4.0).max(128.0);
    let mut out = Vec::new();
    for piece in flow.pieces_in(py - margin, py + margin) {
        let y_off = piece.y as f64;
        let plain = piece.layout.inline_boxes().next().is_none();
        let justified = piece.layout.alignment() == Some(parley::Alignment::Justify);
        let line_count = piece.layout.len();
        let has_para_newline = !piece.is_whole() && piece.index + 1 < para_count;
        for (line_ix, line) in piece.layout.lines().enumerate() {
            let m = line.metrics();
            let line_min = m.block_min_coord.min(m.content_block_min_coord) as f64;
            let line_max = m.block_max_coord.max(m.content_block_max_coord) as f64;
            let y0 = oy + (line_min + y_off) as f32 / px;
            let y1 = oy + (line_max + y_off) as f32 / px;
            if !(y >= y0 && y < y1) {
                continue;
            }
            let lr = line.text_range();
            let start = (m.inline_min_coord + m.offset) as f64;
            let end = if justified && plain {
                start + line.runs().map(|r| r.advance() as f64).sum::<f64>()
            } else {
                start + m.advance as f64
            };
            if plain && lr.start < lr.end && near_x(start, end) {
                out.push(LinkHitCandidate::WholeLine(piece, lr.start, lr.end));
            }
            for run in line.runs() {
                let mut cx = (m.inline_min_coord
                    + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset)) as f64;
                for cl in run.visual_clusters() {
                    let adv = cl.advance() as f64;
                    if adv > 0.0 && near_x(cx, cx + adv) {
                        out.push(LinkHitCandidate::ShapedByte(piece, cl.text_range().start));
                    }
                    cx += adv;
                }
            }
            if line.break_reason() == parley::BreakReason::Explicit && lr.end > lr.start {
                let nl_byte = lr.end - 1;
                if let Some((nl_ws, nl_rtl)) = parley::Cluster::from_byte_index(piece.layout, nl_byte).map(|c| {
                    let run = c.run();
                    let fm = run.font_metrics();
                    ((fm.ascent as f64 + fm.descent as f64) * 0.25, c.is_rtl())
                }) && nl_ws > 0.0
                {
                    let (nx0, nx1) = if nl_rtl { (start - nl_ws, start) } else { (end, end + nl_ws) };
                    if near_x(nx0, nx1) {
                        out.push(LinkHitCandidate::ShapedByte(piece, nl_byte));
                    }
                }
            }
            if has_para_newline && !piece.is_blank() && line_ix + 1 == line_count {
                out.push(LinkHitCandidate::ParaNewline(piece.end));
            }
        }
        if has_para_newline && (piece.is_blank() || line_count == 0) {
            let y0 = oy + y_off as f32 / px;
            let y1 = oy + (piece.height as f64 + y_off) as f32 / px;
            if y >= y0 && y < y1 {
                out.push(LinkHitCandidate::ParaNewline(piece.end));
            }
        }
    }
    out
}

pub(in crate::runtime) use super::link_decos::LinkFx;
#[cfg(test)]
use super::link_decos::{boxes, take_link_deco_visits, take_link_line_end_visits};

pub(in crate::runtime) fn link_count(n: &Node) -> usize {
    n.text.as_ref().map_or(0, |t| t.cold.links.len())
}

impl caret::TextInputState {
    /// Tab inside the focused text's links: moves to its next (or previous) link and returns
    /// true, or false at its last (first) link so Tab leaves it.
    #[inline]
    pub(in crate::runtime) fn link_tab(&mut self, n: &Node, backward: bool) -> bool {
        let count = link_count(n);
        let Some(k) = self.links.focus.filter(|_| count > 0) else { return false };
        let next = if backward { k.checked_sub(1) } else { Some(k + 1).filter(|&j| j < count) };
        if next.is_some() { self.links.focus = next; }
        next.is_some()
    }

    /// Tab focused node `n`: start at its first link, or its last going backward.
    #[inline]
    pub(in crate::runtime) fn enter_links(&mut self, n: &Node, backward: bool) {
        let count = link_count(n);
        self.links.focus = (count > 0).then(|| if backward { count - 1 } else { 0 });
    }

    #[inline]
    pub(in crate::runtime) fn blur_links(&mut self) {
        self.links.focus = None;
    }

    /// The focused text's focused link, if any (it may not be in range for that text).
    #[inline]
    pub(in crate::runtime) fn link_focus(&self) -> Option<usize> { self.links.focus }

    /// Focuses link `k` of the text about to take focus, as Tab into it does.
    #[inline]
    pub(in crate::runtime) fn set_link_focus(&mut self, k: usize) { self.links.focus = Some(k); }

    /// Whether focused node `n` has a link focused, which takes Return.
    #[inline]
    pub(in crate::runtime) fn link_focused(&self, n: &Node) -> bool {
        self.links.focus.is_some_and(|k| k < link_count(n))
    }

    /// The id of focused node `n`'s focused link, which tools list as its own element.
    pub(in crate::runtime) fn focused_link_id(&self, n: &Node) -> Option<String> {
        let k = self.links.focus?;
        n.text.as_ref()?.cold.links.get(k).map(|l| l.id.clone())
    }
}

impl RuntimeCore {
    /// Node `i`'s border box as drawn this frame (points, x0 y0 x1 y1).
    pub(crate) fn drawn_box(&self, i: usize) -> [f32; 4] {
        self.paint.drawn.get(i).filter(|d| d[2] > 0.0 || d[3] > 0.0)
            .map_or(self.tree.nodes[i].target, |d| [d[0], d[1], d[0] + d[2], d[1] + d[3]])
    }

    /// Inline font-metric boxes `(x0, y0, x1, y1)` in window points for each `(start, end)` byte
    /// span in `spans` of node `n`, matching Blink's `Range::getBoundingClientRect()`.
    fn span_inline_boxes(&self, n: &Node, rect: [f32; 4], z: f32, spans: impl IntoIterator<Item = (u32, u32)>) -> Vec<(usize, [f32; 4])> {
        let mut spans = spans.into_iter().peekable();
        if spans.peek().is_none() { return Vec::new(); }
        let Some(spec) = &n.text else { return Vec::new() };
        if let Some(pt) = spec.positioned() {
            let tf = super::positioned::PositionedTransform::from_window_box(n, pt, rect, z);
            let mut out = Vec::new();
            for (k, (start, end)) in spans.enumerate() {
                pt.for_each_range_box(start as usize, end as usize, |r| out.push((k, tf.rect_x0y0x1y1(r))));
            }
            return out;
        }
        let Some(laid) = &n.text_layout else { return Vec::new() };
        let flow = laid.view();
        let s = self.env.scale;
        let [ox, oy] = origin(n, spec, flow, rect, s, z);
        let px = s / z.max(1e-4);
        let len = layout_len(n, spec);
        let mut out = Vec::new();
        for (k, (start, end)) in spans.enumerate() {
            let (a, b) = drawn(n, start, end);
            #[cfg(test)]
            SPAN_BOX_QUERIES.with(|c| c.set(c.get() + 1));
            for bb in flow.inline_range_boxes((a as usize).min(len), (b as usize).min(len)) {
                out.push((k, [ox + bb.x0 as f32 / px, oy + bb.y0 as f32 / px, ox + bb.x1 as f32 / px, oy + bb.y1 as f32 / px]));
            }
        }
        out
    }

    /// Line boxes in window points `(x0, y0, x1, y1)` for each `(index, start, end)` byte span in
    /// `spans` of node `n`, inside border box `rect` drawn at scale `z`.
    fn span_boxes_indexed(&self, n: &Node, rect: [f32; 4], z: f32, spans: impl IntoIterator<Item = (usize, u32, u32)>) -> Vec<(usize, [f32; 4])> {
        let mut spans = spans.into_iter().peekable();
        if spans.peek().is_none() { return Vec::new(); }
        let Some(spec) = &n.text else { return Vec::new() };
        if let Some(pt) = spec.positioned() {
            let tf = super::positioned::PositionedTransform::from_window_box(n, pt, rect, z);
            let mut out = Vec::new();
            for (k, start, end) in spans {
                pt.for_each_range_box(start as usize, end as usize, |r| out.push((k, tf.rect_x0y0x1y1(r))));
            }
            return out;
        }
        let Some(laid) = &n.text_layout else { return Vec::new() };
        let flow = laid.view();
        let s = self.env.scale;
        let [ox, oy] = origin(n, spec, flow, rect, s, z);
        let px = s / z.max(1e-4);
        let len = layout_len(n, spec);
        let mut out = Vec::new();
        for (k, start, end) in spans {
            let (a, b) = drawn(n, start, end);
            #[cfg(test)]
            SPAN_BOX_QUERIES.with(|c| c.set(c.get() + 1));
            for (bb, _) in flow.range_boxes((a as usize).min(len), (b as usize).min(len), false) {
                out.push((k, [ox + bb.x0 as f32 / px, oy + bb.y0 as f32 / px, ox + bb.x1 as f32 / px, oy + bb.y1 as f32 / px]));
            }
        }
        out
    }

    /// Line boxes in window points `(x0, y0, x1, y1)` for each `(start, end)` byte span in `spans`
    /// of node `n`, inside border box `rect` drawn at scale `z`.
    fn span_boxes(&self, n: &Node, rect: [f32; 4], z: f32, spans: impl IntoIterator<Item = (u32, u32)>) -> Vec<(usize, [f32; 4])> {
        self.span_boxes_indexed(n, rect, z, spans.into_iter().enumerate().map(|(k, (a, b))| (k, a, b)))
    }

    /// Node `i`'s link line boxes in window points (x0, y0, x1, y1) for border box `rect`.
    pub(crate) fn link_boxes(&self, i: usize, rect: [f32; 4]) -> Vec<(usize, [f32; 4])> {
        self.link_boxes_in(&self.tree.nodes, i, rect, self.paint.drawn_scale.get(i).copied().unwrap_or(1.0))
    }

    fn link_boxes_in(&self, nodes: &[Node], i: usize, rect: [f32; 4], z: f32) -> Vec<(usize, [f32; 4])> {
        let n = &nodes[i];
        let Some(spec) = n.text.as_deref() else { return Vec::new() };
        if spec.cold.links.is_empty() { return Vec::new(); }
        self.span_boxes(n, rect, z, spec.cold.links.iter().map(|l| (l.start, l.end)))
    }

    /// Each inline icon of node `i` that has a name or a tooltip (`InlineIcon::label`,
    /// `InlineIcon::help`): its pill's index and the icon's own square in window points
    /// (x0, y0, x1, y1), for border box `rect`.
    pub(crate) fn inline_icon_boxes(&self, i: usize, rect: [f32; 4]) -> Vec<(usize, [f32; 4])> {
        let n = &self.tree.nodes[i];
        let (Some(spec), Some(laid)) = (n.text.as_deref(), n.text_layout.as_ref()) else { return Vec::new() };
        let said = |p: &crate::text::PillChip| p.bare.as_deref().is_some_and(|b| b.label.is_some() || b.help.is_some());
        if !spec.cold.pills.iter().any(said) { return Vec::new(); }
        let flow = laid.view();
        let s = self.env.scale;
        let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
        let [ox, oy] = origin(n, spec, flow, rect, s, z);
        let px = s / z.max(1e-4);
        let (mut out, mut found) = (Vec::new(), Vec::new());
        for piece in flow.pieces() {
            let pills = piece.pills(&spec.cold.pills);
            found.clear();
            crate::text::bare_icons(piece.layout, &pills, s, [0.0, piece.y], &mut found);
            for &(k, at, size) in &found {
                if !said(&pills[k]) { continue; }
                // A paragraph's pills are its own: the text's is the one whose word starts there.
                let k = if piece.is_whole() {
                    k
                } else {
                    let start = piece.content(pills[k].start as usize) as u32;
                    let Some(k) = spec.cold.pills.iter().position(|p| p.start == start && said(p)) else { continue };
                    k
                };
                out.push((k, [ox + at[0] / px, oy + at[1] / px, ox + (at[0] + size[0]) / px, oy + (at[1] + size[1]) / px]));
            }
        }
        out
    }

    /// Like [`Self::link_boxes`], laying out only link `k` of node `i`.
    pub(crate) fn link_boxes_for(&self, i: usize, k: usize, rect: [f32; 4]) -> Vec<(usize, [f32; 4])> {
        let n = &self.tree.nodes[i];
        let Some(l) = n.text.as_deref().and_then(|spec| spec.cold.links.get(k)) else { return Vec::new() };
        let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
        self.span_boxes_indexed(n, rect, z, std::iter::once((k, l.start, l.end)))
    }

    /// Like [`Self::link_boxes`], laying out only links of node `i` that match `pred`.
    pub(crate) fn link_boxes_matching(&self, i: usize, rect: [f32; 4], mut pred: impl FnMut(&crate::Link) -> bool) -> Vec<(usize, [f32; 4])> {
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_deref() else { return Vec::new() };
        let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
        let spans = spec.cold.links.iter().enumerate()
            .filter(|(_, l)| pred(l))
            .map(|(k, l)| (k, l.start, l.end));
        self.span_boxes_indexed(n, rect, z, spans)
    }

    /// Index of the first `(start, end)` byte span on node `i` whose `range_boxes(..., false)`
    /// line box (shifted left by `scroll_x` window points) contains window point `(x, y)`.
    fn span_at(
        &self,
        i: usize,
        x: f32,
        y: f32,
        scroll_x: f32,
        spans: impl IntoIterator<Item = (u32, u32)>,
    ) -> Option<usize> {
        let n = &self.tree.nodes[i];
        let spec = n.text.as_deref()?;
        let (rect, ux, uy) = self.unturned_box_and_point(i, x, y);
        if !ux.is_finite() || !uy.is_finite() { return None; }
        if let Some(pt) = spec.positioned() {
            let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
            let tf = super::positioned::PositionedTransform::from_window_box(n, pt, rect, z);
            for (k, (start, end)) in spans.into_iter().enumerate() {
                let mut matched = false;
                pt.for_each_range_box(start as usize, end as usize, |r| {
                    let b = tf.rect_x0y0x1y1(r);
                    if contains([b[0] - scroll_x, b[1], b[2] - scroll_x, b[3]], ux, uy) {
                        matched = true;
                    }
                });
                if matched { return Some(k); }
            }
            return None;
        }
        let laid = n.text_layout.as_ref()?;
        let flow = laid.view();
        let s = self.env.scale;
        let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
        let orig = origin(n, spec, flow, rect, s, z);
        let px = s / z.max(1e-4);
        let hit = |k: usize, start: u32, end: u32| {
            self.span_boxes_indexed(n, rect, z, std::iter::once((k, start, end)))
                .into_iter()
                .any(|(_, r)| contains([r[0] - scroll_x, r[1], r[2] - scroll_x, r[3]], ux, uy))
        };
        let cx = ux + scroll_x;
        if !(px > 0.0 && px.is_finite() && orig[0].is_finite() && orig[1].is_finite() && cx.is_finite()) {
            return spans.into_iter().enumerate().find_map(|(k, (start, end))| hit(k, start, end).then_some(k));
        }
        let candidates = link_hit_candidates(flow, spec, orig, px, s, cx, uy);
        if candidates.is_empty() { return None; }
        let len = layout_len(n, spec);
        for (k, (start, end)) in spans.into_iter().enumerate() {
            let (a, b) = drawn(n, start, end);
            let (a, b) = ((a as usize).min(len), (b as usize).min(len));
            if a >= b || !candidates.iter().any(|c| c.matches(a, b)) {
                continue;
            }
            if hit(k, start, end) {
                return Some(k);
            }
        }
        None
    }

    /// The link of node `i` under window point (x, y).
    pub(in crate::runtime) fn link_at(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        if self.node_disabled(i) { return None; }
        let spec = self.tree.nodes[i].text.as_deref()?;
        if spec.cold.links.is_empty() { return None; }
        self.span_at(i, x, y, 0.0, spec.cold.links.iter().map(|l| (l.start, l.end)))
    }

    /// The id of the `Mark::Pill` in text field `i` under window point `(x, y)`, if any.
    pub(in crate::runtime) fn input_pill_at(&self, i: usize, x: f32, y: f32) -> Option<String> {
        let n = &self.tree.nodes[i];
        let input = n.input.as_deref()?;
        if input.edit.is_secure() || input.edit.preedit().is_some() { return None; }
        let pills = input.edit.display_pills();
        if pills.is_empty() { return None; }
        let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
        let scroll_x = if input.single_line { self.edit.input_scroll.get(&n.path).copied().unwrap_or(0.0) * z } else { 0.0 };
        self.span_at(i, x, y, scroll_x, pills.iter().map(|&(s, e, _)| (s, e)))
            .map(|k| pills[k].2.to_owned())
    }

    /// Hit rects for node `i`: only its links' boxes, clipped like `clipped`. False when it
    /// is a text field, has no links, or is selectable (it takes its whole box).
    pub(in crate::runtime) fn push_link_hits(&mut self, i: usize, rect: [f32; 4], clipped: [f32; 4]) -> bool {
        if self.tree.nodes[i].input.is_some()
            || self.tree.nodes[i].text.as_ref().is_none_or(|t| t.cold.links.is_empty() || t.selectable)
        {
            return false;
        }
        let id = self.tree.nodes[i].id;
        for (_, r) in self.link_boxes(i, rect) {
            let c = [r[0].max(clipped[0]), r[1].max(clipped[1]), r[2].min(clipped[2]), r[3].min(clipped[3])];
            if c[2] > c[0] && c[3] > c[1] { self.hit.push_hit(id, c); }
        }
        true
    }

    /// The text node and link index currently hovered by the pointer, if any.
    pub(in crate::runtime) fn hovered_link(&self) -> Option<(NodeId, usize)> {
        let (x, y) = self.pointer.pos?;
        let path = self.pointer.hovered.as_deref()?;
        let i = self.live_index(path)?;
        if self.tree.nodes[i].text.as_ref().is_none_or(|t| t.cold.links.is_empty()) { return None; }
        Some((self.tree.nodes[i].id, self.link_at(i, x, y)?))
    }

    /// Which of node `i`'s links are hovered, pressed and keyboard-focused.
    pub(in crate::runtime) fn link_states(&self, i: usize) -> (Option<usize>, Option<usize>, Option<usize>) {
        let path = &self.tree.nodes[i].path;
        let hovered = self.pointer.pos.filter(|_| self.pointer.hovered.as_deref() == Some(path))
            .and_then(|(x, y)| self.link_at(i, x, y));
        let pressed = self.pointer.press_held().filter(|h| h.path == *path && h.inside() && !h.moved).and_then(|h| h.link).filter(|&k| hovered == Some(k));
        let focused = self.edit.link_focus().filter(|_| self.kbd.focus.keyboard() && self.kbd.focus.path() == Some(path));
        (hovered, pressed, focused)
    }

    /// The paints node `i`'s links show this frame.
    pub(in crate::runtime) fn link_fx(&self, i: usize) -> LinkFx {
        let Some(spec) = self.tree.nodes[i].text.as_ref().filter(|t| !t.cold.links.is_empty()) else { return LinkFx(Vec::new()) };
        let (hovered, pressed, focused) = self.link_states(i);
        let mut fx = Vec::new();
        for (k, l) in spec.cold.links.iter().enumerate() {
            let paint = if pressed == Some(k) {
                l.pressed.unwrap_or(l.hover)
            } else if hovered == Some(k) {
                l.hover
            } else if focused == Some(k) {
                let p = l.focused.unwrap_or(l.hover);
                if p.is_empty() {
                    let ink = spec.spans.iter().filter(|sp| sp.start <= l.start && sp.end > l.start).find_map(|sp| sp.color);
                    LinkPaint::new().underline(ink.unwrap_or(spec.color))
                } else { p }
            } else { continue };
            let (a, b) = drawn(&self.tree.nodes[i], l.start, l.end);
            let pad = spec.spans.iter().find(|sp| sp.start < l.end && sp.end > l.start && sp.bg.is_some()).map_or(2.0, |sp| sp.bg_pad);
            fx.push((a, b, paint, pad, !spec.cold.rings.is_empty()));
        }
        LinkFx(fx)
    }

    /// Pointer up on text `path` of a press with `button` that began on its link `k`: ending on
    /// the same link activates it.
    pub(in crate::runtime) fn link_up(&mut self, path: &str, k: usize, x: f32, y: f32, button: crate::PointerButton) {
        let Some(i) = self.tree.nodes.iter().position(|n| n.path == path) else { return };
        if self.link_at(i, x, y) == Some(k) {
            self.frame.queue.push(Action::Link(path.to_owned(), k, button));
        }
    }

    /// Drawn line box `[left, top, width, height]` in window points for link `k` of node `i`:
    /// when `at` is `Some((x, y))`, the line box containing `(x, y)` (falling back to the first
    /// drawn line box); when `at` is `None`, the first drawn line box. Returns `None` when node
    /// `i` was not drawn this frame or the link has no drawn line box.
    pub(in crate::runtime) fn link_drawn_rect(&self, i: usize, k: usize, at: Option<(f32, f32)>) -> Option<[f32; 4]> {
        if self.paint.drawn.len() != self.tree.nodes.len() { return None; }
        let d = self.paint.drawn.get(i).copied().filter(|d| d[2] > 0.0 || d[3] > 0.0)?;
        let scroll_x = self.tree.nodes[i].input.as_deref().filter(|inp| inp.single_line).map_or(0.0, |_| {
            let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
            self.edit.input_scroll.get(&self.tree.nodes[i].path).copied().unwrap_or(0.0) * z
        });
        let (rect, hit_at) = match at {
            Some((x, y)) if !self.paint.turn_scratch.is_empty() && self.turn_of(i).is_some() => {
                let (b, ux, uy) = self.unturned_box_and_point(i, x, y);
                (b, Some((ux, uy)))
            }
            Some((x, y)) => ([d[0], d[1], d[0] + d[2], d[1] + d[3]], Some((x, y))),
            None => ([d[0], d[1], d[0] + d[2], d[1] + d[3]], None),
        };
        let mut boxes = self.link_boxes_for(i, k, rect).into_iter().map(|(_, r)| [r[0] - scroll_x, r[1], r[2] - scroll_x, r[3]]);
        let first = boxes.next()?;
        let r = match hit_at {
            Some((ux, uy)) if contains(first, ux, uy) => first,
            Some((ux, uy)) => boxes.find(|&b| contains(b, ux, uy)).unwrap_or(first),
            None => first,
        };
        let r = if !self.paint.turn_scratch.is_empty() && let Some(t) = self.turn_of(i) {
            let s = self.env.scale;
            t.bbox(r.map(|v| v * s)).map(|v| v / s)
        } else {
            r
        };
        Some([r[0], r[1], (r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0)])
    }

    /// First drawn line box `[left, top, width, height]` in window points for link `k` of node `i`,
    /// or `None` when node `i` was not drawn this frame or the link has no drawn line box.
    pub(in crate::runtime) fn link_first_rect(&self, i: usize, k: usize) -> Option<[f32; 4]> {
        self.link_drawn_rect(i, k, None)
    }

    /// Runs the `on_link` handler of node `path` for link `k`.
    pub(in crate::runtime) fn press_link(&mut self, app: &mut dyn AppDriver, path: &str, k: usize, tx: &mut Tx) -> bool {
        let Some((i, f, id)) = self.node_index(path).and_then(|i| {
            let t = self.tree.nodes[i].text.as_ref()?;
            Some((i, t.cold.on_link.clone()?, t.cold.links.get(k)?.id.clone()))
        }) else { return false };
        let prev = std::mem::replace(&mut tx.link_rect, self.link_first_rect(i, k));
        log_event!(self, "link {path} {id}");
        (f.0)(app.states(), &id, tx);
        tx.link_rect = prev;
        true
    }

    /// Node `i`'s link under (x, y), if its text takes secondary presses on links.
    pub(in crate::runtime) fn secondary_link_at(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        self.tree.nodes[i].text.as_ref()?.cold.on_link_secondary.as_ref()?;
        self.link_at(i, x, y)
    }

    /// Runs the `on_link_secondary` handler of node `path` for link `k`.
    pub(in crate::runtime) fn secondary_press_link(&mut self, app: &mut dyn AppDriver, path: &str, k: usize, point: PointerEvent, tx: &mut Tx) -> bool {
        if self.node_index(path).is_some_and(|i| self.node_disabled(i)) { return false; }
        let Some((f, id)) = self.node(path).and_then(|n| {
            let t = n.text.as_ref()?;
            Some((t.cold.on_link_secondary.clone()?, t.cold.links.get(k)?.id.clone()))
        }) else { return false };
        log_event!(self, "link secondary {path} {id}");
        (f.0)(app.states(), &id, point.window(), tx);
        true
    }

    /// Runs the `on_link_hover` handler of node `path` with `(url, rect)`.
    /// A leave (`url.is_none()`) still reaches a node that unmounted, is exiting, or dropped its
    /// handler, via the saved handler from when it heard the enter.
    pub(in crate::runtime) fn dispatch_link_hover(
        &mut self,
        app: &mut dyn AppDriver,
        path: &str,
        url: Option<&str>,
        rect: Option<[f32; 4]>,
        tx: &mut Tx,
    ) -> bool {
        let gone = if url.is_none() { self.pointer.take_link_hover_gone(path) } else { None };
        let live = self.node_index(path).and_then(|i| {
            if url.is_some() && (self.node_disabled(i) || !self.allowed(i)) { return None; }
            self.tree.nodes[i].text.as_deref()?.cold.on_link_hover.clone()
        });
        let exiting = || {
            url.is_none().then(|| {
                self.tree.nodes.iter()
                    .find(|n| n.path == path)
                    .and_then(|n| n.text.as_deref()?.cold.on_link_hover.clone())
            }).flatten()
        };
        let Some(f) = live.or(gone).or_else(exiting) else { return false };
        (f.0)(app.states(), url, rect, tx);
        true
    }

    /// Centre of the first drawn line box of the link whose id is `id`, in points.
    pub(in crate::runtime) fn find_link(&self, id: &str) -> Option<(f32, f32)> {
        (0..self.tree.nodes.len()).filter(|&i| !self.subtree_exiting(i)).find_map(|i| {
            let k = self.tree.nodes[i].text.as_ref()?.cold.links.iter().position(|l| l.id == id)?;
            let (_, r) = self.link_boxes_for(i, k, self.drawn_box(i)).into_iter().next()?;
            Some(((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5))
        })
    }

    /// The focused text's focused link, if any (it may not be in range for that text).
    #[inline]
    pub(crate) fn link_focus(&self) -> Option<usize> { self.edit.link_focus() }

    /// Focuses link `k` of the text about to take focus, as Tab into it does.
    #[inline]
    pub(crate) fn set_link_focus(&mut self, k: usize) { self.edit.set_link_focus(k); }

    /// The Menu key on focused node `i`'s focused link: its `on_link_secondary`, at the
    /// bottom-left of its first line box.
    pub(in crate::runtime) fn focused_link_menu(&mut self, app: &mut dyn AppDriver, i: usize, tx: &mut Tx) -> bool {
        if self.node_disabled(i) { return false; }
        let Some(k) = self.edit.link_focus().filter(|&k| k < link_count(&self.tree.nodes[i])) else { return false };
        if self.tree.nodes[i].text.as_ref().is_none_or(|t| t.cold.on_link_secondary.is_none()) { return false; }
        let Some((_, r)) = self.link_boxes_for(i, k, self.drawn_box(i)).into_iter().next() else { return false };
        let path = self.tree.nodes[i].path.clone();
        let point = self.pointer_event(i, r[0], r[3]);
        self.secondary_press_link(app, &path, k, point, tx)
    }

    /// Enter or Space on a focused link presses it.
    pub(in crate::runtime) fn link_key(&mut self, app: &mut dyn AppDriver, i: usize, tx: &mut Tx) -> bool {
        let Some(k) = self.edit.link_focus().filter(|&k| k < link_count(&self.tree.nodes[i])) else { return false };
        if self.node_disabled(i) { return true; }
        let path = self.tree.nodes[i].path.clone();
        self.press_link(app, &path, k, tx);
        true
    }

    /// Reports each link as its own element after its text, which hands it its focus.
    pub(in crate::runtime) fn link_elements(&self, elements: &mut Vec<ElementInfo>) {
        for i in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let Some(spec) = n.text.as_ref().filter(|t| !t.cold.links.is_empty()) else { continue };
            let Some(at) = elements.iter().position(|e| e.path == n.path) else { continue };
            let text = elements[at].clone();
            elements[at].focusable = false;
            elements[at].focused = false;
            elements[at].focus_visible = false;
            let (hovered, pressed, _) = self.link_states(i);
            let focused = self.edit.link_focus().filter(|_| self.kbd.focus.path() == Some(&n.path));
            let boxes = self.link_boxes(i, self.drawn_box(i));
            let targets = self.link_boxes_in(&self.tree.nodes, i, n.target, 1.0);
            let union = |k: usize, bs: &[(usize, [f32; 4])]| bs.iter().filter(|(j, _)| *j == k).fold(None, |u: Option<[f32; 4]>, (_, r)| {
                Some(u.map_or(*r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]))
            });
            for (k, l) in spec.cold.links.iter().enumerate() {
                let Some(r) = union(k, &boxes) else { continue };
                let t = union(k, &targets).unwrap_or(r);
                let v = text.visible;
                let (x0, y0) = (r[0].max(v[0]), r[1].max(v[1]));
                let (x1, y1) = (r[2].min(v[0] + v[2]), r[3].min(v[1] + v[3]));
                let words = (!spec.redacted).then(|| spec.source_slice(l.start, l.end).map(str::to_owned)).flatten();
                elements.push(ElementInfo {
                    stuck: 0.0,
                    material_fallback: false,
                    border_color: None,
                    description: None,
                    inset: [0.0; 4],
                    text_size: None,
                    wrap: false,
                    truncate: false,
                    text_cut: false,
                    clips: false,
                    scroll_axes: [false, false],
                    raised: false,
                    path: format!("{}#link-{k}", n.path),
                    key: Some(l.id.clone()),
                    role: l.role,
                    label: l.label.clone().or_else(|| words.clone()),
                    text: words,
                    rect: [r[0], r[1], r[2] - r[0], r[3] - r[1]],
                    target: [t[0], t[1], t[2] - t[0], t[3] - t[1]],
                    visible: [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)],
                    focusable: text.focusable,
                    focused: focused == Some(k),
                    focus_visible: self.kbd.focus.keyboard() && focused == Some(k),
                    hover: if hovered == Some(k) { 1.0 } else { 0.0 },
                    press: if pressed == Some(k) { 1.0 } else { 0.0 },
                    parent: Some(i),
                    caret: None,
                    selected: None,
                    level: None,
                    expanded: None,
                    item_selected: false,
                    help: l.help.clone(),
                    ..text.clone()
                });
            }
        }
    }

    /// Node `i`'s highlight line boxes in window points (x0, y0, x1, y1) for border box `rect`.
    pub(in crate::runtime) fn highlight_boxes(&self, i: usize, rect: [f32; 4]) -> Vec<(usize, [f32; 4])> {
        self.highlight_boxes_in(&self.tree.nodes, i, rect, self.paint.drawn_scale.get(i).copied().unwrap_or(1.0))
    }

    pub(in crate::runtime) fn highlight_boxes_in(&self, nodes: &[Node], i: usize, rect: [f32; 4], z: f32) -> Vec<(usize, [f32; 4])> {
        self.highlight_boxes_where(nodes, i, rect, z, |_, _| true)
    }

    /// Like [`Self::highlight_boxes_in`], laying out only highlights `wanted` selects.
    fn highlight_boxes_where(
        &self,
        nodes: &[Node],
        i: usize,
        rect: [f32; 4],
        z: f32,
        mut wanted: impl FnMut(usize, &crate::text::TextHighlight) -> bool,
    ) -> Vec<(usize, [f32; 4])> {
        let n = &nodes[i];
        let Some(spec) = n.text.as_deref() else { return Vec::new() };
        if spec.cold.highlights.is_empty() || n.is_secure() { return Vec::new(); }
        self.span_boxes_indexed(
            n,
            rect,
            z,
            spec.cold.highlights.iter().enumerate().filter_map(|(k, h)| wanted(k, h).then_some((k, h.start, h.end))),
        )
    }

    /// Unscrolled inline font-metric target rect `[x0, y0, x1, y1]` (in window points) and first
    /// node index for the highlight keyed `key` (or scoped `path/key`), matching Blink's
    /// `Range::getBoundingClientRect()`.
    pub(in crate::runtime) fn highlight_inline_target_in(&self, nodes: &[Node], scope: Option<&str>, key: &str) -> Option<(usize, [f32; 4])> {
        let index = self.key_index_for(nodes);
        let mut first_node = None;
        let mut union: Option<[f32; 4]> = None;
        let mut visit = |i: usize| {
            let n = &nodes[i];
            let Some(spec) = n.text.as_ref().filter(|t| !t.cold.highlights.is_empty() && !n.is_secure()) else { return };
            if !n.parent.is_none_or(|p| super::in_scope(&nodes[p].path, scope)) {
                return;
            }
            for (_, r) in self.span_inline_boxes(
                n,
                n.target,
                1.0,
                spec.cold.highlights.iter()
                    .filter(|h| h.key.as_deref().is_some_and(|hkey| crate::flight::selects_under(key, &n.path, hkey)))
                    .map(|h| (h.start, h.end)),
            ) {
                first_node.get_or_insert(i);
                union = Some(union.map_or(r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]));
            }
        };
        match index {
            Some(index) => index.marked(key).into_iter().for_each(&mut visit),
            None => (0..nodes.len()).for_each(&mut visit),
        }
        Some((first_node?, union?))
    }

    /// Unscrolled target rect `[x0, y0, x1, y1]` (in window points) and first node index for
    /// the highlight keyed `key` (or scoped `path/key`).
    pub(in crate::runtime) fn highlight_target_in(&self, nodes: &[Node], scope: Option<&str>, key: &str) -> Option<(usize, [f32; 4])> {
        self.highlight_target_with(nodes, self.key_index_for(nodes), scope, key)
    }

    /// [`Self::highlight_target_in`], looking only at the nodes `index` lists for `key`.
    pub(in crate::runtime) fn highlight_target_with(&self, nodes: &[Node], index: Option<&super::key_index::KeyIndex>, scope: Option<&str>, key: &str) -> Option<(usize, [f32; 4])> {
        let mut first_node = None;
        let mut union: Option<[f32; 4]> = None;
        let mut visit = |i: usize| {
            let n = &nodes[i];
            if !n.text.as_ref().is_some_and(|t| !t.cold.highlights.is_empty() && !n.is_secure()) { return; }
            if !n.parent.is_none_or(|p| super::in_scope(&nodes[p].path, scope)) {
                return;
            }
            for (_, r) in self.highlight_boxes_where(nodes, i, n.target, 1.0, |_, h| {
                h.key.as_deref().is_some_and(|hkey| crate::flight::selects_under(key, &n.path, hkey))
            }) {
                first_node.get_or_insert(i);
                union = Some(union.map_or(r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]));
            }
        };
        match index {
            Some(index) => index.marked(key).into_iter().for_each(&mut visit),
            None => (0..nodes.len()).for_each(&mut visit),
        }
        Some((first_node?, union?))
    }

    /// Centre of the first drawn line box of the highlight keyed `key`, in points.
    pub(in crate::runtime) fn find_highlight(&self, key: &str) -> Option<(f32, f32)> {
        (0..self.tree.nodes.len()).filter(|&i| !self.subtree_exiting(i)).find_map(|i| {
            let n = &self.tree.nodes[i];
            let k = n.text.as_ref()?.cold.highlights.iter().position(|h| {
                h.key.as_deref().is_some_and(|hk| crate::flight::selects_under(key, &n.path, hk))
            })?;
            let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
            let (_, r) = self.highlight_boxes_where(&self.tree.nodes, i, self.drawn_box(i), z, |j, _| j == k).into_iter().next()?;
            Some(((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5))
        })
    }

    /// Reports each text highlight as its own `"mark"` element after its text.
    pub(in crate::runtime) fn highlight_elements(&self, elements: &mut Vec<ElementInfo>) {
        if !self.tree.nodes.iter().any(|n| n.text.as_ref().is_some_and(|t| !t.cold.highlights.is_empty())) { return; }
        let text_elements: std::collections::HashMap<_, _> = elements.iter().enumerate().map(|(i, e)| (e.path.clone(), i)).collect();
        for i in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let Some(spec) = n.text.as_ref().filter(|t| !t.cold.highlights.is_empty()) else { continue };
            let Some(&at) = text_elements.get(&n.path) else { continue };
            let text = elements[at].clone();
            let boxes = self.highlight_boxes(i, self.drawn_box(i));
            let targets = self.highlight_boxes_in(&self.tree.nodes, i, n.target, 1.0);
            let unions = |bs: &[(usize, [f32; 4])]| {
                let mut out = vec![None; spec.cold.highlights.len()];
                for &(k, r) in bs {
                    out[k] = Some(out[k].map_or(r, |u: [f32; 4]| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]));
                }
                out
            };
            let (boxes, targets) = (unions(&boxes), unions(&targets));
            for (k, h) in spec.cold.highlights.iter().enumerate() {
                let Some(r) = boxes[k] else { continue };
                let t = targets[k].unwrap_or(r);
                let v = text.visible;
                let (x0, y0) = (r[0].max(v[0]), r[1].max(v[1]));
                let (x1, y1) = (r[2].min(v[0] + v[2]), r[3].min(v[1] + v[3]));
                let words = spec.source_slice(h.start, h.end).map(str::to_owned);
                elements.push(ElementInfo {
                    stuck: 0.0,
                    material_fallback: false,
                    border_color: None,
                    description: None,
                    inset: [0.0; 4],
                    text_size: None,
                    wrap: false,
                    truncate: false,
                    text_cut: false,
                    clips: false,
                    scroll_axes: [false, false],
                    raised: false,
                    path: format!("{}#mark-{k}", n.path),
                    key: h.key.clone(),
                    role: "mark",
                    label: words.clone(),
                    text: words,
                    rect: [r[0], r[1], r[2] - r[0], r[3] - r[1]],
                    target: [t[0], t[1], t[2] - t[0], t[3] - t[1]],
                    visible: [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)],
                    fill: h.bg,
                    text_color: h.fg.or(text.text_color),
                    radius: 4.0,
                    focusable: false,
                    focused: false,
                    focus_visible: false,
                    hover: 0.0,
                    press: 0.0,
                    parent: Some(i),
                    caret: None,
                    selected: None,
                    level: None,
                    expanded: None,
                    item_selected: false,
                    ..text.clone()
                });
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static SPAN_BOX_QUERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn count_span_box_queries<R>(f: impl FnOnce() -> R) -> (R, usize) {
    SPAN_BOX_QUERIES.with(|c| c.set(0));
    let out = f();
    let count = SPAN_BOX_QUERIES.with(|c| c.get());
    (out, count)
}

#[cfg(test)]
#[path = "links_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pill_hit_tests.rs"]
mod pill_hit_tests;

#[cfg(test)]
#[path = "link_rect_tests.rs"]
mod link_rect_tests;

#[cfg(test)]
#[path = "link_hover_tests.rs"]
mod link_hover_tests;
