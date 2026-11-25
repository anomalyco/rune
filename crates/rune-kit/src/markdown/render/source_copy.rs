//! Source slicing for [`Markdown::copy_source`](crate::Markdown::copy_source): whole top-level
//! blocks and whole selected list-item subsets inside a single list block (preserving inline
//! Markdown, source markers such as `1.`, `3)`, `-`, `*`, task checkboxes, loose blank lines,
//! and nested lists).
use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

enum SrcNode {
    List(Vec<SrcItem>),
    Quote(Vec<SrcNode>),
    Leaf,
}

struct SrcItem {
    range: Range<usize>,
    children: Vec<SrcNode>,
}

enum Frame {
    Root(Vec<SrcNode>),
    Quote(Vec<SrcNode>),
    List(Vec<SrcItem>),
    Item { start: usize, end: usize, children: Vec<SrcNode>, has_inline: bool },
}

fn flush_item_inline(stack: &mut [Frame]) {
    if let Some(Frame::Item { children, has_inline, .. }) = stack.last_mut()
        && *has_inline
    {
        children.push(SrcNode::Leaf);
        *has_inline = false;
    }
}

fn push_node(stack: &mut [Frame], node: SrcNode) {
    match stack.last_mut() {
        Some(Frame::Root(v) | Frame::Quote(v) | Frame::Item { children: v, .. }) => v.push(node),
        _ => {}
    }
}

/// Parses a single top-level block's Markdown source into its list/quote container tree with
/// exact item byte ranges (using the same `Options` and `list_interrupts` as `parser.rs`).
fn parse_block_tree(src: &str) -> Vec<SrcNode> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let events = super::super::interrupt::list_interrupts(src, Parser::new_ext(src, opts).into_offset_iter());
    let mut stack = vec![Frame::Root(Vec::new())];
    let mut in_leaf: usize = 0;
    for (ev, range) in events {
        match ev {
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::Table(_) | Tag::HtmlBlock) => {
                if in_leaf == 0 {
                    flush_item_inline(&mut stack);
                    push_node(&mut stack, SrcNode::Leaf);
                }
                in_leaf += 1;
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Table | TagEnd::HtmlBlock) => {
                in_leaf = in_leaf.saturating_sub(1);
            }
            Event::Rule if in_leaf == 0 => {
                flush_item_inline(&mut stack);
                push_node(&mut stack, SrcNode::Leaf);
            }
            Event::Start(Tag::BlockQuote(_)) if in_leaf == 0 => {
                flush_item_inline(&mut stack);
                stack.push(Frame::Quote(Vec::new()));
            }
            Event::End(TagEnd::BlockQuote(_)) if in_leaf == 0 => {
                if let Some(Frame::Quote(children)) = stack.pop() {
                    push_node(&mut stack, SrcNode::Quote(children));
                }
            }
            Event::Start(Tag::List(_)) if in_leaf == 0 => {
                flush_item_inline(&mut stack);
                stack.push(Frame::List(Vec::new()));
            }
            Event::End(TagEnd::List(_)) if in_leaf == 0 => {
                if let Some(Frame::List(items)) = stack.pop() {
                    push_node(&mut stack, SrcNode::List(items));
                }
            }
            Event::Start(Tag::Item) if in_leaf == 0 => {
                let start = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
                stack.push(Frame::Item { start, end: range.end, children: Vec::new(), has_inline: false });
            }
            Event::End(TagEnd::Item) if in_leaf == 0 => {
                flush_item_inline(&mut stack);
                if let Some(Frame::Item { start, end, children, .. }) = stack.pop() {
                    let end = end.max(range.end);
                    if let Some(Frame::List(items)) = stack.last_mut() {
                        items.push(SrcItem { range: start..end, children });
                    }
                }
            }
            Event::Text(_) | Event::Code(_) | Event::SoftBreak | Event::HardBreak | Event::Html(_) | Event::InlineHtml(_) if in_leaf == 0 => {
                if let Some(Frame::Item { has_inline, .. }) = stack.last_mut() {
                    *has_inline = true;
                }
            }
            _ => {}
        }
    }
    match stack.into_iter().next() {
        Some(Frame::Root(v)) => v,
        _ => Vec::new(),
    }
}

fn under_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if path == prefix {
        return Some("");
    }
    path.strip_prefix(prefix)?.strip_prefix('/')
}

fn first_segment(rest: &str) -> &str {
    rest.split('/').next().unwrap_or(rest)
}

