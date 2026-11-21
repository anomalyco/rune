//! What an app styles a text element's byte ranges with: spans, links, highlights and pills.

use crate::*;

/// Brush carried through Parley: 0 is unspanned, n is the nth layout-affecting span; glyph
/// colour is resolved from [`TextSystem::set_glyph_inks`] at emission.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ink(pub u16);

/// A styled byte range of a text element (markdown inline code, bold, links).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub weight: Option<f32>,
    pub mono: bool,
    pub size: Option<f32>,
    pub color: Option<Color>,
    /// Rounded background behind the span's runs (inline code), padded by `bg_pad` points.
    pub bg: Option<Color>,
    pub bg_pad: f32,
    pub underline: Option<Color>,
    /// The family's italic face, or a synthetic oblique when it has none.
    pub italic: bool,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start: start as u32, end: end as u32, ..Default::default() }
    }

    pub fn weight(mut self, weight: f32) -> Span {
        self.weight = Some(weight);
        self
    }

    /// Weight 600.
    pub fn bold(mut self) -> Span {
        self.weight = Some(600.0);
        self
    }

    pub fn italic(mut self) -> Span {
        self.italic = true;
        self
    }

    pub fn mono(mut self) -> Span {
        self.mono = true;
        self
    }

    pub fn size(mut self, size: f32) -> Span {
        self.size = Some(size);
        self
    }

    pub fn color(mut self, color: Color) -> Span {
        self.color = Some(color);
        self
    }

    /// Whether this span changes shaping, line metrics or painted decorations (`decorations.rs`),
    /// rather than glyph ink alone.
    pub(crate) fn affects_layout(&self) -> bool {
        self.weight.is_some()
            || self.mono
            || self.size.is_some()
            || self.italic
            || self.bg.is_some()
            || self.underline.is_some()
    }

    /// What [`Self::affects_layout`] depends on, hashed into a layout's key.
    pub(crate) fn hash_layout(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        (
            self.start,
            self.end,
            self.mono,
            self.weight.map(f32::to_bits),
            self.size.map(f32::to_bits),
            self.italic,
            self.bg.is_some(),
            self.bg.map(|_| self.bg_pad.to_bits()),
            self.underline.is_some(),
        )
            .hash(h);
    }

    /// A rounded `color` background padded by `pad` points, as inline code draws.
    pub fn bg(mut self, color: Color, pad: f32) -> Span {
        self.bg = Some(color);
        self.bg_pad = pad;
        self
    }

    pub fn underline(mut self, color: Color) -> Span {
        self.underline = Some(color);
        self
    }
}

/// Non-empty spans in `spans` that change shaping, line metrics or painted decorations.
/// Shared by the shaping key, Parley `Brush` ordinals, `decorations_into` and `synthetic_bold`,
/// so a span gets a brush ordinal if and only if it entered the shaping key.
pub(crate) fn layout_spans(spans: &[Span]) -> impl Iterator<Item = &Span> {
    spans.iter().filter(|s| s.start < s.end && s.affects_layout())
}

/// An atomic pill drawn as a chip: byte range `start..end` of the content, and its leading icon.
/// A `bare` one is an [`InlineIcon`](crate::InlineIcon): no chip, only the icon before the
/// word it keeps to its line.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PillChip {
    pub start: u32,
    pub end: u32,
    pub icon: Option<&'static Icon>,
    pub bg: Option<Color>,
    pub fg: Option<Color>,
    pub bare: Option<Box<BareIcon>>,
}

/// An inline icon's room and look in points (see `InlineIcon`).
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct BareIcon {
    pub size: f32,
    pub gap: f32,
    pub radius: f32,
    pub image: Option<rune_image::pixels::Pixels>,
}

/// A pill chip's horizontal geometry in points for text of `size` points: the room before its
/// label (outer margin, padding, then any icon and its gap) and after it (padding, margin).
#[doc(hidden)]
pub struct PillMetrics {
    pub margin: f32,
    pub pad: f32,
    pub icon: f32,
    pub icon_gap: f32,
}

impl PillMetrics {
    pub fn new(size: f32) -> Self {
        Self { margin: (size * 0.12).max(1.5), pad: (size * 0.36).max(3.0), icon: (size * 0.95).round(), icon_gap: (size * 0.22).max(2.0) }
    }

    /// Room before the label, inside the chip and out.
    pub fn lead(&self, icon: bool) -> f32 {
        self.margin + self.pad + if icon { self.icon + self.icon_gap } else { 0.0 }
    }

    pub fn trail(&self) -> f32 {
        self.pad + self.margin
    }
}

