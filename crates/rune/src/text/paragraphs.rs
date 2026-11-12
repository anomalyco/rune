//! A text field laid out one paragraph at a time ([`El::dir_by_paragraph`](crate::El::dir_by_paragraph)):
//! each line a newline ends gets its own layout in the direction of its own first strong
//! character, as CSS's `unicode-bidi: plaintext` lays out a `contenteditable`. Parley gives a
//! layout one base direction, so an Arabic line under an English one needs a layout of its own.
//! The paragraphs stack in one field: one editor, one caret, one selection. An edit shapes only
//! the paragraphs it changed; the others keep their layouts from the field's last one.
//!
//! A mention pill's label is shaped between U+2068 FIRST STRONG ISOLATE and U+2069 POP
//! DIRECTIONAL ISOLATE, as a `dir="auto"` span with `unicode-bidi: isolate` is: `@src/app.ts`
//! stays left to right on an Arabic line, and the line's direction skips it. Positions outside
//! this module are the field's content bytes; [`Piece::shaped`] and [`Piece::content`] map them
//! onto a paragraph's shaped text and back.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};

use super::*;
use crate::edit::Block;
use crate::el::{Direction, Flow, TextSpec};

/// The bytes U+2068 and U+2069 each take.
const ISOLATE: usize = 3;
/// Keeps a by-paragraph layout's cache key apart from a one-layout one of the same text.
const PARAGRAPHS: u64 = 0x3c6e_f372_fe94_f82b;

/// A text node's layout: one paragraph layout, or a by-paragraph field's stack of them.
#[derive(Clone)]
pub(crate) enum TextLayout {
    Single(Rc<Layout<Ink>>),
    Paragraphs(Rc<ParagraphLayout>),
}

impl TextLayout {
    /// The one layout, when it is one: everything but a by-paragraph field.
    pub(crate) fn single(&self) -> Option<&Rc<Layout<Ink>>> {
        match self {
            TextLayout::Single(l) => Some(l),
            TextLayout::Paragraphs(_) => None,
        }
    }

    pub(crate) fn view(&self) -> TextFlow<'_> {
        match self {
            TextLayout::Single(l) => TextFlow::Single(l),
            TextLayout::Paragraphs(p) => TextFlow::Paragraphs(p),
        }
    }

    /// Which layout it is, for telling a changed one apart.
    pub(crate) fn address(&self) -> usize {
        match self {
            TextLayout::Single(l) => Rc::as_ptr(l) as usize,
            TextLayout::Paragraphs(p) => Rc::as_ptr(p) as usize,
        }
    }
}

/// A by-paragraph field's paragraphs, top to bottom, in device px.
pub(crate) struct ParagraphLayout {
    pub(crate) paragraphs: Vec<Paragraph>,
    width: f32,
    full_width: f32,
    height: f32,
    /// What its lines were broken at; the next edit's paragraphs re-break theirs when it changed.
    max_width: Option<f32>,
    align: Alignment,
    /// Its text's shape key, which its layouts at other widths are cached under.
    whole: u64,
    /// The width a selected line break highlights, as a selection across a newline shows.
    newline: f32,
}

/// One paragraph: content bytes `start..end` (its newline left out), laid out at `y`.
pub(crate) struct Paragraph {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) y: f32,
    pub(crate) height: f32,
    pub(crate) flow: Flow,
    /// What its layout depends on but the width: its words, their shaping spans, its pills,
    /// block and direction, in the field's style.
    key: u64,
    /// Its pills' labels, as bytes from its start, each shaped between FSI and PDI.
    isolates: Box<[(u32, u32)]>,
    pub(crate) layout: Rc<Layout<Ink>>,
}

/// One layout of a text and where it sits: its whole layout, or one paragraph of a field's.
#[derive(Clone, Copy)]
pub(crate) struct Piece<'a> {
    pub(crate) layout: &'a Layout<Ink>,
    /// A paragraph's own layout, which glyph caching keys on.
    pub(crate) shared: Option<&'a Rc<Layout<Ink>>>,
    /// Its content bytes; a whole layout's run past the end.
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) y: f32,
    pub(crate) height: f32,
    pub(crate) flow: Flow,
    /// Its paragraph's index in the field, for its [`Block`].
    pub(crate) index: usize,
    isolates: &'a [(u32, u32)],
    whole: bool,
}

