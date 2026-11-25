//! What an app styles a text element's byte ranges with: spans, links, highlights and pills.

use crate::*;

/// Brush carried through Parley: 0 is unspanned, n is the nth layout-affecting span; glyph
/// colour is resolved from [`TextSystem::set_glyph_inks`] at emission.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ink(pub u32);

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
    pub image: Option<rune_image::pixels::Pixels>,
    pub bg: Option<Color>,
    pub fg: Option<Color>,
    pub ring: Option<(Color, f32)>,
    pub radius: Option<f32>,
    pub bare: Option<Box<BareIcon>>,
}

impl PillChip {
    /// Whether this chip or inline icon holds a Pixels image.
    #[doc(hidden)]
    pub fn has_image(&self) -> bool {
        self.image.is_some() || self.bare.as_ref().is_some_and(|b| b.image.is_some())
    }

    /// Whether a non-bare chip reserves leading room for an icon or image.
    pub(crate) fn has_lead_visual(&self) -> bool {
        self.icon.is_some() || self.image.is_some()
    }
}

/// An inline icon's room and look in points (see `InlineIcon`).
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct BareIcon {
    pub size: f32,
    pub gap: f32,
    pub radius: f32,
    pub image: Option<rune_image::pixels::Pixels>,
    /// Set, the image takes the place of its pill's label instead of sitting before it.
    pub replace: Option<Replace>,
    /// Its name for assistive technology (`InlineIcon::label`).
    pub label: Option<String>,
    /// Its tooltip (`InlineIcon::help`).
    pub help: Option<String>,
}

/// An image set on the baseline in place of a pill's label (see `InlineImage`): `width`
/// points wide, `ascent` points above the baseline and `descent` below it. The label still
/// holds its bytes, for selection and copy, but takes no room and draws nothing. The image
/// draws `bleed` points past each edge of that box.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Replace {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub bleed: f32,
}

/// The font size a replaced label shapes at in device px: small enough to take no room.
pub(crate) const HIDDEN_SIZE: f32 = 1e-3;

/// Whether a glyph run shaped at `font_size` device px is a replaced label, which draws nothing.
pub(crate) fn is_hidden_size(font_size: f32) -> bool {
    font_size <= HIDDEN_SIZE * 2.0
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

/// Tags the inline boxes around padded [`SpanBox`] ranges: the rest of the id is the box's index
/// shifted left once, with the low bit set on the trailing spacer.
pub(crate) const SPAN_BOX: u64 = 1 << 61;

/// Tags in-flow element [`InlineBox`] slots: the lower bits carry the index into the slice of
/// [`InlineBoxRun`]s passed to shaping.
#[doc(hidden)]
pub const INLINE_BOX: u64 = 1 << 60;

/// Vertical alignment of an [`InlineBox`] within its text line.
///
/// Follows CSS Inline Layout Level 3 `vertical-align` for [`Baseline`](Self::Baseline),
/// [`Middle`](Self::Middle), [`TextTop`](Self::TextTop), and [`TextBottom`](Self::TextBottom),
/// with [`TextCenter`](Self::TextCenter) centring the box on the font's ascent/descent box so
/// taller boxes grow line height symmetrically.
///
/// ```
/// use rune_text::{InlineAlign, InlineBox};
///
/// let slot = InlineBox::new(72.0, 18.0).align(InlineAlign::TextCenter);
/// assert_eq!(slot, InlineBox::new(72.0, 18.0).align(InlineAlign::TextCenter));
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum InlineAlign {
    /// Aligns the box's baseline (its bottom edge, or [`InlineBox::baseline`] points below its
    /// top) with the text line's baseline (CSS `vertical-align: baseline`).
    #[default]
    Baseline,
    /// Centres the box vertically between the primary font's ascent and descent so boxes taller
    /// than the line grow line height symmetrically above and below the text.
    TextCenter,
    /// Aligns the box's vertical midpoint with the baseline plus half the parent font's x-height
    /// (CSS `vertical-align: middle`).
    Middle,
    /// Aligns the box's top edge with the parent font's ascent (CSS `vertical-align: text-top`).
    TextTop,
    /// Aligns the box's bottom edge with the parent font's descent (CSS `vertical-align: text-bottom`).
    TextBottom,
}

/// Fixed-size room reserved inline in a text run (an atomic CSS `display: inline-block` slot).
///
/// Pass to `El::inline` to lay out a child element inside wrapped text lines, or to
/// [`measure_inline`](crate::measure_inline) / [`measure_inline_with`](crate::measure_inline_with)
/// to measure the resulting paragraph width and height ahead of layout.
///
/// ```
/// use rune_text::{InlineAlign, InlineBox};
///
/// let badge = InlineBox::new(72.0, 18.0).align(InlineAlign::TextCenter);
/// let math = InlineBox::new(24.0, 16.0).baseline(12.0);
/// assert_ne!(badge, math);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineBox {
    width: f32,
    height: f32,
    align: InlineAlign,
    baseline: Option<f32>,
}

