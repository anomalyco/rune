//! Block tree builder: open container stack, nesting depth guards, deep-tail collapsing, inline
//! runs, and image lifting.

use super::super::*;
use super::{BlockMap, Cut, Source, SpanMap, Tail};

/// Maximum open HTML elements under `<body>` in Blink's HTML parser (`kMaximumHTMLParserDOMTreeDepth`
/// = 512, where `AttachLater` reparents only once `StackDepth() > 512`, leaving 511 elements under
/// `<html><body>` while text nodes still attach directly to the current element): 255 nested lists
/// (`<ul><li>` / `<ol><li>`), 1 quote + 255 nested lists, or 510 non-empty `<blockquote>`s.
pub(super) const MAX_ELEMENT_DEPTH: usize = 511;

/// Maximum flattened items/blocks rendered individually inside a past-bound container before the
/// remaining lines are grouped into one selectable, searchable plain block with a count marker.
pub(super) const MAX_FLATTENED_TAIL_ITEMS: usize = 64;

/// Maximum columns kept in a GFM table; cells past this column are dropped the way GFM drops a
/// row's cells past its header's.
pub(super) const MAX_TABLE_COLUMNS: usize = 128;

/// Maximum nested flex containers on targets without `rune-taffy` stack continuation (`wasm32`,
/// `ios` 1 MiB main thread, `android` 2 MiB `NativeActivity` thread, etc.). `770` preserves all
/// `255` nested lists (`765`), `1` quote + `255` lists (`767`), and `10` quotes + `250` lists
/// (`770`), while capping pure `<blockquote>` nesting at `385` (`770` flex containers).
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
const MAX_UNGUARDED_LAYOUT_DEPTH: usize = 770;

