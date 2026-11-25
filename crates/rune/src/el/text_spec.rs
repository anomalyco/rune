//! [`TextSpec`]: what a text element shows and how it lays out.
use crate::cold::{Cold, Empty};
use crate::color::Color;
use crate::el::{Align, LinkFn, LinkHoverFn, LinkSecondaryFn};
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
    pub align: Align,
    /// `color` came from [`El::color`](crate::El::color); without it the text takes the
    /// window's foreground ([`Cx::foreground`](crate::Cx::foreground)) when the view sets one.
    pub(crate) inked: bool,
    /// Wrap at the element's width (its height follows the text).
    pub wrap: bool,
    /// One line, cut with an ellipsis when wider than the element.
    pub truncate: bool,
    /// Which part `truncate` or `max_lines` cuts (see [`El::elide_middle`](crate::El::elide_middle)).
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
    /// Packed internal booleans (`GREEDY`, `SELECTABLE_CURSOR`), keeping `TextSpec` at 224 bytes
    /// without allocating `TextCold` on plain selectable text.
    pub(crate) flags: TextFlags,
    /// See [`El::exact_lines`](crate::El::exact_lines): the height is its lines' own, unrounded,
    /// unless [`El::rounded_lines`](crate::El::rounded_lines) rounds it to device pixels.
    pub(crate) exact_lines: bool,
    /// See [`El::by_line`](crate::El::by_line): one shaped [`Layout`](crate::text::Layout) per line.
    pub(crate) by_line: bool,
    /// The paragraph direction it lays out in (see [`TextDir`]).
    pub(crate) dir: TextDir,
}

/// Packed internal booleans on [`TextSpec`] (one bit each).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TextFlags(u8);

impl TextFlags {
    /// See [`El::wrap_greedy`](crate::El::wrap_greedy).
    pub(crate) const GREEDY: u8 = 1 << 0;
    /// [`El::selectable`](crate::El::selectable) gave the element its `Cursor::Text`, which an
    /// [`El::selection_enabled`](crate::El::selection_enabled)`(false)` subtree takes back.
    pub(crate) const SELECTABLE_CURSOR: u8 = 1 << 1;
    /// [`El::link_menu`](crate::El::link_menu)`(false)`: suppress Rune's default Open Link / Copy Link
    /// context menu on inline [`Link`] spans.
    pub(crate) const NO_LINK_MENU: u8 = 1 << 2;

    #[inline(always)]
    pub(crate) const fn has(self, bit: u8) -> bool { self.0 & bit != 0 }

    #[inline(always)]
    pub(crate) fn set(&mut self, bit: u8, on: bool) {
        if on { self.0 |= bit; } else { self.0 &= !bit; }
    }
}

pub(crate) use rune_text::TextDir;

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

/// Authored byte anchor for an `El::inline` box, preserved across `El::live_text` updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InlineAnchor {
    pub(crate) at: u32,
    /// True when `at == placeholder.len()` on a non-empty placeholder so trailing chips stay at
    /// the end as live text grows or shrinks.
    pub(crate) trailing: bool,
}

