//! Line span layering (syntax colours under word-change highlights) and tab/wrap expansion.

use std::ops::Range;

use rune::{Color, Span};

/// Spans for a line from its syntax colours and the byte ranges given `bg` (word changes):
/// cut where either starts or ends, so each piece has one colour and one background.
pub(super) fn layer(len: usize, syntax: &[(Range<usize>, Color)], words: &[Range<usize>], bg: Option<Color>) -> Vec<Span> {
    let mut cuts: Vec<usize> = syntax.iter().flat_map(|(r, _)| [r.start, r.end]).chain(words.iter().flat_map(|r| [r.start, r.end])).collect();
    cuts.retain(|&c| c <= len);
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<Span> = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let color = syntax.get(syntax.partition_point(|(r, _)| r.end <= a)).filter(|(r, _)| r.start <= a).map(|(_, c)| *c);
        let on = words.get(words.partition_point(|r| r.end <= a)).is_some_and(|r| r.start <= a);
        let bg = bg.filter(|_| on);
        if color.is_none() && bg.is_none() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end as usize == a && last.color == color && last.bg == bg => last.end = b as u32,
            _ => {
                let mut sp = Span::new(a, b);
                sp.color = color;
                sp.bg = bg;
                out.push(sp);
            }
        }
    }
    out
}

/// `code` as drawn: text layout draws a tab as one space, so each tab is followed by spaces up
/// to the next four-column stop; and when `wrap`, a zero-width space after `.`, `(`, `[` and
/// `/` lets a long call chain or path break there. Returns the text, `spans` moved to match,
/// and the inserted ranges (drawn, never copied).
pub(super) fn tabbed(code: &str, mut spans: Vec<Span>, wrap: bool) -> (String, Vec<Span>, Vec<Range<usize>>) {
    if !code.contains('\t') && !(wrap && code.contains(['.', '(', '[', '/'])) {
        return (code.to_owned(), spans, Vec::new());
    }
    let mut shown = String::with_capacity(code.len() + 8);
    // (byte in `code` after the insertion, bytes added up to and including it)
    let mut shifts: Vec<(usize, usize)> = Vec::new();
    let mut pads = Vec::new();
    let mut col = 0;
    let mut chars = code.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        shown.push(ch);
        col += 1;
        let start = shown.len();
        if ch == '\t' {
            let pad = 3 - (col - 1) % 4;
            shown.extend(std::iter::repeat_n(' ', pad));
            col += pad;
        } else if wrap && matches!(ch, '.' | '(' | '[' | '/') && chars.peek().is_some_and(|(_, c)| !c.is_whitespace()) {
            shown.push('\u{200B}');
        }
        if shown.len() > start {
            pads.push(start..shown.len());
            shifts.push((i + ch.len_utf8(), shifts.last().map_or(0, |s| s.1) + shown.len() - start));
        }
    }
    let shift = |b: u32| match shifts.partition_point(|s| s.0 <= b as usize) {
        0 => b,
        n => b + shifts[n - 1].1 as u32,
    };
    for sp in &mut spans {
        (sp.start, sp.end) = (shift(sp.start), shift(sp.end));
    }
    (shown, spans, pads)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn word_backgrounds_cut_syntax_colours() {
        let red = Color::WHITE;
        let green = Color::BLACK;
        // "let total = 1;" with `let` and `1` coloured, `total` changed.
        let spans = layer(14, &[(0..3, red), (12..13, red)], &[4..9], Some(green));
        let got: Vec<(u32, u32, Option<Color>, Option<Color>)> = spans.iter().map(|s| (s.start, s.end, s.color, s.bg)).collect();
        assert_eq!(got, vec![(0, 3, Some(red), None), (4, 9, None, Some(green)), (12, 13, Some(red), None)]);
        // A change across a coloured token splits it.
        let spans = layer(10, &[(0..6, red)], &[3..8], Some(green));
        let got: Vec<(u32, u32, Option<Color>, Option<Color>)> = spans.iter().map(|s| (s.start, s.end, s.color, s.bg)).collect();
        assert_eq!(got, vec![(0, 3, Some(red), None), (3, 6, Some(red), Some(green)), (6, 8, None, Some(green))]);
    }

    #[test]
    fn tabs_pad_to_four_columns_without_copying_the_padding() {
        let (shown, spans, pads) = tabbed("a\tb\t\tc", vec![Span::new(2, 3)], false);
        assert_eq!(shown, "a\t  b\t  \t   c");
        assert_eq!(pads, vec![2..4, 6..8, 9..12]);
        assert_eq!(&shown[spans[0].start as usize..spans[0].end as usize], "b");
        let copied: String = shown.char_indices().filter(|(i, _)| !pads.iter().any(|r| r.contains(i))).map(|(_, c)| c).collect();
        assert_eq!(copied, "a\tb\t\tc");
    }

    #[test]
    fn wrapped_lines_can_break_inside_call_chains() {
        let (shown, spans, pads) = tabbed("a.b(c) x/ y", vec![Span::new(2, 3)], true);
        assert_eq!(shown, "a.\u{200B}b(\u{200B}c) x/ y");
        assert_eq!(&shown[spans[0].start as usize..spans[0].end as usize], "b");
        let copied: String = shown.char_indices().filter(|(i, _)| !pads.iter().any(|r| r.contains(i))).map(|(_, c)| c).collect();
        assert_eq!(copied, "a.b(c) x/ y");
    }
}
