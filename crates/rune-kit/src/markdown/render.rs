//! Blocks to elements: block keys and margins, code and diff blocks, lists, quotes, tables and
//! diagrams.

use super::*;

pub(super) struct Renderer {
    pub(super) s: MarkdownStyle,
    pub(super) highlights: Vec<(Vec<Range<usize>>, Highlight)>,
    pub(super) reveal: f32,
    pub(super) streaming: bool,
    pub(super) on_link: Option<LinkWire>,
    pub(super) on_link_secondary: Option<LinkWire>,
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
    /// `Markdown::dir_tentative`.
    pub(super) tentative: Option<rune::Direction>,
    pub(super) highlighter: Highlighter,
    pub(super) sources: Rc<[Source]>,
    /// The next of `sources`, advanced at each [`Block::Code`] in document order.
    pub(super) next_source: Cell<usize>,
    pub(super) tasks: Rc<[Option<(bool, usize)>]>,
    /// The next of `tasks`, advanced at each list item in document order.
    pub(super) next_task: Cell<usize>,
    pub(super) current_block_key: RefCell<Rc<str>>,
    /// `Markdown::diagram_frame`.
    #[cfg(feature = "mermaid")]
    pub(super) diagram_frame: Option<FrameWire>,
    /// `Markdown::code_frame`.
    pub(super) code_frame: Option<CodeWire>,
    /// `Markdown::quote_frame`.
    pub(super) quote_frame: Option<QuoteWire>,
    /// `Markdown::link_favicon`.
    pub(super) link_favicon: Option<link_icon::FaviconWire>,
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

/// Shell languages, which [`MarkdownStyle::shell_code`] styles apart (`markdown.tsx`'s list).
const SHELLS: [&str; 7] = ["bash", "sh", "shell", "zsh", "fish", "console", "terminal"];
/// Plain-text fence names, which [`MarkdownStyle::plain_code_text`] colours as a fence with none.
const PLAIN_TEXT: [&str; 4] = ["text", "txt", "plain", "plaintext"];

/// The keys [`Renderer::blocks`] gives `n` blocks.
pub(super) fn rendered_keys(n: usize, keys: &[String]) -> Vec<String> {
    let mut used = Vec::new();
    (0..n).map(|i| if keys.is_empty() { format!("b{i}") } else { block_key(i, keys, &mut used) }).collect()
}

/// [`Markdown::copy_source`]: the source of the top-level blocks (keyed `names`, starting at
/// `starts`) that `sel` touches, `None` for part of one block.
pub(super) fn source_copy(src: &str, starts: &[usize], names: &[String], sel: &rune::TextSelection) -> Option<String> {
    // The longest key that is the path or its first segments: an app's key may hold a `/`.
    let block_of = |path: &str| names.iter().enumerate()
        .filter(|(_, k)| path == k.as_str() || path.strip_prefix(k.as_str()).is_some_and(|rest| rest.starts_with('/')))
        .max_by_key(|(_, k)| k.len()).map(|(b, _)| b);
    let mut touched: Option<(usize, usize)> = None;
    for t in sel.texts().iter().filter(|t| t.is_selected()) {
        let Some(b) = block_of(t.path()) else { continue };
        touched = Some(touched.map_or((b, b), |(first, _)| (first, b)));
    }
    let (first, last) = touched?;
    if first == last && !sel.texts().iter().filter(|t| block_of(t.path()) == Some(first)).all(|t| t.is_whole() || t.is_empty()) {
        return None;
    }
    if (first == 0 && last + 1 >= starts.len()) || sel.texts().iter().all(|t| t.is_whole() || t.is_empty()) {
        return Some(src.to_owned());
    }
    let start = *starts.get(first)?;
    let end = starts.get(last + 1).copied().unwrap_or(src.len());
    Some(src.get(start..end)?.trim_end().to_owned())
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
        Block::Para(_) => (0.0, 12.0),
        Block::Heading(1, _) => (28.0, 12.0),
        Block::Heading(2, _) => (24.0, 10.0),
        Block::Heading(..) => (20.0, 8.0),
        Block::Code { .. } | Block::Diff(_) | Block::Mermaid(_) => (s.code_margin[0], s.code_margin[1]),
        Block::List { .. } => (8.0, 12.0),
        Block::Quote(_) => (16.0, 16.0),
        Block::Rule => (32.0, 32.0),
        Block::Table(_) => (s.table_margin[0], s.table_margin[1]),
        Block::Image { .. } => (24.0, 24.0),
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

thread_local! {
    static PAD_SIZES: RefCell<std::collections::HashMap<u64, f32>> = RefCell::new(std::collections::HashMap::new());
}

/// The size that sets inline code's padding spaces (`PAD`, in the text's own `font`) to `pad`
/// points a side, so the text beside the chip sits where a page's `padding: 0 4px` puts it.
/// Solved twice: a variable font's optical size moves its spaces' widths with the size.
fn pad_size(font: TextStyle, pad: f32) -> f32 {
    let key = hash_of((font, pad.to_bits(), rune::text::font_epoch()));
    if let Some(size) = PAD_SIZES.with_borrow(|m| m.get(&key).copied()) {
        return size;
    }
    let probe = PAD.repeat(64);
    let wide = |size: f32| (rune::text::measure(&probe, &[], TextStyle { size, ..font }, None).0 / 64.0).max(0.1);
    let first = font.size * pad / wide(font.size);
    let size = first * pad / wide(first);
    PAD_SIZES.with_borrow_mut(|m| m.insert(key, size));
    size
}

/// Code as rich text with `spans` and `highlights` (byte ranges of `code`). Text layout widens
/// tabs in a monospace `style` to four-column stops itself; in any other family it draws a tab
/// as one space, so each tab is padded with uncopied spaces to the next stop: lines line up and a
/// copy still gets the tab.
fn tabbed(code: &str, mut spans: Vec<Span>, mut highlights: Vec<TextHighlight>, style: TextStyle) -> El {
    if !code.contains('\t') || matches!(style.family, Family::Mono | Family::SystemMono) {
        return rich(code.to_owned(), spans).text_highlights(highlights);
    }
    let mut shown = String::with_capacity(code.len() + code.len() / 8);
    // (source byte, shown byte, pad, padding so far including this tab's)
    let mut tabs: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut col = 0;
    for (i, c) in code.char_indices() {
        if c == '\t' {
            let pad = 3 - col % 4;
            let total = tabs.last().map_or(0, |t| t.3) + pad;
            tabs.push((i, shown.len(), pad, total));
            shown.push('\t');
            shown.extend(std::iter::repeat_n(' ', pad));
            col += pad + 1;
            continue;
        }
        col = if c == '\n' { 0 } else { col + 1 };
        shown.push(c);
    }
    let shift = |b: u32| match tabs.partition_point(|t| (t.0 as u32) < b) {
        0 => b,
        n => b + tabs[n - 1].3 as u32,
    };
    for sp in &mut spans {
        (sp.start, sp.end) = (shift(sp.start), shift(sp.end));
    }
    for hl in &mut highlights {
        let r = hl.range();
        *hl = hl.clone().with_range(shift(r.start as u32) as usize..shift(r.end as u32) as usize);
    }
    tabs.iter().fold(rich(shown, spans).text_highlights(highlights), |el, t| el.uncopied(t.1 + 1..t.1 + 1 + t.2))
}

enum RenderFrame<'a> {
    Blocks {
        blocks: &'a [Block],
        pre: Option<&'a [Rc<super::memo::PreBlock>]>,
        maps: &'a [BlockMap],
        names: Vec<String>,
        top_level: bool,
        in_item: bool,
        open_tail: bool,
        allow_memo: bool,
        idx: usize,
        prev_mb: f32,
        after_heading: bool,
        last_dir: Option<rune::Direction>,
        col: El,
        pending: Option<(String, f32, bool, Option<rune::Direction>)>,
    },
    Quote {
        quote_block: &'a Block,
        inner: &'a [Block],
        inner_maps: &'a [BlockMap],
        saved_flow: Flow,
        first_task: usize,
    },
    List {
        list_block: &'a Block,
        start: Option<u64>,
        items: &'a [Vec<Block>],
        pre_items: Option<&'a [super::memo::PreItem]>,
        item_maps: &'a [Vec<BlockMap>],
        saved_flow: Flow,
        indent: f32,
        space: f32,
        open_tail: bool,
        allow_memo: bool,
        idx: usize,
        col: El,
        pending_item: Option<(usize, f32, Option<(bool, usize)>, bool)>,
    },
}

impl Renderer {
    pub(super) fn spans(&self, inl: &Inline, font: TextStyle) -> Vec<Span> {
        let inl = &*self.with_code_links(inl);
        let s = &self.s;
        let base = font.size;
        let links: Vec<Span> = if inl.links.is_empty() {
            Vec::new()
        } else {
            inl.spans.iter().filter(|s| matches!(s.2, Style::Link)).map(|&(a, b, _)| self.link_span(a, b, inl)).collect()
        };
        let mut next_link = 0;
        let mut out = Vec::with_capacity(inl.spans.len());
        // Code keeps its own weight in bold text, as `markdown.css`'s `:not(pre) > code` sets it, so
        // its spans go after the rest: of overlapping spans, the later one's weight wins.
        let mut code = Vec::new();
        for &(a, b, st) in &inl.spans {
            if matches!(st, Style::Link) {
                let sp = links[next_link].clone();
                next_link += 1;
                let at = out.iter().position(|p: &Span| p.start < sp.end && sp.start < p.end).unwrap_or(out.len());
                out.insert(at, sp);
                continue;
            }
            if matches!(st, Style::Strike) {
                continue;
            }
            let in_link = links.iter().find(|l| l.start <= a as u32 && b as u32 <= l.end);
            let mut sp = Span::new(a, b);
            // Code in a link is a chip in the link's colour, not underlined as well.
            let no_line = in_link.is_some_and(|l| l.underline.is_some()).then_some(Color::TRANSPARENT);
            match st {
                Style::Code if self.code_link_color(a, b, inl).is_some() => {
                    sp.mono = true;
                    sp.weight = Some(s.inline_code_weight);
                    sp.size = Some(base * s.inline_code_scale);
                    sp.color = self.code_link_color(a, b, inl);
                    code.push(sp);
                    continue;
                }
                Style::Code => {
                    let link_base = in_link.and_then(|l| l.size).unwrap_or(base);
                    sp.mono = true;
                    sp.weight = Some(s.inline_code_weight);
                    sp.size = Some(link_base * s.inline_code_scale);
                    sp.underline = no_line;
                    if in_link.is_none_or(|l| l.bg.is_none()) {
                        sp = sp.bg(s.code_bg, s.inline_code_pad);
                    }
                    code.push(sp);
                    continue;
                }
                Style::CodePad => {
                    sp.size = Some(pad_size(font, s.inline_code_pad));
                    sp.underline = no_line;
                }
                Style::Strong => sp.weight = Some(s.strong_weight),
                Style::Em => sp.italic = true,
                Style::Strike | Style::Link => unreachable!(),
            }
            out.push(sp);
        }
        out.extend(code);
        out
    }

