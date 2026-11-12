//! [`TextSpec`]: what a text element shows and how it lays out.
use crate::cold::{Cold, Empty};
use crate::color::Color;
use crate::el::{Align, LinkFn, LinkSecondaryFn};
use crate::text::{Link, Span};
use crate::tok::TextStyle;

/// A text element's record. What most texts set stays inline; what few set (links, rings,
/// pills, copy rules, a run to measure in, live text) waits behind `cold`, allocated on the
/// first write, so building a plain text allocates one small block.
#[derive(Clone)]
pub struct TextSpec {
    pub content: String,
    pub style: TextStyle,
    pub color: Color,
    pub spans: Vec<Span>,
    /// Wrapped lines shown before "…" ends the last (see [`El::max_lines`](crate::El::max_lines)); 0 for all.
    pub(crate) max_lines: usize,
    /// The scale's bits and [`crate::text::shape_key`] at it, once the content is final (see
    /// [`TextSpec::key_shape`]).
    pub(crate) shape: Option<(u32, u64)>,
    /// The authored string once `case` has replaced `content` with the drawn one.
    pub(crate) source: Option<Box<crate::text::CaseSource>>,
    pub(crate) cold: Cold<TextCold>,
    /// Streaming word reveal: words that arrive after mount fade in over this many seconds
    /// (negative: no reveal tracking).
    pub reveal: f32,
    /// The `truncate_fade` edge's width in ems (see [`El::truncate_fade_width`](crate::El::truncate_fade_width)).
    pub(crate) fade_em: f32,
    pub align: Align,
    /// `color` came from [`El::color`](crate::El::color); without it the text takes the
    /// window's foreground ([`Cx::foreground`](crate::Cx::foreground)) when the view sets one.
    pub(crate) inked: bool,
    /// Wrap at the element's width (its height follows the text).
    pub wrap: bool,
    /// One line, cut with an ellipsis when wider than the element.
    pub truncate: bool,
    /// Which part `truncate` cuts (see [`El::truncate_middle`](crate::El::truncate_middle)).
    pub(crate) elide: Elide,
    /// Fade the overflowing trailing edge rather than replacing text with an ellipsis.
    pub truncate_fade: bool,
    /// Spread wrapped lines to fill the width (see [`El::justify`](crate::El::justify)).
    pub justify: bool,
    /// See [`El::text_case`](crate::El::text_case); applied when the element is laid out.
    pub(crate) case: Option<crate::TextCase>,
    /// See [`El::selectable`](crate::El::selectable).
    pub(crate) selectable: bool,
    pub(crate) redacted: bool,
    pub(crate) wrap_anywhere: bool,
    /// See [`El::exact_lines`](crate::El::exact_lines): the height is its lines' own, unrounded,
    /// unless [`El::rounded_lines`](crate::El::rounded_lines) rounds it to device pixels.
    pub(crate) exact_lines: bool,
    /// The paragraph direction it lays out in (see [`TextDir`]).
    pub(crate) dir: TextDir,
}

/// How a text's paragraph direction (bidi base level) resolves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TextDir {
    /// From the first strong character of the whole text: no `El::dir` above it.
    #[default]
    Auto,
    /// The element's resolved `El::dir`, set when it's flattened.
    Fixed(crate::el::Flow),
    /// [`El::dir_by_paragraph`](crate::El::dir_by_paragraph) (CSS `unicode-bidi: plaintext`):
    /// each paragraph in the direction of its own first strong character, whatever the
    /// element's is. A paragraph with none takes the direction of the paragraph before it, and
    /// the first ones the element's (here once it's flattened), as a word processor does: an
    /// empty line under an Arabic one starts at the right, an empty field in an Arabic window too.
    Plaintext(Option<crate::el::Flow>),
}

/// What moves a text's [`TextDir`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextDirEvent {
    /// `El::dir_by_paragraph`.
    ByParagraph,
    /// Flattening resolved the element's direction: `None` when nothing above it sets one.
    Flatten(Option<crate::el::Flow>),
}

/// One step of a text's [`TextDir`]: by-paragraph text keeps its own directions whatever
/// direction the element resolves to.
pub(crate) fn step_text_dir(state: TextDir, event: TextDirEvent) -> TextDir {
    match (state, event) {
        (TextDir::Plaintext(flow), TextDirEvent::ByParagraph) => TextDir::Plaintext(flow),
        (_, TextDirEvent::ByParagraph) => TextDir::Plaintext(None),
        (TextDir::Plaintext(_), TextDirEvent::Flatten(flow)) => TextDir::Plaintext(flow),
        (_, TextDirEvent::Flatten(Some(flow))) => TextDir::Fixed(flow),
        (_, TextDirEvent::Flatten(None)) => TextDir::Auto,
    }
}

