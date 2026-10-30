//! C-like languages: JavaScript, TypeScript, JSX, JSON, Rust, Swift and Go.

use super::*;

const JS_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do",
    "else", "export", "extends", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "let", "new",
    "of", "return", "static", "super", "switch", "this", "throw", "try", "typeof", "var", "void", "while", "with", "yield",
];
const TS_KEYWORDS: &[&str] = &[
    "abstract", "asserts", "declare", "enum", "implements", "infer", "interface", "is", "keyof", "module", "namespace",
    "override", "private", "protected", "public", "readonly", "satisfies", "type", "unique",
];
const TS_TYPES: &[&str] = &["any", "bigint", "boolean", "never", "number", "object", "string", "symbol", "unknown"];
const JS_CONSTANTS: &[&str] = &["true", "false", "null", "undefined", "NaN", "Infinity"];
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "fn", "for", "if",
    "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static",
    "struct", "super", "trait", "type", "union", "unsafe", "use", "where", "while", "yield",
];
const RUST_TYPES: &[&str] = &[
    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8", "u16", "u32", "u64", "u128",
    "usize",
];
const SWIFT_KEYWORDS: &[&str] = &[
    "actor", "any", "as", "associatedtype", "async", "await", "break", "case", "catch", "class", "continue",
    "convenience", "default", "defer", "deinit", "didSet", "do", "else", "enum", "extension", "fallthrough", "final",
    "fileprivate", "for", "func", "get", "guard", "if", "import", "in", "indirect", "init", "inout", "internal", "is",
    "lazy", "let", "mutating", "nonisolated", "open", "operator", "override", "private", "protocol", "public", "repeat",
    "required", "rethrows", "return", "self", "Self", "set", "some", "static", "struct", "subscript", "super", "switch",
    "throw", "throws", "try", "typealias", "unowned", "var", "weak", "where", "while", "willSet",
];
const GO_KEYWORDS: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto",
    "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var",
];
const GO_TYPES: &[&str] = &[
    "any", "bool", "byte", "comparable", "complex64", "complex128", "error", "float32", "float64", "int", "int8", "int16",
    "int32", "int64", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr",
];
const GO_CONSTANTS: &[&str] = &["true", "false", "nil", "iota"];
const BOOLS: &[&str] = &["true", "false"];
const JSON_CONSTANTS: &[&str] = &["true", "false", "null"];
const SWIFT_CONSTANTS: &[&str] = &["true", "false", "nil"];
/// Keywords after which a name is a function or a type being defined.
const DEFINES_FUNCTION: &[&str] = &["fn", "function", "func"];
const DEFINES_TYPE: &[&str] =
    &["class", "struct", "enum", "trait", "interface", "type", "protocol", "extension", "typealias", "actor", "union"];
/// Keywords that end an expression, so a `/` after them divides and a `<` compares.
const VALUE_KEYWORDS: &[&str] = &["this", "super", "self", "Self"];

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Next {
    Any,
    Function,
    Type,
}

