//! Markdown (pulldown-cmark): headings, paragraphs, emphasis, inline code, code blocks, lists
//! (with GFM task checkboxes; see [`Markdown::on_task`]), block quotes, rules, links, GFM tables,
//! ```` ```diff ```` fences and (with the `mermaid` feature) ```` ```mermaid ```` diagrams,
//! rendered from a [`MarkdownStyle`]. Fences in rust,
//! js, ts, tsx, json, python, go, swift, sh, sql, html, css and yaml are syntax-coloured by
//! [`crate::syntax`], or by an app's own highlighter ([`Markdown::highlight`]). Partial input
//! while streaming is fine: an unclosed fence is a code block that keeps growing (a diagram
//! fence draws as it grows; see Diagrams).
//!
//! ```
//! # use rune_kit::{Kit, Markdown, MarkdownStyle};
//! # struct App;
//! # impl App { fn open(&mut self, _: &str) {} }
//! # struct Msg { text: String }
//! # let message = Msg { text: "# Hello".into() };
//! # let dark = true;
//! # let kit = Kit::neutral(dark);
//! let view = Markdown::new(&message.text).reveal(0.18).on_link(|s: &mut App, url| s.open(url)).build(&kit);
//! // Without a Kit:
//! let view = Markdown::new(&message.text).render(&MarkdownStyle::neutral(dark));
//! ```
//!
//! **Cost.** [`parse_cached`] keeps the parse of every source string the last two commits drew
//! (and others up to a memory budget), so re-rendering unchanged text (every frame of an
//! animation, a transcript of any length) parses nothing. A growing stream parses only from the
//! start of its last top-level block each time it grows.
//!
//! **Nesting & stack.** Container nesting (`<blockquote>`, `<ul>`/`<ol>`, `<li>`) is bounded at
//! `511` open elements (`255` nested lists, `1` quote + `255` nested lists, or `510` nested block
//! quotes on Windows, macOS, and Linux; on targets without `rune-taffy` stack continuation such as
//! `wasm32`, `ios`, and `android`, flex-container nesting caps at `770` so pure `<blockquote>`
//! nesting stops at `385` while all `255` list levels still nest). Past the bound, deeper list
//! items flatten as siblings in the innermost list and deeper quotes merge into the innermost
//! quote (with no visual marker at the flattening bound); past `64` flattened items or blocks in a
//! single container, the remaining lines are kept intact in one selectable, searchable plain block
//! preceded by a visible [`rune::Label::MoreNestedLines`] marker so no text is ever dropped.
//! `Markdown`'s own parser, direction pre-pass, plain text formatter and element builder run
//! iteratively (`O(1)` call stack), while Rune's commit flatten and layout recurse up to `~1,020`
//! elements on desktop (`~2.7 MiB` without `rune-taffy` stack continuation, `< 1 MiB` with it) and
//! `~770` elements on unguarded targets (`~1.05 MiB` of WebAssembly shadow stack). Web builds
//! should link with at least `-C link-arg=-zstack-size=2097152` (`2 MiB`), and native targets
//! without `stacker` should reserve at least `4 MiB` of UI-thread stack.
//!
//! **Keys.** Top-level blocks are keyed `b{index}` (or by [`Markdown::block_keys`]), list items
//! `i{index}` and their blocks by `b{index}` inside the item. Appending to a stream only ever
//! changes the last block or adds blocks after it, so earlier blocks keep their keys and never
//! remount. Give the `Markdown` root its own key when more than one is on screen.
//!
//! **Tables** size each column to its content. A table that doesn't fit shrinks its columns in
//! proportion to their content (wrapping cell text), down to each column's longest word capped at
//! [`MarkdownStyle::table_min_column`]; past that it clips. While a stream is inside a table (the
//! last row has no newline yet and [`Markdown::streaming`] is on), the row still arriving doesn't
//! widen columns, so columns move at most once per finished row. Otherwise the text is taken as
//! whole, and a table ending it sizes to every row. A trailing `| a | b |` line is already drawn
//! as the header it is about to become; while streaming, a trailing `| a` line that can't be one
//! yet draws nothing until it is a table or the next line shows it isn't.
//!
//! **Diagrams** (`mermaid` feature) draw from their complete lines while the fence is still
//! open, and the source shows as a code block only for what the subset doesn't cover. With
//! [`Markdown::streaming`] on, nothing shows until the first lines read (never a flash of the
//! source), then each new node fades and grows in, each edge draws on from its source to its
//! target, and what's there glides to its new place; the closing fence changes nothing. Off, a
//! diagram shows whole the frame it mounts.