/// What few texts set: written through `TextSpec::cold`, which allocates it on the first write.
#[derive(Clone)]
pub(crate) struct TextCold {
    /// Plain live run. Layout is measured from the authored placeholder before latching it.
    pub(crate) live: Option<crate::LiveText>,
    pub(crate) links: Vec<Link>,
    pub(crate) on_link: Option<LinkFn>,
    pub(crate) on_link_secondary: Option<LinkSecondaryFn>,
    /// Measured as its slice at this byte of the run (see [`El::advance_in`](crate::El::advance_in)).
    pub(crate) advance_in: Option<(crate::text::Run, usize)>,
    /// See [`El::selection_color`](crate::El::selection_color).
    pub(crate) selection: Color,
    /// See [`El::selection_style`](crate::El::selection_style): it takes over `selection`.
    pub(crate) selection_style: Option<crate::SelectionStyle>,
    /// See [`El::uncopied`](crate::El::uncopied): byte ranges of `content`, sorted.
    pub(crate) uncopied: Vec<(u32, u32)>,
    /// See [`El::copy_breaks`](crate::El::copy_breaks): newlines before and after it.
    pub(crate) copy_breaks: [u8; 2],
    /// See [`El::copy_prefix`](crate::El::copy_prefix).
    pub(crate) copy_prefix: Option<std::rc::Rc<str>>,
    /// Inset border rings `(start, end, color, width)` on rounded span backgrounds.
    pub(crate) rings: Vec<(u32, u32, Color, f32)>,
    /// See [`El::span_radius`](crate::El::span_radius), in points.
    pub(crate) span_radius: f32,
    /// Strikethrough byte ranges `(start, end, color)` of `content`.
    pub(crate) strikes: Vec<(u32, u32, Option<Color>)>,
    /// Paint-only highlight byte ranges of `content`.
    pub(crate) highlights: Vec<crate::text::TextHighlight>,
    /// Per-paragraph [`Block`](crate::Block) attributes for `content`.
    pub(crate) blocks: Vec<crate::edit::Block>,
    /// Line indices (`> 0`) created by `Shift+Return` (`<br>`).
    pub(crate) soft_breaks: Vec<u32>,
    /// [`Mark::Pill`](crate::Mark::Pill) chips in `content`, sorted.
    pub(crate) pills: Vec<crate::text::PillChip>,
    /// See [`El::isolate`](crate::El::isolate): byte ranges of `content` still to mark, sorted.
    pub(crate) isolates: Vec<(u32, u32, crate::Direction)>,
    /// See [`El::pill_colors`](crate::El::pill_colors).
    pub(crate) pill_colors: Option<(Color, Option<Color>)>,
    /// Visual palette for rich block and inline decorations; the default's when unset.
    pub(crate) rich_style: Option<crate::edit::RichStyle>,
    pub(crate) secure_revealed: bool,
}

impl Empty for TextCold {
    const EMPTY: &'static Self = &TextCold {
        live: None,
        links: Vec::new(),
        on_link: None,
        on_link_secondary: None,
        advance_in: None,
        selection: crate::el::DEFAULT_SELECTION,
        selection_style: None,
        uncopied: Vec::new(),
        copy_breaks: [1, 1],
        copy_prefix: None,
        rings: Vec::new(),
        span_radius: crate::text::SPAN_RADIUS,
        strikes: Vec::new(),
        highlights: Vec::new(),
        blocks: Vec::new(),
        soft_breaks: Vec::new(),
        pills: Vec::new(),
        isolates: Vec::new(),
        pill_colors: None,
        rich_style: None,
        secure_revealed: false,
    };
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<TextSpec>() == 224);

