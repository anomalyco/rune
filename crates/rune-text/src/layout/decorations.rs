//! Rects drawn under or over text: span backgrounds, underlines, strikethroughs, highlights
//! and link rings.

use crate::*;
use rune_core::HashMap as FxHashMap;

/// The corner radius in points of a span's rounded background unless its text element sets
/// another ([`El::span_radius`](crate::El::span_radius)).
pub const SPAN_RADIUS: f32 = 4.0;

/// A rect drawn under or over text: span backgrounds and underlines (device px).
#[derive(Clone, Copy, Debug)]
pub struct Deco {
    pub rect: [f32; 4],
    pub color: Color,
    pub radius: f32,
}

/// Internal paint metadata: keep the public `Deco` literal unchanged.
#[doc(hidden)]
pub struct PaintedDeco {
    pub deco: Deco,
    pub ring: Option<(Color, f32)>,
    pub byte: u32,
}

impl std::ops::Deref for PaintedDeco {
    type Target = Deco;
    fn deref(&self) -> &Deco { &self.deco }
}
impl std::ops::DerefMut for PaintedDeco {
    fn deref_mut(&mut self) -> &mut Deco { &mut self.deco }
}

pub(crate) trait Decoration {
    fn new(deco: Deco, ring: Option<(Color, f32)>, byte: u32) -> Self;
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>);
    fn extend_to(&mut self, _byte: u32) {}
}
impl Decoration for Deco {
    fn new(deco: Deco, _: Option<(Color, f32)>, _: u32) -> Self { deco }
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>) { (self, None) }
}
impl Decoration for PaintedDeco {
    fn new(deco: Deco, ring: Option<(Color, f32)>, byte: u32) -> Self { Self { deco, ring, byte } }
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>) { (&mut self.deco, self.ring) }
    fn extend_to(&mut self, byte: u32) { self.byte = self.byte.max(byte); }
}

impl TextSystem {
    /// Backgrounds and underlines for `spans` (device px, layout top-left at `origin`). Touching
    /// runs with the same background merge into one rect rounded by [`SPAN_RADIUS`].
    pub fn decorations(&self, layout: &Layout<Ink>, spans: &[Span], scale: f32, origin: [f32; 2], out: &mut Vec<Deco>) {
        self.decorations_into(layout, spans, &[], SPAN_RADIUS, scale, origin, out);
    }

    /// [`Self::decorations`] with backgrounds rounded by `radius` points, plus inset border
    /// `rings` (`(start_byte, end_byte, color, width_pt)`).
    #[allow(clippy::too_many_arguments)]
    #[doc(hidden)]
    pub fn decorations_with_rings(
        &self,
        layout: &Layout<Ink>,
        spans: &[Span],
        rings: &[(u32, u32, Color, f32)],
        radius: f32,
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        self.decorations_into(layout, spans, rings, radius, scale, origin, out);
    }