pub(super) fn c_like(s: &[u8], mut i: usize, end: usize, lang: Lang, out: &mut Out, depth: u8) {
    use Syntax::*;
    let js = matches!(lang, Lang::Js | Lang::Ts | Lang::Tsx);
    let jsx = matches!(lang, Lang::Js | Lang::Tsx);
    let (keywords, extra_keywords, types, constants): (&[&str], &[&str], &[&str], &[&str]) = match lang {
        Lang::Js => (JS_KEYWORDS, &[], &[], JS_CONSTANTS),
        Lang::Ts | Lang::Tsx => (JS_KEYWORDS, TS_KEYWORDS, TS_TYPES, JS_CONSTANTS),
        Lang::Rust => (RUST_KEYWORDS, &[], RUST_TYPES, BOOLS),
        Lang::Swift => (SWIFT_KEYWORDS, &[], &[], SWIFT_CONSTANTS),
        Lang::Go => (GO_KEYWORDS, &[], GO_TYPES, GO_CONSTANTS),
        _ => (&[], &[], &[], JSON_CONSTANTS),
    };
    // Whether the last token ends an expression (a name, literal or closing bracket).
    let mut value = false;
    let mut next = Next::Any;
    let mut after_dot = false;
    while i < end && !out.full {
        let b = s[i];
        let start = i;
        if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
            continue;
        }
        if b == b'/' && at(s, i + 1, end) == b'/' {
            i = line_end(s, i, end);
            out.push(start, i, Comment);
            continue;
        }
        if b == b'/' && at(s, i + 1, end) == b'*' {
            i = block_comment_end(s, i + 2, end, matches!(lang, Lang::Rust | Lang::Swift));
            out.push(start, i, Comment);
            continue;
        }
        let was_dot = after_dot;
        after_dot = false;
        let was_next = next;
        next = Next::Any;
        if lang == Lang::Rust && (b == b'r' || b == b'b') && let Some(e) = rust_prefixed_literal(s, i, end) {
            out.push(start, e, String);
            (i, value) = (e, true);
        } else if lang == Lang::Rust && b == b'\'' {
            // 'x' and '\n' are characters; 'a with no quote after one character is a lifetime.
            let c = at(s, i + 1, end);
            let after = i + 1 + utf8_len(c);
            if c == b'\\' {
                i = string_end(s, i, end, b'\'', false);
                out.push(start, i, String);
            } else if c != 0 && at(s, after, end) == b'\'' {
                i = after + 1;
                out.push(start, i, String);
            } else if is_ident_start(c) {
                i = ident_end(s, i + 1, end, &[]);
                out.push(start, i, Keyword);
            } else {
                i += 1;
                out.push(start, i, Punctuation);
            }
            value = true;
        } else if lang == Lang::Swift && starts(s, i, end, b"\"\"\"") {
            i = find(s, i + 3, end, b"\"\"\"").map_or(end, |k| k + 3);
            out.push(start, i, String);
            value = true;
        } else if b == b'"' || (b == b'\'' && lang != Lang::Swift) {
            i = string_end(s, i, end, b, lang == Lang::Rust);
            let mut k = i;
            while k < end && s[k].is_ascii_whitespace() {
                k += 1;
            }
            let key = lang == Lang::Json && at(s, k, end) == b':';
            out.push(start, i, if key { Property } else { String });
            value = true;
        } else if b == b'`' && js {
            i = template(s, i, end, lang, out, depth);
            value = true;
        } else if b == b'`' && lang == Lang::Go {
            // A raw string: no escapes, any lines, to the next backtick.
            i = s[i + 1..end].iter().position(|&c| c == b'`').map_or(end, |p| i + p + 2);
            out.push(start, i, String);
            value = true;
        } else if b == b'/' && js && !value && let Some(e) = regex_end(s, i, end) {
            i = e;
            out.push(start, i, String);
            value = true;
        } else if b == b'<'
            && jsx
            && !value
            && (is_ident_start(at(s, i + 1, end)) || at(s, i + 1, end) == b'>')
            && !type_parameters(s, i, end)
        {
            i = jsx_element(s, i, end, lang, out, depth);
            value = true;
        } else if b.is_ascii_digit() || (b == b'.' && !value && at(s, i + 1, end).is_ascii_digit()) {
            i = number_end(s, i, end);
            out.push(start, i, Number);
            value = true;
        } else if (b == b'@' && matches!(lang, Lang::Ts | Lang::Tsx | Lang::Js | Lang::Swift)) || (b == b'#' && lang == Lang::Swift) {
            i = ident_end(s, i + 1, end, &[]);
            out.push(start, i, Keyword);
            value = false;
        } else if is_ident_start(b) || (b == b'$' && js) {
            i = ident_end(s, i, end, if js { b"$" } else { b"" });
            let w = &s[start..i];
            let mut k = i;
            while k < end && matches!(s[k], b' ' | b'\t') {
                k += 1;
            }
            let call = at(s, k, end) == b'(';
            // Go capitalises exported names: `Addr string` and `Addr: x` name a field.
            let go_field =
                lang == Lang::Go && (is_ident_start(at(s, k, end)) || (at(s, k, end) == b':' && at(s, k + 1, end) != b'='));
            value = true;
            let tok = if lang == Lang::Rust && at(s, i, end) == b'!' && at(s, i + 1, end) != b'=' {
                i += 1;
                Some(Function)
            } else if was_dot {
                call.then_some(Function)
            } else if is_word(w, constants) {
                Some(Number)
            } else if is_word(w, keywords) || is_word(w, extra_keywords) {
                value = is_word(w, VALUE_KEYWORDS);
                if is_word(w, DEFINES_FUNCTION) {
                    next = Next::Function;
                } else if is_word(w, DEFINES_TYPE) {
                    next = Next::Type;
                }
                Some(Keyword)
            } else if is_word(w, types) || was_next == Next::Type {
                Some(Type)
            } else if was_next == Next::Function || (lang == Lang::Go && call) {
                // Go capitalises exported functions, so a call is a call before it is a type.
                Some(Function)
            } else if is_all_caps(w) {
                Some(Number)
            } else if w[0].is_ascii_uppercase() && !go_field {
                Some(Type)
            } else {
                call.then_some(Function)
            };
            if let Some(t) = tok {
                out.push(start, i, t);
            }
        } else {
            i += 1;
            after_dot = b == b'.' && at(s, i, end) != b'.';
            value = matches!(b, b')' | b']');
            out.push(start, i, Punctuation);
        }
    }
}