impl<'a> Piece<'a> {
    fn of_paragraph(k: usize, p: &'a Paragraph) -> Piece<'a> {
        Piece { layout: &p.layout, shared: Some(&p.layout), start: p.start, end: p.end, y: p.y, height: p.height, flow: p.flow, index: k, isolates: &p.isolates, whole: false }
    }

    fn whole(layout: &'a Layout<Ink>) -> Piece<'a> {
        Piece { layout, shared: None, start: 0, end: usize::MAX, y: 0.0, height: layout.height(), flow: Flow::of_layout(layout), index: 0, isolates: &[], whole: true }
    }

    /// Whether it's a text's one layout rather than a paragraph of a field's.
    pub(crate) fn is_whole(&self) -> bool {
        self.whole
    }

    /// Whether it's a paragraph with no words.
    pub(crate) fn is_blank(&self) -> bool {
        !self.whole && self.start == self.end
    }

    /// Its shaped text's byte at content byte `at` (clamped into it).
    pub(crate) fn shaped(&self, at: usize) -> usize {
        if self.whole { return at; }
        shaped(self.isolates, at.clamp(self.start, self.end) - self.start)
    }

    /// The content byte at its shaped text's byte `at`.
    pub(crate) fn content(&self, at: usize) -> usize {
        if self.whole { return at; }
        self.start + content(self.isolates, at)
    }

    /// `spans` over its words, on its shaped text (a whole layout's own).
    pub(crate) fn spans<'s>(&self, spans: &'s [Span]) -> Cow<'s, [Span]> {
        if self.whole { return Cow::Borrowed(spans); }
        Cow::Owned(spans_in(spans, self.start, self.end, self.isolates).collect())
    }

    /// `pills` inside it, on its shaped text.
    pub(crate) fn pills<'s>(&self, pills: &'s [PillChip]) -> Cow<'s, [PillChip]> {
        if self.whole { return Cow::Borrowed(pills); }
        Cow::Owned(pills_in(pills, self.start, self.end, self.isolates).collect())
    }

    /// Strikethrough byte ranges over its words, on its shaped text.
    pub(crate) fn strikes<'s>(&self, strikes: &'s [(u32, u32, Option<Color>)]) -> Cow<'s, [(u32, u32, Option<Color>)]> {
        if self.whole { return Cow::Borrowed(strikes); }
        Cow::Owned(strikes.iter().filter_map(|&(a, b, c)| {
            let (a, b) = self.clip(a as usize, b as usize)?;
            Some((a as u32, b as u32, c))
        }).collect())
    }

    /// Content bytes `a..b` on its shaped text, when they meet its words.
    pub(crate) fn clip(&self, a: usize, b: usize) -> Option<(usize, usize)> {
        if self.whole { return (a < b).then_some((a, b)); }
        (a < self.end && self.start < b && a < b).then(|| (self.shaped(a), self.shaped(b)))
    }
}

/// Its shaped text's byte at byte `local` of a paragraph with these isolated pills.
fn shaped(isolates: &[(u32, u32)], local: usize) -> usize {
    local + isolates.iter().map(|&(a, b)| ISOLATE * (usize::from((a as usize) < local) + usize::from(b as usize <= local))).sum::<usize>()
}

/// The paragraph byte at its shaped text's byte `at`: inside an FSI it's the pill's start, inside
/// a PDI its end.
fn content(isolates: &[(u32, u32)], at: usize) -> usize {
    let mut shift = 0;
    for &(a, b) in isolates {
        let (a, b) = (a as usize, b as usize);
        let fsi = a + shift;
        if at <= fsi { return at - shift; }
        if at <= fsi + ISOLATE { return a; }
        let pdi = b + shift + ISOLATE;
        if at < pdi { return at - shift - ISOLATE; }
        if at <= pdi + ISOLATE { return b; }
        shift += 2 * ISOLATE;
    }
    at - shift
}