    #[allow(clippy::too_many_arguments)]
    fn decorations_into<D: Decoration>(
        &self, layout: &Layout<Ink>, spans: &[Span], rings: &[(u32, u32, Color, f32)],
        radius: f32, scale: f32, origin: [f32; 2], out: &mut Vec<D>,
    ) {
        if spans.iter().all(|s| s.bg.is_none() && s.underline.is_none()) {
            return;
        }
        let layout_spans: Vec<&Span> = super::span::layout_spans(spans).collect();
        let mut ring_lookup = RingLookup::new(rings);
        let radius = radius * scale;
        let mut last_bg = None;
        for line in layout.lines() {
            let m = line.metrics();
            let mut line_end = None;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
                let ink = gr.style().brush.0 & !crate::raster::synthetic_bold::SYNTHETIC_BOLD;
                if ink == 0 {
                    continue;
                }
                let Some(&s) = layout_spans.get(ink as usize - 1) else { continue };
                let x0 = origin[0] + gr.offset();
                let w = gr.advance();
                if let Some(bg) = s.bg {
                    let ring = ring_lookup.get(ink, s.start, s.end, scale);
                    let pad = s.bg_pad * scale;
                    // Inline code's box is its font's content area, as a browser paints an inline
                    // element's background: ascent over the baseline and descent under it, each
                    // rounded to device px. Other spans balance on the cap height. Whole device pixels.
                    let fm = gr.run().font_metrics();
                    let cap = fm.cap_height.unwrap_or(gr.run().font_size() * 0.7);
                    let base = origin[1] + m.baseline;
                    let (top, bot) = if s.mono {
                        (base.round() - fm.ascent.round(), base.round() + fm.descent.round())
                    } else {
                        ((base - cap * 1.35).round(), (base + cap * 0.55).round())
                    };
                    let (l, r) = ((x0 - pad).round(), (x0 + w + pad).round());
                    let rect = [l, top, r - l, bot - top];
                    // Touching runs with the same background and ring on one line (a span split by
                    // style, or inline code and its padding spaces in the body font) share one rect,
                    // as tall as its widest run, so a thin run in another font doesn't poke out as a sliver.
                    let byte = (gr.run().text_range().end as u32).min(s.end).saturating_sub(1);
                    let merge = last_bg.is_some_and(|i: usize| {
                        let (d, old_ring) = out[i].paint();
                        d.color == bg && old_ring == ring
                            && (d.radius - radius).abs() < 1e-3
                            && d.rect[1] < bot
                            && top < d.rect[1] + d.rect[3]
                            && (d.rect[0] + d.rect[2] - rect[0] - 2.0 * pad).abs() < 1.5
                    });
                    if merge {
                        let painted = &mut out[last_bg.unwrap()];
                        let (d, _) = painted.paint();
                        if rect[2] > d.rect[2] {
                            (d.rect[1], d.rect[3]) = (rect[1], rect[3]);
                        }
                        d.rect[2] = rect[0] + rect[2] - d.rect[0];
                        painted.extend_to(byte);
                    } else {
                        last_bg = Some(out.len());
                        out.push(D::new(Deco { rect, color: bg, radius }, ring, byte));
                    }
                }
                if let Some(uc) = s.underline.filter(|c| c.a > 0.0) {
                    let y = (origin[1] + m.baseline + 2.0 * scale).round();
                    let end = *line_end.get_or_insert_with(|| super::underline::line_end(&line));
                    let (l, r) = super::underline::underline_edges_with_end(end, origin[0], gr.offset(), gr.offset() + w);
                    out.push(D::new(Deco { rect: [l, y, r - l, scale.max(1.0)], color: uc, radius: 0.0 }, None, 0));
                }
            }
        }
    }

    /// Strikethrough lines for `strikes` (`(start_byte, end_byte, color)`) in device px.
    #[doc(hidden)]
    pub fn strike_decorations(
        &self,
        layout: &Layout<Ink>,
        spans: &[Span],
        strikes: &[(u32, u32, Option<Color>)],
        default_color: Color,
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        if strikes.is_empty() {
            return;
        }
        let mut span_index: Option<StrikeSpanIndex> = None;
        let mut line_cache: FxHashMap<usize, StrikeLine> = FxHashMap::default();
        let multi_strike = strikes.len() > 1;
        for &(s_start, s_end, sc) in strikes {
            let (a, b) = (s_start as usize, s_end as usize);
            if a >= b {
                continue;
            }
            let span_color = sc.or_else(|| {
                if multi_strike && spans.len() > 1 {
                    span_index.get_or_insert_with(|| StrikeSpanIndex::new(spans)).color_for(a, b)
                } else {
                    spans
                        .iter()
                        .find(|sp| {
                            count_visit(Visit::StrikeSpan);
                            (sp.start as usize) <= a && b <= (sp.end as usize) && sp.color.is_some()
                        })
                        .and_then(|sp| sp.color)
                }
            });
            let color = span_color.unwrap_or(default_color);
            let boxes = range_boxes(layout, a, b, false);
            let use_cache = multi_strike || boxes.len() > 1;
            for (bb, line_idx) in boxes {
                let w = (bb.x1 - bb.x0) as f32;
                if w <= 0.0 {
                    continue;
                }
                let Some(line) = layout.get(line_idx) else { continue };
                let m = line.metrics();
                let (bx0, bx1) = (bb.x0 as f32, bb.x1 as f32);
                let best = if use_cache {
                    line_cache
                        .entry(line_idx)
                        .or_insert_with(|| StrikeLine::new(&line, scale))
                        .best(a, b, bx0, bx1)
                } else {
                    scan_strike_line(&line, a, b, bx0, bx1, scale)
                };
                let (strike_off, h) = best.unwrap_or((m.line_height * 0.25, scale.round().max(1.0)));
                let x0 = origin[0] + bb.x0 as f32;
                let y = (origin[1] + m.baseline - strike_off).round();
                out.push(PaintedDeco::new(Deco { rect: [x0, y, w, h], color, radius: 0.0 }, None, 0));
            }
        }
    }

    /// Rounded background fills and rings for `highlights` in device px.
    #[allow(dead_code)]
    pub(crate) fn highlight_decorations(
        &self,
        layout: &Layout<Ink>,
        highlights: &[TextHighlight],
        len: usize,
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        self.highlight_decorations_flow(TextFlow::Single(layout), highlights, len, scale, origin, f32::NEG_INFINITY..f32::INFINITY, out);
    }

    /// [`Self::highlight_decorations`] over a [`TextFlow`] in visible device rows `rows`,
    /// preserving the trailing newline highlight box where a multi-line highlight crosses `\n`.
    #[allow(clippy::too_many_arguments)]
    #[doc(hidden)]
    pub fn highlight_decorations_flow(
        &self,
        flow: TextFlow<'_>,
        highlights: &[TextHighlight],
        len: usize,
        scale: f32,
        origin: [f32; 2],
        rows: std::ops::Range<f32>,
        out: &mut Vec<PaintedDeco>,
    ) {
        if highlights.is_empty() {
            return;
        }
        let mut cache = (highlights.len() > 1).then(HighlightLineCache::default);
        for h in highlights {
            let (a, b) = ((h.start as usize).min(len), (h.end as usize).min(len));
            if a >= b || (h.bg.a <= 0.0 && h.ring.is_none()) {
                continue;
            }
            let pad = h.pad * scale;
            let radius = h.radius * scale;
            let vpad = h.vpad * scale;
            let ring = h.ring.map(|(c, rw)| (c, rw * scale));
            let byte = (b as u32).saturating_sub(1);
            match flow {
                TextFlow::Single(l) => {
                    let boxes = range_boxes(l, a, b, false);
                    push_highlight_boxes(l, 0, cache.as_mut(), boxes, a..b, origin, pad, vpad, radius, h.bg, ring, byte, None, out);
                }
                TextFlow::Paragraphs(pl) => {
                    let from = pl.paragraphs.partition_point(|q| q.end < a);
                    for (k, q) in pl.paragraphs.iter().enumerate().skip(from).take_while(|(_, q)| q.start <= b) {
                        if q.y + q.height < rows.start || q.y > rows.end {
                            continue;
                        }
                        let p = Piece::of_paragraph(k, q, pl.disjoint_spans);
                        let clipped = p.clip(a, b);
                        let mut boxes = clipped.map_or_else(Vec::new, |(sa, sb)| range_boxes(p.layout, sa, sb, false));
                        if k + 1 < pl.paragraphs.len() && a <= p.end && p.end < b {
                            if let Some((li, line)) = p.layout.lines().enumerate().last().filter(|_| !p.is_blank()) {
                                let m = line.metrics();
                                let (top, bot) = (m.block_min_coord.min(m.content_block_min_coord) as f64, m.block_max_coord.max(m.content_block_max_coord) as f64);
                                let ws = line.runs().last().map_or(pl.newline as f64, |r| {
                                    let fm = r.font_metrics();
                                    (fm.ascent as f64 + fm.descent as f64) * 0.25
                                });
                                let rtl = p.flow == crate::Flow::Rtl;
                                let on_line = boxes.iter_mut().filter(|(_, idx)| *idx == li);
                                let edge = if rtl { on_line.min_by(|l, r| l.0.x0.total_cmp(&r.0.x0)) } else { on_line.max_by(|l, r| l.0.x1.total_cmp(&r.0.x1)) };
                                let x = (m.inline_min_coord + m.offset) as f64;
                                match (edge, rtl) {
                                    (Some((bb, _)), false) => bb.x1 += ws,
                                    (Some((bb, _)), true) => bb.x0 -= ws,
                                    (None, false) => boxes.push((parley::BoundingBox::new(x + m.advance as f64, top, x + m.advance as f64 + ws, bot), li)),
                                    (None, true) => boxes.push((parley::BoundingBox::new(x - ws, top, x, bot), li)),
                                }
                            } else {
                                let x = flow.blank_start(&p, pl.newline) as f64;
                                boxes.push((parley::BoundingBox::new(x, 0.0, x + pl.newline as f64, p.height as f64), 0));
                            }
                        }
                        let (sa, sb) = clipped.unwrap_or((p.shaped(p.end), p.shaped(p.end)));
                        push_highlight_boxes(p.layout, k, cache.as_mut(), boxes, sa..sb, [origin[0], origin[1] + p.y], pad, vpad, radius, h.bg, ring, byte, Some(pl.blank_box), out);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_highlight_boxes(
    layout: &Layout<Ink>,
    para_idx: usize,
    mut cache: Option<&mut HighlightLineCache>,
    boxes: Vec<(parley::BoundingBox, usize)>,
    range: std::ops::Range<usize>,
    origin: [f32; 2],
    pad: f32,
    vpad: f32,
    radius: f32,
    color: Color,
    ring: Option<(Color, f32)>,
    byte: u32,
    blank: Option<[f32; 3]>,
    out: &mut Vec<PaintedDeco>,
) {
    let mut last: Option<(usize, usize)> = None;
    let mut last_line_metrics: Option<(usize, (f32, f32))> = None;
    for (bb, line_idx) in boxes {
        let w = (bb.x1 - bb.x0) as f32;
        if w <= 0.0 {
            continue;
        }
        let x0 = origin[0] + bb.x0 as f32;
        let (mut top, mut bot) = ((origin[1] + bb.y0 as f32).round(), (origin[1] + bb.y1 as f32).round());
        if let Some(l) = layout.get(line_idx) {
            let base = origin[1] + l.metrics().baseline;
            let (asc, desc) = if let Some((_, m)) = last_line_metrics.filter(|&(li, _)| li == line_idx) {
                m
            } else {
                let m = match cache.as_deref_mut() {
                    Some(c) => c.metrics(&l, para_idx, line_idx, range.clone()),
                    None => scan_highlight_line(&l, range.clone()),
                };
                last_line_metrics = Some((line_idx, m));
                m
            };
            if asc > 0.0 || desc > 0.0 {
                top = (base - asc - vpad).round();
                bot = (base + desc + vpad).round();
            } else if let Some(b) = blank {
                let bbase = origin[1] + b[0];
                top = (bbase - b[1] - vpad).round();
                bot = (bbase + b[2] + vpad).round();
            }
        } else if let Some(b) = blank {
            let bbase = origin[1] + b[0];
            top = (bbase - b[1] - vpad).round();
            bot = (bbase + b[2] + vpad).round();
        }
        let rect = [x0 - pad, top, w + 2.0 * pad, (bot - top).max(0.0)];
        let merge = last.filter(|&(li, _)| li == line_idx).is_some_and(|(_, idx)| {
            let (d, _) = out[idx].paint();
            (d.rect[0] + d.rect[2] - rect[0] - 2.0 * pad).abs() < 1.0
        });
        if merge {
            let (d, _) = out[last.unwrap().1].paint();
            if rect[3] > d.rect[3] {
                (d.rect[1], d.rect[3]) = (rect[1], rect[3]);
            }
            d.rect[2] = rect[0] + rect[2] - d.rect[0];
        } else {
            last = Some((line_idx, out.len()));
            out.push(PaintedDeco::new(Deco { rect, color, radius }, ring, byte));
        }
    }
}

/// First index in `slice` where `before(&slice[i])` is false, advancing `cursor` monotonically
/// when queries move forward and falling back to binary search otherwise.
fn seek_cursor<T>(slice: &[T], cursor: &mut usize, kind: Visit, before: impl Fn(&T) -> bool) -> usize {
    let mut i = *cursor;
    if i == 0 || before(&slice[i - 1]) {
        while i < slice.len() && before(&slice[i]) {
            count_visit(kind);
            i += 1;
        }
    } else {
        i = slice.partition_point(|e| {
            count_visit(kind);
            before(e)
        });
    }
    *cursor = i;
    i
}

/// Flattened, cursor-walked lookup for the last ring overlapping a span's byte range.
enum RingLookup<'a> {
    Empty,
    Single((u32, u32, Color, f32)),
    Indexed {
        rings: &'a [(u32, u32, Color, f32)],
        entries: Vec<(u32, u32, u32)>,
        degenerate: Vec<u32>,
        cursor: usize,
        by_ink: FxHashMap<u32, Option<(Color, f32)>>,
    },
}

impl<'a> RingLookup<'a> {
    fn new(rings: &'a [(u32, u32, Color, f32)]) -> Self {
        if rings.is_empty() { return Self::Empty; }
        if rings.len() == 1 { return Self::Single(rings[0]); }
        let mut raw = Vec::with_capacity(rings.len());
        let mut degenerate = Vec::new();
        let (mut disjoint, mut last_end) = (true, 0u32);
        for (i, &(ra, rb, _, rw)) in rings.iter().enumerate() {
            count_visit(Visit::DecoRing);
            if rw <= 0.0 { continue; }
            if ra >= rb {
                degenerate.push(i as u32);
                continue;
            }
            if ra < last_end { disjoint = false; }
            last_end = last_end.max(rb);
            raw.push((ra, rb, i as u32));
        }
        if raw.is_empty() && degenerate.is_empty() { return Self::Empty; }
        let entries = if disjoint || raw.len() <= 1 {
            raw
        } else {
            let mut out = Vec::new();
            #[cfg(test)]
            let _ = super::span_map::take_span_scan_steps();
            super::span_map::flatten_into(&raw, &mut out);
            #[cfg(test)]
            add_deco_ring_visits(super::span_map::take_span_scan_steps());
            out
        };
        Self::Indexed { rings, entries, degenerate, cursor: 0, by_ink: FxHashMap::default() }
    }

    fn get(&mut self, ink: u32, start: u32, end: u32, scale: f32) -> Option<(Color, f32)> {
        match self {
            Self::Empty => None,
            Self::Single((ra, rb, c, rw)) => {
                count_visit(Visit::DecoRing);
                (*rw > 0.0 && *ra < end && start < *rb).then_some((*c, *rw * scale))
            }
            Self::Indexed { rings, entries, degenerate, cursor, by_ink } => {
                if let Some(&cached) = by_ink.get(&ink) { return cached; }
                let lo = seek_cursor(entries, cursor, Visit::DecoRing, |e| e.1 <= start);
                let mut best: Option<u32> = None;
                for e in entries[lo..].iter().take_while(|e| e.0 < end) {
                    count_visit(Visit::DecoRing);
                    best = Some(best.map_or(e.2, |b| b.max(e.2)));
                }
                for &idx in degenerate.iter().rev() {
                    if best.is_some_and(|b| b >= idx) { break; }
                    count_visit(Visit::DecoRing);
                    let (ra, rb, _, _) = rings[idx as usize];
                    if ra < end && start < rb { best = Some(idx); break; }
                }
                let res = best.map(|idx| { let (_, _, c, rw) = rings[idx as usize]; (c, rw * scale) });
                by_ink.insert(ink, res);
                res
            }
        }
    }
}

fn strike_metrics(fm: &parley::FontMetrics, scale: f32) -> (f32, f32) {
    let off = if fm.strikethrough_offset != 0.0 { fm.strikethrough_offset } else { fm.ascent * 0.32 };
    (off, fm.strikethrough_size.max(scale).round().max(1.0))
}

fn scan_strike_line(line: &parley::Line<'_, Ink>, a: usize, b: usize, bx0: f32, bx1: f32, scale: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32, f32)> = None;
    for item in line.items() {
        count_visit(Visit::StrikeItem);
        let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
        let run = gr.run().text_range();
        if run.end <= a || b <= run.start { continue; }
        let overlap = (gr.offset() + gr.advance()).min(bx1) - gr.offset().max(bx0);
        if best.is_none_or(|(prev, _, _)| overlap > prev) {
            let (off, h) = strike_metrics(&gr.run().font_metrics(), scale);
            best = Some((overlap, off, h));
        }
    }
    best.map(|(_, off, h)| (off, h))
}

struct StrikeRun { order: usize, start: usize, end: usize, x0: f32, x1: f32, max_x1: f32, strike_off: f32, h: f32 }
struct StrikeLine { runs: Vec<StrikeRun>, cursor: usize }

impl StrikeLine {
    fn new(line: &parley::Line<'_, Ink>, scale: f32) -> Self {
        let mut runs = Vec::new();
        let (mut sorted, mut prev_x0) = (true, f32::NEG_INFINITY);
        for (order, item) in line.items().enumerate() {
            count_visit(Visit::StrikeItem);
            let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
            let r = gr.run().text_range();
            let (x0, x1) = (gr.offset(), gr.offset() + gr.advance());
            if x0 < prev_x0 { sorted = false; }
            prev_x0 = x0;
            let (strike_off, h) = strike_metrics(&gr.run().font_metrics(), scale);
            runs.push(StrikeRun { order, start: r.start, end: r.end, x0, x1, max_x1: x1, strike_off, h });
        }
        if !sorted {
            runs.sort_unstable_by(|a, b| a.x0.total_cmp(&b.x0).then_with(|| a.order.cmp(&b.order)));
        }
        let mut max_x1 = f32::NEG_INFINITY;
        for r in &mut runs { max_x1 = max_x1.max(r.x1); r.max_x1 = max_x1; }
        Self { runs, cursor: 0 }
    }

    fn best(&mut self, a: usize, b: usize, bx0: f32, bx1: f32) -> Option<(f32, f32)> {
        let first = seek_cursor(&self.runs, &mut self.cursor, Visit::StrikeItem, |r| r.max_x1 <= bx0);
        let mut best: Option<(f32, usize, f32, f32)> = None;
        for r in &self.runs[first..] {
            if r.x0 >= bx1 { break; }
            count_visit(Visit::StrikeItem);
            if r.end <= a || b <= r.start || r.x1 <= bx0 { continue; }
            let overlap = r.x1.min(bx1) - r.x0.max(bx0);
            if best.is_none_or(|(ov, ord, _, _)| overlap > ov || (overlap == ov && r.order < ord)) {
                best = Some((overlap, r.order, r.strike_off, r.h));
            }
        }
        if let Some((_, _, off, h)) = best { return Some((off, h)); }
        for r in &self.runs {
            count_visit(Visit::StrikeItem);
            if r.end <= a || b <= r.start { continue; }
            let overlap = r.x1.min(bx1) - r.x0.max(bx0);
            if best.is_none_or(|(ov, ord, _, _)| overlap > ov || (overlap == ov && r.order < ord)) {
                best = Some((overlap, r.order, r.strike_off, r.h));
            }
        }
        best.map(|(_, _, off, h)| (off, h))
    }
}

struct ColoredSpan { start: usize, end: usize, max_end: usize, order: usize, color: Color }
struct StrikeSpanIndex { spans: Vec<ColoredSpan>, cursor: usize }

impl StrikeSpanIndex {
    fn new(spans: &[Span]) -> Self {
        let mut colored = Vec::new();
        let (mut sorted, mut prev_start) = (true, 0usize);
        for sp in spans {
            count_visit(Visit::StrikeSpan);
            let Some(color) = sp.color else { continue };
            let (start, end) = (sp.start as usize, sp.end as usize);
            if start >= end { continue; }
            if start < prev_start { sorted = false; }
            prev_start = start;
            let order = colored.len();
            colored.push(ColoredSpan { start, end, max_end: end, order, color });
        }
        if !sorted { colored.sort_unstable_by_key(|s| (s.start, s.order)); }
        let mut max_end = 0usize;
        for s in &mut colored { max_end = max_end.max(s.end); s.max_end = max_end; }
        Self { spans: colored, cursor: 0 }
    }

    fn color_for(&mut self, a: usize, b: usize) -> Option<Color> {
        let first = seek_cursor(&self.spans, &mut self.cursor, Visit::StrikeSpan, |s| s.max_end < b);
        let mut best: Option<(usize, Color)> = None;
        for s in &self.spans[first..] {
            if s.start > a { break; }
            count_visit(Visit::StrikeSpan);
            if s.end >= b && best.is_none_or(|(ord, _)| s.order < ord) {
                best = Some((s.order, s.color));
                if s.order == 0 { break; }
            }
        }
        best.map(|(_, c)| c)
    }
}

fn scan_highlight_line(l: &parley::Line<'_, Ink>, range: std::ops::Range<usize>) -> (f32, f32) {
    let (mut asc, mut desc) = (0.0f32, 0.0f32);
    for run in l.runs() {
        count_visit(Visit::HighlightRun);
        let r = run.text_range();
        if (r.start < range.end && range.start < r.end) || (range.is_empty() && r.end == range.end) {
            let fm = run.font_metrics();
            (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
        }
    }
    (asc, desc)
}

struct HighlightRun { start: usize, end: usize, max_end: usize, asc: f32, desc: f32 }
struct HighlightLine { runs: Vec<HighlightRun>, cursor: usize }

impl HighlightLine {
    fn new(l: &parley::Line<'_, Ink>) -> Self {
        let mut runs = Vec::new();
        let (mut sorted, mut prev_start) = (true, 0usize);
        for run in l.runs() {
            count_visit(Visit::HighlightRun);
            let r = run.text_range();
            if r.start < prev_start { sorted = false; }
            prev_start = r.start;
            let fm = run.font_metrics();
            runs.push(HighlightRun { start: r.start, end: r.end, max_end: r.end, asc: fm.ascent, desc: fm.descent.abs() });
        }
        if !sorted { runs.sort_unstable_by_key(|r| r.start); }
        let mut max_end = 0usize;
        for r in &mut runs { max_end = max_end.max(r.end); r.max_end = max_end; }
        Self { runs, cursor: 0 }
    }

    fn metrics(&mut self, range: std::ops::Range<usize>) -> (f32, f32) {
        let (mut asc, mut desc) = (0.0f32, 0.0f32);
        if range.is_empty() {
            let pos = range.end;
            let first = self.runs.partition_point(|r| { count_visit(Visit::HighlightRun); r.max_end < pos });
            for r in &self.runs[first..] {
                if r.start > pos { break; }
                count_visit(Visit::HighlightRun);
                if r.end == pos { (asc, desc) = (asc.max(r.asc), desc.max(r.desc)); }
            }
            return (asc, desc);
        }
        let first = seek_cursor(&self.runs, &mut self.cursor, Visit::HighlightRun, |r| r.max_end <= range.start);
        for r in &self.runs[first..] {
            if r.start >= range.end { break; }
            count_visit(Visit::HighlightRun);
            if r.end > range.start { (asc, desc) = (asc.max(r.asc), desc.max(r.desc)); }
        }
        (asc, desc)
    }
}

#[derive(Default)]
struct HighlightLineCache {
    lines: FxHashMap<(usize, usize), HighlightLine>,
}

impl HighlightLineCache {
    fn metrics(&mut self, l: &parley::Line<'_, Ink>, para_idx: usize, line_idx: usize, range: std::ops::Range<usize>) -> (f32, f32) {
        self.lines.entry((para_idx, line_idx)).or_insert_with(|| HighlightLine::new(l)).metrics(range)
    }
}

#[derive(Clone, Copy)]
enum Visit { DecoRing, StrikeItem, StrikeSpan, HighlightRun }

#[cfg(test)]
thread_local! {
    static VISITS: [std::cell::Cell<usize>; 4] = const { [
        std::cell::Cell::new(0), std::cell::Cell::new(0),
        std::cell::Cell::new(0), std::cell::Cell::new(0),
    ] };
}

#[cfg(test)]
#[inline]
fn count_visit(kind: Visit) { VISITS.with(|v| { let c = &v[kind as usize]; c.set(c.get() + 1); }); }
#[cfg(not(test))]
#[inline]
fn count_visit(_: Visit) {}

#[cfg(test)]
fn add_deco_ring_visits(n: usize) { VISITS.with(|v| v[0].set(v[0].get() + n)); }
#[cfg(test)]
fn take_deco_ring_visits() -> usize { VISITS.with(|v| v[0].replace(0)) }
#[cfg(test)]
fn take_strike_visits() -> (usize, usize) { VISITS.with(|v| (v[1].replace(0), v[2].replace(0))) }
#[cfg(test)]
fn take_highlight_run_visits() -> usize { VISITS.with(|v| v[3].replace(0)) }

#[cfg(test)]
#[path = "decorations_tests.rs"]
mod tests;
