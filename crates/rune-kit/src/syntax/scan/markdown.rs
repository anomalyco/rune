//! Markdown: what a reader scans for (headings, list and quote markers, table pipes, fences,
//! code spans, links, escapes, comments and tags), the prose itself left plain. A fence's code
//! is coloured as its info word names; emphasis keeps the prose colour.

use super::markup::tag;
use super::*;

pub(super) fn markdown(s: &[u8], mut i: usize, end: usize, out: &mut Out, depth: u8) {
    use Syntax::*;
    while i < end && !out.full {
        let le = line_end(s, i, end);
        let text_end = if le > i && s[le - 1] == b'\r' { le - 1 } else { le };
        let indent = s[i..text_end].iter().take_while(|&&c| c == b' ').count();
        let j = i + indent;
        if indent < 4 && let Some((fence, n)) = fence_open(s, j, text_end) {
            out.push(j, text_end, Punctuation);
            let lang = s[j + n..text_end].split(|c| c.is_ascii_whitespace()).find(|w| !w.is_empty());
            let lang = lang.and_then(|w| std::str::from_utf8(w).ok()).and_then(Lang::from_name);
            let body = (le + 1).min(end);
            // The closing fence: the same character at least as many times, alone on its line.
            let mut k = body;
            let close = loop {
                if k >= end {
                    break None;
                }
                let ke = line_end(s, k, end);
                let te = if ke > k && s[ke - 1] == b'\r' { ke - 1 } else { ke };
                let c = k + s[k..te].iter().take_while(|&&c| c == b' ').count();
                let run = s[c..te].iter().take_while(|&&b| b == fence).count();
                if c - k < 4 && run >= n && s[c + run..te].iter().all(u8::is_ascii_whitespace) {
                    break Some((k, c, te, ke));
                }
                k = ke + 1;
            };
            let stop = close.map_or(end, |(k, ..)| k);
            if let Some(lang) = lang
                && depth < MAX_DEPTH
                && stop > body
            {
                scan(lang, s, body, stop, out, depth + 1);
            }
            match close {
                Some((_, c, te, ke)) => {
                    out.push(c, te, Punctuation);
                    i = ke + 1;
                }
                None => i = end,
            }
            continue;
        }
        if indent < 4 && heading(s, j, text_end) {
            out.push(j, text_end, Heading);
            i = le + 1;
            continue;
        }
        let mut k = j;
        // Quote markers, then a list marker, each with the space after it.
        while indent < 4 && at(s, k, text_end) == b'>' {
            let e = k + 1 + usize::from(at(s, k + 1, text_end) == b' ');
            out.push(k, e, Punctuation);
            k = e;
            while at(s, k, text_end) == b' ' {
                k += 1;
            }
        }
        if let Some(e) = list_marker(s, k, text_end) {
            out.push(k, e, Punctuation);
            k = e;
        }
        let table = at(s, k, text_end) == b'|';
        k = inline(s, k, text_end, end, table, out);
        i = k.max(le) + 1;
    }
}

/// The fence character and how many open a fence at `j`: three or more backticks (with no
/// backtick in the info string) or tildes.
fn fence_open(s: &[u8], j: usize, end: usize) -> Option<(u8, usize)> {
    let c = at(s, j, end);
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = s[j..end].iter().take_while(|&&b| b == c).count();
    (n >= 3 && (c == b'~' || !s[j + n..end].contains(&b'`'))).then_some((c, n))
}

/// Whether an ATX heading starts at `j`: one to six `#`, then a space or the line's end.
fn heading(s: &[u8], j: usize, end: usize) -> bool {
    let n = s[j..end].iter().take_while(|&&b| b == b'#').count();
    (1..=6).contains(&n) && matches!(at(s, j + n, end), b' ' | b'\t' | 0)
}

/// The end of a list marker at `k` and the space after it: `-`, `*` or `+`, or up to nine
/// digits and `.` or `)`.
fn list_marker(s: &[u8], k: usize, end: usize) -> Option<usize> {
    let c = at(s, k, end);
    let after = if matches!(c, b'-' | b'*' | b'+') {
        k + 1
    } else {
        let n = s[k..end].iter().take(10).take_while(|b| b.is_ascii_digit()).count();
        if n == 0 || n > 9 || !matches!(at(s, k + n, end), b'.' | b')') {
            return None;
        }
        k + n + 1
    };
    matches!(at(s, after, end), b' ' | b'\t').then_some(after + 1)
}

