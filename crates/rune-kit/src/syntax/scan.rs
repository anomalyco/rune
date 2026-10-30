//! The built-in scanners: one small scanner per language family (C-like, Python, shell,
//! markup, CSS, SQL, YAML, diff) walks the bytes once and returns coloured byte ranges. Anything
//! it doesn't recognise stays uncoloured.
//!
//! Input is untrusted. Every scanner only moves forward, lookaheads are either bounded or
//! skip bytes the scanner would visit anyway, and embedded code (template `${}`, JSX `{}`,
//! f-string `{}`, `<script>`, `$()`) nests at most [`MAX_DEPTH`] deep, so time is linear in the
//! source. Colouring stops after [`MAX_SOURCE`] bytes or [`MAX_TOKENS`] ranges; the rest stays
//! plain. Ranges start and end only on ASCII bytes or the ends of the source, so they fall on
//! character boundaries.
//!
//! Written from scratch against each language's reference for its lexical rules (keywords,
//! literals, comments); no other highlighter's grammar was used.

use super::Syntax;

/// A language family member the scanners know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Lang {
    /// JavaScript, with JSX.
    Js,
    Ts,
    Tsx,
    Json,
    Rust,
    Swift,
    Go,
    Python,
    Css,
    Shell,
    /// HTML and XML.
    Html,
    Sql,
    Yaml,
    /// Unified diffs.
    Diff,
}

impl Lang {
    /// The language a fence's info word names (case-insensitive), if it is one we colour.
    pub(crate) fn from_name(name: &str) -> Option<Lang> {
        Some(match name.to_ascii_lowercase().as_str() {
            "js" | "javascript" | "jsx" | "mjs" | "cjs" => Lang::Js,
            "ts" | "typescript" | "mts" | "cts" => Lang::Ts,
            "tsx" => Lang::Tsx,
            "json" | "jsonc" | "json5" => Lang::Json,
            "rust" | "rs" => Lang::Rust,
            "swift" => Lang::Swift,
            "go" | "golang" => Lang::Go,
            "python" | "py" | "python3" | "py3" | "pyi" => Lang::Python,
            "css" => Lang::Css,
            "bash" | "sh" | "shell" | "zsh" | "shellscript" => Lang::Shell,
            "html" | "htm" | "xhtml" | "xml" | "svg" => Lang::Html,
            "sql" | "postgres" | "postgresql" | "mysql" | "sqlite" => Lang::Sql,
            "yaml" | "yml" => Lang::Yaml,
            "diff" | "patch" => Lang::Diff,
            _ => return None,
        })
    }
}

/// Bytes coloured per block; past this (cut at a line end) the code draws plain.
pub(crate) const MAX_SOURCE: usize = 32 * 1024;
/// Coloured ranges per block; past this the code draws plain.
pub(crate) const MAX_TOKENS: usize = 8192;
/// How deep embedded code nests (a template's `${}`, JSX's `{}`, HTML's `<script>`).
const MAX_DEPTH: u8 = 3;
/// The longest a JavaScript regex literal can be, which bounds the lookahead after a `/`.
const MAX_REGEX: usize = 512;

/// A coloured byte range of the source.
pub(crate) type Token = (u32, u32, Syntax);

/// How much of `src` gets colour: all of it, or up to the last line end within [`MAX_SOURCE`].
pub(crate) fn coloured_len(src: &str) -> usize {
    if src.len() <= MAX_SOURCE {
        return src.len();
    }
    src.as_bytes()[..MAX_SOURCE].iter().rposition(|&b| b == b'\n').unwrap_or(0)
}

/// The coloured ranges of `src`, in order and not overlapping.
pub(crate) fn highlight(lang: Lang, src: &str) -> Vec<Token> {
    let end = coloured_len(src);
    let mut out = Out::default();
    scan(lang, src.as_bytes(), 0, end, &mut out, 0);
    // The scanners only cut at ASCII bytes; this keeps a slip from ever splitting a character.
    out.tokens.retain(|&(a, b, _)| src.is_char_boundary(a as usize) && src.is_char_boundary(b as usize));
    out.tokens
}

#[derive(Default)]
struct Out {
    tokens: Vec<Token>,
    full: bool,
}

