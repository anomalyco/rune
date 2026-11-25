//! Markdown source to blocks: the pulldown-cmark event walk, code spans, GFM tables, and the
//! maps from source bytes to rendered text.

use super::*;

mod builder;
mod map;
#[cfg(feature = "math")]
mod math;
mod table;

use builder::{
    Builder, Container, Fence, InlineKind, ListSlot, MAX_ELEMENT_DEPTH, MAX_FLATTENED_TAIL_ITEMS, MAX_TABLE_COLUMNS, QuoteSlot, close, open,
};
use map::{code_span_content_range, map_code_span};
use table::pending_table;

pub(super) use builder::{build_deep_tail, line_indent_cols};
pub(super) use map::{BlockMap, SpanMap};

/// What the source ends inside, from the last block-level start the walk met.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Tail {
    None,
    /// A code or HTML block: its text is literal.
    Code,
    /// Inline text from this source byte on (a paragraph, a tight list item, a heading or a
    /// table cell).
    Inline(usize, Scope),
}

/// How far an inline run can reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Scope {
    /// Across lines, until a blank one.
    Para,
    /// To the end of its line (a heading).
    Line,
    /// To the next `|` or the end of its line.
    Cell,
}

pub(super) type ParsedDoc = (Vec<Block>, Vec<Source>, Vec<Option<(bool, usize)>>, Vec<Cut>, Vec<usize>, Vec<BlockMap>);

/// Where a top-level block starts: the start of its first line, and how many top-level blocks,
/// [`Source`]s and list items come before it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cut {
    pub(super) at: usize,
    pub(super) blocks: usize,
    pub(super) sources: usize,
    pub(super) tasks: usize,
}

/// What colours a [`Block::Code`] and which lines of a [`Block::Mermaid`] draw, one per block in
/// document order (the order the renderer meets them). Kept beside the blocks so `Block` keeps
/// its shape.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Source {
    /// The fence's language as written (its info string's first word), if it names one.
    pub(super) lang: Option<Rc<str>>,
    /// The code's length, to check this belongs to the block it is matched with.
    pub(super) len: usize,
    /// How many leading bytes get colour: all of them, or while an unclosed fence streams in,
    /// the complete lines (the line still arriving draws plain until its newline). A diagram
    /// draws from these lines alone.
    pub(super) coloured: usize,
    /// The fence's closing line hasn't arrived.
    pub(super) open: bool,
}

pub(super) const PAD: &str = "\u{202f}\u{202f}";
/// Inline code's padding before it, ending in a left-to-right isolate (LRI), and after it,
/// starting with its end (PDI): `markdown.css` sets `:not(pre) > code { direction: ltr;
/// unicode-bidi: isolate }`, so code reads left to right and moves as one inside Arabic text.
pub(super) const OPEN: &str = "\u{202f}\u{202f}\u{2066}";
pub(super) const CLOSE: &str = "\u{2069}\u{202f}\u{202f}";
/// Inside a link, inline code keeps its directional isolate without pill padding (`a code { padding: 0 }`).
pub(super) const LINK_OPEN: &str = "\u{2066}";
pub(super) const LINK_CLOSE: &str = "\u{2069}";

/// `text` without inline code's padding and isolates.
pub(super) fn unpadded(text: &str) -> String {
    text.replace(OPEN, "").replace(CLOSE, "").replace(LINK_OPEN, "").replace(LINK_CLOSE, "")
}

/// Parses `src` into blocks. Unclosed constructs are closed at the end of input.
pub fn parse(src: &str) -> Vec<Block> {
    parse_doc(src, true).0
}

pub(super) fn parse_doc(src: &str, autolink: bool) -> ParsedDoc {
    parse_doc_with(src, autolink, false)
}

pub(super) fn parse_doc_with(src: &str, autolink: bool, hard_breaks: bool) -> ParsedDoc {
    parse_doc_tail(src, autolink, hard_breaks).0
}

enum SkipMode {
    SiblingLists { start: usize, open: usize },
    ExtraQuotes { start: usize, open: usize },
}

