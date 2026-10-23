//! Markdown (pulldown-cmark): headings, paragraphs, emphasis, inline code, code blocks, lists,
//! block quotes, rules and links, rendered from a [`Kit`]'s tokens. Partial input while
//! streaming is fine: an unclosed fence is a code block that keeps growing.
//!
//! ```ignore
//! let view = Markdown::new(&message.text).reveal(0.18).on_link(|s: &mut App, url| s.open(url)).build(&kit);
//! ```
//!
//! **Cost.** [`parse_cached`] keeps the parse of the last few source strings, so re-rendering
//! unchanged text (every frame of an animation, a list of finished messages) parses nothing.
//! A growing stream parses once per distinct string, which is the unavoidable minimum.
//!
//! **Keys.** Top-level blocks are keyed `b{index}`, list items `i{index}` and their blocks by
//! the same rule inside the item. Appending to a stream only ever changes the last block or
//! adds blocks after it, so earlier blocks keep their keys and never remount. Give the
//! `Markdown` root its own key when more than one is on screen.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rune::{Align, Color, El, Span, TextStyle, col, el, rich, row, text};

use crate::Kit;

/// An inline style run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Code,
    /// The narrow no-break spaces around inline code (CSS's `padding: 0 4px`).
    CodePad,
    Strong,
    Em,
    Link,
}

/// Text with styled and linked byte ranges.
#[derive(Clone, Debug, Default, PartialEq)]
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
}

struct Builder {
    /// Open containers: (kind, blocks so far).
    stack: Vec<(Container, Vec<Block>)>,
    inline: Option<(InlineKind, Inline, Vec<Open>)>,
    code: Option<String>,
}

/// An emphasis, strong or link run that has started and not yet ended.
struct Open {
    start: usize,
    style: Style,
    url: Option<String>,
}

enum Container {
    Root,
    Quote,
    List(Option<u64>, Vec<Vec<Block>>),
    Item,
}

#[derive(Clone, Copy)]
enum InlineKind {
    Para,
    Heading(u8),
}

impl Builder {
    fn push(&mut self, b: Block) {
        self.stack.last_mut().unwrap().1.push(b);
    }

    fn inline(&mut self) -> &mut (InlineKind, Inline, Vec<Open>) {
        // Tight list items have text without a paragraph.
        self.inline.get_or_insert_with(|| (InlineKind::Para, Inline::default(), Vec::new()))
    }

    fn flush_inline(&mut self) {
        if let Some((kind, mut inl, open)) = self.inline.take() {
            for o in open {
                let end = inl.text.len();
                if let Some(url) = o.url {
                    inl.links.push((o.start, end, url));
                }
                inl.spans.push((o.start, end, o.style));
            }
            let trimmed = inl.text.trim_end().len();
            inl.text.truncate(trimmed);
            inl.spans.retain_mut(|s| {
                s.1 = s.1.min(trimmed);
                s.0 < s.1
            });
            inl.links.retain_mut(|s| {
                s.1 = s.1.min(trimmed);
                s.0 < s.1
            });
            if inl.text.is_empty() {
                return;
            }
            self.push(match kind {
                InlineKind::Para => Block::Para(inl),
                InlineKind::Heading(l) => Block::Heading(l, inl),
            });
        }
    }
}

const PAD: &str = "\u{202f}";