use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rune::{Align, Color, Cursor, El, Family, Link, LinkPaint, Span, TextHighlight, TextStyle, Tx, col, el, icon, rich, row, text};

use crate::Kit;

mod autolink;
mod bidi;
mod cache;
mod emoji;
mod find;
mod highlight;
mod image;
mod interrupt;
mod link_icon;
mod links;
mod memo;
mod parser;
mod plain;
mod render;
mod stream;
mod style;
mod table;

pub use find::{Case, Rendered, find};
pub use image::ImageFrame;
pub use style::MarkdownStyle;
#[cfg(feature = "mermaid")] pub(crate) use style::DiagramPaint;

use crate::budget::Budget;
use crate::syntax::{Highlighter, SyntaxTheme};
pub use crate::syntax::Syntax;
use links::{CodeLink, LinkHelp, LinkLook, LinkRing, LinkWire};
pub use cache::parse_cached;
#[doc(hidden)]
pub use memo::{block_builds, reset_block_builds};
pub use highlight::{Highlight, HighlightItem};
pub use parser::parse;

use autolink::autolink_inline;
#[allow(unused_imports)]
use cache::{doc_cached, doc_cached_with_meta, stream_cached, stream_cached_with_meta};
#[cfg(test)]
use cache::{CACHE, PARSES};
use parser::{BlockMap, CLOSE, Cut, LINK_CLOSE, PAD, ParsedDoc, Source, SpanMap, parse_doc, unpadded};
use render::{Renderer, hash_of, rendered_keys, source_copy};
use stream::parse_streaming;

/// An inline style run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Code,
    /// The narrow no-break spaces around inline code (CSS's `padding: 0 4px`).
    CodePad,
    Strong,
    Em,
    Strike,
    Link,
    /// Inline math (`$…$`, `\(…\)`, `$$…$$`, `math` feature), its source as written: it draws
    /// rendered once ready, and as that source until then or when it fails.
    Math,
}

/// Text with styled and linked byte ranges.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Inline {
    pub text: String,
    /// Byte ranges into `text`.
    pub spans: Vec<(usize, usize, Style)>,
    /// Byte ranges into `text` and the destination of each link.
    pub links: Vec<(usize, usize, String)>,
}

/// A block of the parsed document.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Para(Inline),
    Heading(u8, Inline),
    Code(String),
    List { start: Option<u64>, items: Vec<Vec<Block>> },
    Quote(Vec<Block>),
    Rule,
    /// A GFM table.
    Table(Table),
    /// A ```` ```diff ```` (or `patch`) fence.
    Diff(String),
    /// A ```` ```mermaid ```` fence, closed or still arriving.
    Mermaid(String),
    /// An image (`![alt](src "title")`) that stands as a block of its own, as `markdown.css`'s
    /// `img { display: block }` sets one: a paragraph around it splits into the text before
    /// and after it. An image inside a link or emphasis stays its alt text in the line.
    Image { src: String, alt: String, title: String },
}

/// A GFM table. Rows may have fewer cells than there are columns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    /// One per column: `:--` and none are `Start`, `:-:` is `Center`, `--:` is `End`.
    pub align: Vec<Align>,
    pub head: Vec<Inline>,
    pub rows: Vec<Vec<Inline>>,
    /// The source ends inside the last row (or the header), so it may still grow.
    pub open: bool,
}

