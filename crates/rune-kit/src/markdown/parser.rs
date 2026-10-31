//! Markdown source to blocks: the pulldown-cmark event walk, code spans, GFM tables, and the
//! maps from source bytes to rendered text.

use super::*;

/// Maps source UTF-8 byte ranges to rendered text UTF-8 byte ranges.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SpanMap(pub(super) Vec<(Range<usize>, Range<usize>)>);

impl SpanMap {
    fn push(&mut self, src: Range<usize>, out: Range<usize>) {
        if src.start >= src.end || out.start >= out.end {
            return;
        }
        if let Some((last_src, last_out)) = self.0.last_mut()
            && last_src.end == src.start
            && last_out.end == out.start
            && last_src.len() == last_out.len()
            && src.len() == out.len()
        {
            last_src.end = src.end;
            last_out.end = out.end;
            return;
        }
        self.0.push((src, out));
    }

    fn truncate_out(&mut self, len: usize) {
        self.0.retain_mut(|(src, out)| {
            if out.start >= len {
                return false;
            }
            if out.end > len {
                let cut = out.end - len;
                if src.len() == out.len() {
                    src.end = src.end.saturating_sub(cut);
                }
                out.end = len;
            }
            src.start < src.end && out.start < out.end
        });
    }

    pub(super) fn shift_src(&mut self, offset: usize) {
        if offset == 0 {
            return;
        }
        for (src, _) in &mut self.0 {
            src.start += offset;
            src.end += offset;
        }
    }
}

/// Source-to-rendered byte mappings parallel to [`Block`], kept beside `Block` so `Block` keeps
/// its shape.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum BlockMap {
    Inline(SpanMap),
    Code(SpanMap),
    List(Vec<Vec<BlockMap>>),
    Quote(Vec<BlockMap>),
    Table { head: Vec<SpanMap>, rows: Vec<Vec<SpanMap>> },
    #[default]
    None,
}

impl BlockMap {
    pub(super) fn shift_src(mut self, offset: usize) -> Self {
        if offset == 0 {
            return self;
        }
        match &mut self {
            BlockMap::Inline(m) | BlockMap::Code(m) => m.shift_src(offset),
            BlockMap::List(items) => {
                for item in items {
                    for b in item {
                        *b = std::mem::take(b).shift_src(offset);
                    }
                }
            }
            BlockMap::Quote(inner) => {
                for b in inner {
                    *b = std::mem::take(b).shift_src(offset);
                }
            }
            BlockMap::Table { head, rows } => {
                for m in head {
                    m.shift_src(offset);
                }
                for row in rows {
                    for m in row {
                        m.shift_src(offset);
                    }
                }
            }
            BlockMap::None => {}
        }
        self
    }
}

pub(super) struct Builder {
    /// Open containers: (kind, blocks so far, maps so far).
    stack: Vec<(Container, Vec<Block>, Vec<BlockMap>)>,
    inline: Option<(InlineKind, Inline, Vec<Open>, SpanMap)>,
    code: Option<(String, Fence, SpanMap)>,
    /// The source range of the last paragraph started at the root.
    last_root_para: Option<Range<usize>>,
    sources: Vec<Source>,
    /// One entry per [`Tag::Item`] in document order (`Some(checked)` for GFM task items).
    tasks: Vec<Option<(bool, usize)>>,
    block_starts: Vec<usize>,
    root_start: usize,
    cuts: Vec<Cut>,
    autolink: bool,
    image_starts: Vec<usize>,
    image_ranges: Vec<(usize, usize)>,
}

/// Where a top-level block starts: the start of its first line, and how many top-level blocks,
/// [`Source`]s and list items come before it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cut {
    pub(super) at: usize,
    pub(super) blocks: usize,
    pub(super) sources: usize,
    pub(super) tasks: usize,
}

/// What colours a [`Block::Code`], one per block in document order (the order the renderer
/// meets them). Kept beside the blocks so `Block` keeps its shape.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Source {
    /// The fence's language as written (its info string's first word), if it names one.
    pub(super) lang: Option<Rc<str>>,
    /// The code's length, to check this belongs to the block it is matched with.
    pub(super) len: usize,
    /// How many leading bytes get colour: all of them, or while an unclosed fence streams in,
    /// the complete lines (the line still arriving draws plain until its newline).
    pub(super) coloured: usize,
}