    /// Resolves `hl` against [`MarkdownStyle`] into a [`TextHighlight`] for `range`.
    fn make_text_highlight(&self, idx: usize, range: Range<usize>, hl: &Highlight) -> TextHighlight {
        let s = &self.s;
        let bg = hl.bg.unwrap_or(if hl.current { s.highlight_current_bg } else { s.highlight_bg });
        let fg = hl.fg.or(if hl.current { s.highlight_current_fg } else { s.highlight_fg });
        let ring = hl.ring.or(if hl.current { s.highlight_current_ring } else { s.highlight_ring });
        let key = hl.key.clone().unwrap_or_else(|| Markdown::highlight_key(idx));
        let mut th = TextHighlight::new(range, bg).key(key);
        if let Some(c) = fg {
            th = th.color(c);
        }
        if let Some((rc, rw)) = ring {
            th = th.ring(rc, rw);
        }
        th
    }

    fn text_highlights_for(&self, text: &str, map: Option<&SpanMap>) -> Vec<TextHighlight> {
        let Some(map) = map.filter(|_| !self.highlights.is_empty()) else { return Vec::new() };
        let mut out = Vec::new();
        for (idx, (src_ranges, hl)) in self.highlights.iter().enumerate() {
            for r in map.map_ranges(src_ranges, text) {
                out.push(self.make_text_highlight(idx, r, hl));
            }
        }
        out
    }