/// What few texts set: written through `TextSpec::cold`, which allocates it on the first write.
#[derive(Clone)]
pub(crate) struct TextCold {
    /// Plain live run. Layout is measured from the authored placeholder before latching it.
    pub(crate) live: Option<crate::LiveText>,
    pub(crate) links: Vec<Link>,
    pub(crate) on_link: Option<LinkFn>,
    pub(crate) on_link_secondary: Option<LinkSecondaryFn>,
    pub(crate) on_link_hover: Option<LinkHoverFn>,
    /// See [`El::link_cursor`](crate::El::link_cursor).
    pub(crate) link_cursor: Option<crate::el::Cursor>,
    /// Measured as its slice at this byte of the run (see [`El::advance_in`](crate::El::advance_in)).
    pub(crate) advance_in: Option<(crate::text::Run, usize)>,
    /// See [`El::caret`](crate::El::caret): caret colour for [`El::caret_browsing`](crate::El::caret_browsing) selectable text.
    pub(crate) caret: Option<Color>,
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
    /// First-line indentation in points (negative for hanging indent); see [`El::text_indent`](crate::El::text_indent).
    pub(crate) text_indent: f32,
    /// Vertical alignment of laid-out text inside a taller content box; see [`El::align_y`](crate::El::align_y).
    pub(crate) align_y: Align,
    /// The `truncate_fade` edge's width in ems (see [`El::truncate_fade_width`](crate::El::truncate_fade_width)).
    pub(crate) fade_em: f32,
    /// Strikethrough byte ranges `(start, end, color)` of `content`.
    pub(crate) strikes: Vec<(u32, u32, Option<Color>)>,
    /// Overline byte ranges `(start, end, color)` of `content`.
    pub(crate) overlines: Vec<(u32, u32, Option<Color>)>,
    /// Padded inline boxes over `content` (`El::span_box`), sorted and non-overlapping.
    pub(crate) span_boxes: Vec<crate::text::SpanBoxRun>,
    /// In-flow inline element slots over `content` (`El::inline`), keeping room for child elements.
    pub(crate) inline_boxes: Vec<crate::text::InlineBoxRun>,
    /// Authored byte anchors for `inline_boxes`, kept in lockstep so `El::live_text` updates
    /// remap offsets cleanly across length, UTF-8, and `El::text_case` changes.
    pub(crate) inline_anchors: Vec<InlineAnchor>,
    /// Paint-only highlight byte ranges of `content`.
    pub(crate) highlights: Vec<crate::text::TextHighlight>,
    /// Display-only full-width line fills and left bars over `content` (see [`El::line_fills`](crate::El::line_fills)).
    pub(crate) line_fills: Vec<LineFill>,
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
        on_link_hover: None,
        link_cursor: None,
        advance_in: None,
        caret: None,
        selection: crate::el::DEFAULT_SELECTION,
        selection_style: None,
        uncopied: Vec::new(),
        copy_breaks: [1, 1],
        copy_prefix: None,
        rings: Vec::new(),
        span_radius: crate::text::SPAN_RADIUS,
        text_indent: 0.0,
        align_y: Align::Start,
        fade_em: crate::el::TRUNCATE_FADE_EM,
        strikes: Vec::new(),
        overlines: Vec::new(),
        span_boxes: Vec::new(),
        inline_boxes: Vec::new(),
        inline_anchors: Vec::new(),
        highlights: Vec::new(),
        line_fills: Vec::new(),
        blocks: Vec::new(),
        soft_breaks: Vec::new(),
        pills: Vec::new(),
        isolates: Vec::new(),
        pill_colors: None,
        rich_style: None,
        secure_revealed: false,
    };
}