#[derive(Clone, PartialEq)]
enum Fence {
    /// A code block; its language as written, if it names one, and whether its closing fence
    /// has arrived.
    Plain(Option<Rc<str>>, bool),
    Diff,
    /// A mermaid fence; whether its closing fence has arrived.
    Mermaid(bool),
}

/// An emphasis, strong, strike or link run that has started and not yet ended.
pub(super) struct Open {
    start: usize,
    style: Style,
    url: Option<String>,
}

enum Container {
    Root,
    Quote,
    List(Option<u64>, Vec<Vec<Block>>, Vec<Vec<BlockMap>>),
    Item(usize),
    Table(Table, Vec<Inline>, Vec<SpanMap>, Vec<Vec<SpanMap>>, Vec<SpanMap>),
}

#[derive(Clone, Copy)]
enum InlineKind {
    Para,
    Heading(u8),
    Cell,
}

impl Builder {
    fn push(&mut self, b: Block, map: BlockMap) {
        if self.stack.len() == 1 {
            self.block_starts.push(self.root_start);
        }
        let top = self.stack.last_mut().unwrap();
        top.1.push(b);
        top.2.push(map);
    }

    /// Pushes a code block's text: without its final newline, or at the end of input without
    /// any trailing newlines.
    fn push_code(&mut self, mut text: String, fence: Fence, mut map: SpanMap, eof: bool) {
        let line_ended = text.ends_with('\n');
        if eof {
            text.truncate(text.trim_end_matches(['\n', '\r']).len());
        } else if line_ended {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
        map.truncate_out(text.len());
        let block = code_block(text, &fence);
        if let Block::Code(code) = &block {
            let (lang, closed) = match fence {
                Fence::Plain(lang, closed) => (lang, closed),
                // A mermaid fence still arriving.
                _ => (Some("mermaid".into()), false),
            };
            let coloured = if closed || line_ended { code.len() } else { code.rfind('\n').unwrap_or(0) };
            self.sources.push(Source { lang, len: code.len(), coloured });
        }
        self.push(block, BlockMap::Code(map));
    }

    fn inline(&mut self) -> &mut (InlineKind, Inline, Vec<Open>, SpanMap) {
        // Tight list items have text without a paragraph.
        self.inline.get_or_insert_with(|| (InlineKind::Para, Inline::default(), Vec::new(), SpanMap::default()))
    }

    fn flush_inline(&mut self) {
        if let Some((kind, mut inl, open, mut map)) = self.inline.take() {
            for o in open {
                end_run(&mut inl, o);
            }
            let trimmed = inl.text.trim_end().len();
            inl.text.truncate(trimmed);
            map.truncate_out(trimmed);
            inl.spans.retain_mut(|s| {
                s.1 = s.1.min(trimmed);
                s.0 < s.1
            });
            inl.links.retain_mut(|s| {
                s.1 = s.1.min(trimmed);
                s.0 < s.1
            });
            if self.autolink {
                autolink_inline(&mut inl, &self.image_ranges);
            }
            self.image_ranges.clear();
            self.image_starts.clear();
            if let InlineKind::Cell = kind {
                // Empty cells still hold their column.
                if let Some((Container::Table(_, cells, _, _, cell_maps), _, _)) = self.stack.last_mut() {
                    cells.push(inl);
                    cell_maps.push(map);
                }
                return;
            }
            if inl.text.is_empty() {
                return;
            }
            self.push(
                match kind {
                    InlineKind::Para | InlineKind::Cell => Block::Para(inl),
                    InlineKind::Heading(l) => Block::Heading(l, inl),
                },
                BlockMap::Inline(map),
            );
        }
    }

    /// Closes the innermost container into its parent.
    fn pop_container(&mut self) {
        self.flush_inline();
        let Some((c, blocks, maps)) = self.stack.pop() else { return };
        match c {
            Container::Item(_) => {
                if let Some((Container::List(_, items, item_maps), _, _)) = self.stack.last_mut() {
                    items.push(blocks);
                    item_maps.push(maps);
                }
            }
            Container::List(start, items, item_maps) => self.push(Block::List { start, items }, BlockMap::List(item_maps)),
            Container::Quote => self.push(Block::Quote(blocks), BlockMap::Quote(maps)),
            Container::Table(mut t, cells, head, mut rows, cell_maps) => {
                if !cells.is_empty() {
                    t.rows.push(cells);
                    rows.push(cell_maps);
                }
                self.push(Block::Table(t), BlockMap::Table { head, rows });
            }
            Container::Root => {}
        }
    }

    fn table(&mut self) -> Option<(&mut Table, &mut Vec<Inline>, &mut Vec<SpanMap>, &mut Vec<Vec<SpanMap>>, &mut Vec<SpanMap>)> {
        match self.stack.last_mut() {
            Some((Container::Table(t, cells, head, rows, cell_maps), _, _)) => Some((t, cells, head, rows, cell_maps)),
            _ => None,
        }
    }
}

pub(super) const PAD: &str = "\u{202f}\u{202f}";

/// Source byte range of the inner code text inside a backtick code span `src[range]`.
fn code_span_content_range(src: &str, range: Range<usize>, rendered: &str) -> Range<usize> {
    let Some(raw) = src.get(range.clone()) else { return range };
    let open = raw.bytes().take_while(|&b| b == b'`').count();
    let close = raw[open..].bytes().rev().take_while(|&b| b == b'`').count();
    let mut start = range.start + open;
    let mut end = range.end.saturating_sub(close);
    if !rendered.chars().all(|c| c == ' ')
        && src.as_bytes().get(start).is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n'))
        && src.as_bytes().get(end.saturating_sub(1)).is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n'))
    {
        start += 1;
        end -= 1;
    }
    if start < end { start..end } else { range }
}

/// Code spans normalize newlines and omit continuation-line container prefixes. Map the
/// actual surviving characters, not a ratio of the source and displayed byte lengths.
fn map_code_span(src: &str, range: Range<usize>, rendered: &str, offset: usize, map: &mut SpanMap) {
    if &src[range.clone()] == rendered {
        map.push(range, offset..offset + rendered.len());
        return;
    }
    let mut out = 0;
    let mut at = range.start;
    for line in src[range].split_inclusive(['\r', '\n']) {
        let body = line.trim_end_matches(['\r', '\n']);
        let remaining = &rendered[out..];
        let matched = body.char_indices().find_map(|(skip, _)| {
            let mut chars = body[skip..].char_indices().peekable();
            let mut consumed = 0;
            let mut pairs = Vec::new();
            while let Some((i, mut c)) = chars.next() {
                let mut end = i + c.len_utf8();
                if c == '\\' && chars.peek().is_some_and(|(_, c)| *c == '|') && remaining[consumed..].starts_with('|') {
                    let (j, next) = chars.next().unwrap();
                    c = next;
                    end = j + c.len_utf8();
                }
                if !remaining[consumed..].starts_with(c) { return None; }
                pairs.push((at + skip + i..at + skip + end, offset + out + consumed..offset + out + consumed + c.len_utf8()));
                consumed += c.len_utf8();
            }
            Some((consumed, pairs))
        });
        if let Some((consumed, pairs)) = matched {
            for (src, dest) in pairs { map.push(src, dest); }
            out += consumed;
        }
        if line.ends_with(['\r', '\n']) && rendered[out..].starts_with(' ') {
            map.push(at + body.len()..at + line.len(), offset + out..offset + out + 1);
            out += 1;
        }
        at += line.len();
    }
}

/// Parses `src` into blocks. Unclosed constructs are closed at the end of input.
pub fn parse(src: &str) -> Vec<Block> {
    parse_doc(src, true).0
}

pub(super) fn parse_doc(src: &str, autolink: bool) -> (Vec<Block>, Vec<Source>, Vec<Option<(bool, usize)>>, Vec<Cut>, Vec<usize>, Vec<BlockMap>) {
    let mut b = Builder {
        stack: vec![(Container::Root, Vec::new(), Vec::new())],
        inline: None,
        code: None,
        last_root_para: None,
        sources: Vec::new(),
        tasks: Vec::new(),
        block_starts: Vec::new(),
        root_start: 0,
        cuts: Vec::new(),
        autolink,
        image_starts: Vec::new(),
        image_ranges: Vec::new(),
    };
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for (ev, range) in Parser::new_ext(src, opts).into_offset_iter() {
        let starts_block = matches!(
            ev,
            Event::Start(
                Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::BlockQuote(_) | Tag::List(_) | Tag::Table(_) | Tag::HtmlBlock
            ) | Event::Rule
        );
        if starts_block && b.stack.len() == 1 && b.code.is_none() {
            b.flush_inline();
            let at = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
            b.root_start = at;
            b.cuts.push(Cut { at, blocks: b.stack[0].1.len(), sources: b.sources.len(), tasks: b.tasks.len() });
        }
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    b.flush_inline();
                    if b.stack.len() == 1 {
                        b.last_root_para = Some(range);
                    }
                    b.inline = Some((InlineKind::Para, Inline::default(), Vec::new(), SpanMap::default()));
                }
                Tag::Heading { level, .. } => {
                    b.flush_inline();
                    let l = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        _ => 4,
                    };
                    b.inline = Some((InlineKind::Heading(l), Inline::default(), Vec::new(), SpanMap::default()));
                }
                Tag::CodeBlock(kind) => {
                    b.flush_inline();
                    let fence = match &kind {
                        CodeBlockKind::Fenced(info) => {
                            // `rust,ignore` and `ts title="a.ts"` name their language first.
                            let word = info.split(|c: char| c.is_whitespace() || c == ',').next().unwrap_or("");
                            match word.to_ascii_lowercase().as_str() {
                                "diff" | "patch" => Fence::Diff,
                                "mermaid" => Fence::Mermaid(fence_closed(&src[range])),
                                _ => Fence::Plain((!word.is_empty()).then(|| word.into()), fence_closed(&src[range])),
                            }
                        }
                        CodeBlockKind::Indented => Fence::Plain(None, true),
                    };
                    b.code = Some((String::new(), fence, SpanMap::default()));
                }
                Tag::BlockQuote(_) => {
                    b.flush_inline();
                    b.stack.push((Container::Quote, Vec::new(), Vec::new()));
                }
                Tag::List(start) => {
                    b.flush_inline();
                    b.stack.push((Container::List(start, Vec::new(), Vec::new()), Vec::new(), Vec::new()));
                }
                Tag::Item => {
                    b.flush_inline();
                    let idx = b.tasks.len();
                    b.tasks.push(None);
                    b.stack.push((Container::Item(idx), Vec::new(), Vec::new()));
                }
                Tag::Table(aligns) => {
                    b.flush_inline();
                    let align = aligns
                        .iter()
                        .map(|a| match a {
                            Alignment::Center => Align::Center,
                            Alignment::Right => Align::End,
                            Alignment::None | Alignment::Left => Align::Start,
                        })
                        .collect();
                    b.stack.push((
                        Container::Table(Table { align, ..Table::default() }, Vec::new(), Vec::new(), Vec::new(), Vec::new()),
                        Vec::new(),
                        Vec::new(),
                    ));
                }
                Tag::TableHead | Tag::TableRow => {
                    // A row is still arriving while its line has no newline yet.
                    let open = !src[range.start..].contains('\n');
                    if let Some((t, _, _, _, _)) = b.table() {
                        t.open = open;
                    }
                }
                Tag::TableCell => {
                    b.flush_inline();
                    b.inline = Some((InlineKind::Cell, Inline::default(), Vec::new(), SpanMap::default()));
                }
                Tag::Strong => open(&mut b, Style::Strong, None),
                Tag::Emphasis => open(&mut b, Style::Em, None),
                Tag::Strikethrough => open(&mut b, Style::Strike, None),
                Tag::Link { dest_url, .. } => open(&mut b, Style::Link, Some(dest_url.into_string())),
                Tag::Image { .. } => {
                    let start = b.inline().1.text.len();
                    b.image_starts.push(start);
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::TableCell => b.flush_inline(),
                TagEnd::CodeBlock => {
                    if let Some((text, fence, map)) = b.code.take() {
                        b.push_code(text, fence, map, false);
                    }
                }
                TagEnd::TableHead => {
                    b.flush_inline();
                    if let Some((t, cells, head, _, cell_maps)) = b.table() {
                        t.head = std::mem::take(cells);
                        *head = std::mem::take(cell_maps);
                    }
                }
                TagEnd::TableRow => {
                    b.flush_inline();
                    if let Some((t, cells, _, rows, cell_maps)) = b.table() {
                        t.rows.push(std::mem::take(cells));
                        rows.push(std::mem::take(cell_maps));
                    }
                }
                TagEnd::BlockQuote(_) | TagEnd::Item | TagEnd::List(_) | TagEnd::Table => b.pop_container(),
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => close(&mut b),
                TagEnd::Image => {
                    if let Some(start) = b.image_starts.pop() {
                        let end = b.inline().1.text.len();
                        b.image_ranges.push((start, end));
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((code, _, map)) = &mut b.code {
                    let start = code.len();
                    code.push_str(&t);
                    map.push(range, start..code.len());
                } else {
                    let inl = b.inline();
                    let start = inl.1.text.len();
                    inl.1.text.push_str(&t);
                    inl.3.push(range, start..inl.1.text.len());
                }
            }
            Event::Code(t) => {
                let code_src = code_span_content_range(src, range, &t);
                let inl = b.inline();
                let a = inl.1.text.len();
                inl.1.text.push_str(PAD);
                let start = inl.1.text.len();
                inl.1.text.push_str(&t);
                let end = inl.1.text.len();
                inl.1.text.push_str(PAD);
                inl.1.spans.push((a, start, Style::CodePad));
                inl.1.spans.push((start, end, Style::Code));
                inl.1.spans.push((end, inl.1.text.len(), Style::CodePad));
                map_code_span(src, code_src, &t, start, &mut inl.3);
            }
            Event::SoftBreak => {
                let inl = b.inline();
                let start = inl.1.text.len();
                inl.1.text.push(' ');
                inl.3.push(range, start..inl.1.text.len());
            }
            Event::HardBreak => {
                let inl = b.inline();
                let start = inl.1.text.len();
                inl.1.text.push('\n');
                inl.3.push(range, start..inl.1.text.len());
            }
            Event::TaskListMarker(checked) => {
                if let Some(idx) = b.stack.iter().rev().find_map(|(c, _, _)| match c {
                    Container::Item(idx) => Some(*idx),
                    _ => None,
                }) {
                    b.tasks[idx] = Some((checked, range.start));
                }
            }
            Event::Rule => {
                b.flush_inline();
                b.push(Block::Rule, BlockMap::None);
            }
            _ => {}
        }
    }
    b.flush_inline();
    if let Some((text, fence, map)) = b.code.take() {
        b.push_code(text, fence, map, true);
    }
    // Unwind containers left open by a partial stream.
    while b.stack.len() > 1 {
        b.pop_container();
    }
    let last_para = b.last_root_para.take();
    let (_, mut blocks, mut maps) = b.stack.pop().unwrap();
    if let (Some(Block::Para(_)), Some(range)) = (blocks.last(), last_para)
        && let Some((t, map)) = pending_table(&src[range.start..], range.start, autolink)
    {
        *blocks.last_mut().unwrap() = Block::Table(t);
        *maps.last_mut().unwrap() = map;
    }
    (blocks, b.sources, b.tasks, b.cuts, b.block_starts, maps)
}