#[derive(Clone, PartialEq)]
pub(super) enum Fence {
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ListSlot {
    Nested,
    SiblingItems,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum QuoteSlot {
    Nested,
    Merged { stack_len: usize, list_slots_len: usize },
}

pub(super) enum Container {
    Root,
    Quote,
    List(Option<u64>, Vec<Vec<Block>>, Vec<Vec<BlockMap>>),
    Item(usize),
    Table(Table, Vec<Inline>, Vec<SpanMap>, Vec<Vec<SpanMap>>, Vec<SpanMap>),
}

#[derive(Clone, Copy)]
pub(super) enum InlineKind {
    Para,
    Heading(u8),
    Cell,
}

pub(super) struct Builder {
    /// Open containers: (kind, blocks so far, maps so far).
    pub(super) stack: Vec<(Container, Vec<Block>, Vec<BlockMap>)>,
    pub(super) quote_slots: Vec<QuoteSlot>,
    pub(super) list_slots: Vec<ListSlot>,
    pub(super) past_bound_items: usize,
    pub(super) inline: Option<(InlineKind, Inline, Vec<Open>, SpanMap)>,
    pub(super) code: Option<(String, Fence, SpanMap)>,
    /// The source range of the last paragraph started at the root.
    pub(super) last_root_para: Option<Range<usize>>,
    pub(super) sources: Vec<Source>,
    /// One entry per [`Tag::Item`] in document order (`Some(checked)` for GFM task items).
    pub(super) tasks: Vec<Option<(bool, usize)>>,
    pub(super) block_starts: Vec<usize>,
    pub(super) root_start: usize,
    pub(super) cuts: Vec<Cut>,
    pub(super) autolink: bool,
    pub(super) image_starts: Vec<usize>,
    pub(super) image_ranges: Vec<(usize, usize)>,
    /// Each open image's source and title, beside `image_starts`.
    pub(super) image_dests: Vec<(String, String)>,
    pub(super) skip_cell: bool,
    pub(super) tail: Tail,
}

impl Builder {
    pub(super) fn new(autolink: bool) -> Self {
        Builder {
            stack: vec![(Container::Root, Vec::new(), Vec::new())],
            quote_slots: Vec::new(),
            list_slots: Vec::new(),
            past_bound_items: 0,
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
            image_dests: Vec::new(),
            skip_cell: false,
            tail: Tail::None,
        }
    }

    pub(super) fn depth(&self) -> usize {
        self.stack.len() - 1
    }

    #[inline]
    pub(super) fn layout_fits(&self, add: usize) -> bool {
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            let cur: usize = self.stack[1..]
                .iter()
                .map(|(c, _, _)| match c {
                    Container::Quote => 2,
                    Container::List(..) => 1,
                    Container::Item(_) => 2,
                    Container::Table(..) => 2,
                    Container::Root => 0,
                })
                .sum();
            cur + add <= MAX_UNGUARDED_LAYOUT_DEPTH
        }
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        {
            let _ = add;
            true
        }
    }

    pub(super) fn emit_deep_tail(&mut self, src: &str, range: Range<usize>) {
        if range.start >= range.end {
            return;
        }
        let (block, bmap) = build_deep_tail(src, range);
        if let Some((Container::List(_, items, item_maps), _, _)) = self.stack.last_mut() {
            self.tasks.push(None);
            items.push(vec![block]);
            item_maps.push(vec![bmap]);
        } else {
            self.push(block, bmap);
        }
    }

    pub(super) fn first_merged_quote(&self) -> Option<(usize, usize, usize)> {
        self.quote_slots.iter().enumerate().find_map(|(i, q)| match *q {
            QuoteSlot::Merged { stack_len, list_slots_len } => Some((i, stack_len, list_slots_len)),
            QuoteSlot::Nested => None,
        })
    }

    pub(super) fn unwind_merged_quote(&mut self, q_idx: usize, stack_len: usize, list_slots_len: usize) -> usize {
        self.flush_inline();
        while self.stack.len() > stack_len {
            if matches!(self.stack.last(), Some((Container::List(_, items, _), _, _)) if items.is_empty()) {
                self.stack.pop();
            } else {
                self.pop_container();
            }
        }
        self.list_slots.truncate(list_slots_len);
        let open = self.quote_slots.len() - q_idx;
        self.quote_slots.truncate(q_idx);
        open
    }

    pub(super) fn push(&mut self, b: Block, map: BlockMap) {
        if self.stack.len() == 1 {
            self.block_starts.push(self.root_start);
        }
        let Self { stack, tasks, .. } = self;
        let top = stack.last_mut().unwrap();
        if let Container::List(_, items, item_maps) = &mut top.0 {
            if let (Some(last_b), Some(last_m)) = (items.last_mut(), item_maps.last_mut()) {
                last_b.push(b);
                last_m.push(map);
            } else {
                tasks.push(None);
                items.push(vec![b]);
                item_maps.push(vec![map]);
            }
            return;
        }
        top.1.push(b);
        top.2.push(map);
    }

    /// Pushes a code block's text: without its final newline, or at the end of input without
    /// any trailing newlines.
    pub(super) fn push_code(&mut self, mut text: String, fence: Fence, mut map: SpanMap, eof: bool) {
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
        if let Block::Code(code) | Block::Mermaid(code) = &block {
            let (lang, closed) = match fence {
                Fence::Plain(lang, closed) => (lang, closed),
                Fence::Mermaid(closed) => (Some("mermaid".into()), closed),
                Fence::Diff => (None, true),
            };
            let coloured = if closed || line_ended { code.len() } else { code.rfind('\n').unwrap_or(0) };
            self.sources.push(Source { lang, len: code.len(), coloured, open: !closed });
        }
        self.push(block, BlockMap::Code(map));
    }

    pub(super) fn inline(&mut self) -> &mut (InlineKind, Inline, Vec<Open>, SpanMap) {
        // Tight list items have text without a paragraph.
        self.inline.get_or_insert_with(|| (InlineKind::Para, Inline::default(), Vec::new(), SpanMap::default()))
    }

    pub(super) fn flush_inline(&mut self) {
        if let Some((kind, mut inl, open, mut map)) = self.inline.take() {
            for o in open {
                end_run(&mut inl, o);
            }
            let trimmed = trimmed_len(&inl);
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
                // Empty cells still hold their column, up to MAX_TABLE_COLUMNS.
                if let Some((Container::Table(_, cells, _, _, cell_maps), _, _)) = self.stack.last_mut()
                    && cells.len() < MAX_TABLE_COLUMNS
                {
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

    pub(super) fn in_link(&self) -> bool {
        self.inline.as_ref().is_some_and(|(_, _, open, _)| open.iter().any(|o| o.style == Style::Link))
    }

    /// Closes the innermost container into its parent.
    pub(super) fn pop_container(&mut self) {
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

    /// Lifts the image whose alt text is `start..end` of the paragraph being read out into a
    /// [`Block::Image`] of its own, the text before it ending a paragraph and the text after it
    /// starting one. An image in a heading, a table cell, a link or emphasis stays in the line.
    pub(super) fn lift_image(&mut self, start: usize, end: usize, src: String, title: String) -> bool {
        let Some((InlineKind::Para, inl, open, map)) = self.inline.as_mut() else { return false };
        // An image still arriving (no source yet) stays its alt text until it is whole.
        if !open.is_empty() || end > inl.text.len() || src.is_empty() {
            return false;
        }
        let alt = inl.text[start..end].to_owned();
        let mut alt_map = SpanMap::default();
        for (src, out) in &map.0 {
            let overlap = start.max(out.start)..end.min(out.end);
            if !overlap.is_empty() {
                let source = if src.len() == out.len() {
                    src.start + (overlap.start - out.start)..src.start + (overlap.end - out.start)
                } else { src.clone() };
                alt_map.push(source, overlap.start - start..overlap.end - start);
            }
        }
        inl.text.truncate(start);
        inl.spans.retain_mut(|s| {
            s.1 = s.1.min(start);
            s.0 < s.1
        });
        inl.links.retain_mut(|s| {
            s.1 = s.1.min(start);
            s.0 < s.1
        });
        map.truncate_out(start);
        self.flush_inline();
        self.push(Block::Image { src, alt, title }, BlockMap::Inline(alt_map));
        self.inline = Some((InlineKind::Para, Inline::default(), Vec::new(), SpanMap::default()));
        true
    }

    pub(super) fn table(&mut self) -> Option<(&mut Table, &mut Vec<Inline>, &mut Vec<SpanMap>, &mut Vec<Vec<SpanMap>>, &mut Vec<SpanMap>)> {
        match self.stack.last_mut() {
            Some((Container::Table(t, cells, head, rows, cell_maps), _, _)) => Some((t, cells, head, rows, cell_maps)),
            _ => None,
        }
    }
}

/// `inl`'s text without its trailing whitespace and, when inline code ends it, the rest of that
/// code's padding: the line's end closes the isolate, so text that ends in code is a prefix of
/// the same text with more after the code, as a stream needs.
fn trimmed_len(inl: &Inline) -> usize {
    let text = inl.text.trim_end();
    match text.strip_suffix('\u{2069}') {
        Some(rest) if inl.spans.iter().any(|s| s.2 == Style::CodePad && s.0 == rest.len()) => rest.trim_end().len(),
        _ => text.len(),
    }
}

fn code_block(text: String, fence: &Fence) -> Block {
    match fence {
        Fence::Plain(..) => Block::Code(text),
        Fence::Diff => Block::Diff(text),
        Fence::Mermaid(_) => Block::Mermaid(text),
    }
}

/// Builds a single plain [`Block::Para`] and [`BlockMap::DeepTail`] covering `src[range]`,
/// stripping each line's outer container indentation (`' '`, `'\t'`, `'>'`) while mapping every
/// kept byte 1-to-1 back to `src` so Find, highlights, selection and copy preserve all lines.
pub(in crate::markdown) fn build_deep_tail(src: &str, range: Range<usize>) -> (Block, BlockMap) {
    let bytes = src.as_bytes();
    let mut pos = range.start;
    let end = range.end.min(bytes.len());
    let mut out = String::new();
    let mut map = SpanMap::default();
    let mut lines = 0usize;

    while pos < end {
        let nl = bytes[pos..end].iter().position(|&b| b == b'\n').map_or(end, |d| pos + d);
        let line_end = if nl > pos && bytes[nl - 1] == b'\r' { nl - 1 } else { nl };
        let mut k = pos;
        while k < line_end {
            match bytes[k] {
                b' ' | b'\t' => k += 1,
                b'>' => {
                    k += 1;
                    if k < line_end && bytes[k] == b' ' {
                        k += 1;
                    }
                }
                _ => break,
            }
        }
        if k < line_end {
            lines += 1;
            if !out.is_empty() {
                let nl_out = out.len();
                out.push('\n');
                if pos > range.start && bytes[pos - 1] == b'\n' {
                    map.push(pos - 1..pos, nl_out..nl_out + 1);
                }
            }
            let o0 = out.len();
            out.push_str(&src[k..line_end]);
            map.push(k..line_end, o0..out.len());
        }
        pos = if nl < end { nl + 1 } else { end };
    }
    (Block::Para(Inline { text: out, spans: Vec::new(), links: Vec::new() }), BlockMap::DeepTail { lines: lines.max(1), map })
}

pub(in crate::markdown) fn line_indent_cols(line: &[u8]) -> (usize, bool, bool) {
    let mut i = 0usize;
    let mut cols = 0usize;
    while i < line.len() {
        match line[i] {
            b' ' => { cols += 1; i += 1; }
            b'\t' => { cols += 4; i += 1; }
            b'>' => {
                cols += 2;
                i += 1;
                if i < line.len() && line[i] == b' ' { i += 1; }
            }
            b'\r' => i += 1,
            _ => break,
        }
    }
    let rest = &line[i..];
    let fence = rest.starts_with(b"```") || rest.starts_with(b"~~~");
    (cols, i == line.len(), fence)
}

pub(super) fn open(b: &mut Builder, style: Style, url: Option<String>) {
    let inl = b.inline();
    let start = inl.1.text.len();
    inl.2.push(Open { start, style, url });
}

pub(super) fn close(b: &mut Builder) {
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
