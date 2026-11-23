//! Link hover, press and focus decorations: indexed background, ring, line-metric and
//! underline updates over the frame's `PaintedDeco` scratch buffer.
use super::*;
use crate::text::LinkPaint;

/// Line boxes of bytes `start..end` in layout device px, with each box's line index.
pub(super) fn boxes(layout: &Layout<Ink>, len: usize, start: u32, end: u32) -> Vec<(parley::BoundingBox, usize)> {
    let (a, b) = ((start as usize).min(len), (end as usize).min(len));
    crate::text::range_boxes(layout, a, b, false)
}

/// The link paints to draw this frame, by byte range, with the resting span's `bg_pad`.
pub(in crate::runtime) struct LinkFx(pub(super) Vec<(u32, u32, LinkPaint, f32, bool)>);

impl LinkFx {
    pub(in crate::runtime) fn is_empty(&self) -> bool { self.0.is_empty() }

    pub(in crate::runtime) fn glyph_color(&self, byte: u32) -> Option<Color> {
        self.0.iter().find(|(a, b, p, _, _)| byte >= *a && byte < *b && p.color.is_some()).and_then(|(_, _, p, _, _)| p.color)
    }

    pub(in crate::runtime) fn decos_piece(&self, piece: &crate::text::Piece<'_>, len: usize, s: f32, origin: [f32; 2], out: &mut Vec<PaintedDeco>) {
        if self.is_empty() { return; }
        if piece.is_whole() { return self.decos(piece.layout, len, s, origin, out); }
        let clipped: Vec<_> = self.0.iter().filter_map(|&(a, b, p, pad, rings)| {
            let (sa, sb) = piece.clip((a as usize).min(len), (b as usize).min(len))?;
            Some((sa as u32, sb as u32, p, pad, rings))
        }).collect();
        if !clipped.is_empty() { LinkFx(clipped).decos(piece.layout, piece.shaped(piece.end), s, origin, out); }
    }

    /// Backgrounds and underlines (device px, layout top-left at `origin`).
    pub(in crate::runtime) fn decos(&self, layout: &Layout<Ink>, len: usize, s: f32, origin: [f32; 2], out: &mut Vec<PaintedDeco>) {
        let mut total_bg = 0usize;
        let mut total_ul = 0usize;
        let link_boxes: Vec<_> = self.0.iter().filter_map(|&(a, b, p, bg_pad, has_rings)| {
            let has_bg = p.bg.is_some() || p.ring.is_some();
            if !has_bg && p.underline.is_none() {
                return None;
            }
            let bs = boxes(layout, len, a, b);
            if has_bg { total_bg += bs.len(); }
            if p.underline.is_some() { total_ul += bs.len(); }
            Some((a, b, p, bg_pad, has_rings, bs))
        }).collect();
        if total_bg == 0 && total_ul == 0 {
            return;
        }
        let mut line_ends: HashMap<usize, f32> = HashMap::default();
        let mut line_metrics = (total_bg > 1).then(LineMetricsCache::default);
        let mut bg_index: Option<BgIndex> = None;
        let mut ul_index = (total_ul > 1).then(|| UnderlineIndex::build(out));

        for (a, b, p, bg_pad, has_rings, bs) in link_boxes {
            for (bb, line) in bs {
                let (x0, w) = (origin[0] + bb.x0 as f32, (bb.x1 - bb.x0) as f32);
                if p.bg.is_some() || p.ring.is_some() {
                    let pad = bg_pad * s;
                    let (mut y0, mut y1) = ((origin[1] + bb.y0 as f32).round(), (origin[1] + bb.y1 as f32).round());
                    if let Some(l) = layout.get(line) {
                        let base = origin[1] + l.metrics().baseline;
                        let (asc, desc) = match line_metrics.as_mut() {
                            Some(cache) => cache.metrics(&l, line, a, b),
                            None => scan_line_metrics(&l, a, b),
                        };
                        if asc > 0.0 || desc > 0.0 {
                            y0 = (base - asc - 1.0 * s).round();
                            y1 = (base + desc + 1.0 * s).round();
                        }
                    }
                    let rect = [x0 - pad, y0, w + 2.0 * pad, y1 - y0];
                    let ring = p.ring.map(|(c, rw)| (c, rw * s));
                    let byte = b.saturating_sub(1);
                    if total_bg > 1 {
                        let idx = bg_index.get_or_insert_with(|| BgIndex::build(out, (y1 - y0).max(24.0 * s)));
                        idx.apply(out, rect, p, ring, has_rings, s, byte);
                    } else {
                        apply_bg_single(out, rect, p, ring, has_rings, s, byte);
                    }
                }
                if let (Some(c), Some(l)) = (p.underline, layout.get(line)) {
                    let y = (origin[1] + l.metrics().baseline + 2.0 * s).round();
                    let end = *line_ends.entry(line).or_insert_with(|| {
                        #[cfg(test)]
                        count_link_line_end_visits(&l);
                        crate::text::line_end(&l)
                    });
                    let (x0, x1) = crate::text::underline_edges_with_end(end, origin[0], bb.x0 as f32, bb.x1 as f32);
                    let w = x1 - x0;
                    let found = match ul_index.as_ref() {
                        Some(index) => index.find(out, x0, y, w),
                        None => out.iter().position(|d| {
                            #[cfg(test)]
                            count_link_ul_out_visit();
                            d.radius == 0.0
                                && (d.rect[0] - x0).abs() < 1.5
                                && (d.rect[2] - w).abs() < 1.5
                                && (d.rect[1] - y).abs() < 1.5
                        }),
                    };
                    if let Some(idx) = found {
                        out[idx].color = c;
                    } else {
                        let idx = out.len();
                        out.push(PaintedDeco { deco: Deco { rect: [x0, y, w, s.max(1.0)], color: c, radius: 0.0 }, ring: None, byte: 0 });
                        if let Some(index) = ul_index.as_mut() {
                            index.insert(idx, &out[idx]);
                        }
                    }
                }
            }
        }
    }
}