fn code_block(text: String, fence: &Fence) -> Block {
    match fence {
        Fence::Plain(..) | Fence::Mermaid(false) => Block::Code(text),
        Fence::Diff => Block::Diff(text),
        Fence::Mermaid(true) => Block::Mermaid(text),
    }
}

/// Whether a fenced code block's source (from its opening fence) has its closing fence.
fn fence_closed(block: &str) -> bool {
    fn strip(l: &str) -> &str {
        l.trim_start_matches(|c: char| c == '>' || c.is_whitespace()).trim_end()
    }
    let mut lines = block.lines();
    let Some(first) = lines.next().map(strip) else { return false };
    let Some(ch) = first.chars().next() else { return false };
    let n = first.chars().take_while(|&c| c == ch).count();
    lines.next_back().map(strip).is_some_and(|l| l.len() >= n && l.chars().all(|c| c == ch))
}

/// The header (and partial delimiter row) of a table still streaming in, from the source of the
/// document's last paragraph: `| a | b |` alone, or followed by a delimiter row in progress.
fn pending_table(src: &str, base_offset: usize, autolink: bool) -> Option<(Table, BlockMap)> {
    let mut lines = src.trim_end().lines();
    let raw_head = lines.next()?;
    let lead_ws = raw_head.len() - raw_head.trim_start().len();
    let head = raw_head.trim();
    let delim = lines.next().map(str::trim);
    if lines.next().is_some() || !head.starts_with('|') {
        return None;
    }
    let cells = split_row(head, base_offset + lead_ws);
    if cells.len() < 2 || delim.is_some_and(|d| !d.starts_with('|') || !d.chars().all(|c| "|:- ".contains(c))) {
        return None;
    }
    let mut align = vec![Align::Start; cells.len()];
    if let Some(d) = delim {
        for (a, (_, spec)) in align.iter_mut().zip(split_row(d, 0)) {
            *a = match (spec.starts_with(':'), spec.len() > 1 && spec.ends_with(':')) {
                (true, true) => Align::Center,
                (false, true) => Align::End,
                _ => Align::Start,
            };
        }
    }
    let mut head_inlines = Vec::with_capacity(cells.len());
    let mut head_maps = Vec::with_capacity(cells.len());
    for (cell_off, c) in &cells {
        let (inl, map) = inline_of(c, *cell_off, autolink);
        head_inlines.push(inl);
        head_maps.push(map);
    }
    Some((
        Table { align, head: head_inlines, rows: Vec::new(), open: true },
        BlockMap::Table { head: head_maps, rows: Vec::new() },
    ))
}

