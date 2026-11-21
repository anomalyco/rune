//! Text and layout direction primitives: [`Direction`], resolved [`Flow`], per-text [`TextDir`],
//! and UAX #9 first-strong / paragraph-boundary helpers.

/// Which way an element and what it holds run, as HTML's `dir` attribute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Left to right. Inside a right-to-left subtree it's an island that keeps its own order,
    /// as `dir="ltr"` on a path, a command or code.
    #[default]
    Ltr,
    /// Right to left: Arabic, Hebrew, Persian, Urdu.
    Rtl,
    /// From the first strong character of the element's text (or, on a container, of the text
    /// inside it, skipping subtrees that set their own direction), as `dir="auto"` and `<bdi>`
    /// resolve it: right to left when that's Arabic or Hebrew, else left to right, and its
    /// parent's direction when there's none (a number, a path alone). An empty text field takes
    /// its placeholder's. For user strings such as titles, names, messages and a prompt.
    Auto,
}

impl Direction {
    /// The direction of `text` as [`Direction::Auto`] resolves it: its first strong character's,
    /// outside isolates (UAX #9 rules P2 and P3), or `None` when it has none.
    pub fn of(text: &str) -> Option<Direction> {
        first_strong_char(text, false)
    }

    /// Whether this is [`Direction::Rtl`].
    pub fn is_rtl(self) -> bool {
        self == Direction::Rtl
    }
}

/// The direction an element resolved to, which its subtree lays out in: what `El::dir` and its
/// ancestors' give it, `Direction::Auto` settled by its text.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Flow {
    #[default]
    Ltr,
    Rtl,
}

impl Flow {
    /// A laid-out paragraph's flow: its bidi base level.
    pub fn of_layout<B: parley::Brush>(layout: &parley::Layout<B>) -> Flow {
        if layout.is_rtl() { Flow::Rtl } else { Flow::Ltr }
    }

    /// The paragraph direction text in this flow lays out in.
    pub fn base(self) -> parley::BaseDirection {
        match self {
            Flow::Ltr => parley::BaseDirection::Ltr,
            Flow::Rtl => parley::BaseDirection::Rtl,
        }
    }
}

/// How a text's paragraph direction (bidi base level) resolves.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextDir {
    /// From the first strong character of the whole text: no `El::dir` above it.
    #[default]
    Auto,
    /// The element's resolved `El::dir`, set when it's flattened.
    Fixed(Flow),
    /// `El::dir_by_paragraph` (CSS `unicode-bidi: plaintext`):
    /// each paragraph in the direction of its own first strong character, whatever the
    /// element's is. A paragraph with none takes the direction of the paragraph before it, and
    /// the first ones the element's (here once it's flattened), as a word processor does: an
    /// empty line under an Arabic one starts at the right, an empty field in an Arabic window too.
    Plaintext(Option<Flow>),
}

/// The direction of `text`'s first strong character outside isolates, or `None` when it has
/// none: in its first paragraph, or `across` its paragraphs as HTML's `dir="auto"` reads an
/// element's text.
#[doc(hidden)]
pub fn first_strong_char(text: &str, across: bool) -> Option<Direction> {
    use icu_properties::{CodePointMapData, props::BidiClass};
    let classes = CodePointMapData::<BidiClass>::new();
    let mut isolates = 0u32;
    for c in text.chars() {
        match classes.get(c) {
            BidiClass::LeftToRightIsolate | BidiClass::RightToLeftIsolate | BidiClass::FirstStrongIsolate => isolates += 1,
            BidiClass::PopDirectionalIsolate => isolates = isolates.saturating_sub(1),
            BidiClass::ParagraphSeparator if across => isolates = 0,
            BidiClass::ParagraphSeparator if isolates == 0 => return None,
            BidiClass::LeftToRight if isolates == 0 => return Some(Direction::Ltr),
            BidiClass::RightToLeft | BidiClass::ArabicLetter if isolates == 0 => return Some(Direction::Rtl),
            _ => {}
        }
    }
    None
}

/// The byte ranges of `content`'s paragraphs, the separators between them left out: a UAX #9
/// paragraph separator (a newline) ends each but the last, CR LF as one.
#[doc(hidden)]
pub fn paragraph_ranges(content: &str) -> Vec<(usize, usize)> {
    use icu_properties::{CodePointMapData, props::BidiClass};
    let classes = CodePointMapData::<BidiClass>::new();
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = content.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if classes.get(c) != BidiClass::ParagraphSeparator {
            continue;
        }
        let end = match chars.peek() {
            Some(&(j, '\n')) if c == '\r' => {
                chars.next();
                j + 1
            }
            _ => i + c.len_utf8(),
        };
        out.push((start, i));
        start = end;
    }
    out.push((start, content.len()));
    out
}