#[inline]
fn f32_order_key(v: f32) -> u32 {
    let bits = (if v == 0.0 { 0.0f32 } else { v }).to_bits();
    if bits & 0x8000_0000 != 0 { !bits } else { bits ^ 0x8000_0000 }
}

/// 4 px spatial cell index for exact `< 1.5` px neighbour lookups.
#[inline]
fn deco_cell(v: f32) -> Option<i64> {
    (v.is_finite() && v.abs() < 1e15).then(|| ((v as f64) * 0.25).floor() as i64)
}

/// The 1 or 2 cell indices (4 px wide) that can hold a coordinate within 1.5 px of `v`.
#[inline]
fn near_cells(v: f32) -> Option<(i64, Option<i64>)> {
    let c = deco_cell(v)?;
    let lo = (((v as f64) - 1.5) * 0.25).floor() as i64;
    let hi = (((v as f64) + 1.5) * 0.25).floor() as i64;
    Some((c, if lo < c { Some(lo) } else if hi > c { Some(hi) } else { None }))
}

fn scan_line_metrics(line: &parley::Line<'_, Ink>, a: u32, b: u32) -> (f32, f32) {
    let (mut asc, mut desc) = (0.0f32, 0.0f32);
    for run in line.runs() {
        #[cfg(test)]
        count_link_bg_run_visit();
        let r = run.text_range();
        if r.start < b as usize && (a as usize) < r.end {
            let fm = run.font_metrics();
            (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
        }
    }
    (asc, desc)
}

struct LineRun {
    start: usize,
    end: usize,
    max_end: usize,
    asc: f32,
    desc: f32,
}

#[derive(Default)]
struct LineMetricsCache {
    lines: HashMap<usize, Vec<LineRun>>,
    ranges: HashMap<(usize, u32, u32), (f32, f32)>,
}

impl LineMetricsCache {
    fn metrics(&mut self, l: &parley::Line<'_, Ink>, line: usize, a: u32, b: u32) -> (f32, f32) {
        if let Some(&m) = self.ranges.get(&(line, a, b)) {
            return m;
        }
        let runs = self.lines.entry(line).or_insert_with(|| {
            let mut v: Vec<LineRun> = l.runs().map(|run| {
                #[cfg(test)]
                count_link_bg_run_visit();
                let r = run.text_range();
                let fm = run.font_metrics();
                LineRun { start: r.start, end: r.end, max_end: r.end, asc: fm.ascent, desc: fm.descent.abs() }
            }).collect();
            v.sort_unstable_by_key(|r| r.start);
            let mut max_end = 0;
            for r in &mut v {
                max_end = max_end.max(r.end);
                r.max_end = max_end;
            }
            v
        });
        let (a_us, b_us) = (a as usize, b as usize);
        let start = runs.partition_point(|r| r.max_end <= a_us);
        let (mut asc, mut desc) = (0.0f32, 0.0f32);
        for r in &runs[start..] {
            if r.start >= b_us { break; }
            #[cfg(test)]
            count_link_bg_run_visit();
            if r.end > a_us {
                (asc, desc) = (asc.max(r.asc), desc.max(r.desc));
            }
        }
        self.ranges.insert((line, a, b), (asc, desc));
        (asc, desc)
    }
}

struct UnderlineIndex {
    buckets: HashMap<(i64, i64), Vec<usize>>,
}

impl UnderlineIndex {
    fn build(out: &[PaintedDeco]) -> Self {
        let mut buckets: HashMap<(i64, i64), Vec<usize>> = HashMap::default();
        for (idx, d) in out.iter().enumerate() {
            #[cfg(test)]
            count_link_ul_out_visit();
            if d.radius == 0.0
                && let (Some(cy), Some(cx)) = (deco_cell(d.rect[1]), deco_cell(d.rect[0]))
            {
                buckets.entry((cy, cx)).or_default().push(idx);
            }
        }
        Self { buckets }
    }

    fn insert(&mut self, idx: usize, d: &PaintedDeco) {
        if let (Some(cy), Some(cx)) = (deco_cell(d.rect[1]), deco_cell(d.rect[0])) {
            self.buckets.entry((cy, cx)).or_default().push(idx);
        }
    }

    fn find(&self, out: &[PaintedDeco], x0: f32, y: f32, w: f32) -> Option<usize> {
        let ((cy0, cy1), (cx0, cx1)) = (near_cells(y)?, near_cells(x0)?);
        let mut best: Option<usize> = None;
        for cy in [Some(cy0), cy1].into_iter().flatten() {
            for cx in [Some(cx0), cx1].into_iter().flatten() {
                let Some(bucket) = self.buckets.get(&(cy, cx)) else { continue };
                for &idx in bucket {
                    if best.is_some_and(|b| idx >= b) { break; }
                    #[cfg(test)]
                    count_link_ul_out_visit();
                    let d = &out[idx];
                    if (d.rect[0] - x0).abs() < 1.5
                        && (d.rect[2] - w).abs() < 1.5
                        && (d.rect[1] - y).abs() < 1.5
                    {
                        best = Some(idx);
                        break;
                    }
                }
            }
        }
        best
    }
}

/// Applies link background/ring paint to matching decorations, or pushes a new one and returns its index.
#[allow(clippy::too_many_arguments)]
fn apply_bg_matches(
    out: &mut Vec<PaintedDeco>,
    candidates: &[usize],
    rect: [f32; 4],
    p: LinkPaint,
    ring: Option<(Color, f32)>,
    split: bool,
    s: f32,
    byte: u32,
) -> Option<usize> {
    let mut matched = false;
    for &idx in candidates {
        let d = &mut out[idx];
        if split || ((d.rect[0] - rect[0]).abs() < 1.5 && (d.rect[2] - rect[2]).abs() < 1.5) {
            matched = true;
            if let Some(c) = p.bg { d.color = c; }
            if ring.is_some() { d.ring = ring; }
        }
    }
    if matched {
        return None;
    }
    let idx = out.len();
    out.push(PaintedDeco { deco: Deco { rect, color: p.bg.unwrap_or(Color::TRANSPARENT), radius: 4.0 * s }, ring, byte });
    Some(idx)
}

fn apply_bg_single(out: &mut Vec<PaintedDeco>, rect: [f32; 4], p: LinkPaint, ring: Option<(Color, f32)>, has_rings: bool, s: f32, byte: u32) {
    let (rx0, y0, rw, y1) = (rect[0], rect[1], rect[2], rect[1] + rect[3]);
    let rx1 = rx0 + rw;
    let mut contained = Vec::new();
    let (mut has_ring, mut hits_left, mut hits_right) = (false, false, false);
    for (idx, d) in out.iter().enumerate() {
        #[cfg(test)]
        count_link_bg_out_visit();
        if d.radius > 0.0
            && d.rect[0] >= rx0 - 1.5
            && d.rect[0] + d.rect[2] <= rx1 + 1.5
            && d.rect[1] < y1
            && y0 < d.rect[1] + d.rect[3]
        {
            if has_rings {
                has_ring |= d.ring.is_some();
                hits_left |= (d.rect[0] - rx0).abs() < 1.5;
                hits_right |= (d.rect[0] + d.rect[2] - rx0 - rw).abs() < 1.5;
            }
            contained.push(idx);
        }
    }
    // A ring can split an otherwise uniform link background. Recolor those
    // pieces in place, but don't mistake an isolated inline-code background
    // for the whole link's hover background.
    let split = has_rings && has_ring && hits_left && hits_right;
    apply_bg_matches(out, &contained, rect, p, ring, split, s, byte);
}

struct BgIndex {
    band_h: f32,
    bands: std::collections::BTreeMap<i64, std::collections::BTreeSet<(u32, usize)>>,
    overflow: Vec<usize>,
    candidates: Vec<usize>,
}

impl BgIndex {
    fn build(out: &[PaintedDeco], band_h: f32) -> Self {
        let mut this = Self {
            band_h: band_h.max(16.0),
            bands: std::collections::BTreeMap::new(),
            overflow: Vec::new(),
            candidates: Vec::new(),
        };
        for (idx, d) in out.iter().enumerate() {
            #[cfg(test)]
            count_link_bg_out_visit();
            if d.radius > 0.0 {
                this.insert(idx, d);
            }
        }
        this
    }

    #[inline]
    fn band(&self, y: f32) -> Option<i64> {
        (y.is_finite() && y.abs() < 1e15).then(|| ((y as f64) / (self.band_h as f64)).floor() as i64)
    }

    fn insert(&mut self, idx: usize, d: &PaintedDeco) {
        if !(d.rect[0].is_finite() && d.rect[1].is_finite() && d.rect[2].is_finite() && d.rect[3].is_finite()) {
            return;
        }
        if d.rect[2] < 0.0 {
            self.overflow.push(idx);
            return;
        }
        let (y_lo, y_hi) = (d.rect[1].min(d.rect[1] + d.rect[3]), d.rect[1].max(d.rect[1] + d.rect[3]));
        let (Some(b_lo), Some(b_hi)) = (self.band(y_lo), self.band(y_hi)) else {
            self.overflow.push(idx);
            return;
        };
        if b_hi - b_lo > 8 {
            self.overflow.push(idx);
            return;
        }
        let key = (f32_order_key(d.rect[0]), idx);
        for b in b_lo..=b_hi {
            self.bands.entry(b).or_default().insert(key);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply(&mut self, out: &mut Vec<PaintedDeco>, rect: [f32; 4], p: LinkPaint, ring: Option<(Color, f32)>, has_rings: bool, s: f32, byte: u32) {
        let (rx0, y0, rw, y1) = (rect[0], rect[1], rect[2], rect[1] + rect[3]);
        let rx1 = rx0 + rw;
        let bands_range = (rx0.is_finite() && rw.is_finite())
            .then(|| self.band(y0.min(y1)).zip(self.band(y0.max(y1))))
            .flatten();
        self.candidates.clear();
        let (mut has_ring, mut hits_left, mut hits_right) = (false, false, false);
        let candidates = &mut self.candidates;
        let mut visit = |idx: usize| {
            #[cfg(test)]
            count_link_bg_out_visit();
            let d = &out[idx];
            if d.rect[0] >= rx0 - 1.5
                && d.rect[0] + d.rect[2] <= rx1 + 1.5
                && d.rect[1] < y1
                && y0 < d.rect[1] + d.rect[3]
            {
                if has_rings {
                    has_ring |= d.ring.is_some();
                    hits_left |= (d.rect[0] - rx0).abs() < 1.5;
                    hits_right |= (d.rect[0] + d.rect[2] - rx0 - rw).abs() < 1.5;
                }
                candidates.push(idx);
            }
        };
        if let Some((b_lo, b_hi)) = bands_range {
            let x_lo = f32_order_key(rx0 - 2.0);
            let x_hi = f32_order_key(if has_rings { rx1 + 2.0 } else { rx0 + 2.0 });
            if x_lo <= x_hi {
                for (_, set) in self.bands.range(b_lo..=b_hi) {
                    for &(_, idx) in set.range((x_lo, 0)..=(x_hi, usize::MAX)) {
                        visit(idx);
                    }
                }
            }
            for &idx in &self.overflow {
                visit(idx);
            }
        }
        if self.candidates.len() > 1 {
            self.candidates.sort_unstable();
            self.candidates.dedup();
        }
        let split = has_rings && has_ring && hits_left && hits_right;
        if let Some(idx) = apply_bg_matches(out, &self.candidates, rect, p, ring, split, s, byte) {
            self.insert(idx, &out[idx]);
        }
    }
}

#[cfg(test)]
thread_local! {
    static LINK_LINE_END_ITEM_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LINK_BG_RUN_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LINK_BG_OUT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LINK_UL_OUT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn count_link_line_end_visits(line: &parley::Line<'_, Ink>) {
    let runs = line.items().filter(|it| matches!(it, parley::PositionedLayoutItem::GlyphRun(_))).count();
    LINK_LINE_END_ITEM_VISITS.with(|c| c.set(c.get() + runs));
}

#[cfg(test)]
pub(super) fn take_link_line_end_visits() -> usize {
    LINK_LINE_END_ITEM_VISITS.with(|c| c.replace(0))
}

#[cfg(test)]
fn count_link_bg_run_visit() {
    LINK_BG_RUN_VISITS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn count_link_bg_out_visit() {
    LINK_BG_OUT_VISITS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn count_link_ul_out_visit() {
    LINK_UL_OUT_VISITS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
pub(super) fn take_link_deco_visits() -> (usize, usize, usize) {
    (
        LINK_BG_RUN_VISITS.with(|c| c.replace(0)),
        LINK_BG_OUT_VISITS.with(|c| c.replace(0)),
        LINK_UL_OUT_VISITS.with(|c| c.replace(0)),
    )
}
