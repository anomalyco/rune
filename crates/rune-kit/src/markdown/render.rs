//! Blocks to elements: block keys and margins, code and diff blocks, lists, quotes, tables and
//! diagrams.

use super::*;

mod blocks;
mod code;
mod inline;
mod source_copy;

pub(super) use code::code_overflows_at;
pub(super) use source_copy::source_copy;

pub(super) struct Renderer {
    pub(super) s: MarkdownStyle,
    pub(super) highlights: Vec<(Vec<Range<usize>>, Highlight)>,
    pub(super) reveal: f32,
    pub(super) streaming: bool,
    pub(super) on_link: Option<LinkWire>,
    pub(super) on_link_secondary: Option<LinkWire>,
    pub(super) on_link_hover: Option<LinkWire>,
    pub(super) link_menu: bool,
    pub(super) link_look: Option<LinkLook>,
    pub(super) link_help: Option<LinkHelp>,
    pub(super) on_task: Option<TaskWire>,
    pub(super) tasks_read_only: bool,
    pub(super) link_ring: Option<LinkRing>,
    /// `Markdown::code_link`.
    pub(super) code_link: Option<CodeLink>,
    pub(super) selectable: bool,
    /// `Markdown::scroll_wide`.
    pub(super) scroll_wide: bool,
    /// `Markdown::wrap_anywhere`.
    pub(super) wrap_anywhere: bool,
    /// `Markdown::hard_breaks`.
    pub(super) hard_breaks: bool,
    /// `Markdown::dir_tentative`.
    pub(super) tentative: Option<rune::Direction>,
    pub(super) highlighter: Highlighter,
    pub(super) sources: Rc<[Source]>,
    /// The next of `sources`, advanced at each [`Block::Code`] in document order.
    pub(super) next_source: Cell<usize>,
    /// The next diff fence index, advanced at each [`Block::Diff`] in document order.
    pub(super) next_diff: Cell<usize>,
    pub(super) tasks: Rc<[Option<(bool, usize)>]>,
    /// The next of `tasks`, advanced at each list item in document order.
    pub(super) next_task: Cell<usize>,
    pub(super) current_block_key: RefCell<Rc<str>>,
    /// `Markdown::diagram_frame`.
    #[cfg(feature = "mermaid")]
    pub(super) diagram_frame: Option<FrameWire>,
    /// `Markdown::code_frame`.
    pub(super) code_frame: Option<CodeWire>,
    /// `Markdown::diff_frame`.
    pub(super) diff_frame: Option<DiffFrameFn>,
    /// `Markdown::quote_frame`.
    pub(super) quote_frame: Option<QuoteWire>,
    /// `Markdown::link_favicon`.
    pub(super) link_favicon: Option<link_icon::FaviconWire>,
    /// `Markdown::emoji`.
    pub(super) emoji: Option<emoji::EmojiWire>,
    /// `Markdown::images` and `Markdown::on_image`.
    pub(super) images: Option<image::ImageWire>,
    pub(super) on_image: Option<image::ImagePress>,
    /// Where the blocks being drawn sit: list depth and a quote's text colour.
    pub(super) flow: Cell<Flow>,
}

/// What the blocks being drawn sit in.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Flow {
    /// Lists around them (0 at the top level).
    pub(super) lists: u32,
    /// A block quote's text colour, when they're in one.
    pub(super) ink: Option<Color>,
    /// How much narrower than the text column they are: list indents and quote rules.
    pub(super) inset: f32,
    /// The columns a nested list's copied markers are indented by (`"  - "` under `"- "`).
    pub(super) cols: usize,
    /// The marker a copy puts before the paragraph that opens a list item, its indent, and the
    /// newlines before it: two to set a top-level list apart from the block before it.
    pub(super) lead: Option<(usize, Lead, u8)>,
}

/// A list item's marker as copying writes it.
#[derive(Clone, Copy, Debug)]
pub(super) enum Lead {
    Bullet,
    Number(u64),
    Task(bool),
}

impl Lead {
    pub(super) fn marker(self) -> String {
        match self {
            Lead::Bullet => "- ".into(),
            Lead::Number(n) => format!("{n}. "),
            Lead::Task(done) => if done { "- [x] ".into() } else { "- [ ] ".into() },
        }
    }

    /// The columns a list nested in the item is indented by, past the item's own.
    pub(super) fn width(self) -> usize {
        match self {
            Lead::Number(n) => n.to_string().len() + 2,
            _ => 2,
        }
    }
}