// Every text carries one of these: a field few texts set goes in `TextCold`, not here.
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
            flags: TextFlags::default(),
            exact_lines: true,
            by_line: false,
            dir: TextDir::Auto,
        }
    }

    #[inline(always)]
    pub(crate) const fn greedy(&self) -> bool { self.flags.has(TextFlags::GREEDY) }

    /// Borrowed read-only layout inputs for `rune-text`.
    pub(crate) fn as_text_input(&self, scale: f32, input: bool) -> crate::text::TextInput<'_> {
        crate::text::TextInput {
            content: &self.content,
            spans: &self.spans,
            blocks: &self.cold.blocks,
            pills: &self.cold.pills,
            span_boxes: &self.cold.span_boxes,
            inline_boxes: &self.cold.inline_boxes,
            text_indent: self.cold.text_indent,
            style: self.style,
            shape_key: self.shape_key(scale),
            wrap_anywhere: self.wrap_anywhere,
            pretty: self.pretty(),
            redacted: self.redacted,
            by_line: self.by_line,
            by_paragraph: self.by_paragraph(input),
            dir: self.dir,
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

/// Wipes `s` (its spare capacity included) and frees its allocation, matching `TextEdit`.
pub(crate) fn wipe_string(s: &mut String) {
    drop(crate::edit::Wiped(std::mem::take(s)));
}

/// Wipes every string owned by `links` and clears the list.
pub(crate) fn wipe_links(links: &mut Vec<Link>) {
    for link in links.iter_mut() {
        wipe_string(&mut link.id);
        if let Some(href) = &mut link.href {
            wipe_string(href);
        }
        if let Some(label) = &mut link.label {
            wipe_string(label);
        }
        if let Some(help) = &mut link.help {
            wipe_string(help);
        }
    }
    links.clear();
}

impl Drop for TextSpec {
    fn drop(&mut self) {
        if self.redacted {
            wipe_string(&mut self.content);
            if let Some(src) = &mut self.source {
                wipe_string(&mut src.text);
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

    /// Whether its lines break as Chromium's `text-wrap-style: pretty` does ([`crate::text::set_pretty_wrap`]).
    pub(crate) fn pretty(&self) -> bool {
        crate::text::pretty_wrap() && !self.flags.has(TextFlags::GREEDY) && !self.redacted && !self.justify
    }

    /// [`crate::text::shape_key`] of this text in its direction.
    pub(crate) fn fresh_shape_key(&self, scale: f32) -> u64 {
        let key = crate::text::shape_key(&self.content, &self.spans, self.style, scale, self.wrap_anywhere, &self.cold.pills);
        let key = crate::text::mix_span_boxes(key, &self.cold.span_boxes);
        let key = crate::text::mix_inline_boxes(key, &self.cold.inline_boxes);
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

    /// Whether it lays out one paragraph at a time ([`crate::text::ParagraphLayout`]), so an
    /// edit shapes only the paragraphs it changed: a wrapped, uncut field's text of more than
    /// one paragraph, or of any when each paragraph takes its own direction. Others stay one
    /// layout (a static text splits into one text per paragraph instead).
    pub(crate) fn by_paragraph(&self, input: bool) -> bool {
        (input || self.by_line) && self.wrap && !self.redacted && !self.truncate && !self.truncate_fade
            && self.max_lines == 0 && self.cold.live.is_none() && self.cold.advance_in.is_none()
            && self.cold.inline_boxes.is_empty()
            && (self.by_line || matches!(self.dir, TextDir::Plaintext(_)) || self.content.contains('\n'))
    }

    /// Zeroizes the shown string (spare capacity included) and shows `content` instead.
    pub(crate) fn replace_content(&mut self, content: String) {
        wipe_string(&mut self.content);
        self.content = content;
        self.shape = None;
    }

    /// Re-resolves each `El::inline` box offset from its authored anchor against the new live
    /// `content`: a box placed at the end of a non-empty placeholder (`trailing`) stays at
    /// `content.len()`; any other offset stays at its authored byte clamped down to the nearest
    /// UTF-8 char boundary in `content`.
    pub(crate) fn remap_inline_boxes_for_live(&mut self) {
        if self.cold.inline_boxes.is_empty() {
            return;
        }
        let len = self.content.len();
        let cold = &mut *self.cold;
        for (b, a) in cold.inline_boxes.iter_mut().zip(&cold.inline_anchors) {
            let at = if a.trailing {
                len
            } else {
                let mut p = (a.at as usize).min(len);
                while p > 0 && !self.content.is_char_boundary(p) {
                    p -= 1;
                }
                p
            };
            b.at = at as u32;
        }
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
        wipe_string(&mut self.content);
        self.content = mask;
        if let Some(mut src) = self.source.take() {
            wipe_string(&mut src.text);
        }
        self.case = None;
        self.spans.clear();
        if let Some(cold) = self.cold.get_mut_if_some() {
            cold.secure_revealed = false;
            wipe_links(&mut cold.links);
            cold.rings.clear();
            cold.strikes.clear();
            cold.overlines.clear();
            cold.span_boxes.clear();
            cold.inline_boxes.clear();
            cold.inline_anchors.clear();
            cold.line_fills.clear();
            cold.text_indent = 0.0;
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
            for r in &mut cold.overlines {
                (r.0, r.1) = (source.shown(r.0, false), source.shown(r.1, true));
            }
            for b in &mut cold.span_boxes {
                (b.start, b.end) = (source.shown(b.start, false), source.shown(b.end, true));
            }
            for b in &mut cold.inline_boxes {
                b.at = source.shown(b.at, false);
            }
            for h in &mut cold.highlights {
                (h.start, h.end) = (source.shown(h.start, false), source.shown(h.end, true));
            }
            for f in &mut cold.line_fills {
                (f.start, f.end) = (source.shown(f.start, false), source.shown(f.end, true));
            }
        }
        self.content = shown;
        self.source = Some(Box::new(source));
    }
}

/// A display-only full-width line background fill and/or left vertical bar over the visual lines
/// intersecting UTF-8 bytes `start..end` of a [`text_input`](crate::text_input) or
/// [`text`](crate::text()) element (for fenced code blocks, block quotes, diff hunks, and similar
/// line-level decorations).
///
/// Attach with [`El::line_fill`](crate::El::line_fill) or [`El::line_fills`](crate::El::line_fills).
/// Drawn under the text across the full field width without altering [`TextEdit`](crate::TextEdit),
/// undo history, selection, or line wrapping.
///
/// ```
/// use rune::{Color, LineFill};
///
/// let code = LineFill::new(0, 24).bg(Color { r: 1.0, g: 1.0, b: 1.0, a: 0.06 });
/// let quote = LineFill::new(25, 60).left_bar(2.0, Color { r: 0.6, g: 0.6, b: 0.6, a: 0.8 });
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineFill {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) bg: Option<Color>,
    pub(crate) left_bar: Option<(f32, Color)>,
    pub(crate) radius: f32,
}

impl LineFill {
    /// A line decoration covering every visual line intersected by UTF-8 bytes `start..end`.
    ///
    /// The default corner radius for [`Self::bg`] is `6.0` points, matching rich `Block::Code`.
    pub const fn new(start: usize, end: usize) -> Self {
        let s = if start > u32::MAX as usize { u32::MAX } else { start as u32 };
        let e = if end > u32::MAX as usize { u32::MAX } else { end as u32 };
        Self { start: s, end: e, bg: None, left_bar: None, radius: 6.0 }
    }

    /// Full-width rounded background fill behind the covered lines.
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Vertical bar `width` points wide along the left edge of the covered lines (ignored when
    /// `width` is not finite and positive).
    pub const fn left_bar(mut self, width: f32, color: Color) -> Self {
        self.left_bar = if width > 0.0 && width.is_finite() { Some((width, color)) } else { None };
        self
    }

    /// Corner radius in points for [`Self::bg`] (`6.0` by default). Non-finite or negative
    /// values become `0.0`.
    pub const fn radius(mut self, radius: f32) -> Self {
        self.radius = if radius.is_finite() && radius > 0.0 { radius } else { 0.0 };
        self
    }

    /// The UTF-8 byte range this line fill covers.
    pub const fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }

    pub(crate) const fn is_visible(&self) -> bool {
        self.start < self.end && (self.bg.is_some() || self.left_bar.is_some())
    }
}

/// The part of an overflowing `truncate`d or `max_lines`-clamped text the ellipsis replaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Elide {
    /// [`El::truncate`](crate::El::truncate) and default [`El::max_lines`](crate::El::max_lines): keeps the start.
    #[default]
    End,
    /// [`El::truncate_middle`](crate::El::truncate_middle) and [`El::elide_middle`](crate::El::elide_middle): keeps the start and the end.
    Middle,
    /// [`El::truncate_head`](crate::El::truncate_head) and [`El::elide_head`](crate::El::elide_head): keeps the end.
    Head,
}

impl crate::El {
    /// Whether right-clicking (or pressing the context-menu key on) an inline [`Link`] span with
    /// a real address opens Rune's default Open Link / Copy Link context menu (`true` by default).
    ///
    /// The default menu appears only for non-redacted `role("link")` spans that carry an explicit
    /// [`Link::href`] or an `id` passing [`crate::open_url::check`] (`http:`, `https:`,
    /// `mailto:`), and yields to [`El::on_link_secondary`] and any element or ancestor
    /// [`El::on_secondary_press`]. Pass `false` to opt out while keeping plain selectable-text
    /// Copy.
    ///
    /// ```
    /// use rune::{Link, text};
    ///
    /// let _plain = text("See https://example.com")
    ///     .link(Link::new(4..23, "https://example.com"))
    ///     .link_menu(false);
    /// ```
    pub fn link_menu(mut self, on: bool) -> Self {
        if let Some(t) = &mut self.text {
            t.flags.set(TextFlags::NO_LINK_MENU, !on);
        }
        self
    }
}
