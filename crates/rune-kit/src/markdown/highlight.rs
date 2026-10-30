//! Highlighted source ranges: match and current-match paint.

use super::*;

/// How a source byte range in [`Markdown::highlights`] is painted.
///
/// [`Highlight::default()`] (or [`Highlight::new()`]) draws a standard match background from
/// [`MarkdownStyle`]; [`Highlight::current()`] draws the active match with a stronger background
/// and an inset border ring. Either may be customized with [`Highlight::bg`], [`Highlight::fg`],
/// [`Highlight::ring`] and [`Highlight::key`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Highlight {
    pub(crate) current: bool,
    pub(crate) bg: Option<Color>,
    pub(crate) fg: Option<Color>,
    pub(crate) ring: Option<(Color, f32)>,
    pub(crate) key: Option<String>,
}

impl Highlight {
    /// A standard match highlight using the theme's [`MarkdownStyle::highlight_bg`].
    pub fn new() -> Highlight {
        Highlight::default()
    }
    /// An active/current match highlight using the theme's [`MarkdownStyle::highlight_current_bg`]
    /// and [`MarkdownStyle::highlight_current_ring`].
    pub fn current() -> Highlight {
        Highlight { current: true, ..Highlight::default() }
    }
    /// Alias for [`Highlight::current`].
    pub fn active() -> Highlight {
        Highlight::current()
    }
    /// Sets whether this highlight uses the active/current match style.
    pub fn current_match(mut self, current: bool) -> Self {
        self.current = current;
        self
    }
    /// Whether this highlight uses the active/current match style.
    pub fn is_current(&self) -> bool {
        self.current
    }
    /// Overrides the rounded background colour behind the highlighted glyphs.
    pub fn bg(mut self, c: Color) -> Self {
        self.bg = Some(c);
        self
    }
    /// Overrides the glyph foreground colour over the highlighted range.
    pub fn fg(mut self, c: Color) -> Self {
        self.fg = Some(c);
        self
    }
    /// Alias for [`Highlight::fg`].
    pub fn color(self, c: Color) -> Self {
        self.fg(c)
    }
    /// Overrides the inset border ring `(color, width_pt)` inside the rounded background.
    pub fn ring(mut self, color: Color, width: f32) -> Self {
        self.ring = (width > 0.0 && width.is_finite()).then_some((color, width));
        self
    }
    /// Sets a key for [`Tx::scroll_into_view`](rune::Tx::scroll_into_view),
    /// [`Cx::layout`](rune::Cx::layout) and [`Runtime::find`](rune::Runtime::find). When unset,
    /// the highlight is reachable under [`Markdown::highlight_key`]`(index)`.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
}

/// An item accepted by [`Markdown::highlights`]: a `Range<usize>`, a match `Vec<Range<usize>>`
/// from [`find`], or either paired with a [`Highlight`] (by value or reference).
pub trait HighlightItem: Sized {
    fn into_highlight(self) -> (Range<usize>, Highlight);
    #[doc(hidden)]
    fn into_ranges(self) -> (Vec<Range<usize>>, Highlight) {
        let (r, h) = self.into_highlight();
        (vec![r], h)
    }
}