impl InlineBox {
    /// Reserves a `width` by `height` box in points, aligned on the [`InlineAlign::Baseline`].
    /// Negative or non-finite dimensions clamp to `0.0`.
    pub const fn new(width: f32, height: f32) -> Self {
        Self {
            width: nonneg_pt(width),
            height: nonneg_pt(height),
            align: InlineAlign::Baseline,
            baseline: None,
        }
    }

    /// Sets how the box aligns vertically within its line ([`InlineAlign::Baseline`] by default).
    pub const fn align(mut self, align: InlineAlign) -> Self {
        self.align = align;
        self
    }

    /// Places the box's baseline `points` below its top edge when aligned with
    /// [`InlineAlign::Baseline`] (defaults to the box's bottom edge). Negative or non-finite
    /// values clamp to `0.0`.
    pub const fn baseline(mut self, points: f32) -> Self {
        self.baseline = Some(nonneg_pt(points));
        self
    }

    #[doc(hidden)]
    pub const fn width(&self) -> f32 {
        self.width
    }

    #[doc(hidden)]
    pub const fn height(&self) -> f32 {
        self.height
    }

    #[doc(hidden)]
    pub const fn alignment(&self) -> InlineAlign {
        self.align
    }

    #[doc(hidden)]
    pub const fn baseline_offset(&self) -> Option<f32> {
        self.baseline
    }
}

/// An [`InlineBox`] attached before UTF-8 byte offset `at` of a text element, bound to child
/// ordinal `child`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineBoxRun {
    pub at: u32,
    pub slot: InlineBox,
    pub child: u32,
}

const fn nonneg_pt(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// Style for a padded inline box drawn around a UTF-8 byte range of static text via
/// `El::span_box` or `El::span_boxes`.
///
/// Unlike [`Span::bg`], horizontal padding (`pad` / `pad_x`) reserves layout room in the line so
/// adjacent inline chips (such as a search filter's `from:` key and `ada` value) meet without
/// overlapping, and [`Self::corners`] sets per-corner radii `[top_left, top_right, bottom_right,
/// bottom_left]` in CSS `border-radius` order (matching `El::corners`).
///
/// ```
/// use rune_core::color::Color;
/// use rune_text::SpanBox;
///
/// let key = SpanBox::new()
///     .bg(Color { r: 0.2, g: 0.2, b: 0.25, a: 1.0 })
///     .pad_x(6.0, 4.0)
///     .corners([4.0, 0.0, 0.0, 4.0]);
/// let val = SpanBox::new()
///     .bg(Color { r: 0.15, g: 0.35, b: 0.65, a: 1.0 })
///     .pad_x(4.0, 6.0)
///     .corners([0.0, 4.0, 4.0, 0.0])
///     .border(Color { r: 0.4, g: 0.6, b: 1.0, a: 0.8 }, 1.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanBox {
    bg: Option<Color>,
    pad_left: f32,
    pad_right: f32,
    corners: [f32; 4],
    border: Option<(Color, f32)>,
}

impl Default for SpanBox {
    fn default() -> Self {
        Self::new()
    }
}

impl SpanBox {
    /// An empty box with no fill, no border, zero horizontal padding, and the default
    /// [`SPAN_RADIUS`](crate::SPAN_RADIUS) corner radius on all four corners.
    pub const fn new() -> Self {
        Self {
            bg: None,
            pad_left: 0.0,
            pad_right: 0.0,
            corners: [crate::SPAN_RADIUS; 4],
            border: None,
        }
    }