/// `spans` meeting content bytes `a..b`, cut to them and moved onto the shaped text.
fn spans_in<'s>(spans: &'s [Span], a: usize, b: usize, isolates: &'s [(u32, u32)]) -> impl Iterator<Item = Span> + 's {
    spans.iter().filter(move |s| (s.start as usize) < b && a < s.end as usize).map(move |s| Span {
        start: shaped(isolates, (s.start as usize).max(a) - a) as u32,
        end: shaped(isolates, (s.end as usize).min(b) - a) as u32,
        ..s.clone()
    })
}

/// `pills` inside content bytes `a..b`, each its label's shaped bytes.
fn pills_in<'s>(pills: &'s [PillChip], a: usize, b: usize, isolates: &'s [(u32, u32)]) -> impl Iterator<Item = PillChip> + 's {
    pills.iter().filter(move |p| inside(p, a, b)).map(move |p| PillChip {
        start: (shaped(isolates, p.start as usize - a) + ISOLATE) as u32,
        end: (shaped(isolates, p.end as usize - a) - ISOLATE) as u32,
        ..p.clone()
    })
}

fn inside(p: &PillChip, a: usize, b: usize) -> bool {
    a <= p.start as usize && p.end as usize <= b && p.start < p.end
}

/// A run of paragraphs to draw, or a whole layout.
pub(crate) struct Pieces<'a> {
    whole: Option<Piece<'a>>,
    paragraphs: std::iter::Enumerate<std::slice::Iter<'a, Paragraph>>,
    bottom: f32,
}

impl<'a> Iterator for Pieces<'a> {
    type Item = Piece<'a>;
    fn next(&mut self) -> Option<Piece<'a>> {
        if let Some(w) = self.whole.take() { return Some(w); }
        let (k, p) = self.paragraphs.next()?;
        (p.y <= self.bottom).then(|| Piece::of_paragraph(k, p))
    }
}