fn parse_prefixed_index(seg: &str, ch: char) -> Option<usize> {
    seg.strip_prefix(ch)?.parse().ok()
}

fn slice_within_node(block_src: &str, node: &SrcNode, prefix: &str, sel: &rune::TextSelection) -> Option<String> {
    match node {
        SrcNode::Leaf => None,
        SrcNode::Quote(children) => {
            let body_prefix = format!("{prefix}/#1");
            let mut touched: Option<usize> = None;
            for t in sel.texts().iter().filter(|t| t.is_selected()) {
                let rest = under_prefix(t.path(), &body_prefix)?;
                let idx = parse_prefixed_index(first_segment(rest), 'b')?;
                if let Some(prev) = touched && prev != idx {
                    return None;
                }
                touched = Some(idx);
            }
            let idx = touched?;
            let child = children.get(idx)?;
            let child_prefix = format!("{body_prefix}/b{idx}");
            slice_within_node(block_src, child, &child_prefix, sel)
        }
        SrcNode::List(items) => {
            let mut touched: Option<(usize, usize)> = None;
            for t in sel.texts().iter().filter(|t| t.is_selected()) {
                let rest = under_prefix(t.path(), prefix)?;
                let idx = parse_prefixed_index(first_segment(rest), 'i')?;
                touched = Some(touched.map_or((idx, idx), |(a, b)| (a.min(idx), b.max(idx))));
            }
            let (first, last) = touched?;
            if last >= items.len() {
                return None;
            }
            // Are all non-empty selectable texts belonging to touched items `first..=last` whole?
            let all_touched_whole = sel.texts().iter().filter(|t| {
                under_prefix(t.path(), prefix)
                    .and_then(|r| parse_prefixed_index(first_segment(r), 'i'))
                    .is_some_and(|idx| (first..=last).contains(&idx))
            }).all(|t| t.is_whole() || t.is_empty());
            if all_touched_whole {
                let start = items[first].range.start;
                let mut end = items[last].range.end;
                if let Some(next) = items.get(last + 1) {
                    end = end.min(next.range.start);
                }
                return Some(block_src.get(start..end)?.trim_end().to_owned());
            }
            // Partial selection within a single item: descend into a nested list or quote if
            // all selected texts live inside one child block of that item.
            if first == last {
                let body_prefix = format!("{prefix}/i{first}/#1");
                let mut child_idx: Option<usize> = None;
                for t in sel.texts().iter().filter(|t| t.is_selected()) {
                    let rest = under_prefix(t.path(), &body_prefix)?;
                    let idx = parse_prefixed_index(first_segment(rest), 'b')?;
                    if let Some(prev) = child_idx && prev != idx {
                        return None;
                    }
                    child_idx = Some(idx);
                }
                let idx = child_idx?;
                let child = items[first].children.get(idx)?;
                let child_prefix = format!("{body_prefix}/b{idx}");
                return slice_within_node(block_src, child, &child_prefix, sel);
            }
            None
        }
    }
}

/// [`Markdown::copy_source`]: the source of the top-level blocks (keyed `names`, starting at
/// `starts`) that `sel` touches; when `sel` stays inside a single list block and covers one or
/// more whole list items (top-level or nested), slices those items verbatim from `src`; returns
/// `None` for partial intra-item selections so copying falls back to drawn text.
pub(in crate::markdown) fn source_copy(src: &str, starts: &[usize], names: &[String], sel: &rune::TextSelection) -> Option<String> {
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
        let prefix = names.get(first)?;
        // Fast path: only parse block source when selected texts sit under a list item (`i<n>`).
        let in_list = sel.texts().iter().filter(|t| t.is_selected()).any(|t| {
            under_prefix(t.path(), prefix)
                .is_some_and(|r| r.split('/').any(|seg| parse_prefixed_index(seg, 'i').is_some()))
        });
        if !in_list {
            return None;
        }
        let start = *starts.get(first)?;
        let end = starts.get(last + 1).copied().unwrap_or(src.len());
        let block_src = src.get(start..end)?;
        let tree = parse_block_tree(block_src);
        let root = tree.first()?;
        return slice_within_node(block_src, root, prefix, sel);
    }
    if (first == 0 && last + 1 >= starts.len()) || sel.texts().iter().all(|t| t.is_whole() || t.is_empty()) {
        return Some(src.to_owned());
    }
    let start = *starts.get(first)?;
    let end = starts.get(last + 1).copied().unwrap_or(src.len());
    Some(src.get(start..end)?.trim_end().to_owned())
}
