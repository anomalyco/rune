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
    pub(super) cols: u8,
    /// The marker a copy puts before the paragraph that opens a list item, its indent, and the
    /// newlines before it: two to set a top-level list apart from the block before it.
    pub(super) lead: Option<(u8, Lead, u8)>,
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
    pub(super) fn width(self) -> u8 {
        match self {
            Lead::Number(n) => n.to_string().len() as u8 + 2,
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
    let (mt, mb) = match b {
        Block::Para(_) if in_item => (if i == 0 { 0.0 } else { 8.0 }, if i == 0 { 0.0 } else { 12.0 }),
        Block::List { .. } if in_item => (4.0, 4.0),
        _ => margins(b, s),
    };
    let (first, last) = match b {
        Block::Quote(inner) => (
            inner.first().map(|f| collapsed(f, s, 0, false).0),
            inner.last().map(|l| collapsed(l, s, inner.len() - 1, false).1),
        ),
        Block::List { items, .. } => (
            items.first().and_then(|item| item.first()).map(|f| collapsed(f, s, 0, true).0),
            items.last().map(|item| item.last().map_or(0.0, |l| collapsed(l, s, item.len() - 1, true).1).max(s.list_gap)),
        ),
        _ => (None, None),
    };
    (first.map_or(mt, |f| f.max(mt)), last.map_or(mb, |l| l.max(mb)))
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
        // A copy puts a blank line around a paragraph, one newline around a list item's first
        // with its marker before it.
        let e = match self.flow.get().lead {
            Some((cols, lead, before)) => e.copy_prefix(format!("{}{}", " ".repeat(cols.into()), lead.marker())).copy_breaks(before, 1),
            None => e.copy_breaks(2, 2),
        };
        self.select_inline(e, inl, map)
    }

    /// `blocks` in a column, spaced by their collapsed margins. A heading's bottom margin alone
    /// sets the gap under it (CSS's `h2 + p { margin-top: 0 }`). In a list item (`in_item`) the
    /// first paragraph has no margins, a later one 8 above, and a nested list 4 above and below,
    /// as a web page spaces an `li`'s children.
    pub(super) fn blocks(&self, blocks: &[Block], maps: &[BlockMap], keys: &[String], top_level: bool) -> El {
        self.blocks_in(blocks, maps, keys, top_level, false)
    }

    pub(super) fn blocks_in(&self, blocks: &[Block], maps: &[BlockMap], keys: &[String], top_level: bool, in_item: bool) -> El {
        let mut c = col().w_full();
        let mut prev_mb = 0.0f32;
        let mut after_heading = false;
        let names = rendered_keys(blocks.len(), keys);
        let empty = BlockMap::None;
        for ((i, b), key) in blocks.iter().enumerate().zip(names) {
            // A fence still arriving with no lines yet (its language may be half written:
            // ```` ```mer ````) shows nothing until its first line, then mounts as what it is.
            if self.streaming && matches!(b, Block::Code(c) if c.is_empty()) {
                let n = self.next_source.get();
                if self.sources.get(n).is_some_and(|s| s.open && s.len == 0) {
                    self.next_source.set(n + 1);
                    continue;
                }
            }
            let (mt, mb) = collapsed(b, &self.s, i, in_item);
            let top = match b {
                _ if i == 0 => 0.0,
                // An image keeps its own 24 under a heading: the page zeroes the top margin of
                // the paragraph holding it, not the image's, which collapses through.
                Block::Heading(..) | Block::Rule | Block::Image { .. } => mt.max(prev_mb),
                _ if after_heading => prev_mb,
                _ => mt.max(prev_mb),
            };
            if top_level {
                self.current_block_key.replace(key.clone().into());
            }
            // Only an item's first block copies after its marker.
            if i > 0 {
                self.flow.set(Flow { lead: None, ..self.flow.get() });
            }
            c = c.child(self.block(b, maps.get(i).unwrap_or(&empty)).key(key).mt(top));
            prev_mb = mb;
            after_heading = matches!(b, Block::Heading(..));
        }
        c
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
        let body = self.with_reveal(tabbed(code, spans, highlights, s.code_block).font(s.code_block).color(ink)).copy_breaks(2, 2);
        let frame = col().role("code").w_full().radius(6.0).fill(fill).border(0.5, border);
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
        let mut c = col().role("code").w_full().py(s.code_block_pad[1]).radius(6.0).fill(s.code_block_fill).border(0.5, s.code_block_border);
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
    fn task_item(&self, (k, n): (usize, usize), indent: f32, item: &[Block], item_maps: &[BlockMap], item_idx: usize, checked: bool) -> El {
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
            .child(self.blocks_in(item, item_maps, &[], false, true).grow(1.0).shrink());
        if let Some(on_task) = self.on_task.as_ref().filter(|_| !self.tasks_read_only) {
            let block_key = self.current_block_key.borrow().clone();
            r = on_task(r.cursor(Cursor::Pointer), block_key, item_idx, !checked);
        }
        r
    }

    pub(super) fn block(&self, b: &Block, map: &BlockMap) -> El {
        let s = &self.s;
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
                let card = self.code(code, source, code_map);
                match &self.code_frame {
                    Some(f) => f(card.key("card"), &CodeFrame { index: n, source: code, language: source.and_then(|s| s.lang.as_deref()), open: source.is_some_and(|s| s.open) }),
                    None => card,
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
                self.table(t, head_maps, row_maps)
            }
            Block::List { start, items } => {
                let item_maps = match map {
                    BlockMap::List(m) => m.as_slice(),
                    _ => &[][..],
                };
                let flow = self.flow.get();
                let indent = match s.list_nested {
                    Some([bullets, numbers]) if flow.lists > 0 => if start.is_some() { numbers } else { bullets },
                    _ => s.list_indent,
                };
                // Outside markers end a space's width before the text, as a browser sets "1. ".
                let space = if s.list_outside { rune::text::measure("x x", &[], s.body, None).0 - rune::text::measure("xx", &[], s.body, None).0 } else { 0.0 };
                let mut c = col().role("list").w_full();
                let n = items.len();
                // Between items, their 8 collapsed with the margins passing out of the blocks
                // either side (`collapsed`): an item ending in a quote keeps its 16 below.
                let edge = |item: &[Block], last: bool| match (last, item) {
                    (true, [.., l]) => collapsed(l, s, item.len() - 1, true).1,
                    (false, [f, ..]) => collapsed(f, s, 0, true).0,
                    _ => 0.0,
                };
                let gap = |k: usize| if k == 0 { 0.0 } else { s.list_gap.max(edge(&items[k - 1], true)).max(edge(&items[k], false)) };
                for (k, item) in items.iter().enumerate() {
                    let imap = item_maps.get(k).map_or(&[][..], Vec::as_slice);
                    let task_idx = self.next_task.get();
                    self.next_task.set(task_idx + 1);
                    let task = self.tasks.get(task_idx).copied().flatten();
                    let lead = match (task, start) {
                        (Some((checked, _)), _) => Lead::Task(checked),
                        (None, Some(n)) => Lead::Number(n + k as u64),
                        (None, None) => Lead::Bullet,
                    };
                    self.flow.set(Flow { lists: flow.lists + 1, inset: flow.inset + indent, cols: flow.cols + lead.width(), lead: Some((flow.cols, lead, if k == 0 && flow.lists == 0 { 2 } else { 1 })), ..flow });
                    if let Some((checked, marker_offset)) = task {
                        c = c.child(self.task_item((k, n), indent, item, imap, marker_offset, checked).mt(gap(k)));
                        continue;
                    }
                    let marker = match start {
                        Some(n) => format!("{}.", n + k as u64),
                        None => "•".into(),
                    };
                    let marker = text(marker).font(s.body).color(s.muted).exact_lines().w(indent);
                    let marker = if s.list_outside { marker.align(Align::End).pr(space) } else { marker.align(Align::Center) };
                    // The item's place in the list says what its marker shows.
                    c = c.child(
                        row()
                            .key(format!("i{k}"))
                            .role("listitem")
                            .position_in_set(k + 1, n)
                            .w_full()
                            .mt(gap(k))
                            .child(self.with_reveal(marker).a11y_hidden())
                            .child(self.blocks_in(item, imap, &[], false, true).grow(1.0).shrink()),
                    );
                }
                self.flow.set(flow);
                c
            }
            Block::Quote(inner) => {
                let inner_maps = match map {
                    BlockMap::Quote(m) => m.as_slice(),
                    _ => &[][..],
                };
                let q = s.quote;
                let flow = self.flow.get();
                let first_task = self.next_task.get();
                self.flow.set(Flow { ink: q.text.or(flow.ink), inset: flow.inset + q.width + q.pad, ..flow });
                let drawn = self.blocks(inner, inner_maps, &[], false);
                self.flow.set(flow);
                let quote = row()
                    .role("blockquote")
                    .w_full()
                    .child(el().w(q.width).h_full().radius((q.width / 2.0).min(1.0)).fill(s.quote_rule))
                    .child(drawn.grow(1.0).shrink().pl(q.pad));
                match &self.quote_frame {
                    Some(f) => f(quote, &QuoteFrame { text: &self.plain_text(inner, first_task, Flow { lead: None, ..flow }) }),
                    None => quote,
                }
            }
            // A clear rule is spacing only (`hr { border: none; height: 0 }`).
            Block::Image { src, alt, title } => self.image(src, alt, title),
            Block::Rule if s.rule.a <= 0.0 => el().w_full().h(0.0),
            Block::Rule => el().w_full().h(0.5).fill(s.rule),
        }
    }
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