/// Tags the inline boxes around pill labels: the rest of the id is the pill's index shifted
/// left once, with the low bit set on the box after the label.
pub(crate) const PILL_BOX: u64 = 1 << 62;

/// Flattens overlapping [`Span`]s into non-overlapping elementary intervals so that Parley's
/// single `Brush(Ink)` index per glyph run carries the combined colour, background, underline,
/// weight, italic, and monospace attributes of all overlapping spans.
#[doc(hidden)]
pub fn flatten_spans(spans: &[Span]) -> Vec<Span> {
    if spans.len() <= 1
        || spans
            .windows(2)
            .all(|w| w[0].start < w[0].end && w[0].end <= w[1].start)
    {
        return spans.iter().filter(|s| s.start < s.end).cloned().collect();
    }
    let mut bounds = Vec::with_capacity(spans.len() * 2);
    let mut order: Vec<usize> = Vec::with_capacity(spans.len());
    for (i, s) in spans.iter().enumerate() {
        if s.start < s.end {
            bounds.push(s.start);
            bounds.push(s.end);
            order.push(i);
        }
    }
    bounds.sort_unstable();
    bounds.dedup();
    order.sort_by_key(|&i| spans[i].start);
    // Sweep the elementary intervals, keeping the spans that cover the current one in input
    // order so later spans still win, without rescanning every span per interval.
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut out: Vec<Span> = Vec::with_capacity(bounds.len());
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        active.retain(|&i| spans[i].end > a);
        while next < order.len() && spans[order[next]].start <= a {
            let i = order[next];
            let at = active.partition_point(|&j| j < i);
            active.insert(at, i);
            next += 1;
        }
        let mut merged: Option<Span> = None;
        for &i in &active {
            let s = &spans[i];
            let m = merged.get_or_insert_with(|| Span::new(a as usize, b as usize));
            if s.weight.is_some() {
                m.weight = s.weight;
            }
            m.mono |= s.mono;
            if s.size.is_some() {
                m.size = s.size;
            }
            if s.color.is_some() {
                m.color = s.color;
            }
            if s.bg.is_some() {
                m.bg = s.bg;
                m.bg_pad = s.bg_pad;
            }
            if s.underline.is_some() {
                m.underline = s.underline;
            }
            m.italic |= s.italic;
        }
        if let Some(m) = merged {
            if let Some(last) = out.last_mut()
                && last.end == m.start
                && last.weight == m.weight
                && last.mono == m.mono
                && last.size == m.size
                && last.color == m.color
                && last.bg == m.bg
                && last.bg_pad == m.bg_pad
                && last.underline == m.underline
                && last.italic == m.italic
            {
                last.end = m.end;
            } else {
                out.push(m);
            }
        }
    }
    out
}

/// A pressable byte range of a text element: a link, an @mention, a file chip. Attach with
/// [`El::link`](crate::El::link) and handle with [`El::on_link`](crate::El::on_link). Its resting
/// look is whatever [`Span`]s cover it; [`LinkPaint`]s add the hover, pressed and focus looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    #[doc(hidden)]
    pub start: u32,
    #[doc(hidden)]
    pub end: u32,
    #[doc(hidden)]
    pub id: String,
    #[doc(hidden)]
    pub role: &'static str,
    #[doc(hidden)]
    pub label: Option<String>,
    #[doc(hidden)]
    pub hover: LinkPaint,
    #[doc(hidden)]
    pub pressed: Option<LinkPaint>,
    #[doc(hidden)]
    pub focused: Option<LinkPaint>,
    #[doc(hidden)]
    pub help: Option<String>,
}

impl Link {
    /// Bytes `range` of the text, reported to `on_link` as `id`. Its role is "link".
    pub fn new(range: std::ops::Range<usize>, id: impl Into<String>) -> Link {
        Link {
            start: range.start as u32,
            end: range.end as u32,
            id: id.into(),
            role: "link",
            label: None,
            hover: LinkPaint::default(),
            pressed: None,
            focused: None,
            help: None,
        }
    }
    /// Accessibility role, such as "button" for a file chip.
    pub fn role(mut self, role: &'static str) -> Self {
        self.role = role;
        self
    }
    /// Accessible name; the range's text by default.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// The look while the pointer is over it.
    pub fn hover(mut self, paint: LinkPaint) -> Self {
        self.hover = paint;
        self
    }
    /// The look while it is held down; the hover look by default.
    pub fn pressed(mut self, paint: LinkPaint) -> Self {
        self.pressed = Some(paint);
        self
    }
    /// The look while it has keyboard focus; the hover look by default, or an underline in
    /// its text colour when that is empty.
    pub fn focused(mut self, paint: LinkPaint) -> Self {
        self.focused = Some(paint);
        self
    }
    /// A tooltip for this link alone, as [`El::help`](crate::El::help) is for an element:
    /// shown after the hover delay while the pointer rests on the link's glyphs, and hidden
    /// by a press, scroll, key or the pointer leaving them. An empty string sets none.
    pub fn help(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.help = (!text.is_empty()).then_some(text);
        self
    }
}