/// A text's layout, borrowed: one paragraph's, or a field's paragraphs. Bytes are the text's
/// content bytes; positions are device px from the text's top-left.
#[derive(Clone, Copy)]
pub(crate) enum TextFlow<'a> {
    Single(&'a Layout<Ink>),
    Paragraphs(&'a ParagraphLayout),
}

/// Where a point lands in a text.
pub(crate) struct Hit {
    pub(crate) index: usize,
    /// Which side of `index` the press was on, where two runs meet.
    pub(crate) side: parley::layout::Affinity,
    /// The cluster before a caret that ends a line's text, when it's not the line break.
    pub(crate) before: Option<usize>,
}

/// A laid-out line, for paragraph block decorations.
pub(crate) struct BlockLine {
    pub(crate) top: f32,
    pub(crate) bottom: f32,
    pub(crate) baseline: f32,
    /// A paragraph ends with it.
    pub(crate) last: bool,
}

impl<'a> TextFlow<'a> {
    pub(crate) fn width(self) -> f32 {
        match self { TextFlow::Single(l) => l.width(), TextFlow::Paragraphs(p) => p.width }
    }

    pub(crate) fn full_width(self) -> f32 {
        match self { TextFlow::Single(l) => l.full_width(), TextFlow::Paragraphs(p) => p.full_width }
    }

    pub(crate) fn height(self) -> f32 {
        match self { TextFlow::Single(l) => l.height(), TextFlow::Paragraphs(p) => p.height }
    }

    /// The direction its lines run in, for where an unwrapped line starts: a by-paragraph
    /// field wraps, so each paragraph aligns itself.
    pub(crate) fn flow(self) -> Flow {
        match self { TextFlow::Single(l) => Flow::of_layout(l), TextFlow::Paragraphs(_) => Flow::Ltr }
    }

    /// [`spec_height`](super::spec_height) of it.
    pub(crate) fn spec_height(self, style: TextStyle, scale: f32, exact: bool) -> f32 {
        match self {
            TextFlow::Single(l) => super::spec_height(l, style, scale, exact),
            TextFlow::Paragraphs(p) => {
                let h = p.height.max(style.line_px() * scale);
                if exact { (h * 64.0).round() / 64.0 } else { h.ceil() }
            }
        }
    }

    /// Its pieces that meet device rows `top..bottom`, top to bottom.
    pub(crate) fn pieces_in(self, top: f32, bottom: f32) -> Pieces<'a> {
        match self {
            TextFlow::Single(l) => Pieces { whole: Some(Piece::whole(l)), paragraphs: [].iter().enumerate(), bottom },
            TextFlow::Paragraphs(p) => {
                let from = p.paragraphs.partition_point(|q| q.y + q.height < top);
                let mut paragraphs = p.paragraphs.iter().enumerate();
                if from > 0 { paragraphs.nth(from - 1); }
                Pieces { whole: None, paragraphs, bottom }
            }
        }
    }

    pub(crate) fn pieces(self) -> Pieces<'a> {
        self.pieces_in(f32::NEG_INFINITY, f32::INFINITY)
    }

    /// The piece holding content byte `at`: a paragraph owns the bytes from its start to its end,
    /// the caret after its last word included.
    pub(crate) fn piece_at(self, at: usize) -> Piece<'a> {
        match self {
            TextFlow::Single(l) => Piece::whole(l),
            TextFlow::Paragraphs(p) => {
                let k = p.paragraphs.partition_point(|q| q.end < at).min(p.paragraphs.len() - 1);
                Piece::of_paragraph(k, &p.paragraphs[k])
            }
        }
    }

    /// The piece at device row `y`, the first or last past either end.
    fn piece_at_y(self, y: f32) -> Piece<'a> {
        match self {
            TextFlow::Single(l) => Piece::whole(l),
            TextFlow::Paragraphs(p) => {
                let k = p.paragraphs.partition_point(|q| q.y + q.height <= y).min(p.paragraphs.len() - 1);
                Piece::of_paragraph(k, &p.paragraphs[k])
            }
        }
    }

    /// Where an empty paragraph's caret, `size` px wide, starts: its right edge (the width it
    /// wraps at) when it reads right to left, else 0.
    fn blank_start(self, p: &Piece<'_>, size: f32) -> f32 {
        match (p.flow, self) {
            (Flow::Rtl, TextFlow::Paragraphs(pl)) => (pl.max_width.unwrap_or(pl.width) - size).max(0.0),
            _ => 0.0,
        }
    }

    /// The caret's box at content byte `at`, on `side` of it, `size` px wide.
    pub(crate) fn caret(self, at: usize, side: parley::layout::Affinity, size: f32) -> parley::BoundingBox {
        let p = self.piece_at(at);
        if p.is_blank() {
            // An empty line's caret stands at its start: the right edge of a right-to-left one.
            let x = self.blank_start(&p, size) as f64;
            return parley::BoundingBox::new(x, p.y as f64, x + size as f64, (p.y + p.height) as f64);
        }
        let c = parley::editing::Cursor::from_byte_index(p.layout, p.shaped(at), side);
        let mut bb = cursor_geometry(&c, p.layout, size);
        bb.y0 += p.y as f64;
        bb.y1 += p.y as f64;
        bb
    }

    /// The caret a press at `(x, y)` puts down.
    pub(crate) fn hit(self, x: f32, y: f32) -> Hit {
        let p = self.piece_at_y(y);
        if p.is_blank() {
            return Hit { index: p.start, side: parley::layout::Affinity::Downstream, before: None };
        }
        let cur = cursor_from_point(p.layout, x, y - p.y);
        let before = (cur.affinity() == parley::layout::Affinity::Upstream)
            .then(|| cur.index().checked_sub(1).and_then(|b| parley::Cluster::from_byte_index(p.layout, b)))
            .flatten()
            .filter(|cl| !cl.is_hard_line_break())
            .map(|cl| p.content(cl.text_range().start));
        Hit { index: p.content(cur.index()), side: cur.affinity(), before }
    }

    /// The first (`!end`) or last (`end`) byte of the visual line holding byte `at`.
    pub(crate) fn line_edge(self, at: usize, end: bool) -> usize {
        let p = self.piece_at(at);
        if p.is_blank() { return p.start; }
        p.content(line_edge(p.layout, p.shaped(at), end))
    }

    /// The byte on the visual line above (`!down`) or below (`down`) byte `at`'s, at `goal`'s x
    /// (the column a run of Up and Down keeps) else its own, and that x; the start on the first
    /// line, `None` on the last.
    pub(crate) fn move_line(self, at: usize, down: bool, goal: Option<f32>) -> Option<(usize, f32)> {
        let pl = match self {
            TextFlow::Single(l) => return move_line(l, at, down, goal),
            TextFlow::Paragraphs(pl) => pl,
        };
        let p = self.piece_at(at);
        let (x, line) = if p.is_blank() || p.layout.len() == 0 {
            (self.blank_start(&p, 0.0), 0)
        } else {
            let c = parley::editing::Cursor::from_byte_index(p.layout, p.shaped(at), parley::layout::Affinity::Downstream);
            let bb = cursor_geometry(&c, p.layout, 0.0);
            let mid = ((bb.y0 + bb.y1) * 0.5) as f32;
            (bb.x0 as f32, cursor::line_for_offset(p.layout, mid).map_or(p.layout.len() - 1, |(i, _)| i))
        };
        let x = goal.unwrap_or(x);
        let lines = p.layout.len().max(1);
        let (k, line) = match down {
            true if line + 1 < lines => (p.index, line + 1),
            true if p.index + 1 < pl.paragraphs.len() => (p.index + 1, 0),
            true => return None,
            false if line > 0 => (p.index, line - 1),
            false if p.index > 0 => (p.index - 1, pl.paragraphs[p.index - 1].layout.len().max(1) - 1),
            false => return Some((0, x)),
        };
        let t = Piece::of_paragraph(k, &pl.paragraphs[k]);
        let Some(m) = t.layout.get(line).filter(|_| !t.is_blank()).map(|l| *l.metrics()) else { return Some((t.start, x)) };
        Some((t.content(cursor_from_point(t.layout, x, (m.block_min_coord + m.block_max_coord) * 0.5).index()), x))
    }

    /// Boxes covering content bytes `a..b`, each with its line's baseline (see
    /// [`range_boxes`](super::range_boxes)); a selected line break between paragraphs shows at the
    /// end of the line it ends, on the side its paragraph ends.
    pub(crate) fn range_boxes(self, a: usize, b: usize, pill_boxes: bool) -> Vec<(parley::BoundingBox, f32)> {
        let baseline = |l: &Layout<Ink>, i: usize| l.get(i).map_or(0.0, |line| line.metrics().baseline);
        let pl = match self {
            TextFlow::Single(l) => return range_boxes(l, a, b, pill_boxes).into_iter().map(|(bb, i)| (bb, baseline(l, i))).collect(),
            TextFlow::Paragraphs(pl) => pl,
        };
        let mut out = Vec::new();
        if a >= b { return out; }
        let from = pl.paragraphs.partition_point(|q| q.end < a);
        for (k, q) in pl.paragraphs.iter().enumerate().skip(from).take_while(|(_, q)| q.start <= b) {
            let p = Piece::of_paragraph(k, q);
            let y = p.y as f64;
            let first = out.len();
            if let Some((sa, sb)) = p.clip(a, b) {
                out.extend(range_boxes(p.layout, sa, sb, pill_boxes).into_iter().map(|(mut bb, i)| {
                    bb.y0 += y;
                    bb.y1 += y;
                    (bb, baseline(p.layout, i) + p.y)
                }));
            }
            if k + 1 < pl.paragraphs.len() && a <= p.end && p.end < b {
                let (top, bottom, base, start, end) = match p.layout.lines().last().filter(|_| !p.is_blank()) {
                    Some(line) => {
                        let m = line.metrics();
                        let x = m.inline_min_coord + m.offset;
                        (m.block_min_coord.min(m.content_block_min_coord), m.block_max_coord.max(m.content_block_max_coord), m.baseline, x, x + m.advance)
                    }
                    None => {
                        let x = self.blank_start(&p, 0.0);
                        (0.0, p.height, p.height * 0.8, x, x)
                    }
                };
                let (top, bottom) = (top as f64 + y, bottom as f64 + y);
                let ws = pl.newline as f64;
                let on_line = out[first..].iter_mut().filter(|(bb, _)| bb.y0 == top);
                let rtl = p.flow == Flow::Rtl;
                let edge = if rtl { on_line.min_by(|l, r| l.0.x0.total_cmp(&r.0.x0)) } else { on_line.max_by(|l, r| l.0.x1.total_cmp(&r.0.x1)) };
                match (edge, rtl) {
                    (Some((bb, _)), false) => bb.x1 += ws,
                    (Some((bb, _)), true) => bb.x0 -= ws,
                    (None, false) => out.push((parley::BoundingBox::new(end as f64, top, end as f64 + ws, bottom), base + p.y)),
                    (None, true) => out.push((parley::BoundingBox::new(start as f64 - ws, top, start as f64, bottom), base + p.y)),
                }
            }
        }
        out
    }

    /// The span its lines cover across, from the leftmost line start to the rightmost end, or
    /// `None` with no lines.
    pub(crate) fn extent(self) -> Option<(f32, f32)> {
        let (lo, hi) = self.pieces().flat_map(|p| p.layout.lines().map(|l| (l.metrics().offset, l.metrics().advance)))
            .fold((f32::MAX, 0.0f32), |(lo, hi), (offset, advance)| (lo.min(offset), hi.max(offset + advance)));
        (lo <= hi).then_some((lo, hi))
    }

    /// Its lines top to bottom, each marked when a paragraph ends with it.
    pub(crate) fn block_lines(self, line_px: f32) -> Vec<BlockLine> {
        let mut out = Vec::new();
        for p in self.pieces() {
            let at = out.len();
            for line in p.layout.lines() {
                let m = line.metrics();
                let bottom = if m.block_max_coord > m.block_min_coord { m.block_max_coord } else { m.block_min_coord + line_px };
                out.push(BlockLine { top: p.y + m.block_min_coord, bottom: p.y + bottom, baseline: p.y + m.baseline, last: line.break_reason() == parley::BreakReason::Explicit });
            }
            if !p.whole {
                if out.len() == at {
                    out.push(BlockLine { top: p.y, bottom: p.y + p.height, baseline: p.y + p.height * 0.8, last: false });
                }
                if let Some(l) = out.last_mut() { l.last = true; }
            }
        }
        out
    }
}

