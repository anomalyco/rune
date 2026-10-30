//! YAML.

use super::*;

pub(super) fn yaml(s: &[u8], mut i: usize, end: usize, out: &mut Out) {
    // A block scalar (`|` or `>`) runs over the lines indented past this column.
    let mut block: Option<usize> = None;
    while i < end && !out.full {
        let le = line_end(s, i, end);
        let indent = s[i..le].iter().take_while(|&&c| c == b' ').count();
        let content = i + indent;
        let blank = s[content..le].iter().all(u8::is_ascii_whitespace);
        if let Some(col) = block {
            if blank || indent > col {
                out.push(content, le, Syntax::String);
                i = le + 1;
                continue;
            }
            block = None;
        }
        if !blank {
            yaml_line(s, i, content, le, out, &mut block);
        }
        i = le + 1;
    }
}

fn yaml_line(s: &[u8], line: usize, mut j: usize, le: usize, out: &mut Out, block: &mut Option<usize>) {
    use Syntax::*;
    let content = j;
    let sep = |k: usize| k >= le || matches!(s[k], b' ' | b'\t' | b'\r');
    if j == line && (starts(s, j, le, b"---") || starts(s, j, le, b"...")) && sep(j + 3) {
        out.push(j, j + 3, Punctuation);
        j += 3;
    }
    let mut parent = j - line;
    while j < le && s[j] == b'-' && sep(j + 1) {
        out.push(j, j + 1, Punctuation);
        parent = j - line;
        j += 1;
        while j < le && s[j] == b' ' {
            j += 1;
        }
    }
    if j < le && s[j] != b'#' && let Some((key_end, colon)) = yaml_key(s, j, le) {
        out.push(j, key_end, Property);
        out.push(colon, colon + 1, Punctuation);
        parent = j - line;
        j = colon + 1;
    }
    let mut flow = 0usize;
    while j < le && !out.full {
        let b = s[j];
        let start = j;
        match b {
            b' ' | b'\t' | b'\r' => j += 1,
            b'#' if j == content || matches!(s[j - 1], b' ' | b'\t') => {
                out.push(j, le, Comment);
                return;
            }
            b'&' | b'*' | b'!' => {
                while j < le && !matches!(s[j], b' ' | b'\t' | b'\r' | b',' | b']' | b'}') {
                    j += 1;
                }
                out.push(start, j, Type);
            }
            b'"' | b'\'' => {
                j = string_end(s, j, le, b, false);
                out.push(start, j, String);
            }
            b'|' | b'>' if flow == 0 => {
                j += 1;
                while j < le && (s[j] == b'+' || s[j] == b'-' || s[j].is_ascii_digit()) {
                    j += 1;
                }
                out.push(start, j, Punctuation);
                let mut k = j;
                while k < le && matches!(s[k], b' ' | b'\t' | b'\r') {
                    k += 1;
                }
                if k >= le || s[k] == b'#' {
                    *block = Some(parent);
                }
            }
            b'[' | b'{' => {
                flow += 1;
                j += 1;
                out.push(start, j, Punctuation);
            }
            b']' | b'}' | b',' | b':' if flow > 0 || b == b':' => {
                flow = flow.saturating_sub(matches!(b, b']' | b'}') as usize);
                j += 1;
                out.push(start, j, Punctuation);
            }
            _ => {
                // A plain scalar: to a ` #` comment or the line end, or a flow separator.
                let mut e = j;
                while e < le && !(matches!(s[e], b' ' | b'\t') && at(s, e + 1, le) == b'#') && !(flow > 0 && matches!(s[e], b',' | b']' | b'}' | b':')) {
                    e += 1;
                }
                let mut t = e;
                while t > j && matches!(s[t - 1], b' ' | b'\t' | b'\r') {
                    t -= 1;
                }
                let kind = if flow > 0 && at(s, e, le) == b':' { Property } else { scalar_kind(&s[j..t]) };
                out.push(j, t, kind);
                j = e.max(j + 1);
            }
        }
    }
}

/// A mapping key at `j`: (its end, its colon), when a `:` followed by a space or the line end
/// comes before any comment.
fn yaml_key(s: &[u8], j: usize, le: usize) -> Option<(usize, usize)> {
    let colon = |k: usize| at(s, k, le) == b':' && (k + 1 >= le || matches!(s[k + 1], b' ' | b'\t' | b'\r'));
    if matches!(s[j], b'"' | b'\'') {
        let e = string_end(s, j, le, s[j], false);
        let mut k = e;
        while k < le && s[k] == b' ' {
            k += 1;
        }
        return colon(k).then_some((e, k));
    }
    if matches!(s[j], b'[' | b'{' | b'&' | b'*' | b'!' | b'|' | b'>' | b'%' | b'@' | b'`' | b'?') {
        return None;
    }
    let mut k = j;
    while k < le {
        if colon(k) {
            let mut e = k;
            while e > j && s[e - 1] == b' ' {
                e -= 1;
            }
            return Some((e, k));
        }
        if s[k] == b'#' && k > j && matches!(s[k - 1], b' ' | b'\t') {
            return None;
        }
        k += 1;
    }
    None
}

fn scalar_kind(w: &[u8]) -> Syntax {
    const CONSTANTS: &[&str] = &["true", "false", "null", "~", "yes", "no", "on", "off", ".inf", "-.inf", ".nan"];
    if is_word_ci(w, CONSTANTS) {
        return Syntax::Number;
    }
    let digits = w.strip_prefix(b"-").or_else(|| w.strip_prefix(b"+")).unwrap_or(w);
    let numeric = digits.first().is_some_and(u8::is_ascii_digit)
        && (digits.starts_with(b"0x") || digits.starts_with(b"0o") || digits.iter().all(|&c| c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'e' | b'E' | b'+' | b'-')));
    if numeric { Syntax::Number } else { Syntax::String }
}