/// A link's look in one state, painted over its resting span style without reshaping. Unset
/// parts keep the span's own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LinkPaint {
    #[doc(hidden)]
    pub color: Option<Color>,
    #[doc(hidden)]
    pub bg: Option<Color>,
    #[doc(hidden)]
    pub ring: Option<(Color, f32)>,
    #[doc(hidden)]
    pub underline: Option<Color>,
}

impl LinkPaint {
    pub fn new() -> LinkPaint {
        LinkPaint::default()
    }
    /// Glyph colour.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// A rounded background behind each line of the range.
    pub fn bg(mut self, c: Color) -> Self {
        self.bg = Some(c);
        self
    }
    /// An inset `c` border ring `width` points wide inside the rounded background on each line.
    pub fn ring(mut self, c: Color, width: f32) -> Self {
        self.ring = (width > 0.0 && width.is_finite()).then_some((c, width));
        self
    }
    pub fn underline(mut self, c: Color) -> Self {
        self.underline = Some(c);
        self
    }
    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.color.is_none() && self.bg.is_none() && self.ring.is_none() && self.underline.is_none()
    }
}

/// A paint-only highlight over `start..end` UTF-8 bytes of a `text` element.
///
/// Drawn from the shaped layout's glyph boxes without splitting runs, so adding, moving or
/// clearing highlights never reshapes text or shifts line breaks.
#[derive(Clone, Debug, PartialEq)]
pub struct TextHighlight {
    #[doc(hidden)]
    pub start: u32,
    #[doc(hidden)]
    pub end: u32,
    #[doc(hidden)]
    pub bg: Color,
    #[doc(hidden)]
    pub fg: Option<Color>,
    #[doc(hidden)]
    pub ring: Option<(Color, f32)>,
    #[doc(hidden)]
    pub pad: f32,
    #[doc(hidden)]
    pub radius: f32,
    #[doc(hidden)]
    pub vpad: f32,
    #[doc(hidden)]
    pub key: Option<String>,
}

impl TextHighlight {
    /// Bytes `range` of the text, painted with a rounded `bg` fill behind each line's glyphs.
    pub fn new(range: std::ops::Range<usize>, bg: Color) -> TextHighlight {
        TextHighlight {
            start: range.start.min(u32::MAX as usize) as u32,
            end: range.end.min(u32::MAX as usize) as u32,
            bg,
            fg: None,
            ring: None,
            pad: 1.5,
            radius: 4.0,
            vpad: 1.0,
            key: None,
        }
    }
    /// Glyph colour over the highlighted range.
    pub fn color(mut self, c: Color) -> Self {
        self.fg = Some(c);
        self
    }
    /// Inset `c` border ring `width` points wide inside the rounded background on each line.
    pub fn ring(mut self, c: Color, width: f32) -> Self {
        self.ring = (width > 0.0 && width.is_finite()).then_some((c, width));
        self
    }
    /// Horizontal padding around each line's background, in points (`1.5` by default).
    pub fn pad(mut self, pad: f32) -> Self {
        self.pad = pad;
        self
    }
    /// Corner radius of each line's background, in points (`4.0` by default; `0.0` for CSS `::highlight`).
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius.max(0.0);
        self
    }
    /// Vertical padding above ascent and below descent on each line, in points (`1.0` by default).
    pub fn vpad(mut self, vpad: f32) -> Self {
        self.vpad = vpad.max(0.0);
        self
    }
    /// Optional key for [`Tx::scroll_into_view`](crate::Tx::scroll_into_view),
    /// [`Cx::layout`](crate::Cx::layout) and [`Runtime::find`](crate::Runtime::find).
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
    /// The UTF-8 byte range of this highlight.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
    /// Returns this highlight with its UTF-8 byte range replaced by `range`.
    pub fn with_range(mut self, range: std::ops::Range<usize>) -> Self {
        self.start = range.start.min(u32::MAX as usize) as u32;
        self.end = range.end.min(u32::MAX as usize) as u32;
        self
    }
}
