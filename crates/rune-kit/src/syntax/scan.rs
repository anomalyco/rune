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

mod c_like;
mod markup;
mod python;
mod shell;
mod sql;
mod yaml;

use super::Syntax;
use c_like::{Next, c_like, code_line, type_parameters, utf8_len};
use markup::{css, markup};
use python::python;
use shell::shell;
use sql::sql;
use yaml::yaml;

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

#[cfg(test)]
mod tests;