type TaskWire = Rc<dyn Fn(El, Rc<str>, usize, bool) -> El>;
#[cfg(feature = "mermaid")]
type FrameWire = Rc<dyn Fn(El, &DiagramFrame<'_>) -> El>;
type CodeFrameFn = Rc<dyn Fn(El, &CodeFrame<'_>) -> El>;
type DiffFrameFn = Rc<dyn Fn(El, &DiffFrame<'_>) -> El>;
type CodeWrapFn = Rc<dyn Fn(usize, &str) -> bool>;
pub(super) struct CodeWire {
    pub(super) frame: Option<CodeFrameFn>,
    pub(super) wrap: Option<CodeWrapFn>,
    pub(super) wrap_default: bool,
}
type QuoteWire = Rc<dyn Fn(El, &QuoteFrame<'_>) -> El>;

/// A block quote [`Markdown::quote_frame`] is wrapping.
#[non_exhaustive]
pub struct QuoteFrame<'a> {
    /// What it says, as copying a selection of all of it gives it: a blank line between
    /// paragraphs, list items after their markers, no quote marks.
    pub text: &'a str,
}

/// A code block [`Markdown::code_frame`] is wrapping.
#[non_exhaustive]
pub struct CodeFrame<'a> {
    /// Its place among the document's fenced blocks (code and diagrams), counted from 0, as
    /// a diagram's frame counts them. A stream only adds blocks after it.
    pub index: usize,
    /// The code it shows.
    pub source: &'a str,
    /// The fence's language as written (its info string's first word), if it has one.
    pub language: Option<&'a str>,
    /// The fence is still arriving.
    pub open: bool,
    /// Whether this block currently wraps long lines instead of scrolling sideways.
    pub wrapped: bool,
    pub(super) digest: Option<(u64, u64)>,
    pub(super) font: TextStyle,
    pub(super) text_width: Option<f32>,
    pub(super) inset: f32,
    pub(super) pad_x: f32,
}

impl CodeFrame<'_> {
    /// Whether at least one unwrapped line exceeds this block's inner width at `scale = 1.0`
    /// (`MarkdownStyle::text_width` less surrounding list/quote indents, padding, and snapped
    /// border). Computed lazily on demand and cached per `(content, font, width, font_epoch)`.
    /// Always `false` when `text_width` is unset or `source` is empty.
    pub fn overflows(&self) -> bool {
        self.overflows_at(1.0)
    }

    /// Like [`Self::overflows`], using the exact `border_layout()` border thickness for device
    /// `scale` (for example `2.0 pt` total at `1.0`, `1.333 pt` at `1.5`, `1.0 pt` at `2.0`).
    pub fn overflows_at(&self, scale: f32) -> bool {
        render::code_overflows_at(self.source, self.digest, self.font, self.text_width, self.inset, self.pad_x, scale)
    }
}

/// A diff block [`Markdown::diff_frame`] is wrapping.
///
/// ```
/// # use rune::{El, col};
/// # use rune_kit::Markdown;
/// # fn copy_button(_: &str) -> El { col() }
/// # let src = "";
/// # let _ =
/// Markdown::new(src).diff_frame(|card, d| {
///     col().key(format!("diff-{}", d.index)).child(copy_button(d.source)).child(card)
/// })
/// # ;
/// ```
#[non_exhaustive]
pub struct DiffFrame<'a> {
    /// Its place among the document's `diff` / `patch` fences, counted from 0. A stream only
    /// adds blocks after it, so this stays put while the reply arrives.
    pub index: usize,
    /// The raw patch text inside the fence.
    pub source: &'a str,
    /// The fence is still arriving (its closing fence has not arrived yet).
    pub open: bool,
}

/// A diagram [`Markdown::diagram_frame`] is wrapping.
#[cfg(feature = "mermaid")]
#[non_exhaustive]
pub struct DiagramFrame<'a> {
    /// Its place among the document's fenced blocks (code and diagrams), counted from 0. A
    /// stream only adds blocks after it, so this stays put while the reply arrives.
    pub index: usize,
    /// The lines it draws: the whole fence, or while the fence is still arriving, its
    /// complete lines.
    pub source: &'a str,
    /// The fence is still arriving.
    pub open: bool,
}