/// Code spans, links, autolinks, tags, comments, escapes and, in a table row, its pipes, from
/// `i` to the line's end `le`. A comment may run on past it, to `end`; returns where scanning
/// stopped.
fn inline(s: &[u8], mut i: usize, le: usize, end: usize, table: bool, out: &mut Out) -> usize {
    use Syntax::*;
    while i < le && !out.full {
        let b = s[i];
        if b == b'\\' && at(s, i + 1, le).is_ascii_punctuation() {
            out.push(i, i + 2, Number);
            i += 2;
        } else if b == b'`' {
            let n = s[i..le].iter().take_while(|&&c| c == b'`').count();
            // The next run of exactly as many backticks closes the span.
            let mut k = i + n;
            let close = loop {
                match s[k..le].iter().position(|&c| c == b'`') {
                    None => break None,
                    Some(p) => {
                        let run = s[k + p..le].iter().take_while(|&&c| c == b'`').count();
                        if run == n {
                            break Some(k + p + run);
                        }
                        k += p + run;
                    }
                }
            };
            match close {
                Some(e) => {
                    out.push(i, e, String);
                    i = e;
                }
                None => i += n,
            }
        } else if b == b'[' && let Some(e) = link(s, i, le, out) {
            i = e;
        } else if starts(s, i, le, b"<!--") {
            let e = find(s, i + 4, end, b"-->").map_or(end, |k| k + 3);
            out.push(i, e, Comment);
            if e > le {
                // The rest of the line the comment ends on stays plain.
                return line_end(s, e, end);
            }
            i = e;
        } else if b == b'<' && let Some(e) = autolink(s, i, le) {
            out.push(i, i + 1, Punctuation);
            out.push(i + 1, e - 1, Keyword);
            out.push(e - 1, e, Punctuation);
            i = e;
        } else if b == b'<' && (is_ident_start(at(s, i + 1, le)) || (at(s, i + 1, le) == b'/' && is_ident_start(at(s, i + 2, le)))) {
            i = tag(s, i, le, out).0;
        } else if b == b'|' && table {
            out.push(i, i + 1, Punctuation);
            i += 1;
        } else {
            i += 1;
        }
    }
    i
}

/// `[text](destination)` at `i`: the brackets and parentheses as punctuation, the text as a
/// link and the destination (with any title) as a keyword. Its end, or `None` when no inline
/// link starts here.
fn link(s: &[u8], i: usize, le: usize, out: &mut Out) -> Option<usize> {
    use Syntax::*;
    let mut depth = 0usize;
    let mut k = i;
    let close = loop {
        match at(s, k, le) {
            0 => return None,
            b'\\' => k += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break k;
                }
            }
            _ => {}
        }
        k += 1;
    };
    if at(s, close + 1, le) != b'(' {
        return None;
    }
    let mut parens = 1usize;
    let mut e = close + 2;
    while e < le {
        match s[e] {
            b'\\' => e += 1,
            b'(' => parens += 1,
            b')' => {
                parens -= 1;
                if parens == 0 {
                    break;
                }
            }
            _ => {}
        }
        e += 1;
    }
    if e >= le {
        return None;
    }
    out.push(i, i + 1, Punctuation);
    out.push(i + 1, close, Link);
    out.push(close, close + 2, Punctuation);
    out.push(close + 2, e, Keyword);
    out.push(e, e + 1, Punctuation);
    Some(e + 1)
}

/// The end of an autolink at `i` (`<https://…>`, `<mailto:…>`): a scheme of letters, digits,
/// `+`, `-` or `.`, a colon, then anything but spaces and `<` up to `>`.
fn autolink(s: &[u8], i: usize, le: usize) -> Option<usize> {
    let scheme = s[i + 1..le].iter().take(33).take_while(|&&c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.')).count();
    if !(2..=32).contains(&scheme) || !at(s, i + 1, le).is_ascii_alphabetic() || at(s, i + 1 + scheme, le) != b':' {
        return None;
    }
    let rest = i + 2 + scheme;
    let p = s[rest..le].iter().position(|&c| matches!(c, b'>' | b' ' | b'<'))?;
    (s[rest + p] == b'>').then_some(rest + p + 1)
}