    /// `e`, a text, selectable when the markdown is (after its links, so it shows the I-beam).
    pub(super) fn select(&self, e: El) -> El {
        if self.selectable { e.selectable(true).selection_color(self.s.selection) } else { e }
    }

    /// `e`, the text of `inl`, with its strikethroughs, highlights, links and selectable, copying
    /// without inline code's padding.
    pub(super) fn select_inline(&self, mut e: El, inl: &Inline, map: Option<&SpanMap>) -> El {
        for &(a, b, st) in &inl.spans {
            if matches!(st, Style::Strike) {
                e = e.strike(a, b, None);
            }
        }
        e = e.text_highlights(self.text_highlights_for(&inl.text, map));
        let e = self.select(self.wire_link(self.link_icons(e, inl), inl));
        if !self.selectable { return e; }
        inl.spans.iter().filter(|s| matches!(s.2, Style::CodePad)).fold(e, |e, s| e.uncopied(s.0..s.1))
    }

    pub(super) fn with_reveal(&self, e: El) -> El {
        if self.reveal >= 0.0 {
            e.reveal(self.reveal)
        } else if self.streaming {
            e.reveal(0.0)
        } else {
            e
        }
    }

    pub(super) fn para(&self, inl: &Inline, map: Option<&SpanMap>, style: TextStyle, color: Color) -> El {
        // Lines keep their own 22.4 points, as a page's do, so a long list doesn't drift a
        // point per item.
        let e = self.with_reveal(rich(inl.text.clone(), self.spans(inl, style)).font(style).color(color).exact_lines()).w_full();
        let e = bidi::directed(bidi::isolated(e, inl), bidi::inline_dir(inl));
        // A copy puts a blank line around a paragraph, one newline around a list item's first
        // with its marker before it.
        let e = match self.flow.get().lead {
            Some((cols, lead, before)) => e.copy_prefix(format!("{}{}", " ".repeat(cols), lead.marker())).copy_breaks(before, 1),
            None => e.copy_breaks(2, 2),
        };
        self.select_inline(e, inl, map)
    }

    /// `blocks` in a column, spaced by their collapsed margins. A heading's bottom margin alone
    /// sets the gap under it (CSS's `h2 + p { margin-top: 0 }`). In a list item (`in_item`) the
    /// first paragraph has no margins, a later one 8 above, and a nested list 4 above and below,
    /// as a web page spaces an `li`'s children.
    pub(super) fn blocks_with_dirs(
        &self,
        blocks: &[Block],
        pre: Option<&[Rc<super::memo::PreBlock>]>,
        maps: &[BlockMap],
        keys: &[String],
        top_level: bool,
        dirs: &bidi::Dirs,
    ) -> El {
        let mut quote_plain_cache = plain::QuotePlainCache::default();
        self.run_frames(
            vec![RenderFrame::Blocks {
                blocks,
                pre,
                maps,
                names: rendered_keys(blocks.len(), keys),
                top_level,
                in_item: false,
                open_tail: self.streaming,
                allow_memo: true,
                idx: 0,
                prev_mb: 0.0,
                after_heading: false,
                last_dir: if top_level { self.tentative } else { None },
                col: col().w_full(),
                pending: None,
            }],
            dirs,
            &mut quote_plain_cache,
        )
    }

    fn unmemoised_block(
        &self,
        b: &Block,
        pb: Option<&super::memo::PreBlock>,
        map: &BlockMap,
        dirs: &bidi::Dirs,
        quote_plain_cache: &mut plain::QuotePlainCache,
    ) -> El {
        match b {
            Block::Quote(inner) => {
                let inner_maps = match map { BlockMap::Quote(m) => m.as_slice(), _ => &[][..] };
                let q = self.s.quote;
                let saved = self.flow.get();
                let first_task = self.next_task.get();
                self.flow.set(Flow { ink: q.text.or(saved.ink), inset: saved.inset + q.width + q.pad, ..saved });
                self.run_frames(
                    vec![
                        RenderFrame::Quote { quote_block: b, inner, inner_maps, saved_flow: saved, first_task },
                        RenderFrame::Blocks {
                            blocks: inner,
                            pre: pb.map(|p| &*p.inner),
                            maps: inner_maps,
                            names: rendered_keys(inner.len(), &[]),
                            top_level: false,
                            in_item: false,
                            open_tail: false,
                            allow_memo: false,
                            idx: 0,
                            prev_mb: 0.0,
                            after_heading: false,
                            last_dir: None,
                            col: col().w_full(),
                            pending: None,
                        },
                    ],
                    dirs,
                    quote_plain_cache,
                )
            }
            Block::List { start, items } => {
                let item_maps = match map { BlockMap::List(m) => m.as_slice(), _ => &[][..] };
                let saved = self.flow.get();
                let indent = match self.s.list_nested {
                    Some([bullets, numbers]) if saved.lists > 0 => if start.is_some() { numbers } else { bullets },
                    _ => self.s.list_indent,
                };
                let space = if self.s.list_outside { rune::text::measure("x x", &[], self.s.body, None).0 - rune::text::measure("xx", &[], self.s.body, None).0 } else { 0.0 };
                self.run_frames(
                    vec![RenderFrame::List {
                        list_block: b,
                        start: *start,
                        items,
                        pre_items: pb.map(|p| &*p.items),
                        item_maps,
                        saved_flow: saved,
                        indent,
                        space,
                        open_tail: false,
                        allow_memo: false,
                        idx: 0,
                        col: col().role("list").w_full(),
                        pending_item: None,
                    }],
                    dirs,
                    quote_plain_cache,
                )
            }
            _ => self.leaf_block(b, pb, map),
        }
    }