impl TextSystem {
    /// `spec`'s layout as a text node shows it: a by-paragraph field's paragraphs (`prev` its
    /// last layout, whose unchanged paragraphs it keeps), else one layout.
    pub(crate) fn layout_text(
        &mut self,
        spec: &TextSpec,
        input: bool,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        prev: Option<&TextLayout>,
    ) -> TextLayout {
        if !spec.by_paragraph(input) {
            return TextLayout::Single(self.layout_for_spec(spec, scale, max_width, align));
        }
        // One paragraph and no pills, as most drafts are: one layout in its own direction.
        if spec.cold.pills.is_empty() && !spec.content.contains('\n') {
            let base = flow_of(&spec.content, &[]).unwrap_or(spec.plaintext_flow()).base();
            let shape = spec.shape_key(scale).map(|k| hash::directed(k, base));
            let style = (&spec.content, &spec.spans, &spec.cold.blocks);
            return TextLayout::Single(self.layout_keyed(shape, style.0, style.1, style.2, &[], spec.style, scale, max_width, align, spec.wrap_anywhere, Keep::Cached, base));
        }
        let whole = spec.shape_key(scale).unwrap_or_else(|| spec.fresh_shape_key(scale));
        // Lines with no strong character take the element's flow, so it keys the whole too.
        let whole = if spec.plaintext_flow() == Flow::Rtl { whole ^ 0x3c6e_f372_fe94_f82b } else { whole };
        let blocks = &spec.cold.blocks;
        let indented = blocks.iter().any(|&b| b != Block::Paragraph).then_some(blocks.as_slice());
        let key = hash::break_key(whole ^ PARAGRAPHS, max_width, align, indented);
        let g = self.generation;
        if let Some(e) = self.paragraphs.get_mut(&key) {
            e.used = g;
            return TextLayout::Paragraphs(e.value.clone());
        }
        // Measuring asks at more than one width: the last text's layout at this one, when it
        // was laid out there too, keeps its paragraphs without breaking them again.
        let prev = match prev {
            Some(TextLayout::Paragraphs(p)) if p.max_width == max_width && p.align == align => Some(p.clone()),
            Some(TextLayout::Paragraphs(p)) => {
                let there = hash::break_key(p.whole ^ PARAGRAPHS, max_width, align, indented);
                Some(self.paragraphs.get(&there).map_or_else(|| p.clone(), |e| e.value.clone()))
            }
            _ => None,
        };
        // This text laid out at another width measuring asked for this commit: a paragraph
        // neither keeps is broken again from it rather than shaped twice.
        let other = self.last_paragraphs.clone().filter(|o| o.whole == whole && (o.max_width != max_width || o.align != align));
        let (built, fresh) = self.lay_out_paragraphs(spec, scale, max_width, align, whole, prev.as_deref(), other.as_deref());
        let built = Rc::new(built);
        self.last_paragraphs = Some(built.clone());
        // It holds what it shaped or broke anew; the paragraphs it kept are its last layout's.
        self.paragraphs.insert(key, Entry::new(built.clone(), g, fresh + built.paragraphs.len() * size_of::<Paragraph>() / 64, whole));
        TextLayout::Paragraphs(built)
    }