impl Out {
    fn push(&mut self, a: usize, b: usize, s: Syntax) {
        if a >= b || self.full {
            return;
        }
        if let Some(last) = self.tokens.last_mut()
            && last.1 as usize == a
            && last.2 == s
        {
            last.1 = b as u32;
            return;
        }
        if self.tokens.len() >= MAX_TOKENS {
            self.full = true;
            return;
        }
        self.tokens.push((a as u32, b as u32, s));
    }
}

fn scan(lang: Lang, s: &[u8], i: usize, end: usize, out: &mut Out, depth: u8) {
    match lang {
        Lang::Js | Lang::Ts | Lang::Tsx | Lang::Json | Lang::Rust | Lang::Swift | Lang::Go => c_like(s, i, end, lang, out, depth),
        Lang::Python => python(s, i, end, out, depth),
        Lang::Diff => diff(s, i, end, out),
        Lang::Css => css(s, i, end, out),
        Lang::Shell => shell(s, i, end, out, depth),
        Lang::Html => markup(s, i, end, out, depth),
        Lang::Sql => sql(s, i, end, out),
        Lang::Yaml => yaml(s, i, end, out),
    }
}

// ---------------------------------------------------------------------------------------------
// Shared helpers

fn at(s: &[u8], i: usize, end: usize) -> u8 {
    if i < end { s[i] } else { 0 }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn ident_end(s: &[u8], mut i: usize, end: usize, extra: &[u8]) -> usize {
    while i < end && (is_ident(s[i]) || extra.contains(&s[i])) {
        i += 1;
    }
    i
}

fn line_end(s: &[u8], i: usize, end: usize) -> usize {
    s[i..end].iter().position(|&b| b == b'\n').map_or(end, |p| i + p)
}

fn starts(s: &[u8], i: usize, end: usize, pat: &[u8]) -> bool {
    end - i.min(end) >= pat.len() && &s[i..i + pat.len()] == pat
}

/// Where `pat` (a few bytes long) next starts at or after `i`, ignoring ASCII case.
fn find(s: &[u8], i: usize, end: usize, pat: &[u8]) -> Option<usize> {
    if end < pat.len() {
        return None;
    }
    (i..=end - pat.len()).find(|&k| s[k..k + pat.len()].eq_ignore_ascii_case(pat))
}

/// Past the string whose quote `q` is at `i`: after the closing quote, at the line end for a
/// single-line string, or at `end`.
fn string_end(s: &[u8], i: usize, end: usize, q: u8, multiline: bool) -> usize {
    let mut j = i + 1;
    while j < end {
        match s[j] {
            b'\\' => j += 2,
            b'\n' if !multiline => return j,
            c if c == q => return j + 1,
            _ => j += 1,
        }
    }
    end
}

/// The comments [`matching`] skips: JavaScript's `//` and `/* */`, or the shell's `#`. `Jsx` is
/// `Js` that also steps over JSX elements, whose text is not code (`<p>don't</p>`).
#[derive(Clone, Copy, PartialEq)]
enum Comments {
    Js,
    Jsx,
    Shell,
}

/// Where [`matching`] is inside JSX: code between brackets (with its bracket depth), or an
/// element (with how many of its tags are open, and whether it is in a tag rather than text).
#[derive(Clone, Copy)]
enum Frame {
    Code(usize),
    Element { open: usize, in_tag: bool, closing: bool },
}

/// How deep [`matching`] follows elements and the code inside them before treating `<` as an
/// operator.
const MAX_FRAMES: usize = 32;

/// Whether the `<` at `j` (code from `i`) starts a JSX element: it follows an operator, an opening
/// bracket or `return`, and opens a tag rather than type parameters.
fn jsx_starts(s: &[u8], i: usize, j: usize, end: usize) -> bool {
    if !(is_ident_start(at(s, j + 1, end)) || at(s, j + 1, end) == b'>') || type_parameters(s, j, end) {
        return false;
    }
    let mut k = j;
    while k > i && j - k < 64 && s[k - 1].is_ascii_whitespace() {
        k -= 1;
    }
    match if k > i { s[k - 1] } else { b'{' } {
        b'(' | b',' | b'=' | b':' | b'?' | b'&' | b'|' | b'{' | b'[' | b'!' | b';' | b'>' => true,
        _ => k >= i + 6 && &s[k - 6..k] == b"return" && (k == i + 6 || !is_ident(s[k - 7])),
    }
}

/// The index of the bracket closing the one at `i`, skipping quoted strings and comments, or
/// `end`.
fn matching(s: &[u8], i: usize, end: usize, open: u8, close: u8, comments: Comments) -> usize {
    let mut stack = vec![Frame::Code(0)];
    let mut j = i;
    while j < end {
        let top = stack.len() - 1;
        if let Frame::Element { open: tags, in_tag, closing } = &mut stack[top] {
            let c = s[j];
            if *in_tag {
                match c {
                    b'"' | b'\'' => {
                        j = string_end(s, j, end, c, true);
                        continue;
                    }
                    b'{' => {
                        stack.push(Frame::Code(0));
                        continue;
                    }
                    b'>' | b'/' if c == b'>' || at(s, j + 1, end) == b'>' => {
                        j += if c == b'>' { 1 } else { 2 };
                        if *closing {
                            *tags = tags.saturating_sub(1);
                        } else if c == b'>' {
                            *tags += 1;
                        }
                        *in_tag = false;
                        if *tags == 0 {
                            stack.pop();
                        }
                        continue;
                    }
                    b'<' => {
                        // A tag cut short; read on as code.
                        stack.pop();
                        continue;
                    }
                    _ => {}
                }
            } else {
                match c {
                    b'{' => {
                        stack.push(Frame::Code(0));
                        continue;
                    }
                    b'<' if at(s, j + 1, end) == b'/' => {
                        (*in_tag, *closing) = (true, true);
                        j += 2;
                        continue;
                    }
                    b'<' if is_ident_start(at(s, j + 1, end)) || at(s, j + 1, end) == b'>' => {
                        (*in_tag, *closing) = (true, false);
                    }
                    b'\n' if code_line(s, j + 1, end) => {
                        stack.pop();
                    }
                    _ => {}
                }
            }
            j += 1;
            continue;
        }
        let Frame::Code(depth) = &mut stack[top] else { unreachable!() };
        match s[j] {
            b'<' if comments == Comments::Jsx && top + 3 <= MAX_FRAMES && jsx_starts(s, i, j, end) => {
                stack.push(Frame::Element { open: 0, in_tag: true, closing: false });
            }
            b'\\' => j += 1,
            b'"' | b'\'' | b'`' => {
                j = string_end(s, j, end, s[j], s[j] == b'`');
                continue;
            }
            b'/' if comments != Comments::Shell && at(s, j + 1, end) == b'/' => {
                j = line_end(s, j, end);
                continue;
            }
            b'/' if comments != Comments::Shell && at(s, j + 1, end) == b'*' => {
                j = block_comment_end(s, j + 2, end, false);
                continue;
            }
            // A `#` starting a word; `$#` and `a#b` are not comments.
            b'#' if comments == Comments::Shell && j > i && matches!(s[j - 1], b' ' | b'\t' | b'\n' | b'(' | b';' | b'|' | b'&') => {
                j = line_end(s, j, end);
                continue;
            }
            c if c == open => *depth += 1,
            c if c == close => {
                *depth -= 1;
                if *depth == 0 {
                    if top == 0 {
                        return j;
                    }
                    stack.pop();
                }
            }
            _ => {}
        }
        j += 1;
    }
    end
}

fn number_end(s: &[u8], mut i: usize, end: usize) -> usize {
    let hex = s[i] == b'0' && matches!(at(s, i + 1, end), b'x' | b'X');
    i += 1;
    while i < end {
        let c = s[i];
        if c.is_ascii_alphanumeric() || c == b'_' {
            i += if !hex && matches!(c, b'e' | b'E') && matches!(at(s, i + 1, end), b'+' | b'-') { 2 } else { 1 };
        } else if c == b'.' && at(s, i + 1, end).is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    i.min(end)
}

fn block_comment_end(s: &[u8], mut i: usize, end: usize, nested: bool) -> usize {
    let mut depth = 1u32;
    while i < end {
        if s[i] == b'*' && at(s, i + 1, end) == b'/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else if nested && s[i] == b'/' && at(s, i + 1, end) == b'*' {
            depth += 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    end
}

fn is_word(w: &[u8], list: &[&str]) -> bool {
    list.iter().any(|k| k.as_bytes() == w)
}

fn is_word_ci(w: &[u8], list: &[&str]) -> bool {
    list.iter().any(|k| k.as_bytes().eq_ignore_ascii_case(w))
}

fn is_all_caps(w: &[u8]) -> bool {
    w.len() > 1 && w[0].is_ascii_uppercase() && w.iter().all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}

// ---------------------------------------------------------------------------------------------
// C-like: JavaScript, TypeScript, JSX, JSON, Rust, Swift, Go

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
enum Next {
    Any,
    Function,
    Type,
}

fn c_like(s: &[u8], mut i: usize, end: usize, lang: Lang, out: &mut Out, depth: u8) {
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

fn utf8_len(lead: u8) -> usize {
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
fn type_parameters(s: &[u8], i: usize, end: usize) -> bool {
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
fn code_line(s: &[u8], mut i: usize, end: usize) -> bool {
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

// ---------------------------------------------------------------------------------------------
// Python

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

fn python(s: &[u8], mut i: usize, end: usize, out: &mut Out, depth: u8) {
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

// ---------------------------------------------------------------------------------------------
// Diff

/// Whole lines: added, removed, or a header (`@@`, `diff`, `index`, `---`, `+++`, `\`).
fn diff(s: &[u8], mut i: usize, end: usize, out: &mut Out) {
    const HEADERS: &[&[u8]] = &[b"@@", b"diff ", b"index ", b"--- ", b"+++ ", b"\\"];
    while i < end && !out.full {
        let le = line_end(s, i, end);
        let line = &s[i..le];
        let kind = if HEADERS.iter().any(|h| line.starts_with(h)) {
            Some(Syntax::Comment)
        } else {
            match line.first() {
                Some(b'+') => Some(Syntax::Inserted),
                Some(b'-') => Some(Syntax::Deleted),
                _ => None,
            }
        };
        if let Some(k) = kind {
            let text_end = if line.ends_with(b"\r") { le - 1 } else { le };
            out.push(i, text_end, k);
        }
        i = le + 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Shell

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "select",
    "time", "return", "exit", "export", "local", "readonly", "declare", "unset", "shift", "break", "continue",
];
/// Words after which the next word is a command too.
const SHELL_LEADS: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "time", "function", "!"];
const SHELL_WRAPPERS: &[&str] = &["sudo", "exec", "nohup", "xargs", "env", "command", "builtin"];

fn shell(s: &[u8], mut i: usize, end: usize, out: &mut Out, depth: u8) {
    use Syntax::*;
    let mut command = true;
    // A heredoc's delimiter (its byte range) and whether `<<-` strips leading tabs.
    let mut heredoc: Option<(usize, usize, bool)> = None;
    while i < end && !out.full {
        let b = s[i];
        let start = i;
        match b {
            b'\n' => {
                i += 1;
                command = true;
                if let Some((a, z, dash)) = heredoc.take() {
                    i = heredoc_body(s, i, end, &s[a..z], dash, out);
                }
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b'#' => {
                i = line_end(s, i, end);
                out.push(start, i, Comment);
            }
            b'\'' => {
                i = s[i + 1..end].iter().position(|&c| c == b'\'').map_or(end, |p| i + p + 2);
                out.push(start, i, String);
                command = false;
            }
            b'"' => {
                i = shell_string(s, i, end, out, depth);
                command = false;
            }
            b'$' => {
                i = dollar(s, i, end, out, depth);
                command = false;
            }
            b'`' => {
                let close = s[i + 1..end].iter().position(|&c| c == b'`').map_or(end, |p| i + 1 + p);
                out.push(i, i + 1, Punctuation);
                if depth < MAX_DEPTH {
                    shell(s, i + 1, close, out, depth + 1);
                }
                out.push(close, (close + 1).min(end), Punctuation);
                i = (close + 1).min(end);
                command = false;
            }
            b'\\' => {
                // A line continuation keeps whether the next word is a command (after `&&`).
                let crlf = at(s, i + 1, end) == b'\r' && at(s, i + 2, end) == b'\n';
                let continues = crlf || at(s, i + 1, end) == b'\n';
                let step = if crlf { 3 } else { 1 + utf8_len(at(s, i + 1, end)) };
                i = (i + step).min(end);
                command &= continues;
            }
            b'<' if at(s, i + 1, end) == b'<' && at(s, i + 2, end) != b'<' => {
                i += 2;
                let dash = at(s, i, end) == b'-';
                i += dash as usize;
                out.push(start, i, Punctuation);
                while matches!(at(s, i, end), b' ' | b'\t') {
                    i += 1;
                }
                let q = at(s, i, end);
                let quoted = q == b'\'' || q == b'"';
                let name = i + quoted as usize;
                let name_end = ident_end(s, name, end, b"-");
                if name_end > name {
                    i = name_end + (quoted && at(s, name_end, end) == q) as usize;
                    out.push(start + 2 + dash as usize, i, String);
                    heredoc = Some((name, name_end, dash));
                }
                command = false;
            }
            b'|' | b'&' | b';' | b'(' | b')' | b'{' | b'}' | b'<' | b'>' => {
                i += 1;
                out.push(start, i, Punctuation);
                command = !matches!(b, b')' | b'<' | b'>');
            }
            _ => {
                while i < end && !matches!(s[i], b' ' | b'\t' | b'\r' | b'\n' | b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' | b'\'' | b'"' | b'$' | b'`' | b'\\') {
                    i += 1;
                }
                let w = &s[start..i];
                let eq = w.iter().position(|&c| c == b'=');
                if matches!(w, b"[" | b"[[") {
                    out.push(start, i, Punctuation);
                    command = false;
                } else if matches!(w, b"]" | b"]]") {
                    out.push(start, i, Punctuation);
                } else if command && let Some(eq) = eq.filter(|&e| e > 0 && w[..e].iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_')) {
                    // FOO=bar before a command.
                    out.push(start, start + eq, Property);
                    out.push(start + eq, start + eq + 1, Punctuation);
                } else if command {
                    if is_word(w, SHELL_KEYWORDS) {
                        out.push(start, i, Keyword);
                        command = is_word(w, SHELL_LEADS);
                    } else {
                        out.push(start, i, Function);
                        command = is_word(w, SHELL_WRAPPERS);
                    }
                } else if w == b"in" {
                    out.push(start, i, Keyword);
                } else if w.iter().all(u8::is_ascii_digit) {
                    out.push(start, i, Number);
                }
                if w.is_empty() {
                    i += 1;
                }
            }
        }
    }
}

/// A double-quoted shell string from `i`, its `$` expansions coloured as variables.
fn shell_string(s: &[u8], i: usize, end: usize, out: &mut Out, depth: u8) -> usize {
    let mut seg = i;
    let mut j = i + 1;
    while j < end && !out.full {
        match s[j] {
            b'\\' => j += 2,
            b'"' => {
                out.push(seg, j + 1, Syntax::String);
                return j + 1;
            }
            b'$' if matches!(at(s, j + 1, end), b'{' | b'(') || is_ident_start(at(s, j + 1, end)) => {
                out.push(seg, j, Syntax::String);
                j = dollar(s, j, end, out, depth);
                seg = j;
            }
            _ => j += 1,
        }
    }
    let j = j.min(end);
    out.push(seg, j, Syntax::String);
    j
}

/// A `$` expansion from `i`: `$name`, `${…}`, `$1`, `$?`, or a `$(…)` command substitution.
fn dollar(s: &[u8], i: usize, end: usize, out: &mut Out, depth: u8) -> usize {
    let c = at(s, i + 1, end);
    if c == b'{' {
        let e = s[i..end].iter().position(|&b| b == b'}' || b == b'\n').map_or(end, |p| i + p + (s[i + p] == b'}') as usize);
        out.push(i, e, Syntax::Property);
        e
    } else if c == b'(' {
        let close = matching(s, i + 1, end, b'(', b')', Comments::Shell);
        out.push(i, i + 2, Syntax::Punctuation);
        if depth < MAX_DEPTH {
            shell(s, i + 2, close, out, depth + 1);
        }
        if close >= end {
            return end;
        }
        out.push(close, close + 1, Syntax::Punctuation);
        close + 1
    } else if c.is_ascii_alphabetic() || c == b'_' {
        let mut e = i + 1;
        while e < end && (s[e].is_ascii_alphanumeric() || s[e] == b'_') {
            e += 1;
        }
        out.push(i, e, Syntax::Property);
        e
    } else if b"?@#*$!-0123456789".contains(&c) && c != 0 {
        out.push(i, i + 2, Syntax::Property);
        i + 2
    } else {
        i + 1
    }
}

/// A heredoc's lines from `i` up to and including the line that is just `delim`.
fn heredoc_body(s: &[u8], i: usize, end: usize, delim: &[u8], dash: bool, out: &mut Out) -> usize {
    let mut j = i;
    while j < end {
        let le = line_end(s, j, end);
        let mut line = &s[j..le];
        if dash {
            while let [b'\t', rest @ ..] = line {
                line = rest;
            }
        }
        if line.strip_suffix(b"\r").unwrap_or(line) == delim {
            out.push(i, le, Syntax::String);
            return le;
        }
        j = le + 1;
    }
    out.push(i, end, Syntax::String);
    end
}

// ---------------------------------------------------------------------------------------------
// Markup: HTML and XML

fn markup(s: &[u8], mut i: usize, end: usize, out: &mut Out, depth: u8) {
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

// ---------------------------------------------------------------------------------------------
// CSS

fn css(s: &[u8], mut i: usize, end: usize, out: &mut Out) {
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

// ---------------------------------------------------------------------------------------------
// SQL

const SQL_KEYWORDS: &[&str] = &[
    "add", "all", "alter", "analyze", "and", "any", "as", "asc", "begin", "between", "by", "cascade", "case", "check",
    "column", "commit", "conflict", "constraint", "create", "cross", "database", "declare", "default", "delete", "desc",
    "distinct", "do", "drop", "else", "end", "except", "exists", "explain", "fetch", "filter", "first", "for", "foreign",
    "from", "full", "function", "grant", "group", "having", "if", "ilike", "in", "index", "inner", "insert", "intersect",
    "into", "is", "join", "key", "language", "lateral", "left", "like", "limit", "materialized", "next", "not", "nothing",
    "offset", "on", "only", "or", "order", "outer", "over", "partition", "primary", "procedure", "recursive",
    "references", "rename", "replace", "return", "returning", "returns", "revoke", "right", "rollback", "rows", "schema",
    "select", "set", "some", "table", "temp", "temporary", "then", "to", "transaction", "trigger", "truncate", "union",
    "unique", "update", "using", "vacuum", "values", "view", "when", "where", "window", "with",
];
const SQL_TYPES: &[&str] = &[
    "bigint", "bigserial", "blob", "bool", "boolean", "bytea", "char", "character", "date", "decimal", "double", "float",
    "int", "integer", "interval", "json", "jsonb", "money", "numeric", "precision", "real", "serial", "smallint", "text",
    "time", "timestamp", "timestamptz", "tinyint", "uuid", "varchar",
];
const SQL_CONSTANTS: &[&str] = &["true", "false", "null"];

fn sql(s: &[u8], mut i: usize, end: usize, out: &mut Out) {
    use Syntax::*;
    while i < end && !out.full {
        let b = s[i];
        let start = i;
        if b.is_ascii_whitespace() {
            i += 1;
        } else if b == b'-' && at(s, i + 1, end) == b'-' {
            i = line_end(s, i, end);
            out.push(start, i, Comment);
        } else if b == b'/' && at(s, i + 1, end) == b'*' {
            i = block_comment_end(s, i + 2, end, false);
            out.push(start, i, Comment);
        } else if b == b'\'' {
            // '' inside a string is a quote.
            i += 1;
            while i < end {
                if s[i] == b'\'' {
                    if at(s, i + 1, end) == b'\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(start, i.min(end), String);
        } else if b == b'"' || b == b'`' {
            i = string_end(s, i, end, b, true);
            out.push(start, i, Property);
        } else if b.is_ascii_digit() {
            i = number_end(s, i, end);
            out.push(start, i, Number);
        } else if matches!(b, b'$' | b':' | b'@') && (is_ident(at(s, i + 1, end))) {
            i = ident_end(s, i + 1, end, b"");
            out.push(start, i, Property);
        } else if b == b'?' {
            i += 1;
            out.push(start, i, Property);
        } else if is_ident_start(b) {
            i = ident_end(s, i, end, b"");
            let w = &s[start..i];
            let mut k = i;
            while k < end && matches!(s[k], b' ' | b'\t') {
                k += 1;
            }
            if is_word_ci(w, SQL_CONSTANTS) {
                out.push(start, i, Number);
            } else if is_word_ci(w, SQL_KEYWORDS) {
                out.push(start, i, Keyword);
            } else if is_word_ci(w, SQL_TYPES) {
                out.push(start, i, Type);
            } else if at(s, k, end) == b'(' {
                out.push(start, i, Function);
            }
        } else {
            i += 1;
            out.push(start, i, Punctuation);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// YAML

fn yaml(s: &[u8], mut i: usize, end: usize, out: &mut Out) {
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

#[cfg(test)]
mod tests;
