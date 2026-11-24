//! The rich-text model's value types: inline marks, paragraph blocks and their palette.

use std::ops::Range;

use rune_core::color::{Color, oklch, oklcha};

pub use rune_text::{Block, MAX_LIST_LEVEL};

/// An inline formatting mark stored alongside [`TextEdit::text`](super::TextEdit::text). More
/// marks may come, so a `match` needs a `_` arm.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Link(String),
    /// An atomic inline object (an `@file` or `@agent` mention, an attachment) carrying the
    /// app's id; its text is the label. The caret steps over it, deleting takes it whole, a
    /// selection that touches it covers it, and typing never extends it. A field draws it as a
    /// chip (`El::pill_style`, `El::pill_colors`, `El::pill_icon`). Insert one with
    /// [`TextEdit::insert_pill`](super::TextEdit::insert_pill).
    Pill(String),
    /// Inline text colour stored as lowercase `#rrggbb` (sRGB hex).
    ///
    /// Apply or remove it on the selection or pending typing style with
    /// [`TextEdit::set_color`](super::TextEdit::set_color), and query it with
    /// [`TextEdit::active_color`](super::TextEdit::active_color).
    Color(String),
}

impl Mark {
    /// Whether `self` and `other` are the same kind of mark (`Link(_)` matches any `Link(_)`,
    /// `Color(_)` matches any `Color(_)`).
    pub fn same_kind(&self, other: &Mark) -> bool {
        match (self, other) {
            (Mark::Bold, Mark::Bold)
            | (Mark::Italic, Mark::Italic)
            | (Mark::Underline, Mark::Underline)
            | (Mark::Strike, Mark::Strike)
            | (Mark::Code, Mark::Code)
            | (Mark::Link(_), Mark::Link(_))
            | (Mark::Pill(_), Mark::Pill(_))
            | (Mark::Color(_), Mark::Color(_)) => true,
            _ => false,
        }
    }

    /// The URL when `self` is [`Mark::Link`].
    pub fn link_url(&self) -> Option<&str> {
        match self {
            Mark::Link(url) => Some(url),
            _ => None,
        }
    }

    /// The app's id when `self` is [`Mark::Pill`].
    pub fn pill_id(&self) -> Option<&str> {
        match self {
            Mark::Pill(id) => Some(id),
            _ => None,
        }
    }

    /// The normalized lowercase `#rrggbb` hex colour when `self` is [`Mark::Color`].
    ///
    /// ```
    /// use rune_edit::Mark;
    /// assert_eq!(Mark::Color("#d93025".into()).color_hex(), Some("#d93025"));
    /// assert_eq!(Mark::Bold.color_hex(), None);
    /// ```
    pub fn color_hex(&self) -> Option<&str> {
        match self {
            Mark::Color(hex) => Some(hex),
            _ => None,
        }
    }

    pub(crate) fn is_pill(&self) -> bool {
        matches!(self, Mark::Pill(_))
    }

    pub(crate) fn kind_order(&self) -> u8 {
        match self {
            Mark::Bold => 0,
            Mark::Italic => 1,
            Mark::Underline => 2,
            Mark::Strike => 3,
            Mark::Code => 4,
            Mark::Link(_) => 5,
            Mark::Pill(_) => 6,
            Mark::Color(_) => 7,
        }
    }
}

/// A non-empty byte range `[start, end)` in [`TextEdit::text`](super::TextEdit::text) carrying an
/// inline [`Mark`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MarkRange {
    pub start: usize,
    pub end: usize,
    pub mark: Mark,
}

impl MarkRange {
    pub fn new(range: Range<usize>, mark: Mark) -> Self {
        Self { start: range.start, end: range.end, mark }
    }

    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// A paragraph slice of [`TextEdit`](super::TextEdit), returned by
/// [`TextEdit::paragraphs`](super::TextEdit::paragraphs).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParagraphInfo {
    pub index: usize,
    pub range: Range<usize>,
    pub block: Block,
    /// True when this line was split from the previous line by `Shift+Return` (`<br>`).
    pub soft_break: bool,
}

/// Visual palette for rich marks and block attributes inside `text_input`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RichStyle {
    pub code_fg: Option<Color>,
    pub code_bg: Color,
    pub code_block_bg: Color,
    pub link: Color,
    pub quote_bar: Color,
    pub marker: Option<Color>,
}

impl Default for RichStyle {
    fn default() -> Self {
        Self {
            code_fg: None,
            code_bg: oklcha(0.55, 0.02, 260.0, 0.16),
            code_block_bg: oklcha(0.5, 0.02, 260.0, 0.12),
            link: oklch(0.68, 0.16, 250.0),
            quote_bar: oklcha(0.6, 0.04, 255.0, 0.45),
            marker: None,
        }
    }
}