    /// Lays out each of `spec`'s paragraphs, keeping `prev`'s layout of each it still has: those
    /// before an edit and after it. A paragraph is the same when its words, the spans and pills
    /// over them that shape them, its block and its direction are.
    fn lay_out_paragraphs(&mut self, spec: &TextSpec, scale: f32, max_width: Option<f32>, align: Alignment, whole: u64, prev: Option<&ParagraphLayout>, other: Option<&ParagraphLayout>) -> (ParagraphLayout, usize) {
        let (content, style) = (spec.content.as_str(), spec.style);
        let style_key = {
            let mut h = hash::KeyHasher::default();
            (hash::text_hash("", &[], style, scale, None), spec.wrap_anywhere).hash(&mut h);
            h.finish()
        };
        let mut found: Vec<(usize, usize, Flow, u64, Box<[(u32, u32)]>)> = Vec::new();
        let mut start = 0;
        // A paragraph with no strong character reads the way the one before it does.
        let mut last = spec.plaintext_flow();
        for (k, words) in content.split('\n').enumerate() {
            let (a, b) = (start, start + words.len());
            start = b + 1;
            let isolates: Box<[(u32, u32)]> = spec.cold.pills.iter().filter(|p| inside(p, a, b)).map(|p| (p.start - a as u32, p.end - a as u32)).collect();
            let flow = flow_of(words, &isolates).unwrap_or(last);
            last = flow;
            let mut h = hash::KeyHasher::default();
            (words, style_key, flow == Flow::Rtl, spec.cold.blocks.get(k).copied().unwrap_or(Block::Paragraph)).hash(&mut h);
            for s in spans_in(&spec.spans, a, b, &isolates) {
                (s.start, s.end, s.mono, s.weight.map(f32::to_bits), s.size.map(f32::to_bits), s.italic).hash(&mut h);
            }
            for p in pills_in(&spec.cold.pills, a, b, &isolates) {
                (p.start, p.end, p.icon.is_some(), p.bare.as_ref().map(|b| (b.size.to_bits(), b.gap.to_bits()))).hash(&mut h);
            }
            found.push((a, b, flow, h.finish(), isolates));
        }
        // What the edit left alone: the paragraphs before it, and those after it.
        let old = prev.map_or(&[][..], |p| p.paragraphs.as_slice());
        let n = found.len();
        let before = found.iter().zip(old).take_while(|(f, o)| f.3 == o.key).count();
        let after = found[before..].iter().rev().zip(old[before..].iter().rev()).take_while(|(f, o)| f.3 == o.key).count();
        let rebreak = prev.is_some_and(|p| p.max_width != max_width || p.align != align);
        let line = style.line_px() * scale;
        let (mut paragraphs, mut y) = (Vec::with_capacity(n), 0.0f64);
        let (mut width, mut full_width, mut fresh) = (0.0f32, 0.0f32, 0);
        for (k, (a, b, flow, key, isolates)) in found.into_iter().enumerate() {
            let block = spec.cold.blocks.get(k).copied().unwrap_or(Block::Paragraph);
            let blocks: &[Block] = if block == Block::Paragraph { &[] } else { std::slice::from_ref(&block) };
            let kept = if k < before { Some(&old[k]) } else if k >= n - after { Some(&old[old.len() - (n - k)]) } else { None };
            // The same paragraph at the other width (the same text, so the same index).
            let twin = other.and_then(|o| o.paragraphs.get(k)).filter(|o| o.key == key);
            // A kept paragraph broken at another width, or this text's own paragraph at the
            // other width measuring asks for, breaks again here rather than shaping anew.
            let again = match kept { Some(o) if rebreak => Some(o), Some(_) => None, None => twin };
            let layout = match (kept, again) {
                (_, Some(o)) => {
                    fresh += b - a;
                    let mut l = o.layout.clone();
                    line_break::rebreak(Rc::make_mut(&mut l), blocks, style.size, scale, max_width, align);
                    l
                }
                (Some(o), None) => o.layout.clone(),
                (None, None) => {
                    fresh += b - a;
                    let words = &content[a..b];
                    let text: Cow<str> = if isolates.is_empty() { Cow::Borrowed(words) } else { Cow::Owned(with_isolates(words, &isolates)) };
                    let spans: Vec<Span> = spans_in(&spec.spans, a, b, &isolates).collect();
                    let pills: Vec<PillChip> = pills_in(&spec.cold.pills, a, b, &isolates).collect();
                    self.layout_keyed(Some(key), &text, &spans, blocks, &pills, style, scale, max_width, align, spec.wrap_anywhere, Keep::Owned, flow.base())
                }
            };
            let height = if a == b { layout.height().max(line) } else { layout.height() };
            (width, full_width) = (width.max(layout.width()), full_width.max(layout.full_width()));
            paragraphs.push(Paragraph { start: a, end: b, y: y.round() as f32, height, flow, key, isolates, layout });
            y += f64::from(height);
        }
        let (asc, desc) = self.primary_extents(style.family, style.weight, style.italic).unwrap_or((0.8, 0.2));
        let newline = 0.25 * (asc + desc.abs()) * style.size * scale;
        (ParagraphLayout { paragraphs, width, full_width, height: y as f32, max_width, align, whole, newline }, fresh)
    }
}

/// A paragraph's direction: its first strong character's outside its pills, or None with none.
fn flow_of(words: &str, isolates: &[(u32, u32)]) -> Option<Flow> {
    let mut at = 0;
    for &(a, b) in isolates.iter().chain([&(words.len() as u32, words.len() as u32)]) {
        if let Some(d) = words.get(at..a as usize).and_then(Direction::of) {
            return Some(if d == Direction::Rtl { Flow::Rtl } else { Flow::Ltr });
        }
        at = at.max(b as usize);
    }
    None
}

/// `words` with each isolated pill's label between FSI and PDI.
fn with_isolates(words: &str, isolates: &[(u32, u32)]) -> String {
    let mut out = String::with_capacity(words.len() + 2 * ISOLATE * isolates.len());
    let mut at = 0;
    for &(a, b) in isolates {
        out.push_str(&words[at..a as usize]);
        out.push('\u{2068}');
        out.push_str(&words[a as usize..b as usize]);
        out.push('\u{2069}');
        at = b as usize;
    }
    out.push_str(&words[at..]);
    out
}
