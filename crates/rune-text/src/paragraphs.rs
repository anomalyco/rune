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
use crate::Block;
use crate::{Direction, Flow, TextInput};

mod keying;
mod lazy;
#[cfg(test)]
mod tests;
use keying::{Found, Runs, count_newlines, flow_of, key_paragraphs, next_line_start, reuse_keys, with_isolates};
pub(crate) use keying::LineScan;
pub use keying::{common_end, common_start};
pub use lazy::AsciiLine;
pub(crate) use lazy::Lazy;

/// The bytes U+2068 and U+2069 each take.
const ISOLATE: usize = 3;
/// Keeps a by-paragraph layout's cache key apart from a one-layout one of the same text.
const PARAGRAPHS: u64 = 0x3c6e_f372_fe94_f82b;
/// More new paragraphs than this in one layout (a long paste) shape only those near the edit;
/// the others wait (`Lazy`).
const LAZY_MIN: usize = 64;
/// How many paragraphs before an edit's end a long paste shapes at once (a field's lines above
/// its caret, which follows the paste's end), and after its start.
const EAGER_END: usize = 24;
const EAGER_START: usize = 4;
/// The most lines one run holds: a keystroke inside a long paste keys its run's lines again,
/// and the other runs keep theirs.
const RUN_LINES: usize = 512;

/// A text node's layout: one paragraph layout, or a by-paragraph field's stack of them.
#[derive(Clone)]
pub enum TextLayout {
    Single(Rc<Layout<Ink>>),
    Paragraphs(Rc<ParagraphLayout>),
}

impl TextLayout {
    /// The one layout, when it is one: everything but a by-paragraph field.
    pub fn single(&self) -> Option<&Rc<Layout<Ink>>> {
        match self {
            TextLayout::Single(l) => Some(l),
            TextLayout::Paragraphs(_) => None,
        }
    }

    pub fn view(&self) -> TextFlow<'_> {
        match self {
            TextLayout::Single(l) => TextFlow::Single(l),
            TextLayout::Paragraphs(p) => TextFlow::Paragraphs(p),
        }
    }

    /// Which layout it is, for telling a changed one apart.
    pub fn address(&self) -> usize {
        match self {
            TextLayout::Single(l) => Rc::as_ptr(l) as usize,
            TextLayout::Paragraphs(p) => Rc::as_ptr(p) as usize,
        }
    }

    pub fn content_key(&self) -> Option<u64> {
        match self {
            TextLayout::Single(_) => None,
            TextLayout::Paragraphs(p) => Some(p.whole),
        }
    }
}

/// A by-paragraph field's paragraphs, top to bottom, in device px.
pub struct ParagraphLayout {
    pub paragraphs: Vec<Paragraph>,
    pub(crate) width: f32,
    full_width: f32,
    height: f32,
    /// What its lines were broken at; the next edit's paragraphs re-break theirs when it changed.
    pub(crate) max_width: Option<f32>,
    align: Alignment,
    /// First-line `text_indent` in points and device pixels (paragraph 0, line 0 only).
    text_indent: f32,
    text_indent_px: f32,
    /// Its text's shape key, which its layouts at other widths are cached under.
    whole: u64,
    /// The [`TextSystem::shaped_epoch`] that built this layout; older epochs are not reused.
    shaped_epoch: u64,
    /// The width a selected line break highlights, as a selection across a newline shows.
    pub(super) newline: f32,
    /// `[baseline, ascent, descent]` of one line in the base font (device px), for blank lines.
    pub(super) blank_box: [f32; 3],
    /// Whether `spec.spans` is sorted and non-overlapping, allowing binary-search slicing.
    pub(super) disjoint_spans: bool,
    /// What the next edit's keys start from (`Keys`): the text it laid out, when it had no
    /// spans or pills, in the style and starting direction its keys were made in.
    keyed: Option<Keyed>,
    /// While some of its paragraphs wait to be shaped: what shapes them.
    lazy: Option<Rc<Lazy>>,
}

impl ParagraphLayout {
    /// Whether some of its paragraphs wait to be shaped.
    pub fn waiting(&self) -> bool {
        self.lazy.is_some()
    }

    /// Character advances for waiting start-aligned ASCII paragraphs (`a11y::text::lines`).
    pub fn waiting_ascii(&self) -> Option<&AsciiLine> {
        self.lazy.as_deref().filter(|_| self.align == Alignment::Start).map(|l| &*l.ascii)
    }
}


/// A plain text's content and the key inputs that aren't per paragraph, so an edit keys only the
/// paragraphs it touched and takes the others' keys as they were.
pub(crate) struct Keyed {
    content: Rc<str>,
    style_key: u64,
    start_flow: Flow,
}

/// One paragraph: content bytes `start..end` (its newline left out), laid out at `y`.
#[derive(Clone)]
pub struct Paragraph {
    pub start: usize,
    pub end: usize,
    pub y: f32,
    /// Fractional vertical phase (`y - y.floor()`) a wrapped paragraph's lines were broken from.
    pub(super) y_frac: f32,
    pub height: f32,
    /// Its layout's width and full width, so a later edit that keeps it reads neither from it.
    widths: [f32; 2],
    /// Left indent and right padding in device px for this paragraph's [`Block`].
    pad: [f32; 2],
    pub flow: Flow,
    /// What its layout depends on but the width: its words, their shaping spans, its pills,
    /// block and direction, in the field's style.
    key: u64,
    /// Its pills' labels, as bytes from its start, each shaped between FSI and PDI.
    isolates: Box<[(u32, u32)]>,
    pub layout: Rc<Layout<Ink>>,
    /// False while it waits for a fill: `layout` is an empty stand-in, its height exact.
    pub shaped: bool,
    /// How many lines of the text it holds: more than one for a run of waiting lines, which a
    /// long paste keeps as one record so its frame doesn't touch every line.
    pub lines: u32,
    /// A run as keying found it, keyed by its bytes: a fill takes it apart into its lines first.
    run: bool,
}