/// Parses `src` into blocks. Unclosed constructs are closed at the end of input.
pub fn parse(src: &str) -> Vec<Block> {
    let mut b = Builder { stack: vec![(Container::Root, Vec::new())], inline: None, code: None };
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    b.flush_inline();
                    b.inline = Some((InlineKind::Para, Inline::default(), Vec::new()));
                }
                Tag::Heading { level, .. } => {
                    b.flush_inline();
                    let l = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        _ => 4,
                    };
                    b.inline = Some((InlineKind::Heading(l), Inline::default(), Vec::new()));
                }
                Tag::CodeBlock(_) => {
                    b.flush_inline();
                    b.code = Some(String::new());
                }
                Tag::BlockQuote(_) => {
                    b.flush_inline();
                    b.stack.push((Container::Quote, Vec::new()));
                }
                Tag::List(start) => {
                    b.flush_inline();
                    b.stack.push((Container::List(start, Vec::new()), Vec::new()));
                }
                Tag::Item => {
                    b.flush_inline();
                    b.stack.push((Container::Item, Vec::new()));
                }
                Tag::Strong => open(&mut b, Style::Strong, None),
                Tag::Emphasis => open(&mut b, Style::Em, None),
                Tag::Link { dest_url, .. } => open(&mut b, Style::Link, Some(dest_url.into_string())),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) => b.flush_inline(),
                TagEnd::CodeBlock => {
                    if let Some(mut text) = b.code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        b.push(Block::Code(text));
                    }
                }
                TagEnd::BlockQuote(_) => {
                    b.flush_inline();
                    if let Some((_, blocks)) = b.stack.pop() {
                        b.push(Block::Quote(blocks));
                    }
                }
                TagEnd::Item => {
                    b.flush_inline();
                    if let Some((_, blocks)) = b.stack.pop()
                        && let Some((Container::List(_, items), _)) = b.stack.last_mut()
                    {
                        items.push(blocks);
                    }
                }
                TagEnd::List(_) => {
                    b.flush_inline();
                    if let Some((Container::List(start, items), _)) = b.stack.pop() {
                        b.push(Block::List { start, items });
                    }
                }
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Link => close(&mut b),
                _ => {}
            },
            Event::Text(t) => {
                if let Some(code) = &mut b.code {
                    code.push_str(&t);
                } else {
                    b.inline().1.text.push_str(&t);
                }
            }
            Event::Code(t) => {
                let inl = &mut b.inline().1;
                let a = inl.text.len();
                inl.text.push_str(PAD);
                let start = inl.text.len();
                inl.text.push_str(&t);
                let end = inl.text.len();
                inl.text.push_str(PAD);
                inl.spans.push((a, start, Style::CodePad));
                inl.spans.push((start, end, Style::Code));
                inl.spans.push((end, inl.text.len(), Style::CodePad));
            }
            Event::SoftBreak => b.inline().1.text.push(' '),
            Event::HardBreak => b.inline().1.text.push('\n'),
            Event::Rule => {
                b.flush_inline();
                b.push(Block::Rule);
            }
            _ => {}
        }
    }
    b.flush_inline();
    if let Some(text) = b.code.take() {
        b.push(Block::Code(text.trim_end_matches('\n').to_owned()));
    }
    // Unwind containers left open by a partial stream.
    while b.stack.len() > 1 {
        let (c, blocks) = b.stack.pop().unwrap();
        match c {
            Container::Item => {
                if let Some((Container::List(_, items), _)) = b.stack.last_mut() {
                    items.push(blocks);
                }
            }
            Container::List(start, items) => b.push(Block::List { start, items }),
            Container::Quote => b.push(Block::Quote(blocks)),
            Container::Root => {}
        }
    }
    b.stack.pop().unwrap().1
}

fn open(b: &mut Builder, style: Style, url: Option<String>) {
    let inl = b.inline();
    let start = inl.1.text.len();
    inl.2.push(Open { start, style, url });
}

fn close(b: &mut Builder) {
    let inl = b.inline();
    if let Some(o) = inl.2.pop() {
        let end = inl.1.text.len();
        if let Some(url) = o.url {
            inl.1.links.push((o.start, end, url));
        }
        inl.1.spans.push((o.start, end, o.style));
    }
}


const CACHE_LEN: usize = 32;