    fn run_frames<'a>(
        &self,
        mut stack: Vec<RenderFrame<'a>>,
        dirs: &bidi::Dirs,
        quote_plain_cache: &mut plain::QuotePlainCache,
    ) -> El {
        const EMPTY: &BlockMap = &BlockMap::None;
        let mut ret: Option<El> = None;

        while let Some(top_frame) = stack.last_mut() {
            match top_frame {
                RenderFrame::Blocks { blocks, pre, maps, names, top_level, in_item, open_tail, allow_memo, idx, prev_mb, after_heading, last_dir, col: cur_col, pending } => {
                    if let Some(child_el) = ret.take() {
                        let (key, top, prose, own) = pending.take().unwrap();
                        let drawn = if prose { bidi::directed(child_el, own.or(*last_dir)) } else { child_el };
                        if own.is_some() { *last_dir = own; }
                        *cur_col = std::mem::replace(cur_col, el()).child(drawn.key(key).mt(top));
                        continue;
                    }
                    let mut descended = false;
                    let blocks_slice = *blocks;
                    let pre_slice = *pre;
                    let maps_slice = *maps;
                    while *idx < blocks_slice.len() {
                        let i = *idx;
                        *idx += 1;
                        let b = &blocks_slice[i];
                        if self.streaming && matches!(b, Block::Code(c) if c.is_empty()) {
                            let n = self.next_source.get();
                            if self.sources.get(n).is_some_and(|s| s.open && s.len == 0) {
                                self.next_source.set(n + 1);
                                continue;
                            }
                        }
                        let (mt, mb) = collapsed(b, &self.s, i, *in_item);
                        let top = match b {
                            _ if i == 0 => 0.0,
                            Block::Heading(..) | Block::Rule | Block::Image { .. } => mt.max(*prev_mb),
                            _ if *after_heading => *prev_mb,
                            _ => mt.max(*prev_mb),
                        };
                        *prev_mb = mb;
                        *after_heading = matches!(b, Block::Heading(..));
                        let key = std::mem::take(&mut names[i]);
                        if *top_level { self.current_block_key.replace(key.clone().into()); }
                        if i > 0 { self.flow.set(Flow { lead: None, ..self.flow.get() }); }
                        let own = dirs.of(b);
                        let prose = !matches!(b, Block::Code(_) | Block::Diff(_) | Block::Mermaid(_) | Block::Image { .. } | Block::Rule);
                        let dir = prose.then(|| own.or(*last_dir)).flatten();
                        let map = maps_slice.get(i).unwrap_or(EMPTY);
                        let pb = pre_slice.and_then(|ps| ps.get(i)).map(Rc::as_ref);
                        let child_open_tail = *open_tail && i + 1 == blocks_slice.len();
                        let is_deep_tail = matches!((b, map), (Block::Para(_), BlockMap::DeepTail { .. }));
                        let can_memo_here = *allow_memo && !child_open_tail && !(self.streaming && is_deep_tail);
                        if can_memo_here {
                            let (src_at, src_n) = (self.next_source.get(), pb.map_or_else(|| super::memo::source_count(b), |p| p.src_n as usize));
                            let (task_at, task_n) = (self.next_task.get(), pb.map_or_else(|| super::memo::task_count(b), |p| p.task_n as usize));
                            if let Some(digest) = self.settled_block_digest(b, pb, map, &key, top, dir, (src_at, src_n), (task_at, task_n)) {
                                let key_c = key.clone();
                                let cached_el = rune::memo::cached(digest, || {
                                    super::memo::note_block_build();
                                    self.next_source.set(src_at);
                                    self.next_task.set(task_at);
                                    let mut sub_qpc = plain::QuotePlainCache::default();
                                    let child_el = self.unmemoised_block(b, pb, map, dirs, &mut sub_qpc);
                                    debug_assert_eq!(self.next_source.get(), src_at + src_n);
                                    debug_assert_eq!(self.next_task.get(), task_at + task_n);
                                    let drawn = if prose { bidi::directed(child_el, dir) } else { child_el };
                                    drawn.key(key_c).mt(top)
                                });
                                self.next_source.set(src_at + src_n);
                                self.next_task.set(task_at + task_n);
                                if own.is_some() { *last_dir = own; }
                                *cur_col = std::mem::replace(cur_col, el()).child(cached_el);
                                continue;
                            }
                        }
                        let am = *allow_memo;
                        match b {
                            Block::Quote(inner) => {
                                *pending = Some((key, top, prose, own));
                                let inner_maps = match map { BlockMap::Quote(m) => m.as_slice(), _ => &[][..] };
                                let q = self.s.quote;
                                let saved = self.flow.get();
                                let first_task = self.next_task.get();
                                self.flow.set(Flow { ink: q.text.or(saved.ink), inset: saved.inset + q.width + q.pad, ..saved });
                                stack.push(RenderFrame::Quote { quote_block: b, inner, inner_maps, saved_flow: saved, first_task });
                                stack.push(RenderFrame::Blocks { blocks: inner, pre: pb.map(|p| &*p.inner), maps: inner_maps, names: rendered_keys(inner.len(), &[]), top_level: false, in_item: false, open_tail: child_open_tail, allow_memo: am, idx: 0, prev_mb: 0.0, after_heading: false, last_dir: None, col: col().w_full(), pending: None });
                                descended = true;
                                break;
                            }
                            Block::List { start, items } => {
                                *pending = Some((key, top, prose, own));
                                let item_maps = match map { BlockMap::List(m) => m.as_slice(), _ => &[][..] };
                                let saved = self.flow.get();
                                let indent = match self.s.list_nested {
                                    Some([bullets, numbers]) if saved.lists > 0 => if start.is_some() { numbers } else { bullets },
                                    _ => self.s.list_indent,
                                };
                                let space = if self.s.list_outside { rune::text::measure("x x", &[], self.s.body, None).0 - rune::text::measure("xx", &[], self.s.body, None).0 } else { 0.0 };
                                stack.push(RenderFrame::List { list_block: b, start: *start, items, pre_items: pb.map(|p| &*p.items), item_maps, saved_flow: saved, indent, space, open_tail: child_open_tail, allow_memo: am, idx: 0, col: col().role("list").w_full(), pending_item: None });
                                descended = true;
                                break;
                            }
                            _ => {
                                let child_el = self.leaf_block(b, pb, map);
                                let drawn = if prose { bidi::directed(child_el, dir) } else { child_el };
                                if own.is_some() { *last_dir = own; }
                                *cur_col = std::mem::replace(cur_col, el()).child(drawn.key(key).mt(top));
                            }
                        }
                    }
                    if !descended {
                        let Some(RenderFrame::Blocks { col: done, .. }) = stack.pop() else { unreachable!() };
                        ret = Some(done);
                    }
                }
                RenderFrame::Quote { .. } => {
                    let Some(RenderFrame::Quote { quote_block, inner, inner_maps, saved_flow, first_task }) = stack.pop() else { unreachable!() };
                    let drawn = ret.take().unwrap();
                    self.flow.set(saved_flow);
                    let q = self.s.quote;
                    let quote = bidi::directed(
                        row().role("blockquote").w_full()
                            .child(el().w(q.width).h_full().radius((q.width / 2.0).min(1.0)).fill(self.s.quote_rule))
                            .child(drawn.grow(1.0).shrink().pl(q.pad)),
                        dirs.of(quote_block),
                    );
                    ret = Some(match &self.quote_frame {
                        Some(f) => {
                            let txt = self.plain_text_rc(quote_block, inner, inner_maps, first_task, Flow { lead: None, ..saved_flow }, quote_plain_cache);
                            f(quote, &QuoteFrame { text: &txt })
                        }
                        None => quote,
                    });
                }
                RenderFrame::List { start, items, pre_items, item_maps, saved_flow, indent, space, open_tail, allow_memo, idx, col: list_col, pending_item, .. } => {
                    let n = items.len();
                    let real_n = n - usize::from(n > 0 && matches!(item_maps.last().map(Vec::as_slice), Some([BlockMap::DeepTail { .. }])));
                    if let Some(item_body) = ret.take() {
                        let (k, gap_mt, task, is_deep_tail) = pending_item.take().unwrap();
                        if is_deep_tail {
                            *list_col = std::mem::replace(list_col, el()).child(
                                row().key(format!("i{k}")).w_full().mt(gap_mt).pl(*indent).child(item_body.grow(1.0).shrink()),
                            );
                        } else if let Some((checked, marker_offset)) = task {
                            *list_col = std::mem::replace(list_col, el()).child(self.task_item_body((k, real_n), *indent, &items[k], item_body, marker_offset, checked).mt(gap_mt));
                        } else {
                            let marker = self.list_marker(*start, k, *indent, *space);
                            *list_col = std::mem::replace(list_col, el()).child(
                                row().key(format!("i{k}")).role("listitem").position_in_set(k + 1, real_n).w_full().mt(gap_mt)
                                    .child(self.with_reveal(marker).a11y_hidden())
                                    .child(item_body.grow(1.0).shrink()),
                            );
                        }
                        continue;
                    }
                    if *idx < n {
                        let k = *idx;
                        *idx += 1;
                        let items_slice = *items;
                        let pre_items_slice = *pre_items;
                        let imaps_slice = *item_maps;
                        let item = &items_slice[k];
                        let ipre = pre_items_slice.and_then(|ps| ps.get(k));
                        let imap = imaps_slice.get(k).map_or(&[][..], Vec::as_slice);
                        let is_deep_tail = matches!(imap, [BlockMap::DeepTail { .. }]);
                        let item_open_tail = *open_tail && k + 1 == n;
                        let am = *allow_memo;
                        let gap_mt = list_item_gap(&self.s, items_slice, k);
                        let task_idx = self.next_task.get();
                        self.next_task.set(task_idx + 1);
                        let sf = *saved_flow;
                        let ind = *indent;
                        let sp = *space;
                        let st = *start;
                        if is_deep_tail {
                            self.flow.set(Flow { lists: sf.lists + 1, inset: sf.inset + ind, cols: sf.cols, lead: None, ..sf });
                            *pending_item = Some((k, gap_mt, None, true));
                            stack.push(RenderFrame::Blocks { blocks: item, pre: ipre.map(|p| &*p.inner), maps: imap, names: rendered_keys(item.len(), &[]), top_level: false, in_item: true, open_tail: item_open_tail, allow_memo: false, idx: 0, prev_mb: 0.0, after_heading: false, last_dir: None, col: col().w_full(), pending: None });
                        } else {
                            let task = self.tasks.get(task_idx).copied().flatten();
                            let lead = match (task, st) {
                                (Some((checked, _)), _) => Lead::Task(checked),
                                (None, Some(num)) => Lead::Number(num + k as u64),
                                (None, None) => Lead::Bullet,
                            };
                            self.flow.set(Flow { lists: sf.lists + 1, inset: sf.inset + ind, cols: sf.cols + lead.width(), lead: Some((sf.cols, lead, if k == 0 && sf.lists == 0 { 2 } else { 1 })), ..sf });
                            if task.is_none() && am && !item_open_tail {
                                let (src_at, src_n) = (self.next_source.get(), ipre.map_or_else(|| item.iter().map(super::memo::source_count).sum::<usize>(), |p| p.src_n as usize));
                                let (task_at, task_n) = (self.next_task.get(), ipre.map_or_else(|| item.iter().map(super::memo::task_count).sum::<usize>(), |p| p.task_n as usize));
                                if let Some(digest) = self.settled_item_digest(item, ipre, imap, (src_at, src_n), (task_at, task_n)) {
                                    let body = rune::memo::cached(digest, || {
                                        super::memo::note_block_build();
                                        self.next_source.set(src_at);
                                        self.next_task.set(task_at);
                                        let mut sub_qpc = plain::QuotePlainCache::default();
                                        let b = self.run_frames(
                                            vec![RenderFrame::Blocks { blocks: item, pre: ipre.map(|p| &*p.inner), maps: imap, names: rendered_keys(item.len(), &[]), top_level: false, in_item: true, open_tail: false, allow_memo: false, idx: 0, prev_mb: 0.0, after_heading: false, last_dir: None, col: col().w_full(), pending: None }],
                                            dirs,
                                            &mut sub_qpc,
                                        ).grow(1.0).shrink();
                                        debug_assert_eq!(self.next_source.get(), src_at + src_n);
                                        debug_assert_eq!(self.next_task.get(), task_at + task_n);
                                        b
                                    });
                                    self.next_source.set(src_at + src_n);
                                    self.next_task.set(task_at + task_n);
                                    let marker = self.list_marker(st, k, ind, sp);
                                    *list_col = std::mem::replace(list_col, el()).child(
                                        row().key(format!("i{k}")).role("listitem").position_in_set(k + 1, real_n).w_full().mt(gap_mt)
                                            .child(self.with_reveal(marker).a11y_hidden())
                                            .child(body),
                                    );
                                    continue;
                                }
                            }
                            *pending_item = Some((k, gap_mt, task, false));
                            stack.push(RenderFrame::Blocks { blocks: item, pre: ipre.map(|p| &*p.inner), maps: imap, names: rendered_keys(item.len(), &[]), top_level: false, in_item: true, open_tail: item_open_tail, allow_memo: am, idx: 0, prev_mb: 0.0, after_heading: false, last_dir: None, col: col().w_full(), pending: None });
                        }
                    } else {
                        let Some(RenderFrame::List { list_block, saved_flow, col: done, .. }) = stack.pop() else { unreachable!() };
                        self.flow.set(saved_flow);
                        ret = Some(bidi::directed(done, dirs.of(list_block)));
                    }
                }
            }
        }
        ret.unwrap()
    }