/// Markdown source as an element. See the module docs for cost and keys.
pub struct Markdown {
    src: String,
    style: Option<MarkdownStyle>,
    block_keys: Vec<(usize, String)>,
    highlights: Vec<(Vec<Range<usize>>, Highlight)>,
    autolink: bool,
    reveal: f32,
    streaming: bool,
    on_link: Option<LinkWire>,
    on_link_secondary: Option<LinkWire>,
    on_link_hover: Option<LinkWire>,
    link_menu: bool,
    link_look: Option<LinkLook>,
    link_help: Option<LinkHelp>,
    on_task: Option<TaskWire>,
    tasks_read_only: bool,
    link_ring: Option<LinkRing>,
    code_link: Option<CodeLink>,
    selectable: bool,
    copy_source: bool,
    scroll_wide: bool,
    wrap_code: bool,
    wrap_anywhere: bool,
    tentative: Option<rune::Direction>,
    highlighter: Highlighter,
    #[cfg(feature = "mermaid")]
    diagram_frame: Option<FrameWire>,
    code_frame: Option<CodeFrameFn>,
    diff_frame: Option<DiffFrameFn>,
    code_wrap: Option<CodeWrapFn>,
    quote_frame: Option<QuoteWire>,
    link_favicon: Option<link_icon::FaviconWire>,
    emoji: Option<emoji::EmojiWire>,
    images: Option<image::ImageWire>,
    on_image: Option<image::ImagePress>,
}

#[cfg(test)]
thread_local! {
    static RENDERS: Cell<usize> = const { Cell::new(0) };
}

