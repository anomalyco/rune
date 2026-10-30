//! Markup (HTML and XML) and CSS.

use super::*;

pub(super) fn markup(s: &[u8], mut i: usize, end: usize, out: &mut Out, depth: u8) {
    while i < end && !out.full {
        let b = s[i];
        if starts(s, i, end, b"<!--") {
            let e = find(s, i + 4, end, b"-->").map_or(end, |k| k + 3);
            out.push(i, e, Syntax::Comment);
            i = e;
        } else if starts(s, i, end, b"<![CDATA[") {
            let e = find(s, i + 9, end, b"]]>").map_or(end, |k| k + 3);
            out.push(i, e, Syntax::String);
            i = e;
        } else if b == b'<' && (is_ident_start(at(s, i + 1, end)) || matches!(at(s, i + 1, end), b'/' | b'!' | b'?')) {
            let (e, name, closing, self_closing) = tag(s, i, end, out);
            i = e;
            let name = &s[name.0..name.1];
            let embedded = if name.eq_ignore_ascii_case(b"script") {
                Some((Lang::Js, b"</script".as_slice()))
            } else if name.eq_ignore_ascii_case(b"style") {
                Some((Lang::Css, b"</style".as_slice()))
            } else {
                None
            };
            if let (Some((lang, close)), false, false) = (embedded, closing, self_closing) {
                let stop = find(s, i, end, close).unwrap_or(end);
                if depth < MAX_DEPTH {
                    scan(lang, s, i, stop, out, depth + 1);
                }
                i = stop;
            }
        } else if b == b'&' {
            let limit = end.min(i + 32);
            let e = (i + 1..limit).find(|&k| !(s[k].is_ascii_alphanumeric() || s[k] == b'#'));
            match e {
                Some(e) if e > i + 1 && s[e] == b';' => {
                    out.push(i, e + 1, Syntax::Number);
                    i = e + 1;
                }
                _ => i += 1,
            }
        } else {
            i += 1;
        }
    }
}

/// A tag from the `<` at `i`: (its end, its name's range, whether it closes, whether it is
/// self-closing or a declaration).
fn tag(s: &[u8], i: usize, end: usize, out: &mut Out) -> (usize, (usize, usize), bool, bool) {
    use Syntax::*;
    let mut j = i + 1;
    let closing = at(s, j, end) == b'/';
    let decl = matches!(at(s, j, end), b'!' | b'?');
    if closing || decl {
        j += 1;
    }
    out.push(i, j, Punctuation);
    let name = (j, ident_end(s, j, end, b"-:."));
    out.push(name.0, name.1, Keyword);
    j = name.1;
    while j < end && !out.full {
        match s[j] {
            b'>' => {
                out.push(j, j + 1, Punctuation);
                return (j + 1, name, closing, decl);
            }
            b'/' | b'?' if at(s, j + 1, end) == b'>' => {
                out.push(j, j + 2, Punctuation);
                return (j + 2, name, closing, true);
            }
            b'<' => return (j, name, closing, true),
            b'"' | b'\'' => {
                let e = string_end(s, j, end, s[j], true);
                out.push(j, e, String);
                j = e;
            }
            b'=' => {
                out.push(j, j + 1, Punctuation);
                j += 1;
                let v = j;
                while j < end && !matches!(s[j], b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'"' | b'\'' | b'<') {
                    j += 1;
                }
                out.push(v, j, String);
            }
            c if is_ident_start(c) => {
                let e = ident_end(s, j, end, b"-:.");
                out.push(j, e, Property);
                j = e;
            }
            _ => j += 1,
        }
    }
    (end, name, closing, true)
}

// CSS

pub(super) fn css(s: &[u8], mut i: usize, end: usize, out: &mut Out) {
    use Syntax::*;
    let mut blocks = 0usize;
    // At a statement's start, before its first token; inside a declaration; after its `:`.
    let (mut fresh, mut decl, mut value) = (true, false, false);
    while i < end && !out.full {
        let b = s[i];
        let start = i;
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'/' && at(s, i + 1, end) == b'*' {
            i = block_comment_end(s, i + 2, end, false);
            out.push(start, i, Comment);
            continue;
        }
        if matches!(b, b'{' | b'}' | b';') {
            match b {
                b'{' => blocks += 1,
                b'}' => blocks = blocks.saturating_sub(1),
                _ => {}
            }
            i += 1;
            out.push(start, i, Punctuation);
            (fresh, decl, value) = (true, false, false);
            continue;
        }
        if fresh {
            fresh = false;
            // Inside braces, a statement that reaches `;` or `}` (or the end) before `{` is a
            // declaration; otherwise it is a nested rule's selector.
            decl = blocks > 0 && s[i..end].iter().find(|&&c| matches!(c, b'{' | b';' | b'}')) != Some(&b'{');
        }
        let ident_start = |k: usize| is_ident_start(at(s, k, end)) || (at(s, k, end) == b'-' && is_ident_start(at(s, k + 1, end)));
        if b == b'"' || b == b'\'' {
            i = string_end(s, i, end, b, false);
            out.push(start, i, String);
        } else if b == b':' && decl && !value {
            i += 1;
            out.push(start, i, Punctuation);
            value = true;
        } else if b == b'@' {
            i = ident_end(s, i + 1, end, b"-");
            out.push(start, i, Keyword);
        } else if b == b'#' && value {
            i = ident_end(s, i + 1, end, b"");
            out.push(start, i, Number);
        } else if (b == b'#' || b == b'.') && !value && ident_start(i + 1) {
            i = ident_end(s, i + 1, end, b"-");
            out.push(start, i, Type);
        } else if b == b':' && !value {
            while at(s, i, end) == b':' {
                i += 1;
            }
            i = ident_end(s, i, end, b"-");
            out.push(start, i, Keyword);
        } else if b == b'!' && value {
            i = ident_end(s, i + 1, end, b"");
            out.push(start, i, Keyword);
        } else if b.is_ascii_digit() || (matches!(b, b'.' | b'-' | b'+') && value && at(s, i + 1, end).is_ascii_digit()) {
            i = number_end(s, i + (b != b'.' && !b.is_ascii_digit()) as usize, end);
            if at(s, i, end) == b'%' {
                i += 1;
            }
            out.push(start, i, Number);
        } else if b == b'-' && at(s, i + 1, end) == b'-' {
            i = ident_end(s, i + 2, end, b"-");
            out.push(start, i, Property);
        } else if ident_start(i) {
            i = ident_end(s, i + 1, end, b"-");
            if decl && !value {
                out.push(start, i, Property);
            } else if at(s, i, end) == b'(' {
                out.push(start, i, Function);
            }
        } else {
            i += 1;
            out.push(start, i, Punctuation);
        }
    }
}