pub(super) fn utf8_len(lead: u8) -> usize {
    match lead {
        0xf0.. => 4,
        0xe0.. => 3,
        0xc0.. => 2,
        _ => 1,
    }
}

/// Past a Rust `r"…"`, `r#"…"#`, `b"…"`, `br"…"` or `b'…'` literal starting at `i`.
fn rust_prefixed_literal(s: &[u8], i: usize, end: usize) -> Option<usize> {
    let mut j = i;
    if s[j] == b'b' {
        j += 1;
        match at(s, j, end) {
            b'"' => return Some(string_end(s, j, end, b'"', true)),
            b'\'' => return Some(string_end(s, j, end, b'\'', false)),
            b'r' => {}
            _ => return None,
        }
    }
    // A raw string: r, up to 255 #, a quote, and the same quote and #s to close.
    j += 1;
    let mut hashes = 0;
    while at(s, j, end) == b'#' && hashes < 255 {
        hashes += 1;
        j += 1;
    }
    if at(s, j, end) != b'"' {
        return None;
    }
    j += 1;
    while j < end {
        if s[j] == b'"' {
            let mut k = j + 1;
            let mut n = 0;
            while n < hashes && at(s, k, end) == b'#' {
                n += 1;
                k += 1;
            }
            if n == hashes {
                return Some(k);
            }
            j = k;
        } else {
            j += 1;
        }
    }
    Some(end)
}