impl Markdown {
    pub fn new(src: impl Into<String>) -> Markdown {
        Markdown { src: src.into(), style: None, block_keys: Vec::new(), highlights: Vec::new(), autolink: true, reveal: -1.0, streaming: false, on_link: None, on_link_secondary: None, on_link_hover: None, link_menu: true, link_look: None, link_help: None, link_ring: None, code_link: None,
            on_task: None, tasks_read_only: false, selectable: false, copy_source: false, scroll_wide: false, wrap_code: false, wrap_anywhere: false, tentative: None, highlighter: Highlighter::builtin(),
            #[cfg(feature = "mermaid")]
            diagram_frame: None, code_frame: None, diff_frame: None, code_wrap: None, quote_frame: None, link_favicon: None, emoji: None, images: None, on_image: None }
    }
    /// Highlight UTF-8 byte ranges of the Markdown source string (see [`find`]).
    ///
    /// Ranges refer to the source passed to [`Markdown::new`] (as `pulldown_cmark`'s offset
    /// iterator reports them) and map across inline formatting, code blocks, headings, lists,
    /// blockquotes and tables without reshaping text or shifting layout. Each highlight is
    /// reachable by [`Tx::scroll_into_view`](rune::Tx::scroll_into_view) and
    /// [`Cx::layout`](rune::Cx::layout) under its [`Highlight::key`], or under
    /// [`Markdown::highlight_key`]`(i)` when no key was set on it.
    /// Backgrounds, rings and foreground colours are paint-only. Matches add no layout nodes;
    /// [`Runtime::elements`](rune::Runtime::elements) reports a `mark` per rendered fragment.
    /// Empty/out-of-source ranges paint nothing; partial UTF-8 characters expand to boundaries.
    ///
    /// ```
    /// # use rune_kit::{Case, Kit, Markdown, markdown};
    /// # let (src, query) = ("Hello **world**", "world");
    /// # let kit = Kit::neutral(true);
    /// let view = Markdown::new(src).highlights(markdown::find(src, query, Case::Insensitive)).build(&kit);
    /// ```
    pub fn highlights(mut self, highlights: impl IntoIterator<Item = impl HighlightItem>) -> Self {
        self.highlights = highlights.into_iter().map(HighlightItem::into_ranges).collect();
        self
    }
    /// Default key (`"md-mark-{index}"`) for the `index`-th highlight in [`Self::highlights`]
    /// when [`Highlight::key`] was not set on it.
    pub fn highlight_key(index: usize) -> String {
        format!("md-mark-{index}")
    }
    /// Whether bare `https://…`, `http://…` and `www.…` URLs in plain text are autolinked
    /// (`true` by default).
    /// This is URL-only: bare email addresses remain text; `<name@example.org>` still links.
    pub fn autolink(mut self, on: bool) -> Self {
        self.autolink = on;
        self
    }
    /// Stable keys as `(source_byte_offset, key)` pairs. The offset is the start of the first
    /// source line of a rendered top-level block (including any indentation), not its ordinal.
    /// Recompute offsets against the current source after edits; keep the IDs themselves stable.
    /// Unmatched offsets are ignored: differing parser boundaries cannot shift later IDs.
    /// Missing, empty or duplicate IDs fall back to an unused `b{index}`. For duplicate offsets,
    /// the first pair wins. Nested blocks keep index keys beneath their top-level parent's key.
    /// Retained blocks spring to new positions and sizes; additions enter and removals stay
    /// mounted during their exit transition.
    pub fn block_keys<I, K>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = (usize, K)>,
        K: Into<String>,
    {
        self.block_keys = keys.into_iter().map(|(at, key)| (at, key.into())).collect();
        self
    }
    /// Replaces the kit-derived style in [`Self::build`].
    pub fn style(mut self, style: MarkdownStyle) -> Self {
        self.style = Some(style);
        self
    }
    /// Streaming word reveal in seconds (see [`El::reveal`]); 0 shows words at once.
    pub fn reveal(mut self, word_fade: f32) -> Self {
        self.reveal = word_fade.max(0.0);
        self
    }
    /// The source is still arriving: a table's last row, with no newline after it yet, is left
    /// out of the column widths until it ends, and a diagram brings in each node and edge as
    /// its line arrives. The tail draws as what it's about to become: an emphasis, strike or
    /// code span still open draws styled (never its markers), markers at the very end wait for
    /// the next character, and a trailing `| …` line waits until it's a table or isn't. Off,
    /// the text is whole.
    pub fn streaming(mut self, on: bool) -> Self {
        self.streaming = on;
        self
    }
    /// Called when a GFM task list item (`- [ ]` or `- [x]`) is clicked, or pressed with Space
    /// while focused: `(state, block_key, marker_offset, checked)`. Enter does not toggle.
    /// `block_key` is the top-level block's key ([`Self::block_keys`], or `b{index}`),
    /// `marker_offset` is the byte offset of `[` in the source's `[ ]`/`[x]` marker, including
    /// nested items, and `checked` is the new state. The offset refers to the source used to
    /// build this view, so the caller can replace `source[offset + 1..offset + 2]`.
    /// Without a handler, or with [`Self::tasks_read_only`], task checkboxes
    /// are read-only. Shares its slot with [`Self::on_task_tx`] (the later call wins).
    pub fn on_task<S: 'static>(self, f: impl Fn(&mut S, &str, usize, bool) + 'static) -> Self {
        self.on_task_tx(move |s: &mut S, key, item, checked, _| f(s, key, item, checked))
    }
    /// [`Self::on_task`] with the [`Tx`] of the press. Replaces any earlier `on_task` or
    /// `on_task_tx` handler (the later call wins).
    pub fn on_task_tx<S: 'static>(mut self, f: impl Fn(&mut S, &str, usize, bool, &mut Tx) + 'static) -> Self {
        let f = Rc::new(f);
        self.on_task = Some(Rc::new(move |e: El, key: Rc<str>, item: usize, checked: bool| {
            let f = f.clone();
            e.on_press_tx(move |s: &mut S, tx| f(s, &key, item, checked, tx))
        }));
        self
    }
    /// Keeps GFM task list checkboxes read-only (unfocusable and unpressable), even when an
    /// [`Self::on_task`] or [`Self::on_task_tx`] handler was set.
    pub fn tasks_read_only(mut self) -> Self {
        self.tasks_read_only = true;
        self
    }
    /// Lets people select and copy the text (see [`El::selectable`]) as one selection across
    /// paragraphs, headings, list items, quotes, code blocks and table cells
    /// ([`El::selection_group`]): Cmd+A selects the whole block, and Cmd+C copies the text as
    /// drawn, laid out as plain text: a blank line around paragraphs, headings, quotes and code
    /// blocks, list items on their own lines after `- `, `1. ` or `- [ ] ` (a nested list indented
    /// past its parent's marker), and a tab between table cells, without code fences or quote marks (or the
    /// source, with [`Self::copy_source`]). Links still open on a click.
    /// The highlight is [`MarkdownStyle::selection`].
    pub fn selectable(mut self, on: bool) -> Self {
        self.selectable = on;
        self
    }

    /// With [`Self::selectable`], copying a selection copies the Markdown source instead of the
    /// text as drawn ([`El::selection_copy`]), by whole top-level blocks: each block the
    /// selection touches, from its first line to the next block, without trailing blank
    /// lines, and the whole source verbatim when it touches every block (Cmd+A, Cmd+C). A
    /// selection inside one block that leaves part of it out (a word, a line, one list item)
    /// still copies the text as drawn.
    pub fn copy_source(mut self, on: bool) -> Self {
        self.copy_source = on;
        self
    }

    /// The direction the document reads in until its first strong character arrives (and a block
    /// with none in it, before any block has one): a reply's, the direction of the prompt it
    /// answers. Each block then reads the way its own words do, a block with none the way the
    /// block before it does. Code and diffs read left to right always. Without it that's the
    /// direction around the document.
    pub fn dir_tentative(mut self, dir: rune::Direction) -> Self {
        self.tentative = Some(dir);
        self
    }
    /// Code blocks keep their lines whole and tables their columns' natural widths, each
    /// scrolling sideways in its own [`El::scroll_x`] viewport when wider than the text column,
    /// with soft left and right edges on code. Off (the default), code wraps and columns shrink.
    pub fn scroll_wide(mut self, on: bool) -> Self {
        self.scroll_wide = on;
        self
    }

    /// Breaks overly long unbroken tokens (URLs, hashes, file paths) inside a word when the
    /// token alone would overflow its line (CSS `overflow-wrap: anywhere`), while normal words
    /// still break at spaces (`false` by default). Applies to paragraphs, headings, lists,
    /// block quotes, table cells, diff fences, and code fences when [`Self::scroll_wide`] is off.
    ///
    /// ```
    /// # use rune_kit::{Kit, Markdown};
    /// # let src = "";
    /// # let kit = Kit::neutral(true);
    /// let view = Markdown::new(src).wrap_anywhere(true).build(&kit);
    /// ```
    pub fn wrap_anywhere(mut self, on: bool) -> Self {
        self.wrap_anywhere = on;
        self
    }

    /// Draws `f(card, frame)` in place of each diagram's card, so an app can put its own
    /// controls on it (a button that opens it larger) or key it. Fences that draw as their
    /// source, or nothing yet while streaming, don't come here.
    ///
    /// ```
    /// # use rune::{El, col};
    /// # use rune_kit::Markdown;
    /// # fn open_button(_: &str) -> El { col() }
    /// # let src = "";
    /// # let _ =
    /// Markdown::new(src).diagram_frame(|card, d| col().key(format!("diagram-{}", d.index)).child(card).child(open_button(d.source)))
    /// # ;
    /// ```
    #[cfg(feature = "mermaid")]
    pub fn diagram_frame(mut self, f: impl Fn(El, &DiagramFrame<'_>) -> El + 'static) -> Self {
        self.diagram_frame = Some(Rc::new(f));
        self
    }

    /// Draws `f(card, frame)` in place of each code block's card, so an app can put its own
    /// controls on it (a Copy button). Diagrams that draw as their source and diff fences don't
    /// come here (see [`Self::diff_frame`]).
    ///
    /// ```
    /// # use rune::{El, col};
    /// # use rune_kit::Markdown;
    /// # fn copy_button(_: &str) -> El { col() }
    /// # let src = "";
    /// # let _ =
    /// Markdown::new(src).code_frame(|card, c| col().key(format!("code-{}", c.index)).child(card).child(copy_button(c.source)))
    /// # ;
    /// ```
    pub fn code_frame(mut self, f: impl Fn(El, &CodeFrame<'_>) -> El + 'static) -> Self {
        self.code_frame = Some(Rc::new(f));
        self
    }

    /// Draws `f(card, frame)` in place of each ```` ```diff ```` (or `patch`) fence's card, so an
    /// app can put its own controls on it (a Copy patch button) or add a header. Ordinary code
    /// fences still go to [`Self::code_frame`].
    ///
    /// ```
    /// # use rune::{El, col};
    /// # use rune_kit::Markdown;
    /// # fn copy_button(_: &str) -> El { col() }
    /// # let src = "";
    /// # let _ =
    /// Markdown::new(src).diff_frame(|card, d| col().key(format!("diff-{}", d.index)).child(copy_button(d.source)).child(card))
    /// # ;
    /// ```
    pub fn diff_frame(mut self, f: impl Fn(El, &DiffFrame<'_>) -> El + 'static) -> Self {
        self.diff_frame = Some(Rc::new(f));
        self
    }

    /// Wraps long lines in code blocks (`white-space: pre-wrap; overflow-wrap: anywhere`,
    /// overriding [`Self::scroll_wide`] for code blocks) instead of scrolling them sideways
    /// (`false` by default). Wrapped blocks keep per-line shaping (`El::by_line`), syntax
    /// highlighting, Find highlights, screen-reader line runs, and a stable text `NodeId` path
    /// across toggles so reveal does not replay and active selection stays intact.
    pub fn wrap_code(mut self, wrap: bool) -> Self {
        self.wrap_code = wrap;
        self
    }

    /// Per-block override for code-block wrapping: called with `(block_index, source)` (where
    /// `block_index` counts both code and Mermaid blocks, matching [`CodeFrame::index`]) and
    /// returns whether that block wraps long lines (overriding [`Self::wrap_code`]).
    pub fn code_wrap(mut self, f: impl Fn(usize, &str) -> bool + 'static) -> Self {
        self.code_wrap = Some(Rc::new(f));
        self
    }

    /// Draws `f(quote, frame)` in place of each block quote, so an app can put its own controls
    /// on it (a button that copies the whole quote). Quotes inside a quote come here too.
    ///
    /// ```
    /// # use rune::{El, col};
    /// # use rune_kit::Markdown;
    /// # fn copy_button(_: &str) -> El { col() }
    /// # let src = "";
    /// # let _ =
    /// Markdown::new(src).quote_frame(|quote, q| col().child(quote).child(copy_button(q.text)))
    /// # ;
    /// ```
    pub fn quote_frame(mut self, f: impl Fn(El, &QuoteFrame<'_>) -> El + 'static) -> Self {
        self.quote_frame = Some(Rc::new(f));
        self
    }

    /// Colours code blocks with `f` instead of the built-in highlighter: it gets a fence's language
    /// as written (its info string's first word; `None` for an indented block or a bare fence)
    /// and the code, and returns byte ranges of the code with a token kind, which the style's
    /// [`SyntaxTheme`] colours, or a colour of its own. See [`Highlighter`] for caching, and
    /// [`syntax::builtin`](crate::syntax::builtin) to fall back on for other languages.
    ///
    /// ```
    /// # use rune_kit::{Markdown, syntax::Highlight};
    /// # fn my_cobol(_: &str) -> Vec<Highlight> { Vec::new() }
    /// # let src = "";
    /// # let _ =
    /// Markdown::new(src).highlight(|lang, code| match lang {
    ///     Some("cobol") => my_cobol(code),
    ///     _ => rune_kit::syntax::builtin(lang, code),
    /// })
    /// # ;
    /// ```
    pub fn highlight(self, f: impl Fn(Option<&str>, &str) -> Vec<crate::syntax::Highlight> + 'static) -> Self {
        self.highlighter(Highlighter::new(f))
    }

    /// Colours code blocks with `h` (by default [`Highlighter::builtin`]).
    pub fn highlighter(mut self, h: Highlighter) -> Self {
        self.highlighter = h;
        self
    }

    pub fn build(mut self, k: &Kit) -> El {
        let style = self.style.take().unwrap_or_else(|| MarkdownStyle::from_kit(k));
        self.render(&style)
    }

    /// Builds with `style`, for apps without a [`Kit`] (see [`MarkdownStyle::neutral`]).
    pub fn render(self, style: &MarkdownStyle) -> El {
        #[cfg(feature = "math")]
        let has_math = crate::math::may_have_math(&self.src);
        #[cfg(not(feature = "math"))]
        let has_math = false;
        let can_cache = !self.streaming
            && self.on_link.is_none()
            && self.on_link_secondary.is_none()
            && self.on_link_hover.is_none()
            && self.link_look.is_none()
            && self.link_help.is_none()
            && self.link_ring.is_none()
            && self.code_link.is_none()
            && self.on_task.is_none()
            && !(self.selectable && self.copy_source)
            && self.highlighter.is_builtin()
            && self.code_frame.is_none()
            && self.diff_frame.is_none()
            && self.code_wrap.is_none()
            && self.quote_frame.is_none()
            && self.link_favicon.is_none()
            && self.emoji.is_none()
            && self.images.is_none()
            && self.on_image.is_none()
            && !(cfg!(feature = "mermaid") && self.src.contains("mermaid"))
            && !has_math;
        if can_cache {
            let h = rune::memo::digest(&(
                &self.src,
                style,
                &self.block_keys,
                &self.highlights,
                self.reveal.to_bits(),
                [
                    self.autolink,
                    self.tasks_read_only,
                    self.link_menu,
                    self.selectable,
                    self.scroll_wide,
                    self.wrap_code,
                    self.wrap_anywhere,
                ],
                self.tentative,
            ));
            return rune::memo::cached(h, || self.render_uncached(style));
        }
        self.render_uncached(style)
    }

    fn render_uncached(self, style: &MarkdownStyle) -> El {
        #[cfg(test)]
        RENDERS.with(|n| n.set(n.get() + 1));
        let ((blocks, sources, tasks, starts, maps), meta) = match (self.autolink, self.streaming) {
            (true, false) => doc_cached_with_meta(&self.src),
            (true, true) => stream_cached_with_meta(&self.src),
            (false, streaming) => {
                let (b, s, t, _, st, m) = if streaming { parse_streaming(&self.src, false) } else { parse_doc(&self.src, false) };
                let meta: Rc<[Rc<memo::PreBlock>]> = b.iter().enumerate().map(|(i, blk)| memo::precompute_block(blk, m.get(i))).collect();
                ((Rc::new(b), s.into(), t.into(), st.into(), m.into()), meta)
            }
        };
        let keys = if self.block_keys.is_empty() { Vec::new() } else {
            starts.iter().map(|at| self.block_keys.iter().find(|(offset, _)| offset == at)
                .map_or_else(String::new, |(_, key)| key.clone())).collect::<Vec<_>>()
        };
        let dirs = bidi::Dirs::new(&blocks);
        let root_dir = dirs.of_slice(&blocks).or(self.tentative);
        let code_frame = (self.code_frame.is_some() || self.code_wrap.is_some() || self.wrap_code)
            .then(|| CodeWire { frame: self.code_frame, wrap: self.code_wrap, wrap_default: self.wrap_code });
        let r = Renderer { s: style.clone(), highlights: self.highlights, reveal: self.reveal, streaming: self.streaming, on_link: self.on_link, on_link_secondary: self.on_link_secondary, on_link_hover: self.on_link_hover, link_menu: self.link_menu, link_look: self.link_look, link_help: self.link_help, link_ring: self.link_ring, code_link: self.code_link,
            on_task: self.on_task, tasks_read_only: self.tasks_read_only, selectable: self.selectable, scroll_wide: self.scroll_wide, wrap_anywhere: self.wrap_anywhere, tentative: self.tentative,
            highlighter: self.highlighter, sources, next_source: Cell::new(0), next_diff: Cell::new(0), tasks, next_task: Cell::new(0),
            current_block_key: RefCell::new("b0".into()),
            #[cfg(feature = "mermaid")]
            diagram_frame: self.diagram_frame, code_frame, diff_frame: self.diff_frame, quote_frame: self.quote_frame, link_favicon: self.link_favicon, emoji: self.emoji, images: self.images, on_image: self.on_image, flow: Cell::default() };
        // The document reads the way its first words do; until any arrive, the way it was asked.
        let root = memo::with_renderer(&r, || {
            bidi::directed(r.blocks_with_dirs(&blocks, Some(&meta), &maps, &keys, true, &dirs), root_dir).selection_group(self.selectable)
        });
        if !(self.selectable && self.copy_source) { return root; }
        let names: Rc<[String]> = rendered_keys(blocks.len(), &keys).into();
        let src: Rc<str> = self.src.into();
        root.selection_copy(move |sel| source_copy(&src, &starts, &names, sel))
    }
}

#[cfg(test)]
mod style_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wrap_diff_tests;