/// One layout of a text and where it sits: its whole layout, or one paragraph of a field's.
#[derive(Clone, Copy)]
pub struct Piece<'a> {
    pub layout: &'a Layout<Ink>,
    /// A paragraph's own layout, which glyph caching keys on.
    pub shared: Option<&'a Rc<Layout<Ink>>>,
    /// Its content bytes; a whole layout's run past the end.
    pub start: usize,
    pub end: usize,
    pub y: f32,
    pub height: f32,
    pub flow: Flow,
    /// Its paragraph's index in the field, for its [`Block`].
    pub index: usize,
    pub shaped: bool,
    pad: [f32; 2],
    isolates: &'a [(u32, u32)],
    disjoint_spans: bool,
    whole: bool,
}

impl<'a> Piece<'a> {
    pub(crate) fn of_paragraph(k: usize, p: &'a Paragraph, disjoint_spans: bool) -> Piece<'a> {
        Piece { layout: &p.layout, shared: Some(&p.layout), start: p.start, end: p.end, y: p.y, height: p.height, flow: p.flow, index: k, shaped: p.shaped, pad: p.pad, isolates: &p.isolates, disjoint_spans, whole: false }
    }

    fn whole(layout: &'a Layout<Ink>) -> Piece<'a> {
        Piece { layout, shared: None, start: 0, end: usize::MAX, y: 0.0, height: layout.height(), flow: Flow::of_layout(layout), index: 0, shaped: true, pad: [0.0, 0.0], isolates: &[], disjoint_spans: false, whole: true }
    }

    /// Whether it's a text's one layout rather than a paragraph of a field's.
    pub fn is_whole(&self) -> bool {
        self.whole
    }

    /// Whether it's a paragraph with no words.
    pub fn is_blank(&self) -> bool {
        !self.whole && self.start == self.end
    }

    /// Its shaped text's byte at content byte `at` (clamped into it).
    pub fn shaped(&self, at: usize) -> usize {
        if self.whole { return at; }
        shaped(self.isolates, at.clamp(self.start, self.end) - self.start)
    }

    /// The content byte at its shaped text's byte `at`.
    pub fn content(&self, at: usize) -> usize {
        if self.whole { return at; }
        self.start + content(self.isolates, at)
    }

    /// `spans` over its words, on its shaped text (a whole layout's own).
    pub fn spans<'s>(&self, spans: &'s [Span]) -> Cow<'s, [Span]> {
        if self.whole || spans.is_empty() { return Cow::Borrowed(spans); }
        Cow::Owned(spans_in(spans, self.start, self.end, self.isolates, self.disjoint_spans).collect())
    }

    /// `pills` inside it, on its shaped text.
    pub fn pills<'s>(&self, pills: &'s [PillChip]) -> Cow<'s, [PillChip]> {
        if self.whole || pills.is_empty() { return Cow::Borrowed(pills); }
        Cow::Owned(pills_in(pills, self.start, self.end, self.isolates).collect())
    }

    /// Strikethrough byte ranges over its words, on its shaped text.
    pub fn strikes<'s>(&self, strikes: &'s [(u32, u32, Option<Color>)]) -> Cow<'s, [(u32, u32, Option<Color>)]> {
        if self.whole || strikes.is_empty() { return Cow::Borrowed(strikes); }
        Cow::Owned(strikes.iter().filter_map(|&(a, b, c)| {
            let (a, b) = self.clip(a as usize, b as usize)?;
            Some((a as u32, b as u32, c))
        }).collect())
    }

    /// Overline byte ranges over its words, on its shaped text.
    pub fn overlines<'s>(&self, overlines: &'s [(u32, u32, Option<Color>)]) -> Cow<'s, [(u32, u32, Option<Color>)]> {
        self.strikes(overlines)
    }

    /// [`SpanBoxRun`]s meeting its words, clipped to its paragraph on its shaped text.
    pub fn span_boxes<'s>(&self, span_boxes: &'s [SpanBoxRun]) -> Cow<'s, [SpanBoxRun]> {
        if self.whole || span_boxes.is_empty() { return Cow::Borrowed(span_boxes); }
        Cow::Owned(span_boxes_in(span_boxes, self.start, self.end, self.isolates).collect())
    }

    /// Content bytes `a..b` on its shaped text, when they meet its words.
    pub fn clip(&self, a: usize, b: usize) -> Option<(usize, usize)> {
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
pub(super) fn spans_in<'s>(spans: &'s [Span], a: usize, b: usize, isolates: &'s [(u32, u32)], disjoint: bool) -> impl Iterator<Item = Span> + 's {
    let window = if disjoint && spans.len() > 8 {
        let lo = spans.partition_point(|s| (s.end as usize) <= a);
        let hi = lo + spans[lo..].partition_point(|s| (s.start as usize) < b);
        &spans[lo..hi]
    } else {
        spans
    };
    window.iter().filter(move |s| (s.start as usize) < b && a < s.end as usize).map(move |s| Span {
        start: shaped(isolates, (s.start as usize).max(a) - a) as u32,
        end: shaped(isolates, (s.end as usize).min(b) - a) as u32,
        ..s.clone()
    })
}

/// `pills` inside content bytes `a..b`, each its label's shaped bytes.
pub(super) fn pills_in<'s>(pills: &'s [PillChip], a: usize, b: usize, isolates: &'s [(u32, u32)]) -> impl Iterator<Item = PillChip> + 's {
    pills.iter().filter(move |p| inside(p, a, b)).map(move |p| PillChip {
        start: (shaped(isolates, p.start as usize - a) + ISOLATE) as u32,
        end: (shaped(isolates, p.end as usize - a) - ISOLATE) as u32,
        ..p.clone()
    })
}

pub(super) fn inside(p: &PillChip, a: usize, b: usize) -> bool {
    a <= p.start as usize && p.end as usize <= b && p.start < p.end
}

/// `span_boxes` meeting content bytes `a..b`, clipped to them and moved onto the shaped text.
pub(super) fn span_boxes_in<'s>(span_boxes: &'s [SpanBoxRun], a: usize, b: usize, isolates: &'s [(u32, u32)]) -> impl Iterator<Item = SpanBoxRun> + 's {
    span_boxes.iter().filter(move |sb| (sb.start as usize) < b && a < sb.end as usize && sb.start < sb.end).map(move |sb| {
        let clip_start = (sb.start as usize) < a;
        let clip_end = (sb.end as usize) > b;
        let start = shaped(isolates, (sb.start as usize).max(a) - a) as u32;
        let end = shaped(isolates, (sb.end as usize).min(b) - a) as u32;
        let mut style = sb.style;
        if clip_start || clip_end {
            let pl = if clip_start { 0.0 } else { style.pad_left() };
            let pr = if clip_end { 0.0 } else { style.pad_right() };
            let c = style.corner_radii();
            style = style.pad_x(pl, pr).corners([
                if clip_start { 0.0 } else { c[0] },
                if clip_end { 0.0 } else { c[1] },
                if clip_end { 0.0 } else { c[2] },
                if clip_start { 0.0 } else { c[3] },
            ]);
        }
        SpanBoxRun { start, end, style }
    })
}

/// A run of paragraphs to draw, or a whole layout.
pub struct Pieces<'a> {
    whole: Option<Piece<'a>>,
    paragraphs: std::iter::Enumerate<std::slice::Iter<'a, Paragraph>>,
    disjoint_spans: bool,
    bottom: f32,
}

