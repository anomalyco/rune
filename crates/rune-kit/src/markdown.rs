//! Markdown (pulldown-cmark): headings, paragraphs, emphasis, inline code, code blocks, lists
//! (with GFM task checkboxes; see [`Markdown::on_task`]), block quotes, rules, links, GFM tables,
//! ```` ```diff ```` fences and (with the `mermaid` feature) ```` ```mermaid ```` diagrams,
//! rendered from a [`MarkdownStyle`]. Fences in rust,
//! js, ts, tsx, json, python, go, swift, sh, sql, html, css and yaml are syntax-coloured by
//! [`crate::syntax`], or by an app's own highlighter ([`Markdown::highlight`]). Partial input
//! while streaming is fine: an unclosed fence is a code block that keeps growing.
//!
//! ```ignore
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
//! as the header it is about to become.
//!
//! **Diagrams** (`mermaid` feature) render once their fence closes; until then, and for anything
//! the subset doesn't cover, the source shows as a code block.

use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rune::{Align, Color, Cursor, El, Family, Link, LinkPaint, Span, TextHighlight, TextStyle, Tx, col, el, icon, rich, row, text};

use crate::Kit;

mod autolink;
mod cache;
mod find;
mod highlight;
mod links;
mod parser;
mod render;
mod style;
mod table;

pub use find::{Case, find};
pub use style::MarkdownStyle;
#[cfg(feature = "mermaid")] pub(crate) use style::DiagramPaint;

use crate::budget::Budget;
use crate::syntax::{Highlighter, SyntaxTheme};
pub use crate::syntax::Syntax;
use links::{LinkLook, LinkRing, LinkWire};
pub use cache::parse_cached;
pub use highlight::{Highlight, HighlightItem};
pub use parser::parse;

use autolink::autolink_inline;
use cache::doc_cached;
#[cfg(test)]
use cache::{CACHE, PARSES};
use parser::{BlockMap, Cut, PAD, Source, SpanMap, parse_doc};
use render::{Renderer, hash_of, rendered_keys, source_copy};

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
    /// A closed ```` ```mermaid ```` fence (an unclosed one is still [`Block::Code`]).
    Mermaid(String),
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
    link_look: Option<LinkLook>,
    on_task: Option<TaskWire>,
    tasks_read_only: bool,
    link_ring: Option<LinkRing>,
    selectable: bool,
    copy_source: bool,
    scroll_wide: bool,
    highlighter: Highlighter,
}

impl Markdown {
    pub fn new(src: impl Into<String>) -> Markdown {
        Markdown { src: src.into(), style: None, block_keys: Vec::new(), highlights: Vec::new(), autolink: true, reveal: 0.0, streaming: false, on_link: None, on_link_secondary: None, link_look: None, link_ring: None,
            on_task: None, tasks_read_only: false, selectable: false, copy_source: false, scroll_wide: false, highlighter: Highlighter::builtin() }
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
    /// ```ignore
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
        self.reveal = word_fade;
        self
    }
    /// The source is still arriving: a table's last row, with no newline after it yet, is left
    /// out of the column widths until it ends. Off, the text is whole.
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
    /// drawn, a newline between blocks and a tab between table cells, without list markers or
    /// code fences (or the source, with [`Self::copy_source`]). Links still open on a click.
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

    /// Code blocks keep their lines whole and tables their columns' natural widths, each
    /// scrolling sideways in its own [`El::scroll_x`] viewport when wider than the text column,
    /// with soft left and right edges on code. Off (the default), code wraps and columns shrink.
    pub fn scroll_wide(mut self, on: bool) -> Self {
        self.scroll_wide = on;
        self
    }

    /// Colours code blocks with `f` instead of the built-in scanners: it gets a fence's language
    /// as written (its info string's first word; `None` for an indented block or a bare fence)
    /// and the code, and returns byte ranges of the code with a token kind, which the style's
    /// [`SyntaxTheme`] colours, or a colour of its own. See [`Highlighter`] for caching, and
    /// [`syntax::builtin`](crate::syntax::builtin) to fall back on for other languages.
    ///
    /// ```ignore
    /// Markdown::new(src).highlight(|lang, code| match lang {
    ///     Some("kotlin") => my_kotlin(code),
    ///     _ => rune_kit::syntax::builtin(lang, code),
    /// })
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

    /// Builds with `style`, for apps without a [`Kit`] (see [`MarkdownStyle::neutral`] and
    /// [`MarkdownStyle::from_seed`]).
    pub fn render(self, style: &MarkdownStyle) -> El {
        let (blocks, sources, tasks, starts, maps) = if self.autolink {
            doc_cached(&self.src)
        } else {
            let (b, s, t, _, st, m) = parse_doc(&self.src, false);
            (Rc::new(b), s.into(), t.into(), st.into(), m.into())
        };
        let keys = if self.block_keys.is_empty() { Vec::new() } else {
            starts.iter().map(|at| self.block_keys.iter().find(|(offset, _)| offset == at)
                .map_or_else(String::new, |(_, key)| key.clone())).collect::<Vec<_>>()
        };
        let r = Renderer { s: style.clone(), highlights: self.highlights, reveal: self.reveal, streaming: self.streaming, on_link: self.on_link, on_link_secondary: self.on_link_secondary, link_look: self.link_look, link_ring: self.link_ring,
            on_task: self.on_task, tasks_read_only: self.tasks_read_only, selectable: self.selectable, scroll_wide: self.scroll_wide,
            highlighter: self.highlighter, sources, next_source: Cell::new(0), tasks, next_task: Cell::new(0),
            current_block_key: RefCell::new("b0".into()) };
        let root = r.blocks(&blocks, &maps, &keys, true).selection_group(self.selectable);
        if !(self.selectable && self.copy_source) { return root; }
        let names: Rc<[String]> = rendered_keys(blocks.len(), &keys).into();
        let src: Rc<str> = self.src.into();
        root.selection_copy(move |sel| source_copy(&src, &starts, &names, sel))
    }
}

#[cfg(test)]
mod tests;