#[derive(Default)]
struct Cache {
    map: HashMap<String, Rc<Vec<Block>>>,
    order: VecDeque<String>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// [`parse`] memoised by source on this thread (the last 32 distinct strings).
pub fn parse_cached(src: &str) -> Rc<Vec<Block>> {
    CACHE.with_borrow_mut(|c| {
        if let Some(hit) = c.map.get(src) {
            return hit.clone();
        }
        let blocks = Rc::new(parse(src));
        if c.order.len() >= CACHE_LEN
            && let Some(old) = c.order.pop_front()
        {
            c.map.remove(&old);
        }
        c.order.push_back(src.to_owned());
        c.map.insert(src.to_owned(), blocks.clone());
        blocks
    })
}

/// How markdown looks. [`MarkdownStyle::from_kit`] takes every colour and font from the kit's
/// tokens; each builder overrides one thing.
#[derive(Clone, Debug)]
pub struct MarkdownStyle {
    text: Color,
    muted: Color,
    accent: Color,
    link_underline: Color,
    code_bg: Color,
    code_block_fill: Color,
    code_block_border: Color,
    quote_rule: Color,
    rule: Color,
    body: TextStyle,
    headings: [TextStyle; 3],
    code_block: TextStyle,
    inline_code_scale: f32,
    inline_code_weight: f32,
    strong_weight: f32,
}

impl MarkdownStyle {
    pub fn from_kit(k: &Kit) -> MarkdownStyle {
        let c = &k.c;
        let body = TextStyle { line_height: 22.4, ..k.ty.body };
        let heading = |size, lh| TextStyle { size, weight: 600.0, line_height: lh, ..body };
        MarkdownStyle {
            text: c.text_base,
            muted: c.text_muted,
            accent: c.text_accent,
            link_underline: k.alpha(c.text_accent, 0.4),
            code_bg: k.alpha(c.text_base, 0.08),
            code_block_fill: c.layer_01,
            code_block_border: c.border_muted,
            quote_rule: c.layer_03,
            rule: c.border_base,
            body,
            headings: [heading(17.0, 24.0), heading(15.0, 24.0), heading(13.0, 20.0)],
            code_block: TextStyle { size: 13.0, line_height: 20.0, ..k.ty.mono },
            inline_code_scale: 0.9,
            inline_code_weight: 400.0,
            strong_weight: 670.0,
        }
    }
    /// Body, heading (levels 1 to 3 and the muted level 4+) and emphasis colour.
    pub fn text(mut self, c: Color) -> Self {
        self.text = c;
        self
    }
    /// Level 4+ headings and list markers.
    pub fn muted(mut self, c: Color) -> Self {
        self.muted = c;
        self
    }
    /// Link text; the underline is the same colour at 40% unless set with [`Self::link_underline`].
    pub fn link(mut self, c: Color) -> Self {
        self.accent = c;
        self.link_underline = c.alpha(0.4);
        self
    }
    pub fn link_underline(mut self, c: Color) -> Self {
        self.link_underline = c;
        self
    }
    /// The chip behind inline code.
    pub fn inline_code_bg(mut self, c: Color) -> Self {
        self.code_bg = c;
        self
    }
    pub fn code_block(mut self, fill: Color, border: Color) -> Self {
        self.code_block_fill = fill;
        self.code_block_border = border;
        self
    }
    pub fn quote_rule(mut self, c: Color) -> Self {
        self.quote_rule = c;
        self
    }
    pub fn rule(mut self, c: Color) -> Self {
        self.rule = c;
        self
    }
    /// Paragraphs, list items and list markers.
    pub fn body_font(mut self, f: TextStyle) -> Self {
        self.body = f;
        self
    }
    /// The font of heading `level` (1 to 3; level 4 and deeper share 3).
    pub fn heading_font(mut self, level: u8, f: TextStyle) -> Self {
        self.headings[(level.clamp(1, 3) - 1) as usize] = f;
        self
    }
    pub fn code_block_font(mut self, f: TextStyle) -> Self {
        self.code_block = f;
        self
    }
    /// Inline code's size as a multiple of the surrounding text, and its weight (the family is
    /// always the mono face).
    pub fn inline_code_font(mut self, scale: f32, weight: f32) -> Self {
        self.inline_code_scale = scale;
        self.inline_code_weight = weight;
        self
    }
    pub fn strong_weight(mut self, w: f32) -> Self {
        self.strong_weight = w;
        self
    }
}

type LinkWire = Rc<dyn Fn(El, &str) -> El>;

/// Markdown source as an element. See the module docs for cost and keys.
pub struct Markdown {
    src: String,
    style: Option<MarkdownStyle>,
    reveal: f32,
    on_link: Option<LinkWire>,
}

impl Markdown {
    pub fn new(src: impl Into<String>) -> Markdown {
        Markdown { src: src.into(), style: None, reveal: 0.0, on_link: None }
    }
    /// Replaces the kit-derived style.
    pub fn style(mut self, style: MarkdownStyle) -> Self {
        self.style = Some(style);
        self
    }
    /// Streaming word reveal in seconds (see [`El::reveal`]); 0 shows words at once.
    pub fn reveal(mut self, word_fade: f32) -> Self {
        self.reveal = word_fade;
        self
    }
    /// Called with a link's destination when it is pressed. Links are styled either way, but
    /// pressing needs clickable rich-text spans in the runtime: until they exist this handler
    /// is stored and not yet called (see `Renderer::wire_link`, the one place to connect it).
    pub fn on_link<S: 'static>(mut self, f: impl Fn(&mut S, &str) + 'static) -> Self {
        let f = Rc::new(f);
        self.on_link = Some(Rc::new(move |e: El, url: &str| {
            let (f, url) = (f.clone(), url.to_owned());
            e.on_press(move |s: &mut S| f(s, &url))
        }));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let style = self.style.unwrap_or_else(|| MarkdownStyle::from_kit(k));
        let r = Renderer { s: style, reveal: self.reveal, on_link: self.on_link };
        r.blocks(&parse_cached(&self.src))
    }
}

