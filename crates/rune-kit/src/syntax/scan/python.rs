//! Python.

use super::*;

const PY_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "cls", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
    "return", "self", "try", "while", "with", "yield",
];
/// Keywords only at the start of a statement that ends in `:` (`match x:`, `case [a, b]:`).
const PY_SOFT_KEYWORDS: &[&str] = &["match", "case"];
const PY_CONSTANTS: &[&str] = &["True", "False", "None", "Ellipsis", "NotImplemented"];
const PY_TYPES: &[&str] = &[
    "bool", "bytearray", "bytes", "complex", "dict", "float", "frozenset", "int", "list", "object", "set", "str", "tuple",
    "type",
];

pub(super) fn python(s: &[u8], mut i: usize, end: usize, out: &mut Out, depth: u8) {
    use Syntax::*;
    let mut next = Next::Any;
    let mut after_dot = false;
    // Before the line's first token (decorators and soft keywords only count there).
    let mut line_start = depth == 0;
    while i < end && !out.full {
        let b = s[i];
        let start = i;
        if b == b'\n' {
            i += 1;
            line_start = true;
            continue;
        }
        if matches!(b, b' ' | b'\t' | b'\r') {
            i += 1;
            continue;
        }
        if b == b'#' {
            i = line_end(s, i, end);
            out.push(start, i, Comment);
            continue;
        }
        let first = std::mem::replace(&mut line_start, false);
        let was_dot = std::mem::replace(&mut after_dot, false);
        let was_next = std::mem::replace(&mut next, Next::Any);
        if let Some((quote, f)) = py_string_start(s, i, end) {
            i = py_string(s, i, quote, f, end, out, depth);
        } else if b.is_ascii_digit() || (b == b'.' && at(s, i + 1, end).is_ascii_digit()) {
            i = number_end(s, i, end);
            out.push(start, i, Number);
        } else if b == b'@' && first && is_ident_start(at(s, i + 1, end)) {
            i = ident_end(s, i + 1, end, b".");
            out.push(start, i, Keyword);
        } else if is_ident_start(b) {
            i = ident_end(s, i, end, b"");
            let w = &s[start..i];
            let mut k = i;
            while k < end && matches!(s[k], b' ' | b'\t') {
                k += 1;
            }
            let call = at(s, k, end) == b'(';
            let tok = if was_dot {
                call.then_some(Function)
            } else if is_word(w, PY_CONSTANTS) {
                Some(Number)
            } else if is_word(w, PY_KEYWORDS) {
                next = match w {
                    b"def" => Next::Function,
                    b"class" => Next::Type,
                    _ => Next::Any,
                };
                Some(Keyword)
            } else if first && is_word(w, PY_SOFT_KEYWORDS) && soft_keyword(s, i, end) {
                Some(Keyword)
            } else if was_next == Next::Type {
                Some(Type)
            } else if was_next == Next::Function {
                Some(Function)
            } else if is_word(w, PY_TYPES) {
                Some(Type)
            } else if is_all_caps(w) {
                Some(Number)
            } else if w[0].is_ascii_uppercase() {
                Some(Type)
            } else {
                call.then_some(Function)
            };
            if let Some(t) = tok {
                out.push(start, i, t);
            }
        } else {
            i += 1;
            after_dot = b == b'.';
            out.push(start, i, Punctuation);
        }
    }
}

/// Whether a string literal starts at `i`: its quote's index, and whether it is an f-string. Up
/// to two prefix letters (`r`, `b`, `u`, `f`, `rb`, `fr`, in either case) come before the quote.
fn py_string_start(s: &[u8], i: usize, end: usize) -> Option<(usize, bool)> {
    let mut f = false;
    for k in i..(i + 3).min(end) {
        match s[k] {
            b'"' | b'\'' => return Some((k, f)),
            b'f' | b'F' => f = true,
            b'r' | b'R' | b'b' | b'B' | b'u' | b'U' => {}
            _ => return None,
        }
    }
    None
}

/// A string from `start` (its prefix) whose quote is at `q`: single or triple quoted, an
/// f-string's `{}` fields coloured as code.
fn py_string(s: &[u8], start: usize, q: usize, f: bool, end: usize, out: &mut Out, depth: u8) -> usize {
    let qc = s[q];
    let triple = at(s, q + 1, end) == qc && at(s, q + 2, end) == qc;
    let e = if triple {
        let mut j = q + 3;
        loop {
            if j >= end {
                break end;
            }
            if s[j] == b'\\' {
                j += 2;
            } else if s[j] == qc && at(s, j + 1, end) == qc && at(s, j + 2, end) == qc {
                break j + 3;
            } else {
                j += 1;
            }
        }
    } else {
        string_end(s, q, end, qc, false)
    }
    .min(end);
    if !f || depth >= MAX_DEPTH {
        out.push(start, e, Syntax::String);
        return e;
    }
    let mut seg = start;
    let mut j = q + if triple { 3 } else { 1 };
    while j < e && !out.full {
        match s[j] {
            b'\\' => j += 2,
            b'{' if at(s, j + 1, e) == b'{' => j += 2,
            b'{' => {
                // The field runs to its matching `}`, stepping over strings in other quotes.
                let mut k = j + 1;
                let mut braces = 1u32;
                while k < e {
                    match s[k] {
                        b'{' => braces += 1,
                        b'}' => {
                            braces -= 1;
                            if braces == 0 {
                                break;
                            }
                        }
                        c @ (b'"' | b'\'') if c != qc => {
                            k = string_end(s, k, e, c, false);
                            continue;
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if k >= e {
                    break;
                }
                out.push(seg, j, Syntax::String);
                out.push(j, j + 1, Syntax::Punctuation);
                python(s, j + 1, k, out, depth + 1);
                out.push(k, k + 1, Syntax::Punctuation);
                j = k + 1;
                seg = j;
            }
            _ => j += 1,
        }
    }
    out.push(seg, e, Syntax::String);
    e
}

/// Whether the soft keyword ending at `i` starts a statement: a space follows it and the line
/// ends with `:` (before any comment).
fn soft_keyword(s: &[u8], i: usize, end: usize) -> bool {
    if at(s, i, end) != b' ' {
        return false;
    }
    let le = line_end(s, i, end);
    let line = &s[i..le];
    let code = line.iter().position(|&c| c == b'#').map_or(line, |p| &line[..p]);
    code.trim_ascii_end().ends_with(b":")
}