    pub(super) fn code(&self, code: &str, source: Option<&Source>, map: Option<&SpanMap>) -> El {
        let s = &self.s;
        let highlights = self.text_highlights_for(code, map);
        let shell = s.shell.filter(|_| source.and_then(|src| src.lang.as_deref()).is_some_and(|l| SHELLS.iter().any(|sh| l.eq_ignore_ascii_case(sh))));
        let plain = source.and_then(|src| src.lang.as_deref()).is_none_or(|l| PLAIN_TEXT.iter().any(|p| l.eq_ignore_ascii_case(p)));
        let (fill, border, ink) = match shell {
            Some(sh) => (sh.fill, sh.border, sh.text),
            None if plain => (s.code_block_fill, s.code_block_border, s.plain_code_text.or(s.code_text).unwrap_or(s.text)),
            None => (s.code_block_fill, s.code_block_border, s.code_text.unwrap_or(s.text)),
        };
        let spans = if shell.is_some() { Vec::new() } else { self.syntax_spans(code, source) };
        let body = self.with_reveal(tabbed(code, spans, highlights, s.code_block).font(s.code_block).color(ink).by_line()).copy_breaks(2, 2);
        // `.highlights`' `border: 0.5px` takes its device pixel each side, inside which the padding sits.
        // Code reads left to right in any document, as `markdown.css` isolates it.
        let frame = col().role("code").dir(rune::Direction::Ltr).w_full().radius(6.0).fill(fill).border(0.5, border).border_layout();
        if self.scroll_wide {
            // The text sizes to its longest line inside the viewport, which scrolls under the
            // padding so lines run to the frame's edge.
            let viewport = col().key("scroll").w_full().scroll_x();
            let viewport = if s.wide_fade > 0.0 { viewport.scroll_edge_fade(s.wide_fade) } else { viewport };
            return frame.child(viewport.child(col().px(s.code_block_pad[0]).py(s.code_block_pad[1]).child(self.select(body))));
        }
        frame.px(s.code_block_pad[0]).py(s.code_block_pad[1]).child(self.select(body.w_full()))
    }