struct Renderer {
    s: MarkdownStyle,
    reveal: f32,
    on_link: Option<LinkWire>,
}

/// (margin-top, margin-bottom) per block, collapsed like CSS margins.
fn margins(b: &Block) -> (f32, f32) {
    match b {
        Block::Para(_) => (0.0, 12.0),
        Block::Heading(1, _) => (28.0, 12.0),
        Block::Heading(2, _) => (24.0, 10.0),
        Block::Heading(..) => (20.0, 8.0),
        Block::Code { .. } => (12.0, 24.0),
        Block::List { .. } => (8.0, 12.0),
        Block::Quote(_) => (16.0, 16.0),
        Block::Rule => (32.0, 32.0),
    }
}

impl Renderer {
    fn spans(&self, inl: &Inline, base: f32) -> Vec<Span> {
        let s = &self.s;
        inl.spans
            .iter()
            .map(|&(a, b, st)| {
                let mut sp = Span::new(a, b);
                match st {
                    Style::Code => {
                        sp.mono = true;
                        sp.weight = Some(s.inline_code_weight);
                        sp.size = Some(base * s.inline_code_scale);
                        sp.bg = Some(s.code_bg);
                    }
                    Style::CodePad => sp.bg = Some(s.code_bg),
                    Style::Strong => sp.weight = Some(s.strong_weight),
                    Style::Em => sp.color = Some(s.text),
                    Style::Link => {
                        sp.color = Some(s.accent);
                        sp.underline = Some(s.link_underline);
                    }
                }
                sp
            })
            .collect()
    }

    /// The single place a link press is connected. `e` is the text element holding the link
    /// and `inl.links` has each link's byte range and destination. Once rich text can attach a
    /// press to a span, call `on_link(<span target>, url)` here; until then links are styled only.
    fn wire_link(&self, e: El, _inl: &Inline) -> El {
        let _ = &self.on_link;
        e
    }

    fn para(&self, inl: &Inline, style: TextStyle, color: Color) -> El {
        let e = rich(inl.text.clone(), self.spans(inl, style.size)).font(style).color(color).reveal(self.reveal).w_full();
        self.wire_link(e, inl)
    }

    fn blocks(&self, blocks: &[Block]) -> El {
        let mut c = col().w_full();
        let mut prev_mb = 0.0f32;
        for (i, b) in blocks.iter().enumerate() {
            let (mt, mb) = margins(b);
            let top = if i == 0 { 0.0 } else { mt.max(prev_mb) };
            c = c.child(self.block(b).key(format!("b{i}")).mt(top));
            prev_mb = mb;
        }
        c
    }

    fn block(&self, b: &Block) -> El {
        let s = &self.s;
        match b {
            Block::Para(inl) => self.para(inl, s.body, s.text),
            Block::Heading(l, inl) => {
                self.para(inl, s.headings[(*l as usize - 1).min(2)], if *l >= 4 { s.muted } else { s.text })
            }
            Block::Code(code) => col()
                .w_full()
                .padding(12.0)
                .radius(6.0)
                .fill(s.code_block_fill)
                .border(0.5, s.code_block_border)
                .child(text(code.clone()).font(s.code_block).color(s.text).wrap().reveal(self.reveal).w_full()),
            Block::List { start, items } => {
                let mut c = col().w_full().gap(4.0);
                for (k, item) in items.iter().enumerate() {
                    let marker = match start {
                        Some(n) => format!("{}.", n + k as u64),
                        None => "•".into(),
                    };
                    c = c.child(
                        row()
                            .key(format!("i{k}"))
                            .w_full()
                            .child(
                                text(marker).font(s.body).color(s.muted).w(32.0).align(Align::Center).reveal(self.reveal),
                            )
                            .child(self.blocks(item).grow(1.0).shrink()),
                    );
                }
                c
            }
            Block::Quote(inner) => row()
                .w_full()
                .child(el().w(2.0).h_full().radius(1.0).fill(s.quote_rule))
                .child(self.blocks(inner).grow(1.0).shrink().pl(12.0)),
            Block::Rule => el().w_full().h(0.5).fill(s.rule),
        }
    }
}