impl TextSpec {
    /// A plain text showing `content`, in the body style.
    pub(crate) fn new(content: String) -> TextSpec {
        TextSpec {
            content,
            style: crate::tok::type_::BODY,
            color: Color::WHITE,
            spans: Vec::new(),
            max_lines: 0,
            shape: None,
            source: None,
            cold: Cold::NONE,
            reveal: -1.0,
            fade_em: crate::el::TRUNCATE_FADE_EM,
            align: Align::Start,
            inked: false,
            wrap: false,
            truncate: false,
            elide: Elide::End,
            truncate_fade: false,
            justify: false,
            case: None,
            selectable: false,
            redacted: false,
            wrap_anywhere: false,
            exact_lines: true,
            dir: TextDir::Auto,
        }
    }

    /// The palette its rich decorations draw in.
    pub(crate) fn rich_style(&self) -> crate::edit::RichStyle {
        self.cold.rich_style.unwrap_or_default()
    }
}
impl std::fmt::Debug for TextSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextSpec")
            .field("content", &if self.redacted { "[redacted]" } else { &self.content })
            .field("style", &self.style)
            .field("color", &self.color)
            .field("align", &self.align)
            .field("spans", &self.spans)
            .field("wrap", &self.wrap)
            .field("reveal", &self.reveal)
            .field("truncate", &self.truncate)
            .field("max_lines", &self.max_lines)
            .field("truncate_fade", &self.truncate_fade)
            .field("justify", &self.justify)
            .field("selectable", &self.selectable)
            .finish_non_exhaustive()
    }
}

impl Drop for TextSpec {
    fn drop(&mut self) {
        if self.redacted {
            use zeroize::Zeroize;
            self.content.zeroize();
            if let Some(src) = &mut self.source {
                src.text.zeroize();
            }
        }
    }
}

impl TextSpec {
    /// Hashes what shaping depends on once, so measuring and laying the text out at each width
    /// this commit reuses it. Secure text keeps no content-derived key.
    pub(crate) fn key_shape(&mut self, scale: f32) {
        self.shape = (!self.redacted).then(|| (scale.to_bits(), self.fresh_shape_key(scale)));
    }

    /// The key [`Self::key_shape`] kept at `scale`.
    pub(crate) fn shape_key(&self, scale: f32) -> Option<u64> {
        let (_, key) = self.shape.filter(|s| s.0 == scale.to_bits())?;
        debug_assert_eq!(key, self.fresh_shape_key(scale), "a text changed after its shape key was taken");
        Some(key)
    }

    /// The flow a by-paragraph text's paragraphs with no strong character start from: the
    /// element's, left to right when nothing sets one.
    pub(crate) fn plaintext_flow(&self) -> crate::el::Flow {
        match self.dir {
            TextDir::Plaintext(flow) => flow.unwrap_or_default(),
            _ => crate::el::Flow::Ltr,
        }
    }

    /// [`crate::text::shape_key`] of this text in its direction.
    pub(crate) fn fresh_shape_key(&self, scale: f32) -> u64 {
        let key = crate::text::shape_key(&self.content, &self.spans, self.style, scale, self.wrap_anywhere, &self.cold.pills);
        crate::text::directed(key, self.base())
    }

    /// The paragraph direction it lays out in as one layout.
    pub(crate) fn base(&self) -> parley::BaseDirection {
        match self.dir {
            TextDir::Fixed(flow) => flow.base(),
            TextDir::Auto => parley::BaseDirection::Auto,
            // A paragraph with no strong character reads the way the element does.
            TextDir::Plaintext(flow) => match crate::Direction::of(&self.content) {
                Some(crate::Direction::Rtl) => parley::BaseDirection::Rtl,
                Some(_) => parley::BaseDirection::Ltr,
                None => flow.unwrap_or_default().base(),
            },
        }
    }

    /// Whether it lays out one paragraph at a time, each in its own direction
    /// ([`crate::text::ParagraphLayout`]): a wrapped, uncut field's by-paragraph text. Others
    /// stay one layout (a static text splits into one text per paragraph instead).
    pub(crate) fn by_paragraph(&self, input: bool) -> bool {
        matches!(self.dir, TextDir::Plaintext(_)) && input && self.wrap && !self.redacted && !self.truncate && !self.truncate_fade
            && self.max_lines == 0 && self.cold.live.is_none() && self.cold.advance_in.is_none()
    }

    /// Zeroizes the shown string and shows `content` instead.
    pub(crate) fn replace_content(&mut self, content: String) {
        use zeroize::Zeroize;
        self.content.zeroize();
        self.content = content;
        self.shape = None;
    }

    /// The string as authored, before [`El::text_case`](crate::El::text_case).
    pub(crate) fn source_text(&self) -> &str {
        self.source.as_ref().map_or(&self.content, |s| &s.text)
    }

