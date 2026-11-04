//! Numbered lines that carry on a list under a heading line. CommonMark lets only a list that
//! starts at 1 interrupt a paragraph, so `**More juice**\n4. Hash ripple.` is one paragraph with
//! "4." mid-line. Models write lists that way, numbering on from an earlier list, so here a line
//! inside a paragraph that opens with `N. ` ends the paragraph and starts (or continues) a list.
//! Source ranges are kept as written.

use std::collections::VecDeque;
use std::iter::Peekable;
use std::ops::Range;

use pulldown_cmark::{Event, Tag, TagEnd};

type Ev<'a> = (Event<'a>, Range<usize>);

pub(super) struct ListInterrupts<'a, I: Iterator<Item = Ev<'a>>> {
    src: &'a str,
    inner: Peekable<I>,
    queue: VecDeque<Ev<'a>>,
    para: bool,
    /// Emphasis, links and images open in the paragraph; a list can't start inside one.
    depth: usize,
    /// This paragraph has turned into a list.
    list: bool,
    /// The last event came from here, not from the source as CommonMark reads it.
    made: bool,
}

pub(super) fn list_interrupts<'a, I: Iterator<Item = Ev<'a>>>(src: &'a str, inner: I) -> ListInterrupts<'a, I> {
    ListInterrupts { src, inner: inner.peekable(), queue: VecDeque::new(), para: false, depth: 0, list: false, made: false }
}

impl<'a, I: Iterator<Item = Ev<'a>>> Iterator for ListInterrupts<'a, I> {
    type Item = Ev<'a>;

    fn next(&mut self) -> Option<Ev<'a>> {
        self.made = true;
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        let (ev, range) = self.inner.next()?;
        match &ev {
            Event::Start(Tag::Paragraph) => {
                self.para = true;
                self.depth = 0;
                self.list = false;
            }
            Event::End(TagEnd::Paragraph) => {
                self.para = false;
                if std::mem::take(&mut self.list) {
                    self.queue.push_back((Event::End(TagEnd::List(true)), range.clone()));
                    return Some((Event::End(TagEnd::Item), range));
                }
            }
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }) => self.depth += 1,
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image) => {
                self.depth = self.depth.saturating_sub(1)
            }
            Event::SoftBreak | Event::HardBreak if self.para && self.depth == 0 => {
                let Some((Event::Text(t), text)) = self.inner.peek() else {
                    self.made = false;
                    return Some((ev, range));
                };
                let Some((n, len)) = marker(self.src, text, t) else {
                    self.made = false;
                    return Some((ev, range));
                };
                let (rest, text) = (t[len..].to_owned(), text.clone());
                self.inner.next();
                let line = text.start..text.end;
                if self.list {
                    self.queue.push_back((Event::End(TagEnd::Item), range.clone()));
                } else {
                    self.queue.push_back((Event::End(TagEnd::Paragraph), range.clone()));
                    self.queue.push_back((Event::Start(Tag::List(Some(n))), line.clone()));
                }
                self.queue.push_back((Event::Start(Tag::Item), line));
                if !rest.is_empty() {
                    self.queue.push_back((Event::Text(rest.into()), text.start + len..text.end));
                }
                self.list = true;
                return self.queue.pop_front();
            }
            _ => {}
        }
        self.made = false;
        Some((ev, range))
    }
}

impl<'a, I: Iterator<Item = Ev<'a>>> ListInterrupts<'a, I> {
    /// Whether the last event was made here. A list made here is no place to restart a parse
    /// from, since parsed from its own line it would read differently.
    pub(super) fn made(&self) -> bool {
        self.made
    }
}

/// The number and marker length (`4. `) a line's first text opens with, when that text starts
/// its line (after any indent or quote marks) and something follows the marker.
fn marker(src: &str, text: &Range<usize>, t: &str) -> Option<(u64, usize)> {
    let line_start = src[..text.start].rfind('\n').map_or(0, |i| i + 1);
    if !src[line_start..text.start].chars().all(|c| matches!(c, ' ' | '\t' | '>')) {
        return None;
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=9).contains(&digits) || !t[digits..].starts_with(". ") && !t[digits..].starts_with(".\t") {
        return None;
    }
    let len = digits + 2;
    let line_end = src[text.start..].find('\n').map_or(src.len(), |i| text.start + i);
    if src[text.start + len..line_end].trim().is_empty() {
        return None;
    }
    Some((t[..digits].parse().ok()?, len))
}

#[cfg(test)]
#[path = "interrupt_tests.rs"]
mod tests;