    /// Colour-only spans, so highlighting never moves a glyph: a block streaming in draws its
    /// newest line plain and colours it in place once the line ends.
    pub(super) fn syntax_spans(&self, code: &str, source: Option<&Source>) -> Vec<Span> {
        let Some(src) = source.filter(|_| self.s.highlight) else { return Vec::new() };
        let lang = src.lang.as_deref();
        // The built-in highlighter leaves other languages plain; skip the cache for those.
        if self.highlighter.is_builtin() && !lang.is_some_and(crate::syntax::known) {
            return Vec::new();
        }
        let Some(prefix) = code.get(..src.coloured) else { return Vec::new() };
        self.s.syntax.spans(&self.highlighter.highlight(lang, prefix), 0)
    }

    /// One row per line, so added and removed lines are tinted edge to edge.
    pub(super) fn diff(&self, src: &str, map: Option<&SpanMap>) -> El {
        let s = &self.s;
        let all_hl = self.text_highlights_for(src, map);
        let mut c = col().role("code").dir(rune::Direction::Ltr).w_full().py(s.code_block_pad[1]).radius(6.0).fill(s.code_block_fill).border(0.5, s.code_block_border).border_layout();
        let mut line_start = 0usize;
        let last = src.split('\n').count() - 1;
        for (n, line) in src.split('\n').enumerate() {
            let line_end = line_start + line.len();
            let line_hl: Vec<TextHighlight> = all_hl
                .iter()
                .filter_map(|h| {
                    let r = h.range();
                    let a = r.start.max(line_start);
                    let b = r.end.min(line_end);
                    (a < b).then(|| h.clone().with_range((a - line_start)..(b - line_start)))
                })
                .collect();
            line_start = line_end + 1;
            let line = line.strip_suffix('\r').unwrap_or(line);
            let header = ["@@", "diff ", "index ", "--- ", "+++ "].iter().any(|p| line.starts_with(p));
            let (tint, color) = match line.as_bytes().first() {
                _ if header => (None, s.diff_hunk),
                Some(b'+') => (Some(s.diff_add), s.diff_add_text),
                Some(b'-') => (Some(s.diff_remove), s.diff_remove_text),
                _ => (None, s.text),
            };
            let shown = if line.is_empty() { " " } else { line };
            let mut r = row()
                .key(format!("l{n}"))
                .w_full()
                .px(s.code_block_pad[0])
                .child(self.select(self.with_reveal(tabbed(shown, Vec::new(), line_hl, s.code_block).font(s.code_block).color(color).wrap()).w_full()
                    .copy_breaks(if n == 0 { 2 } else { 1 }, if n == last { 2 } else { 1 })));
            if let Some(t) = tint {
                r = r.fill(t);
            }
            c = c.child(r);
        }
        c
    }

