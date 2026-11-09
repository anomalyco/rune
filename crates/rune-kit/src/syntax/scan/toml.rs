//! TOML.

use super::*;

pub(super) fn toml(s: &[u8], mut i: usize, end: usize, out: &mut Out) {
    use Syntax::*;
    // Open arrays (`false`) and inline tables (`true`), and whether a key comes next.
    let mut nest: Vec<bool> = Vec::new();
    let mut key = true;
    while i < end && !out.full {
        let b = s[i];
        let start = i;
        if b == b'\n' {
            i += 1;
            key |= nest.is_empty();
            continue;
        }
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'#' {
            i = line_end(s, i, end);
            let e = if i > start && s[i - 1] == b'\r' { i - 1 } else { i };
            out.push(start, e, Comment);
            continue;
        }
        if b == b'[' && key && nest.is_empty() {
            // A table header: `[a.b]` or `[[a.b]]`.
            let open = if at(s, i + 1, end) == b'[' { 2 } else { 1 };
            out.push(i, i + open, Punctuation);
            i += open;
            let le = line_end(s, i, end);
            while i < le && !out.full {
                let c = s[i];
                let at_ = i;
                if c == b']' {
                    let close = if at(s, i + 1, le) == b']' { 2 } else { 1 };
                    out.push(i, i + close, Punctuation);
                    i += close;
                    break;
                } else if c == b'"' || c == b'\'' {
                    i = string_end(s, i, le, c, false);
                    out.push(at_, i, Property);
                } else if c == b'.' {
                    out.push(i, i + 1, Punctuation);
                    i += 1;
                } else if c.is_ascii_whitespace() {
                    i += 1;
                } else {
                    while i < le && !matches!(s[i], b']' | b'.' | b' ' | b'\t' | b'"' | b'\'' | b'\r') {
                        i += 1;
                    }
                    out.push(at_, i, Property);
                }
            }
            continue;
        }
        match b {
            b'"' | b'\'' => {
                let triple = [b; 3];
                i = if starts(s, i, end, &triple) {
                    find(s, i + 3, end, &triple).map_or(end, |k| k + 3)
                } else {
                    string_end(s, i, end, b, false)
                };
                out.push(start, i, if key { Property } else { String });
            }
            b'=' => {
                out.push(i, i + 1, Punctuation);
                i += 1;
                key = false;
            }
            b'.' if key => {
                out.push(i, i + 1, Punctuation);
                i += 1;
            }
            b'[' | b'{' => {
                nest.push(b == b'{');
                out.push(i, i + 1, Punctuation);
                i += 1;
                key = b == b'{';
            }
            b']' | b'}' => {
                nest.pop();
                out.push(i, i + 1, Punctuation);
                i += 1;
                key = false;
            }
            b',' => {
                out.push(i, i + 1, Punctuation);
                i += 1;
                key = nest.last() == Some(&true);
            }
            _ => {
                let word_end = |mut k: usize| {
                    while k < end && !matches!(s[k], b' ' | b'\t' | b'\r' | b'\n' | b'=' | b',' | b']' | b'}' | b'#' | b'[' | b'{') && !(key && s[k] == b'.') {
                        k += 1;
                    }
                    k
                };
                i = word_end(i + 1);
                // A date's time may follow it after one space: `1979-05-27 07:32:00`.
                if !key && at(s, i, end) == b' ' && at(s, i + 1, end).is_ascii_digit() && s[start..i].contains(&b'-') {
                    i = word_end(i + 1);
                }
                let w = &s[start..i];
                let value = w.first().is_some_and(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-')) || is_word(w, &["true", "false", "inf", "nan"]);
                if key {
                    out.push(start, i, Property);
                } else if value {
                    out.push(start, i, Number);
                }
            }
        }
    }
}