impl<'a> Iterator for Pieces<'a> {
    type Item = Piece<'a>;
    fn next(&mut self) -> Option<Piece<'a>> {
        if let Some(w) = self.whole.take() { return Some(w); }
        let (k, p) = self.paragraphs.next()?;
        (p.y <= self.bottom).then(|| Piece::of_paragraph(k, p, self.disjoint_spans))
    }
}

/// A text's layout, borrowed: one paragraph's, or a field's paragraphs. Bytes are the text's
/// content bytes; positions are device px from the text's top-left.
#[derive(Clone, Copy)]
pub enum TextFlow<'a> {
    Single(&'a Layout<Ink>),
    Paragraphs(&'a ParagraphLayout),
}

/// Where a point lands in a text.
pub struct Hit {
    pub index: usize,
    /// Which side of `index` the press was on, where two runs meet.
    pub side: parley::layout::Affinity,
    /// The cluster before a caret that ends a line's text, when it's not the line break.
    pub before: Option<usize>,
}

/// A laid-out line, for paragraph block decorations.
pub struct BlockLine {
    pub top: f32,
    pub bottom: f32,
    pub baseline: f32,
    /// A paragraph ends with it.
    pub last: bool,
}

impl<'a> TextFlow<'a> {
    pub fn width(self) -> f32 {
        match self { TextFlow::Single(l) => l.width(), TextFlow::Paragraphs(p) => p.width }
    }

    pub fn full_width(self) -> f32 {
        match self { TextFlow::Single(l) => l.full_width(), TextFlow::Paragraphs(p) => p.full_width }
    }

    pub fn height(self) -> f32 {
        match self { TextFlow::Single(l) => l.height(), TextFlow::Paragraphs(p) => p.height }
    }

    /// The direction its lines run in, for where an unwrapped line starts: a by-paragraph
    /// field wraps, so each paragraph aligns itself.
    #[doc(hidden)] pub fn flow(self) -> Flow {
        match self { TextFlow::Single(l) => Flow::of_layout(l), TextFlow::Paragraphs(p) => p.paragraphs.first().map_or(Flow::Ltr, |q| q.flow) }
    }

    /// [`stream_height_lag`](super::stream_height_lag) of it.
    pub fn stream_height_lag(self, width: f32, scale: f32) -> f32 {
        match self {
            TextFlow::Single(l) => super::stream_height_lag(l, width, scale),
            TextFlow::Paragraphs(p) => {
                let Some(last) = p.paragraphs.last() else { return 0.0 };
                let Some(line) = last.layout.lines().next_back() else { return 0.0 };
                let multi = p.paragraphs.len() > 1 || last.layout.len() > 1;
                let cap = if multi { 0.82 } else { 0.52 };
                let m = line.metrics();
                let fill = ((m.advance - m.hanging_advance) / width.max(1.0)).clamp(0.0, 1.0);
                let lag = m.line_height / scale * cap * (1.0 - fill);
                (lag * 64.0).round() / 64.0
            }
        }
    }

    /// [`spec_height`](super::spec_height) of it.
    #[doc(hidden)] pub fn spec_height(self, style: TextStyle, scale: f32, exact: bool) -> f32 {
        match self {
            TextFlow::Single(l) => super::spec_height(l, style, scale, exact),
            TextFlow::Paragraphs(p) => {
                let h = p.height.max(style.line_px() * scale);
                if exact { (h * 64.0).round() / 64.0 } else { h.ceil() }
            }
        }
    }

    /// Its pieces that meet device rows `top..bottom`, top to bottom.
    #[doc(hidden)] pub fn pieces_in(self, top: f32, bottom: f32) -> Pieces<'a> {
        match self {
            TextFlow::Single(l) => Pieces { whole: Some(Piece::whole(l)), paragraphs: [].iter().enumerate(), disjoint_spans: false, bottom },
            TextFlow::Paragraphs(p) => {
                let from = p.paragraphs.partition_point(|q| q.y + q.height < top);
                let mut paragraphs = p.paragraphs.iter().enumerate();
                if from > 0 { paragraphs.nth(from - 1); }
                Pieces { whole: None, paragraphs, disjoint_spans: p.disjoint_spans, bottom }
            }
        }
    }

    #[doc(hidden)] pub fn pieces(self) -> Pieces<'a> {
        self.pieces_in(f32::NEG_INFINITY, f32::INFINITY)
    }

    /// The piece holding content byte `at`: a paragraph owns the bytes from its start to its end,
    /// the caret after its last word included.
    #[doc(hidden)] pub fn piece_at(self, at: usize) -> Piece<'a> {
        match self {
            TextFlow::Single(l) => Piece::whole(l),
            TextFlow::Paragraphs(p) => {
                let k = p.paragraphs.partition_point(|q| q.end < at).min(p.paragraphs.len() - 1);
                Piece::of_paragraph(k, &p.paragraphs[k], p.disjoint_spans)
            }
        }
    }

    /// The piece at device row `y`, the first or last past either end.
    fn piece_at_y(self, y: f32) -> Piece<'a> {
        match self {
            TextFlow::Single(l) => Piece::whole(l),
            TextFlow::Paragraphs(p) => {
                let k = p.paragraphs.partition_point(|q| q.y + q.height <= y).min(p.paragraphs.len() - 1);
                Piece::of_paragraph(k, &p.paragraphs[k], p.disjoint_spans)
            }
        }
    }

    /// Where an empty paragraph's caret or newline box, `size` px wide, starts: after its left
    /// indent when reading left to right, or inside its right padding from the wrap width when
    /// reading right to left.
    pub(crate) fn blank_start(self, p: &Piece<'_>, size: f32) -> f32 {
        let indent = if p.index == 0 {
            match self {
                TextFlow::Paragraphs(pl) => pl.text_indent_px,
                TextFlow::Single(_) => 0.0,
            }
        } else {
            0.0
        };
        match (p.flow, self) {
            (Flow::Rtl, TextFlow::Paragraphs(pl)) => (pl.max_width.unwrap_or(pl.width) - p.pad[1] - indent - size).max(p.pad[0]),
            _ => p.pad[0] + indent,
        }
    }

    /// The caret's box at content byte `at`, on `side` of it, `size` px wide.
    pub fn caret(self, at: usize, side: parley::layout::Affinity, size: f32) -> parley::BoundingBox {
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
    #[doc(hidden)] pub fn hit(self, x: f32, y: f32) -> Hit {
        let p = self.piece_at_y(y);
        if p.is_blank() {
            return Hit { index: p.start, side: parley::layout::Affinity::Downstream, before: None };
        }
        let cur = cursor_from_point(p.layout, x, y - p.y);
        let before = cursor::cluster_start_at_point(p.layout, x, y - p.y)
            .map(|b| p.content(b))
            .or_else(|| {
                (cur.affinity() == parley::layout::Affinity::Upstream)
                    .then(|| cur.index().checked_sub(1).and_then(|b| parley::Cluster::from_byte_index(p.layout, b)))
                    .flatten()
                    .filter(|cl| !cl.is_hard_line_break())
                    .map(|cl| p.content(cl.text_range().start))
            });
        Hit { index: p.content(cur.index()), side: cur.affinity(), before }
    }

    /// The first (`!end`) or last (`end`) byte of the visual line holding byte `at`.
    pub fn line_edge(self, at: usize, end: bool) -> usize {
        let p = self.piece_at(at);
        if p.is_blank() { return p.start; }
        p.content(line_edge(p.layout, p.shaped(at), end))
    }

    /// The byte on the visual line above (`!down`) or below (`down`) byte `at`'s, at `goal`'s x
    /// (the column a run of Up and Down keeps) else its own, and that x; the start on the first
    /// line, `None` on the last.
    pub fn move_line(self, at: usize, down: bool, goal: Option<f32>) -> Option<(usize, f32)> {
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
        let t = Piece::of_paragraph(k, &pl.paragraphs[k], pl.disjoint_spans);
        let Some(m) = t.layout.get(line).filter(|_| !t.is_blank()).map(|l| *l.metrics()) else { return Some((t.start, x)) };
        Some((t.content(cursor_from_point(t.layout, x, (m.block_min_coord + m.block_max_coord) * 0.5).index()), x))
    }

    /// Boxes covering content bytes `a..b`, each with its line's baseline (see
    /// [`range_boxes`](super::range_boxes)); a selected line break between paragraphs shows at the
    /// end of the line it ends, on the side its paragraph ends.
    pub fn range_boxes(self, a: usize, b: usize, pill_boxes: bool) -> Vec<(parley::BoundingBox, f32)> {
        let baseline = |l: &Layout<Ink>, i: usize| l.get(i).map_or(0.0, |line| line.metrics().baseline);
        let pl = match self {
            TextFlow::Single(l) => return range_boxes(l, a, b, pill_boxes).into_iter().map(|(bb, i)| (bb, baseline(l, i))).collect(),
            TextFlow::Paragraphs(pl) => pl,
        };
        let mut out = Vec::new();
        if a >= b { return out; }
        let from = pl.paragraphs.partition_point(|q| q.end < a);
        for (k, q) in pl.paragraphs.iter().enumerate().skip(from).take_while(|(_, q)| q.start <= b) {
            let p = Piece::of_paragraph(k, q, pl.disjoint_spans);
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
                let (top, bottom, base, start, end, ws) = match p.layout.lines().last().filter(|_| !p.is_blank()) {
                    Some(line) => {
                        let m = line.metrics();
                        let x = m.inline_min_coord + m.offset;
                        let ws = line.runs().last().map_or(pl.newline as f64, |r| {
                            let fm = r.font_metrics();
                            (fm.ascent as f64 + fm.descent as f64) * 0.25
                        });
                        (m.block_min_coord.min(m.content_block_min_coord), m.block_max_coord.max(m.content_block_max_coord), m.baseline, x, x + m.advance, ws)
                    }
                    None => {
                        let x = self.blank_start(&p, 0.0);
                        (0.0, p.height, pl.blank_box[0], x, x, pl.newline as f64)
                    }
                };
                let (top, bottom) = (top as f64 + y, bottom as f64 + y);
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

    /// Inline font-metric boxes (`[x0, baseline - ascent, x1, baseline + descent]` in device px)
    /// covering content bytes `a..b`, matching Blink's `Range::getBoundingClientRect()`. Falls
    /// back to the line box when no glyph run overlaps the range.
    pub fn inline_range_boxes(self, a: usize, b: usize) -> Vec<parley::BoundingBox> {
        let piece_boxes = |l: &Layout<Ink>, sa: usize, sb: usize, y_off: f64, out: &mut Vec<parley::BoundingBox>| {
            for (bb, line_idx) in range_boxes(l, sa, sb, false) {
                let (mut y0, mut y1) = (bb.y0, bb.y1);
                if let Some(line) = l.get(line_idx) {
                    let base = line.metrics().baseline;
                    let (mut asc, mut desc) = (0.0f32, 0.0f32);
                    for run in line.runs() {
                        let r = run.text_range();
                        if r.start < sb && sa < r.end {
                            let fm = run.font_metrics();
                            (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                        }
                    }
                    if asc > 0.0 || desc > 0.0 {
                        y0 = f64::from(base - asc);
                        y1 = f64::from(base + desc);
                    }
                }
                out.push(parley::BoundingBox::new(bb.x0, y0 + y_off, bb.x1, y1 + y_off));
            }
        };
        let mut out = Vec::new();
        if a >= b {
            return out;
        }
        match self {
            TextFlow::Single(l) => piece_boxes(l, a, b, 0.0, &mut out),
            TextFlow::Paragraphs(pl) => {
                let from = pl.paragraphs.partition_point(|q| q.end < a);
                for (k, q) in pl.paragraphs.iter().enumerate().skip(from).take_while(|(_, q)| q.start <= b) {
                    let p = Piece::of_paragraph(k, q, pl.disjoint_spans);
                    if let Some((sa, sb)) = p.clip(a, b) {
                        piece_boxes(p.layout, sa, sb, p.y as f64, &mut out);
                    }
                }
            }
        }
        out
    }

    /// The span its lines cover across, from the leftmost line start to the rightmost end, or
    /// `None` with no lines.
    pub fn extent(self) -> Option<(f32, f32)> {
        let (lo, hi) = self.pieces().flat_map(|p| p.layout.lines().map(|l| {
            let m = l.metrics();
            (m.inline_min_coord + m.offset, m.advance)
        }))
            .fold((f32::MAX, f32::NEG_INFINITY), |(lo, hi), (start, advance)| (lo.min(start), hi.max(start + advance)));
        (lo <= hi).then_some((lo, hi))
    }

    /// Contiguous vertical extent `(top, bottom)` in device px of all visual lines intersecting
    /// content bytes `start..end`, or `None` when no line intersects it.
    #[doc(hidden)]
    pub fn line_span(self, start: usize, end: usize, line_px: f32) -> Option<(f32, f32)> {
        if start >= end {
            return None;
        }
        let line_bounds = |line: &parley::Line<'_, Ink>| {
            let m = line.metrics();
            let bot = if m.block_max_coord > m.block_min_coord {
                m.block_max_coord
            } else {
                m.block_min_coord + line_px
            };
            (m.block_min_coord, bot)
        };
        match self {
            TextFlow::Single(layout) => {
                let mut span: Option<(f32, f32)> = None;
                for line in layout.lines() {
                    let r = line.text_range();
                    if r.start < end && start < r.end {
                        let (t, b) = line_bounds(&line);
                        span = Some(match span {
                            Some((lo, hi)) => (lo.min(t), hi.max(b)),
                            None => (t, b),
                        });
                    }
                }
                span
            }
            TextFlow::Paragraphs(pl) => {
                let lo = pl.paragraphs.partition_point(|q| q.end < start || (q.end == start && q.start < q.end));
                let hi = pl.paragraphs.partition_point(|q| q.start < end);
                if lo >= hi {
                    return None;
                }
                let para_span = |k: usize, q: &Paragraph| -> (f32, f32) {
                    if !q.shaped || q.start == q.end {
                        return (q.y, q.y + q.height);
                    }
                    let piece = Piece::of_paragraph(k, q, pl.disjoint_spans);
                    let Some((sa, sb)) = piece.clip(start, end) else {
                        return (q.y, q.y + q.height);
                    };
                    let mut span: Option<(f32, f32)> = None;
                    for line in piece.layout.lines() {
                        let r = line.text_range();
                        if r.start < sb && sa < r.end {
                            let (t, b) = line_bounds(&line);
                            span = Some(match span {
                                Some((a, c)) => (a.min(t), c.max(b)),
                                None => (t, b),
                            });
                        }
                    }
                    match span {
                        Some((t, b)) => (q.y + t, q.y + b),
                        None => (q.y, q.y + q.height),
                    }
                };
                let (top, bot0) = para_span(lo, &pl.paragraphs[lo]);
                let bot = if hi - 1 == lo { bot0 } else { para_span(hi - 1, &pl.paragraphs[hi - 1]).1 };
                Some((top, bot.max(top)))
            }
        }
    }

    /// Its lines top to bottom, each marked when a paragraph ends with it.
    #[doc(hidden)] pub fn block_lines(self, line_px: f32) -> Vec<BlockLine> {
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
    #[doc(hidden)]
    pub fn layout_text(
        &mut self,
        spec: &TextInput<'_>,
        input: bool,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        prev: Option<&TextLayout>,
    ) -> TextLayout {
        if !spec.by_paragraph {
            return TextLayout::Single(self.layout_for_spec(spec, scale, max_width, align));
        }
        let forced = spec.paragraphs_flow();
        // One paragraph and no pills, as most drafts are: one layout in its own direction.
        if !spec.by_line && spec.pills.is_empty() && spec.span_boxes.is_empty() && !spec.content.contains('\n') {
            let base = forced.or_else(|| flow_of(spec.content, &[])).unwrap_or(spec.plaintext_flow()).base();
            let shape = spec.shape_key.map(|k| hash::directed(k, base));
            return TextLayout::Single(self.layout_keyed(shape, spec.content, spec.spans, spec.blocks, spec.text_indent, &[], &[], spec.style, scale, max_width, align, spec.wrap_anywhere, false, Keep::Cached, base));
        }
        self.follow_language();
        let whole = spec.shape_key.unwrap_or_else(|| spec.fresh_shape_key(scale));
        // Lines with no strong character take the element's flow, so it keys the whole too.
        let whole = if forced.unwrap_or(spec.plaintext_flow()) == Flow::Rtl { whole ^ 0x3c6e_f372_fe94_f82b } else { whole };
        let blocks = spec.blocks;
        let indented = blocks.iter().any(|&b| b != Block::Paragraph).then_some(blocks);
        let pretty = spec.by_line && spec.pretty && !input && max_width.is_some() && !matches!(spec.style.family, Family::Mono | Family::SystemMono);
        let pretty_bit = if pretty { 0x7072_6574_7479_0001 } else { 0 };
        let key = hash::break_key(whole ^ PARAGRAPHS, max_width, align, indented, spec.text_indent) ^ pretty_bit;
        let g = self.generation;
        if let Some(e) = self.paragraphs.get_mut(&key) {
            e.used = g;
            return TextLayout::Paragraphs(e.value.clone());
        }
        // Measuring asks at more than one width: the last text's layout at this one, when it
        // was laid out there too, keeps its paragraphs without breaking them again.
        let prev = match prev {
            Some(TextLayout::Paragraphs(p)) if p.shaped_epoch != self.shaped_epoch => None,
            Some(TextLayout::Paragraphs(p)) if p.max_width == max_width && p.align == align && p.text_indent == spec.text_indent => Some(p.clone()),
            Some(TextLayout::Paragraphs(p)) => {
                let there = hash::break_key(p.whole ^ PARAGRAPHS, max_width, align, indented, spec.text_indent) ^ pretty_bit;
                Some(self.paragraphs.get(&there).map_or_else(|| p.clone(), |e| e.value.clone()))
            }
            _ => None,
        };
        // This text laid out at another width measuring asked for this commit: a paragraph
        // neither keeps is broken again from it rather than shaped twice.
        let other = self.last_paragraphs.clone().filter(|o| o.whole == whole && (o.max_width != max_width || o.align != align || o.text_indent != spec.text_indent));
        let (built, fresh) = self.lay_out_paragraphs(spec, input, scale, max_width, align, (whole, key), prev.as_deref(), other.as_deref());
        let built = Rc::new(built);
        self.last_paragraphs = Some(built.clone());
        // Count LAYOUT_BASE (32 * LAYOUT_PER_BYTE) once per shaped line, not once per entry.
        let shaped_n = built.paragraphs.iter().filter(|p| p.shaped).count();
        let cost = fresh + shaped_n.saturating_sub(1) * 32 + built.paragraphs.len() * size_of::<Paragraph>() / 64;
        self.paragraphs.insert(key, Entry::new(built.clone(), g, cost, whole));
        TextLayout::Paragraphs(built)
    }

    /// Lays out each of `spec`'s paragraphs, keeping `prev`'s layout of each it still has: those
    /// before an edit and after it. A paragraph is the same when its words, the spans and pills
    /// over them that shape them, its block and its direction are.
    #[allow(clippy::too_many_arguments)]
    fn lay_out_paragraphs(&mut self, spec: &TextInput<'_>, input: bool, scale: f32, max_width: Option<f32>, align: Alignment, (whole, cache_key): (u64, u64), prev: Option<&ParagraphLayout>, other: Option<&ParagraphLayout>) -> (ParagraphLayout, usize) {
        let (content, style) = (spec.content, spec.style);
        let style_key = {
            let mut h = hash::KeyHasher::default();
            (hash::text_hash("", &[], style, scale, None), spec.wrap_anywhere).hash(&mut h);
            h.finish()
        };
        // A plain or colour-only text's edit keys only the paragraphs it touched: those before it
        // and after it keep their keys (`reuse_keys`), so a keystroke or recolour costs no full scan.
        let layout_spans: Vec<Span> = span::layout_spans(spec.spans).cloned().collect();
        let layout_disjoint = layout_spans.len() <= 1 || layout_spans.windows(2).all(|w| w[0].start < w[0].end && w[0].end <= w[1].start);
        let no_pills_or_blocks = spec.pills.is_empty() && spec.span_boxes.is_empty() && spec.text_indent == 0.0 && spec.blocks.iter().all(|&b| b == Block::Paragraph);
        let plain = layout_spans.is_empty() && no_pills_or_blocks;
        let disjoint_spans = spec.spans.len() <= 1 || spec.spans.windows(2).all(|w| w[0].start < w[0].end && w[0].end <= w[1].start);
        let forced = spec.paragraphs_flow();
        let start_flow = forced.unwrap_or(spec.plaintext_flow());
        let reused = prev.filter(|_| plain).and_then(|p| reuse_keys(p, content, style_key, start_flow, &mut self.last_diff));
        let (mut found, tail, entering) = reused.map_or((Vec::new(), Vec::new(), start_flow), |r| (r.head, r.tail, r.entering));
        let bytes = content.as_bytes();
        let at = found.last().map_or(0, |f| next_line_start(bytes, f.1));
        // A long paste's plain ASCII lines that fit on one line, away from the edit's ends, wait
        // as runs: one record each, at their exact height, for `fill_paragraphs`.
        let edited = tail.first().map_or(content.len(), |t| t.0).saturating_sub(at);
        // A min-content measure of a text whose lines wait keys an edit's lines into runs too,
        // however few: none of them is shown, and its run held them as one record.
        let measuring = prev.is_some_and(|p| p.waiting()) && max_width.is_some_and(|m| m < style.line_px() * scale);
        let can_defer = no_pills_or_blocks && (!spec.by_line || (matches!(style.family, Family::Mono | Family::SystemMono)
            && align == Alignment::Start && start_flow == Flow::Ltr && forced != Some(Flow::Rtl)));
        let ascii = (can_defer && (measuring || edited > LAZY_MIN * 8 && count_newlines(&bytes[at..at + edited]) > LAZY_MIN))
            .then(|| self.ascii_line(style, scale, spec.wrap_anywhere)).flatten()
            .filter(|a| !spec.by_line || a.fixed_pitch);
        let (eager_s, eager_e) = if spec.by_line { (48, 48) } else { (EAGER_START, EAGER_END) };
        let runs = ascii.as_deref().map(|m| Runs { ascii: m, max_width, edit: true, eager: (eager_s, eager_e) });
        match tail.first().map(|t| t.0) {
            Some(t) => {
                let sep = if t >= 2 && bytes.get(t - 2) == Some(&b'\r') { 2 } else { 1 };
                key_paragraphs(spec, &layout_spans, style_key, layout_disjoint, at, t - sep, start_flow, forced, runs, &mut self.line_scan, &mut found);
                // The paragraphs after the edit keep their keys if they start from the direction
                // they were keyed in (a neutral one takes the one before it).
                if found.last().map(|f| f.2) == Some(entering) {
                    found.extend(tail);
                } else {
                    key_paragraphs(spec, &layout_spans, style_key, layout_disjoint, t, content.len(), start_flow, forced, None, &mut self.line_scan, &mut found);
                }
            }
            None => key_paragraphs(spec, &layout_spans, style_key, layout_disjoint, at, content.len(), start_flow, forced, runs, &mut self.line_scan, &mut found),
        }
        // What the edit left alone: the paragraphs before it, and those after it.
        let old = prev.map_or(&[][..], |p| p.paragraphs.as_slice());
        let n = found.len();
        let before = found.iter().zip(old).take_while(|(f, o)| f.3 == o.key).count();
        let after = found[before..].iter().rev().zip(old[before..].iter().rev()).take_while(|(f, o)| f.3 == o.key).count();
        let rebreak = prev.is_some_and(|p| p.max_width != max_width || p.align != align);
        let rebreak_first = prev.is_some_and(|p| p.text_indent != spec.text_indent);
        let line = style.line_px() * scale;
        // Each record found with the one the last layout had for it. A waiting one broken at
        // another width is keyed again there: its lines may not all fit any more.
        let mut items: Vec<(Found, Option<&Paragraph>)> = Vec::with_capacity(n);
        for (k, f) in found.into_iter().enumerate() {
            let kept = if k < before { Some(&old[k]) } else if k >= n - after { Some(&old[old.len() - (n - k)]) } else { None };
            match kept {
                Some(o) if !o.shaped && rebreak => {
                    let m = can_defer.then(|| ascii.clone().or_else(|| self.ascii_line(style, scale, spec.wrap_anywhere).filter(|a| !spec.by_line || a.fixed_pitch))).flatten();
                    let mut again = Vec::new();
                    let from = items.last().map_or(start_flow, |(f, _)| f.2);
                    key_paragraphs(spec, &layout_spans, style_key, layout_disjoint, f.0, f.1, from, forced, m.as_deref().map(|m| Runs { ascii: m, max_width, edit: false, eager: (eager_s, eager_e) }), &mut self.line_scan, &mut again);
                    items.extend(again.into_iter().map(|f| (f, None)));
                }
                _ => items.push((f, kept)),
            }
        }
        let pretty = spec.by_line && spec.pretty && !input;
        let pretty_active = pretty && max_width.is_some() && !matches!(style.family, Family::Mono | Family::SystemMono);
        let mut waiting = 0;
        let (mut paragraphs, mut y) = (Vec::with_capacity(items.len()), 0.0f64);
        let (mut width, mut full_width, mut fresh) = (0.0f32, 0.0f32, 0);
        for (k, ((a, b, flow, key, isolates, run), kept)) in items.into_iter().enumerate() {
            let block = spec.blocks.get(k).copied().unwrap_or(Block::Paragraph);
            let pad = block.pad_px(style.size, scale);
            let blocks: &[Block] = if block == Block::Paragraph { &[] } else { std::slice::from_ref(&block) };
            // A waiting record kept at the same width stays as it was; a run found here waits.
            let wait = match (kept, run) {
                (Some(o), _) if !o.shaped && can_defer => ascii.clone().or_else(|| self.ascii_line(style, scale, spec.wrap_anywhere).filter(|a| !spec.by_line || a.fixed_pitch))
                    .map(|m| (m.height, o.height, o.widths[0], o.lines, o.layout.clone(), o.run)),
                (None, Some((height, w, lines))) => ascii.clone().or_else(|| self.ascii_line(style, scale, spec.wrap_anywhere).filter(|a| !spec.by_line || a.fixed_pitch))
                    .map(|m| (m.height, height, w, lines, m.empty.clone(), true)),
                _ => None,
            };
            if let Some((line_h, height, w, lines, layout, run)) = wait {
                waiting += 1;
                (width, full_width) = (width.max(w), full_width.max(w));
                paragraphs.push(Paragraph { start: a, end: b, y: y.round() as f32, y_frac: 0.0, height, widths: [w, w], pad, flow, key, isolates, layout, shaped: false, lines, run });
                for _ in 0..lines { y += f64::from(line_h); }
                continue;
            }
            let para_indent = if k == 0 { spec.text_indent } else { 0.0 };
            let para_rebreak = rebreak || (k == 0 && rebreak_first);
            // The same paragraph at the other width (the same text, so the same index).
            let twin = other.and_then(|o| o.paragraphs.get(k)).filter(|o| !pretty_active && o.key == key && o.shaped);
            let kept = kept.filter(|_| !(para_rebreak && pretty_active));
            // A kept paragraph broken at another width, or this text's own paragraph at the
            // other width measuring asks for, breaks again here rather than shaping anew.
            let again = match kept { Some(o) if para_rebreak => Some(o), Some(_) => None, None => twin };
            let mut layout = match (kept, again) {
                (_, Some(o)) => {
                    fresh += b - a;
                    let mut l = o.layout.clone();
                    line_break::rebreak(Rc::make_mut(&mut l), blocks, para_indent, style.size, scale, max_width, align);
                    // Re-broken, in place or as a copy: the lines it draws now are new.
                    self.layout_ids.build(&l);
                    l
                }
                (Some(o), None) => o.layout.clone(),
                (None, None) => {
                    fresh += b - a;
                    let words = &content[a..b];
                    let text: Cow<str> = if isolates.is_empty() { Cow::Borrowed(words) } else { Cow::Owned(with_isolates(words, &isolates)) };
                    let spans: Vec<Span> = spans_in(&layout_spans, a, b, &isolates, layout_disjoint).collect();
                    let pills: Vec<PillChip> = pills_in(spec.pills, a, b, &isolates).collect();
                    let span_boxes: Vec<SpanBoxRun> = span_boxes_in(spec.span_boxes, a, b, &isolates).collect();
                    self.layout_keyed(Some(key), &text, &spans, blocks, para_indent, &pills, &span_boxes, style, scale, max_width, align, spec.wrap_anywhere, pretty, Keep::Owned, flow.base())
                }
            };
            // A paragraph kept as it was keeps its measures too: touching 2,000 layouts to read
            // them again costs more than the edit.
            let (mut height, mut widths) = match (kept, again) {
                (Some(o), None) => (o.height, o.widths),
                _ => (if a == b { layout.height().max(line) } else { layout.height() }, [layout.width(), layout.full_width()]),
            };
            let want_frac = if layout.len() > 1 { (y - y.floor()) as f32 } else { 0.0 };
            let have_frac = match (kept, again) { (Some(o), None) => o.y_frac, _ => 0.0 };
            if (want_frac - have_frac).abs() > 1e-5 {
                line_break::rebreak_at_y(Rc::make_mut(&mut layout), blocks, para_indent, style.size, scale, max_width, align, f64::from(want_frac));
                self.layout_ids.build(&layout);
                (height, widths) = (if a == b { layout.height().max(line) } else { layout.height() }, [layout.width(), layout.full_width()]);
            }
            let py = if want_frac != 0.0 { y.floor() as f32 } else { y.round() as f32 };
            (width, full_width) = (width.max(widths[0]), full_width.max(widths[1]));
            paragraphs.push(Paragraph { start: a, end: b, y: py, y_frac: want_frac, height, widths, pad, flow, key, isolates, layout, shaped: true, lines: 1, run: false });
            y += f64::from(height);
        }
        let (asc, desc) = self.primary_extents(style.family, style.weight, style.italic).unwrap_or((0.8, 0.2));
        let (asc_px, desc_px) = (asc * style.size * scale, desc.abs() * style.size * scale);
        let newline = 0.25 * (asc_px + desc_px);
        let (qa, qd) = (asc_px.round(), desc_px.round());
        let blank_box = [qa + ((line - (qa + qd)) * 0.5).floor(), asc_px, desc_px];
        // One copy of a long text for both widths a measure asks for, not one each.
        let shared = self.last_paragraphs.as_ref().and_then(|l| l.keyed.as_ref()).map(|k| k.content.clone()).filter(|c| **c == *content);
        let keyed = plain.then(|| Keyed { content: shared.unwrap_or_else(|| Rc::from(content)), style_key, start_flow });
        let lazy = (waiting > 0).then(|| self.ascii_line(style, scale, spec.wrap_anywhere)).flatten().map(|ascii| Rc::new(Lazy {
            content: keyed.as_ref().map_or_else(|| Rc::from(content), |k| k.content.clone()),
            spans: if layout_spans.is_empty() { Rc::from(&[][..]) } else { Rc::from(layout_spans.as_slice()) },
            disjoint_spans: layout_disjoint,
            style, scale, wrap_anywhere: spec.wrap_anywhere, cache_key, style_key, forced, start_flow, ascii,
        }));
        let text_indent_px = line_break::indent_px(spec.text_indent, scale);
        (ParagraphLayout { paragraphs, width, full_width, height: y as f32, max_width, align, text_indent: spec.text_indent, text_indent_px, whole, shaped_epoch: self.shaped_epoch, newline, blank_box, disjoint_spans, keyed, lazy }, fresh)
    }
}