/// Past a JavaScript regex literal starting at the `/` at `i`, if one closes on this line within
/// [`MAX_REGEX`] bytes.
fn regex_end(s: &[u8], i: usize, end: usize) -> Option<usize> {
    let limit = end.min(i + MAX_REGEX);
    let mut j = i + 1;
    let mut class = false;
    while j < limit {
        match s[j] {
            b'\\' => j += 1,
            b'[' => class = true,
            b']' => class = false,
            b'\n' => return None,
            b'/' if !class => {
                j += 1;
                while j < end && s[j].is_ascii_alphabetic() {
                    j += 1;
                }
                return (j > i + 2).then_some(j);
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// A template literal from the backtick at `i`, its `${}` expressions coloured as code.
fn template(s: &[u8], i: usize, end: usize, lang: Lang, out: &mut Out, depth: u8) -> usize {
    let mut seg = i;
    let mut j = i + 1;
    while j < end && !out.full {
        match s[j] {
            b'\\' => j += 2,
            b'`' => {
                out.push(seg, j + 1, Syntax::String);
                return j + 1;
            }
            b'$' if at(s, j + 1, end) == b'{' => {
                out.push(seg, j, Syntax::String);
                let close = matching(s, j + 1, end, b'{', b'}', if matches!(lang, Lang::Js | Lang::Tsx) { Comments::Jsx } else { Comments::Js });
                out.push(j, j + 2, Syntax::Punctuation);
                if depth < MAX_DEPTH {
                    c_like(s, j + 2, close, lang, out, depth + 1);
                }
                if close >= end {
                    return end;
                }
                out.push(close, close + 1, Syntax::Punctuation);
                j = close + 1;
                seg = j;
            }
            _ => j += 1,
        }
    }
    let j = j.min(end);
    out.push(seg, j, Syntax::String);
    j
}

/// Whether the `<` at `i` opens TypeScript type parameters rather than a JSX tag: `<T,>(…)`,
/// `<T extends U>(…)`, or `<T>(…)` followed by `=>` or `:`. Looks at most [`MAX_REGEX`] bytes
/// ahead, on this line.
pub(super) fn type_parameters(s: &[u8], i: usize, end: usize) -> bool {
    let limit = end.min(i + MAX_REGEX);
    let mut angles = 0u32;
    let mut j = i + 1;
    while j < limit {
        match s[j] {
            b'<' => angles += 1,
            b'>' if angles > 0 => angles -= 1,
            b'>' => {
                if at(s, j + 1, limit) != b'(' {
                    return false;
                }
                // `<T>(x: T) => T` is a signature; `<b>(note)</b>` is an element.
                let mut parens = 0u32;
                let mut k = j + 1;
                while k < limit && s[k] != b'\n' {
                    match s[k] {
                        b'(' => parens += 1,
                        b')' => {
                            parens -= 1;
                            if parens == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if at(s, k, limit) != b')' {
                    return true;
                }
                k += 1;
                while matches!(at(s, k, limit), b' ' | b'\t') {
                    k += 1;
                }
                return starts(s, k, limit, b"=>") || at(s, k, limit) == b':';
            }
            b',' if angles == 0 => return true,
            b'=' | b'{' | b'"' | b'\'' | b'/' | b'\n' => return false,
            c if is_ident_start(c) => {
                let e = ident_end(s, j, limit, &[]);
                if &s[j..e] == b"extends" {
                    return true;
                }
                j = e;
                continue;
            }
            _ => {}
        }
        j += 1;
    }
    false
}

/// Words that start a statement, not JSX text.
const STATEMENT_STARTS: &[&str] =
    &["export", "import", "const", "let", "var", "function", "class", "interface", "enum", "return"];

/// Whether the line from `i` starts like code rather than JSX text: with a statement keyword or a
/// closing `}`, `)` or `;`. An element still open there never closed, so scanning goes back to
/// code instead of taking the rest of the block as children.
pub(super) fn code_line(s: &[u8], mut i: usize, end: usize) -> bool {
    while matches!(at(s, i, end), b' ' | b'\t' | b'\r') {
        i += 1;
    }
    match at(s, i, end) {
        b'}' | b')' | b';' => true,
        c if is_ident_start(c) => is_word(&s[i..ident_end(s, i, end, &[])], STATEMENT_STARTS),
        _ => false,
    }
}

/// A JSX element (with its children and nested elements) from the `<` at `i`. One that never
/// closes ends at a stray `<` in a tag or at the first line that reads as code.
fn jsx_element(s: &[u8], mut i: usize, end: usize, lang: Lang, out: &mut Out, depth: u8) -> usize {
    use Syntax::*;
    let mut open = 0usize;
    loop {
        let closing = at(s, i + 1, end) == b'/';
        let mut j = i + 1 + closing as usize;
        out.push(i, j, Punctuation);
        let name = j;
        j = ident_end(s, j, end, b".:-$");
        if j > name {
            out.push(name, j, if s[name].is_ascii_uppercase() { Type } else { Keyword });
        }
        let mut self_closing = false;
        let mut closed = false;
        while j < end && !out.full {
            match s[j] {
                b'>' => {
                    out.push(j, j + 1, Punctuation);
                    j += 1;
                    closed = true;
                    break;
                }
                b'/' if at(s, j + 1, end) == b'>' => {
                    out.push(j, j + 2, Punctuation);
                    j += 2;
                    (self_closing, closed) = (true, true);
                    break;
                }
                b'"' | b'\'' => {
                    let e = string_end(s, j, end, s[j], true);
                    out.push(j, e, String);
                    j = e;
                }
                b'{' => j = jsx_expression(s, j, end, lang, out, depth),
                b'=' => {
                    out.push(j, j + 1, Punctuation);
                    j += 1;
                }
                b'<' => break,
                b'\n' if code_line(s, j + 1, end) => break,
                c if is_ident_start(c) => {
                    let e = ident_end(s, j, end, b"-:");
                    out.push(j, e, Property);
                    j = e;
                }
                _ => j += 1,
            }
        }
        if !closed {
            return j.min(end);
        }
        if closing {
            open = open.saturating_sub(1);
        } else if !self_closing {
            open += 1;
        }
        if open == 0 {
            return j;
        }
        // Children: text stays plain, `{…}` is code, `<` before a name, `/` or `>` starts the next
        // tag (`a < b` is text).
        loop {
            if j >= end || out.full {
                return end;
            }
            match s[j] {
                b'<' if matches!(at(s, j + 1, end), b'/' | b'>') || is_ident_start(at(s, j + 1, end)) => break,
                b'{' => j = jsx_expression(s, j, end, lang, out, depth),
                b'\n' if code_line(s, j + 1, end) => return j,
                _ => j += 1,
            }
        }
        i = j;
    }
}

fn jsx_expression(s: &[u8], j: usize, end: usize, lang: Lang, out: &mut Out, depth: u8) -> usize {
    let close = matching(s, j, end, b'{', b'}', Comments::Jsx);
    out.push(j, j + 1, Syntax::Punctuation);
    if depth < MAX_DEPTH {
        c_like(s, j + 1, close, lang, out, depth + 1);
    }
    if close >= end {
        return end;
    }
    out.push(close, close + 1, Syntax::Punctuation);
    close + 1
}