    /// Fill colour drawn behind the range and its horizontal padding.
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Symmetric layout-affecting horizontal padding in points on both sides of the range.
    /// Negative or non-finite values become `0.0`.
    pub const fn pad(mut self, pad: f32) -> Self {
        let p = nonneg_pt(pad);
        self.pad_left = p;
        self.pad_right = p;
        self
    }

    /// Asymmetric layout-affecting horizontal padding in points: `left` at the range's start and
    /// `right` at its end (in RTL text, leading and trailing inline padding). Negative or
    /// non-finite values become `0.0`.
    pub const fn pad_x(mut self, left: f32, right: f32) -> Self {
        self.pad_left = nonneg_pt(left);
        self.pad_right = nonneg_pt(right);
        self
    }

    /// Uniform corner radius in points for all four corners. Negative or non-finite values become
    /// `0.0`. Clamped to half the box's shorter side at paint time.
    pub const fn radius(mut self, radius: f32) -> Self {
        let r = nonneg_pt(radius);
        self.corners = [r; 4];
        self
    }

    /// Per-corner radii in points: `[top_left, top_right, bottom_right, bottom_left]`, matching
    /// CSS `border-radius` order and `El::corners`. Negative or non-finite values become `0.0`.
    pub const fn corners(mut self, corners: [f32; 4]) -> Self {
        self.corners = [
            nonneg_pt(corners[0]),
            nonneg_pt(corners[1]),
            nonneg_pt(corners[2]),
            nonneg_pt(corners[3]),
        ];
        self
    }

    /// Inset border ring `width` points wide inside the box edges (ignored when `width` is not
    /// finite and positive).
    pub const fn border(mut self, color: Color, width: f32) -> Self {
        self.border = if width > 0.0 && width.is_finite() { Some((color, width)) } else { None };
        self
    }

    #[doc(hidden)]
    pub const fn bg_color(&self) -> Option<Color> {
        self.bg
    }

    #[doc(hidden)]
    pub const fn pad_left(&self) -> f32 {
        self.pad_left
    }

    #[doc(hidden)]
    pub const fn pad_right(&self) -> f32 {
        self.pad_right
    }

    #[doc(hidden)]
    pub const fn corner_radii(&self) -> [f32; 4] {
        self.corners
    }

    #[doc(hidden)]
    pub const fn border_ring(&self) -> Option<(Color, f32)> {
        self.border
    }

    #[doc(hidden)]
    pub const fn has_padding(&self) -> bool {
        self.pad_left > 0.0 || self.pad_right > 0.0
    }

    #[doc(hidden)]
    pub const fn is_painted(&self) -> bool {
        self.bg.is_some() || self.border.is_some()
    }

    #[doc(hidden)]
    pub const fn has_effect(&self) -> bool {
        self.is_painted() || self.has_padding()
    }
}

/// A [`SpanBox`] attached to UTF-8 byte range `start..end`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanBoxRun {
    pub start: u32,
    pub end: u32,
    pub style: SpanBox,
}

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
/// `El::link` and handle with `El::on_link`. Its resting
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
    pub href: Option<String>,
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
            href: None,
            role: "link",
            label: None,
            hover: LinkPaint::default(),
            pressed: None,
            focused: None,
            help: None,
        }
    }
    /// Real address behind this link (such as an `http:`, `https:` or `mailto:` URL) when `id`
    /// is an internal key rather than a URL. Used by Rune's default link context menu
    /// (`Copy Link`) and accessibility metadata; an empty string sets none.
    ///
    /// ```
    /// use rune_text::Link;
    ///
    /// let link = Link::new(0..4, "doc-42").href("https://example.com/42");
    /// assert_eq!(link.href.as_deref(), Some("https://example.com/42"));
    /// ```
    pub fn href(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.href = (!url.is_empty()).then_some(url);
        self
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
    /// A tooltip for this link alone, as `El::help` is for an element:
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
    /// Optional key for `Tx::scroll_into_view`, `Cx::layout` and `Runtime::find`.
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