    /// A diagram, drawn from its complete lines while its fence is still open; until any read
    /// (or for a type it doesn't draw), its source as a code block. While [`Renderer::streaming`],
    /// a fence whose header may yet draw shows nothing until it does, so the source never
    /// flashes before the diagram.
    pub(super) fn mermaid(&self, src: &str, source: Option<&Source>, map: Option<&SpanMap>, index: usize) -> El {
        #[cfg(not(feature = "mermaid"))]
        let _ = index;
        #[cfg(feature = "mermaid")]
        {
            let open = source.is_some_and(|s| s.open);
            let lines = if open { &src[..source.map_or(src.len(), |s| s.coloured.min(src.len()))] } else { src };
            // The card sits in a bare holder, so a fence that draws nothing yet holds its place
            // and the card mounts into it (fading in) rather than the block turning into one.
            let holder = || col().w_full().transition(rune::Transition::none());
            if let Some(d) = crate::mermaid::diagram(lines, &self.s.diagram, self.streaming, open) {
                let card = d.key("card");
                let card = match &self.diagram_frame {
                    Some(f) => f(card, &DiagramFrame { index, source: lines, open }),
                    None => card,
                };
                return holder().child(card);
            }
            if open && self.streaming && crate::mermaid::may_draw(lines) {
                return holder();
            }
            col().w_full().child(self.code(src, source, map).key("source"))
        }
        #[cfg(not(feature = "mermaid"))]
        self.code(src, source, map)
    }

    #[allow(clippy::too_many_arguments)]
    fn task_item_body(&self, (k, n): (usize, usize), indent: f32, item: &[Block], item_body: El, item_idx: usize, checked: bool) -> El {
        let t = &self.s.task;
        let interactive = self.on_task.is_some() && !self.tasks_read_only;
        let (fill, border) = if checked {
            (t.fill_on, t.border_on)
        } else {
            (t.fill_off, t.border_off)
        };
        let mut control = el()
            .key("control")
            .size(t.size, t.size)
            .ml(t.left)
            .mr((indent - t.left - t.size).max(0.0))
            .mt(t.top.unwrap_or(((self.s.body.line_height - t.size) * 0.5).max(0.0)))
            .center()
            .radius(t.radius)
            .fill(fill)
            .border(1.0, border)
            .token_motion(t.motion)
            .focus_ring(1.0, 0.0, t.focus_ring)
            .child(icon(t.check, t.size * 0.625).key("mark").color(t.mark).opacity(if checked { 1.0 } else { 0.0 }))
            .a11y_hidden();
        if interactive {
            control = control.hover(t.hover);
        }
        let mut r = row()
            .key(format!("i{k}"))
            .role("checkbox")
            .label(item_label(item))
            .checked(checked)
            .read_only(!interactive)
            .position_in_set(k + 1, n)
            .focus_selected(checked)
            .focusable(interactive)
            .w_full()
            .child(control)
            .child(item_body.grow(1.0).shrink());
        if let Some(on_task) = self.on_task.as_ref().filter(|_| !self.tasks_read_only) {
            let block_key = self.current_block_key.borrow().clone();
            r = on_task(r.cursor(Cursor::Pointer), block_key, item_idx, !checked);
        }
        r
    }

    fn list_marker(&self, start: Option<u64>, k: usize, indent: f32, space: f32) -> El {
        let s = &self.s;
        if start.is_none() && s.list_outside {
            el().w(indent).h(s.body.line_px()).child(el().outside_list_disc(s.body).fill(s.muted))
        } else {
            let words = start.map_or_else(|| "•".into(), |num| format!("{}.", num + k as u64));
            let marker = text(words).font(s.body).color(s.muted).exact_lines().w(indent);
            if s.list_outside { marker.align(Align::End).pr(space) } else { marker.align(Align::Center) }
        }
    }