/// The cells of a `| a | b |` line (a trailing pipe is optional; `\|` doesn't split, and stays
/// escaped for [`inline_of`] to unescape), with each trimmed cell's starting byte offset.
fn split_row(line: &str, base_offset: usize) -> Vec<(usize, String)> {
    let pipe_off = usize::from(line.starts_with('|'));
    let inner = &line[pipe_off..];
    let mut cells: Vec<(usize, String)> = vec![(base_offset + pipe_off, String::new())];
    let mut chars = inner.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        match c {
            '\\' if chars.peek().is_some_and(|&(_, next)| next == '|') => {
                cells.last_mut().unwrap().1.extend(['\\', chars.next().unwrap().1]);
            }
            '|' => cells.push((base_offset + pipe_off + idx + 1, String::new())),
            c => cells.last_mut().unwrap().1.push(c),
        }
    }
    if cells.last().is_some_and(|(_, c)| c.trim().is_empty()) && inner.trim_end().ends_with('|') {
        cells.pop();
    }
    cells
        .into_iter()
        .map(|(off, c)| {
            let lead = c.len() - c.trim_start().len();
            (off + lead, c.trim().to_owned())
        })
        .collect()
}

fn inline_of(src: &str, offset: usize, autolink: bool) -> (Inline, SpanMap) {
    let (blocks, _, _, _, _, maps) = parse_doc(src, autolink);
    match (blocks.into_iter().next(), maps.into_iter().next()) {
        (Some(Block::Para(i)), Some(BlockMap::Inline(mut m))) => {
            m.shift_src(offset);
            (i, m)
        }
        _ => {
            let mut m = SpanMap::default();
            m.push(offset..offset + src.len(), 0..src.len());
            (Inline { text: src.to_owned(), ..Inline::default() }, m)
        }
    }
}

pub(super) fn open(b: &mut Builder, style: Style, url: Option<String>) {
    let inl = b.inline();
    let start = inl.1.text.len();
    inl.2.push(Open { start, style, url });
}

fn close(b: &mut Builder) {
    let (_, inl, open, _) = b.inline();
    if let Some(o) = open.pop() {
        end_run(inl, o);
    }
}

/// Ends run `o` at the current end of the text.
fn end_run(inl: &mut Inline, o: Open) {
    let end = inl.text.len();
    if let Some(url) = o.url {
        inl.links.push((o.start, end, url));
    }
    inl.spans.push((o.start, end, o.style));
}