/// The keys [`Renderer::blocks`] gives `n` blocks.
pub(super) fn rendered_keys(n: usize, keys: &[String]) -> Vec<String> {
    let mut used = Vec::new();
    (0..n).map(|i| if keys.is_empty() { format!("b{i}") } else { block_key(i, keys, &mut used) }).collect()
}

fn block_key(i: usize, keys: &[String], used: &mut Vec<String>) -> String {
    if let Some(k) = keys.get(i).filter(|k| !k.is_empty() && !used.iter().any(|u| u == *k)) {
        used.push(k.clone());
        return k.clone();
    }
    let taken = |cand: &str, used: &[String]| {
        used.iter().any(|u| u == cand) || keys.get(i + 1..).is_some_and(|rest| rest.iter().any(|k| k == cand))
    };
    let mut cand = format!("b{i}");
    let mut n = 0usize;
    while taken(&cand, used) {
        cand = format!("b{n}");
        n += 1;
    }
    used.push(cand.clone());
    cand
}

/// (margin-top, margin-bottom) per block, collapsed like CSS margins.
pub(super) fn margins(b: &Block, s: &MarkdownStyle) -> (f32, f32) {
    match b {
        Block::Para(_) => (s.paragraph_margin[0], s.paragraph_margin[1]),
        Block::Heading(level, _) => {
            let [top, bottom] = s.heading_margin[usize::from((*level).clamp(1, 4) - 1)];
            (top, bottom)
        }
        Block::Code { .. } | Block::Diff(_) | Block::Mermaid(_) => (s.code_margin[0], s.code_margin[1]),
        Block::List { .. } => (s.block_margin[0][0], s.block_margin[0][1]),
        Block::Quote(_) => (s.block_margin[1][0], s.block_margin[1][1]),
        Block::Rule => (s.block_margin[2][0], s.block_margin[2][1]),
        Block::Table(_) => (s.table_margin[0], s.table_margin[1]),
        Block::Image { .. } => (s.image_margin[0], s.image_margin[1]),
    }
}

/// Block `b`'s (margin-top, margin-bottom) as CSS collapses them, `i` its place among its
/// siblings and `in_item` whether they're a list item's: a list item's first paragraph has
/// none, a later one 8 above and 12 below, a nested list 4 each way. A quote or a list has no
/// vertical padding or border, so its first child's top margin and its last child's bottom
/// margin pass through it, and a list item's 8 through the list: a quote that ends a list item
/// spaces the list 16 from what follows, as it would standing alone.
pub(super) fn collapsed(b: &Block, s: &MarkdownStyle, i: usize, in_item: bool) -> (f32, f32) {
    (collapsed_top(b, s, i, in_item), collapsed_bottom(b, s, i, in_item))
}

fn own_margins(b: &Block, s: &MarkdownStyle, i: usize, in_item: bool) -> (f32, f32) {
    match b {
        Block::Para(_) if in_item => (if i == 0 { 0.0 } else { 8.0 }, if i == 0 { 0.0 } else { 12.0 }),
        Block::List { .. } if in_item => (4.0, 4.0),
        _ => margins(b, s),
    }
}

fn collapsed_top(mut b: &Block, s: &MarkdownStyle, mut i: usize, mut in_item: bool) -> f32 {
    let mut top = 0.0f32;
    loop {
        top = top.max(own_margins(b, s, i, in_item).0);
        match b {
            Block::Quote(inner) => match inner.first() {
                Some(f) => (b, i, in_item) = (f, 0, false),
                None => return top,
            },
            Block::List { items, .. } => match items.first().and_then(|item| item.first()) {
                Some(f) => (b, i, in_item) = (f, 0, true),
                None => return top,
            },
            _ => return top,
        }
    }
}

fn collapsed_bottom(mut b: &Block, s: &MarkdownStyle, mut i: usize, mut in_item: bool) -> f32 {
    let mut bot = 0.0f32;
    loop {
        bot = bot.max(own_margins(b, s, i, in_item).1);
        match b {
            Block::Quote(inner) => match inner.last() {
                Some(l) => (b, i, in_item) = (l, inner.len() - 1, false),
                None => return bot,
            },
            Block::List { items, .. } => match items.last() {
                Some(item) => {
                    bot = bot.max(s.list_gap);
                    match item.last() {
                        Some(l) => (b, i, in_item) = (l, item.len() - 1, true),
                        None => return bot,
                    }
                }
                None => return bot,
            },
            _ => return bot,
        }
    }
}

pub(super) fn hash_of(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}