    fn deep_tail(&self, inl: &Inline, lines: usize, map: &SpanMap) -> El {
        const CHUNK_CAP: usize = 32 * 1024;
        let s = &self.s;
        let flow = self.flow.get();
        let pad = " ".repeat(flow.cols);
        let label = rune::labels::label(rune::Label::MoreNestedLines(lines));
        let header = el()
            .key("more")
            .role("note")
            .label(label.clone())
            .w_full()
            .child(self.with_reveal(text(label)).font(s.code_block).color(s.muted).w_full().a11y_hidden());
        let all_hl = self.text_highlights_for(&inl.text, Some(map));
        let ink = flow.ink.unwrap_or(s.text);
        let mut card = col()
            .key("deep-tail")
            .w_full()
            .gap(4.0)
            .px(10.0)
            .py(8.0)
            .radius(6.0)
            .fill(s.code_block_fill)
            .border(0.5, s.code_block_border)
            .border_layout()
            .child(header);

        let full = inl.text.as_str();
        let mut start = 0usize;
        let mut chunk_idx = 0usize;
        while start < full.len() || chunk_idx == 0 {
            let max_end = (start + CHUNK_CAP).min(full.len());
            let end = if max_end == full.len() {
                full.len()
            } else if let Some(rel_nl) = full.as_bytes()[start..max_end].iter().rposition(|&b| b == b'\n') {
                start + rel_nl
            } else {
                let mut boundary = max_end;
                while boundary > start && !full.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                if boundary > start {
                    boundary
                } else {
                    let mut fwd = max_end + 1;
                    while fwd < full.len() && !full.is_char_boundary(fwd) {
                        fwd += 1;
                    }
                    fwd
                }
            };
            let slice = &full[start..end];
            let chunk_hl: Vec<TextHighlight> = all_hl
                .iter()
                .filter_map(|h| {
                    let r = h.range();
                    let a = r.start.max(start);
                    let b = r.end.min(end);
                    (a < b).then(|| h.clone().with_range((a - start)..(b - start)))
                })
                .collect();
            let is_last = end >= full.len();
            let node = self
                .select(
                    self.with_reveal(tabbed(slice, Vec::new(), chunk_hl, s.code_block).font(s.code_block).color(ink).wrap())
                        .w_full()
                        .copy_prefix(pad.clone())
                        .copy_breaks(if chunk_idx == 0 { 2 } else { 1 }, if is_last { 2 } else { 1 }),
                )
                .key(format!("c{chunk_idx}"));
            card = card.child(node);
            chunk_idx += 1;
            if is_last {
                break;
            }
            start = if full.as_bytes().get(end) == Some(&b'\n') { end + 1 } else { end };
        }
        card
    }

    fn leaf_block(&self, b: &Block, pre: Option<&super::memo::PreBlock>, map: &BlockMap) -> El {
        let s = &self.s;
        if let (Block::Para(inl), BlockMap::DeepTail { lines, map: tail_map }) = (b, map) {
            return self.deep_tail(inl, *lines, tail_map);
        }
        let inl_map = match map {
            BlockMap::Inline(m) => Some(m),
            _ => None,
        };
        let code_map = match map {
            BlockMap::Code(m) => Some(m),
            _ => None,
        };
        match b {
            Block::Para(inl) => self.para(inl, inl_map, s.body, self.flow.get().ink.unwrap_or(s.text)),
            Block::Heading(l, inl) => self.para(inl, inl_map, s.headings[(*l as usize).clamp(1, 4) - 1], if *l >= 4 { s.muted } else { s.text }).role("heading").level(u32::from(*l)),
            Block::Code(code) => {
                let n = self.next_source.get();
                self.next_source.set(n + 1);
                let source = self.sources.get(n).filter(|src| src.len == code.len());
                match &self.code_frame {
                    Some(f) => {
                        let card = match self.settled_code_card_digest(code, source, code_map) {
                            Some(digest) => rune::memo::cached(digest, || {
                                super::memo::note_block_build();
                                self.code(code, source, code_map).key("card")
                            }),
                            None => self.code(code, source, code_map).key("card"),
                        };
                        f(card, &CodeFrame { index: n, source: code, language: source.and_then(|s| s.lang.as_deref()), open: source.is_some_and(|s| s.open) })
                    }
                    None => self.code(code, source, code_map),
                }
            }
            Block::Diff(src) => self.diff(src, code_map),
            Block::Mermaid(src) => {
                let n = self.next_source.get();
                self.next_source.set(n + 1);
                self.mermaid(src, self.sources.get(n).filter(|s| s.len == src.len()), code_map, n)
            }
            Block::Table(t) => {
                let (head_maps, row_maps) = match map {
                    BlockMap::Table { head, rows } => (head.as_slice(), rows.as_slice()),
                    _ => (&[][..], &[][..]),
                };
                bidi::directed(self.table(t, pre.and_then(|p| p.table.as_deref()), head_maps, row_maps), bidi::table_dir(t))
            }
            Block::Image { src, alt, title } => self.image(src, alt, title),
            Block::Rule if s.rule.a <= 0.0 => el().w_full().h(0.0),
            Block::Rule => el().w_full().h(0.5).fill(s.rule),
            Block::List { .. } | Block::Quote(_) => unreachable!(),
        }
    }
}

fn list_item_gap(s: &MarkdownStyle, items: &[Vec<Block>], k: usize) -> f32 {
    if k == 0 {
        return 0.0;
    }
    let prev_edge = items[k - 1].last().map_or(0.0, |l| collapsed_bottom(l, s, items[k - 1].len() - 1, true));
    let cur_edge = items[k].first().map_or(0.0, |f| collapsed_top(f, s, 0, true));
    s.list_gap.max(prev_edge).max(cur_edge)
}

fn item_label(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        let text = match b {
            Block::Para(inl) | Block::Heading(_, inl) => unpadded(&inl.text),
            Block::Code(s) | Block::Diff(s) | Block::Mermaid(s) => s.clone(),
            Block::Image { alt, .. } => alt.clone(),
            _ => continue,
        };
        if !text.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&text);
        }
    }
    out
}

pub(super) fn hash_of(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}