    /// The authored text of `content[start..end]`.
    pub(crate) fn source_slice(&self, start: u32, end: u32) -> Option<&str> {
        match &self.source {
            None => self.content.get(start as usize..end as usize),
            Some(s) => s.text.get(s.source(start, false) as usize..s.source(end, true) as usize),
        }
    }

    /// The authored text of `content[start..end]` as copying takes it, without its
    /// [`El::uncopied`](crate::El::uncopied) bytes.
    pub(crate) fn copy_slice(&self, start: u32, end: u32) -> Option<String> {
        if self.redacted {
            return None;
        }
        let mut out = String::new();
        let mut at = start;
        for &(a, b) in self.cold.uncopied.iter().filter(|r| r.1 > start && r.0 < end) {
            if a > at { out.push_str(self.source_slice(at, a)?); }
            at = at.max(b);
        }
        if at < end { out.push_str(self.source_slice(at, end)?); }
        Some(out)
    }

    /// Masks `self.content` in place and zeroizes the plain string (for layout of a hidden
    /// secure label or an exiting revealed one).
    pub(crate) fn conceal(&mut self) {
        if !self.redacted {
            return;
        }

        self.shape = None;
        let mask = if self.content.is_empty() {
            String::new()
        } else {
            crate::edit::MULTILINE_SECURE_MASK.to_owned()
        };
        use zeroize::Zeroize;
        self.content.zeroize();
        self.content = mask;
        if let Some(mut src) = self.source.take() {
            src.text.zeroize();
        }
        self.case = None;
        self.spans.clear();
        if let Some(cold) = self.cold.get_mut_if_some() {
            cold.secure_revealed = false;
            cold.links.clear();
            cold.rings.clear();
            cold.strikes.clear();
            cold.blocks.clear();
            cold.soft_breaks.clear();
            cold.pills.clear();
            cold.uncopied.clear();
        }
        self.wrap = false;
        self.wrap_anywhere = false;
        self.max_lines = 0;
    }

    /// Replaces `content` (the authored string) with its cased form and moves span and link
    /// ranges onto it.
    pub(crate) fn apply_case(&mut self) {
        self.shape = None;
        if self.redacted {
            self.case = None;
            return;
        }
        // Ring boundaries need distinct brushes even when the surrounding styles match.
        // Do this once before shaping, not once per glyph run at paint time.
        if !self.cold.rings.is_empty() {
            let mut spans = Vec::new();
            for span in &self.spans {
                let mut bounds = vec![span.start, span.end];
                for &(a, b, _, _) in &self.cold.rings {
                    bounds.extend([a, b].into_iter().filter(|&p| span.start < p && p < span.end));
                }
                bounds.sort_unstable();
                bounds.dedup();
                spans.extend(bounds.windows(2).map(|w| Span { start: w[0], end: w[1], ..span.clone() }));
            }
            self.spans = spans;
        }
        let Some(case) = self.case else { return };
        let (shown, source) = crate::text::CaseSource::new(case, std::mem::take(&mut self.content));
        for span in &mut self.spans {
            (span.start, span.end) = (source.shown(span.start, false), source.shown(span.end, true));
        }
        if let Some(cold) = self.cold.get_mut_if_some() {
            for link in &mut cold.links {
                (link.start, link.end) = (source.shown(link.start, false), source.shown(link.end, true));
            }
            for r in &mut cold.uncopied {
                *r = (source.shown(r.0, false), source.shown(r.1, true));
            }
            for r in &mut cold.rings {
                (r.0, r.1) = (source.shown(r.0, false), source.shown(r.1, true));
            }
            for r in &mut cold.strikes {
                (r.0, r.1) = (source.shown(r.0, false), source.shown(r.1, true));
            }
            for h in &mut cold.highlights {
                (h.start, h.end) = (source.shown(h.start, false), source.shown(h.end, true));
            }
        }
        self.content = shown;
        self.source = Some(Box::new(source));
    }
}

/// The part of an overflowing `truncate`d text the ellipsis replaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Elide {
    /// [`El::truncate`](crate::El::truncate): keeps the start.
    #[default]
    End,
    /// [`El::truncate_middle`](crate::El::truncate_middle): keeps the start and the end.
    Middle,
    /// [`El::truncate_head`](crate::El::truncate_head): keeps the end.
    Head,
}