/// [`parse_doc_with`], and what the source ends inside.
pub(super) fn parse_doc_tail(src: &str, autolink: bool, hard_breaks: bool) -> (ParsedDoc, Tail) {
    let mut b = Builder::new(autolink);
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let mut events = super::interrupt::list_interrupts(src, Parser::new_ext(src, opts).into_offset_iter());
    let mut skip: Option<SkipMode> = None;
    #[cfg(feature = "math")]
    let mut math = math::Math::find(src, opts);
    while let Some((ev, range)) = events.next() {
        if let Some(mode) = &mut skip {
            match (mode, &ev) {
                (SkipMode::SiblingLists { open, .. }, Event::Start(Tag::List(_))) => *open += 1,
                (SkipMode::SiblingLists { start, open }, Event::End(TagEnd::List(_))) => {
                    *open -= 1;
                    if *open == 0 {
                        let (cs, ce) = (*start, range.end);
                        skip = None;
                        b.past_bound_items = 0;
                        b.tail = Tail::None;
                        b.emit_deep_tail(src, cs..ce);
                    }
                }
                (SkipMode::ExtraQuotes { open, .. }, Event::Start(Tag::BlockQuote(_))) => *open += 1,
                (SkipMode::ExtraQuotes { start, open }, Event::End(TagEnd::BlockQuote(_))) => {
                    *open -= 1;
                    if *open == 0 {
                        let (cs, ce) = (*start, range.end);
                        skip = None;
                        b.past_bound_items = 0;
                        b.tail = Tail::None;
                        b.emit_deep_tail(src, cs..ce);
                    }
                }
                _ => {}
            }
            continue;
        }
        if b.skip_cell {
            match ev {
                Event::End(TagEnd::TableCell) => {
                    b.skip_cell = false;
                    continue;
                }
                Event::End(TagEnd::TableHead | TagEnd::TableRow | TagEnd::Table) => {
                    b.skip_cell = false;
                }
                _ => continue,
            }
        }
        let starts_block = matches!(
            ev,
            Event::Start(
                Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::BlockQuote(_) | Tag::List(_) | Tag::Table(_) | Tag::HtmlBlock
            ) | Event::Rule
        );
        if starts_block && b.stack.len() == 1 && b.code.is_none() {
            b.past_bound_items = 0;
            b.flush_inline();
            let at = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
            b.root_start = at;
            if !events.made() {
                b.cuts.push(Cut { at, blocks: b.stack[0].1.len(), sources: b.sources.len(), tasks: b.tasks.len() });
            }
        }
        if matches!(
            &ev,
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::Table(_))
        ) && b.code.is_none()
            && let Some((q_idx, stack_len, list_slots_len)) = b.first_merged_quote()
        {
            let in_fresh_item = matches!(b.stack.last(), Some((Container::Item(_), blocks, _)) if blocks.is_empty())
                && b.inline.is_none();
            if !in_fresh_item {
                b.past_bound_items += 1;
                if b.past_bound_items > MAX_FLATTENED_TAIL_ITEMS {
                    let open = b.unwind_merged_quote(q_idx, stack_len, list_slots_len);
                    let cs = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
                    skip = Some(SkipMode::ExtraQuotes { start: cs, open });
                    b.tail = Tail::None;
                    continue;
                }
            }
        }
        b.tail = match &ev {
            Event::Start(Tag::Paragraph | Tag::Item) => Tail::Inline(range.start, Scope::Para),
            Event::Start(Tag::Heading { .. }) => Tail::Inline(range.start, Scope::Line),
            // A row short of cells is padded with empty ones after its last.
            Event::Start(Tag::TableCell)
                if !range.is_empty() && b.table().is_none_or(|(_, cells, ..)| cells.len() < MAX_TABLE_COLUMNS) =>
            {
                Tail::Inline(range.start, Scope::Cell)
            }
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => Tail::Code,
            Event::Start(Tag::Table(_)) | Event::Rule => Tail::None,
            _ => b.tail,
        };
        #[cfg(feature = "math")]
        if b.code.is_none() && math.claim(&mut b, src, &ev, &range) {
            continue;
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
                                "diff" | "patch" => Fence::Diff(fence_closed(&src[range])),
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
                    if b.depth() + 2 <= MAX_ELEMENT_DEPTH && b.layout_fits(2) {
                        b.quote_slots.push(QuoteSlot::Nested);
                        b.stack.push((Container::Quote, Vec::new(), Vec::new()));
                    } else {
                        b.quote_slots.push(QuoteSlot::Merged { stack_len: b.stack.len(), list_slots_len: b.list_slots.len() });
                    }
                }
                Tag::List(start) => {
                    b.flush_inline();
                    if (b.depth() + 2 <= MAX_ELEMENT_DEPTH && b.layout_fits(3))
                        || !matches!(b.stack.last(), Some((Container::List(..) | Container::Item(_), _, _)))
                    {
                        b.list_slots.push(ListSlot::Nested);
                        b.stack.push((Container::List(start, Vec::new(), Vec::new()), Vec::new(), Vec::new()));
                    } else {
                        b.list_slots.push(ListSlot::SiblingItems);
                    }
                }
                Tag::Item => {
                    b.flush_inline();
                    if matches!(b.stack.last(), Some((Container::Item(_), _, _))) {
                        b.pop_container();
                    }
                    let merged_quote = b.first_merged_quote();
                    if merged_quote.is_some() || b.list_slots.contains(&ListSlot::SiblingItems) {
                        b.past_bound_items += 1;
                        if b.past_bound_items > MAX_FLATTENED_TAIL_ITEMS {
                            let cs = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
                            skip = Some(if let Some((q_idx, stack_len, list_slots_len)) = merged_quote {
                                let open = b.unwind_merged_quote(q_idx, stack_len, list_slots_len);
                                SkipMode::ExtraQuotes { start: cs, open }
                            } else {
                                let first_sib = b.list_slots.iter().position(|s| *s == ListSlot::SiblingItems).unwrap();
                                let open = b.list_slots.len() - first_sib;
                                b.list_slots.truncate(first_sib);
                                SkipMode::SiblingLists { start: cs, open }
                            });
                            b.tail = Tail::None;
                            continue;
                        }
                    }
                    let idx = b.tasks.len();
                    b.tasks.push(None);
                    b.stack.push((Container::Item(idx), Vec::new(), Vec::new()));
                }
                Tag::Table(aligns) => {
                    b.flush_inline();
                    let align = aligns
                        .iter()
                        .take(MAX_TABLE_COLUMNS)
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
                    if b.table().is_some_and(|(_, cells, ..)| cells.len() >= MAX_TABLE_COLUMNS) {
                        b.skip_cell = true;
                    } else {
                        b.inline = Some((InlineKind::Cell, Inline::default(), Vec::new(), SpanMap::default()));
                    }
                }
                Tag::Strong => open(&mut b, Style::Strong, None),
                Tag::Emphasis => open(&mut b, Style::Em, None),
                Tag::Strikethrough => open(&mut b, Style::Strike, None),
                Tag::Link { dest_url, .. } => open(&mut b, Style::Link, Some(dest_url.into_string())),
                Tag::Image { dest_url, title, .. } => {
                    let start = b.inline().1.text.len();
                    b.image_starts.push(start);
                    b.image_dests.push((dest_url.into_string(), title.into_string()));
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
                TagEnd::BlockQuote(_) => {
                    if matches!(b.quote_slots.pop(), Some(QuoteSlot::Nested)) {
                        b.pop_container();
                    } else {
                        b.flush_inline();
                        if b.first_merged_quote().is_none() && !b.list_slots.contains(&ListSlot::SiblingItems) {
                            b.past_bound_items = 0;
                        }
                    }
                }
                TagEnd::Item => {
                    b.flush_inline();
                    if matches!(b.stack.last(), Some((Container::Item(_), _, _))) {
                        b.pop_container();
                    }
                }
                TagEnd::List(_) => match b.list_slots.pop().unwrap_or(ListSlot::Nested) {
                    ListSlot::Nested => {
                        if matches!(b.stack.last(), Some((Container::Item(_), _, _))) {
                            b.pop_container();
                        }
                        b.pop_container();
                    }
                    ListSlot::SiblingItems => {
                        b.flush_inline();
                        if !b.list_slots.contains(&ListSlot::SiblingItems) && b.first_merged_quote().is_none() {
                            b.past_bound_items = 0;
                        }
                    }
                },
                TagEnd::Table => b.pop_container(),
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => close(&mut b),
                TagEnd::Image => {
                    if let Some(start) = b.image_starts.pop() {
                        let end = b.inline().1.text.len();
                        let (src, title) = b.image_dests.pop().unwrap_or_default();
                        if !b.lift_image(start, end, src, title) {
                            b.image_ranges.push((start, end));
                        }
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
                let (open, close) = if b.in_link() { (LINK_OPEN, LINK_CLOSE) } else { (OPEN, CLOSE) };
                let inl = b.inline();
                let a = inl.1.text.len();
                inl.1.text.push_str(open);
                let start = inl.1.text.len();
                inl.1.text.push_str(&t);
                let end = inl.1.text.len();
                inl.1.text.push_str(close);
                inl.1.spans.push((a, start, Style::CodePad));
                inl.1.spans.push((start, end, Style::Code));
                inl.1.spans.push((end, inl.1.text.len(), Style::CodePad));
                map_code_span(src, code_src, &t, start, &mut inl.3);
            }
            Event::SoftBreak if !hard_breaks => {
                let inl = b.inline();
                // The line after an image starts the paragraph that follows it.
                if inl.1.text.is_empty() {
                    continue;
                }
                let start = inl.1.text.len();
                inl.1.text.push(' ');
                inl.3.push(range, start..inl.1.text.len());
            }
            Event::HardBreak | Event::SoftBreak => {
                let inl = b.inline();
                // The line after an image starts the paragraph that follows it.
                if matches!(ev, Event::SoftBreak) && inl.1.text.is_empty() {
                    continue;
                }
                let start = inl.1.text.len();
                inl.1.text.push('\n');
                inl.3.push(range, start..inl.1.text.len());
            }
            Event::TaskListMarker(checked) => {
                if let Some((Container::Item(idx), _, _)) = b.stack.last() {
                    b.tasks[*idx] = Some((checked, range.start));
                }
            }
            Event::Rule => {
                b.flush_inline();
                b.push(Block::Rule, BlockMap::None);
            }
            _ => {}
        }
    }
    if let Some(SkipMode::SiblingLists { start, .. } | SkipMode::ExtraQuotes { start, .. }) = skip.take() {
        b.emit_deep_tail(src, start..src.len());
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
        && let Some((t, map)) = pending_table(&src[range.start..], range.start, autolink, hard_breaks)
    {
        *blocks.last_mut().unwrap() = Block::Table(t);
        *maps.last_mut().unwrap() = map;
    }
    ((blocks, b.sources, b.tasks, b.cuts, b.block_starts, maps), b.tail)
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
