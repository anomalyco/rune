//! Strips WGSL comments and redundant whitespace at build time for `wasm32` builds.
//!
//! WGSL has no string literals, so stripping `//` line comments, nested `/* ... */` block
//! comments, blank lines, and extra horizontal whitespace preserves token boundaries and shader
//! semantics while keeping line breaks between statements.

/// Returns `src` with comments, leading/trailing line whitespace, blank lines, and runs of
/// interior horizontal whitespace removed.
pub(crate) fn minify(src: &str) -> String {
    let stripped = strip_block_comments(src);
    let mut out = String::with_capacity(stripped.len());
    for raw_line in stripped.lines() {
        let code = raw_line.split_once("//").map_or(raw_line, |(code, _)| code).trim();
        if code.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let mut prev_ws = false;
        for b in code.bytes() {
            if b == b' ' || b == b'\t' {
                if !prev_ws {
                    out.push(' ');
                    prev_ws = true;
                }
            } else {
                out.push(b as char);
                prev_ws = false;
            }
        }
    }
    out
}

/// Removes WGSL block comments (`/* ... */`, which may nest), inserting a space when a comment
/// separates two identifier/number characters so adjacent tokens never merge.
fn strip_block_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    let mut depth = 0usize;
    let mut in_line_comment = false;
    while i < bytes.len() {
        if in_line_comment {
            let b = bytes[i];
            out.push(b as char);
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if depth == 0 && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            in_line_comment = true;
            out.push_str("//");
            i += 2;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            if depth == 0 && out.bytes().last().is_some_and(is_ident) {
                out.push(' ');
            }
            depth += 1;
            i += 2;
            continue;
        }
        if depth > 0 && i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
            depth -= 1;
            i += 2;
            continue;
        }
        if depth == 0 {
            out.push(bytes[i] as char);
        } else if bytes[i] == b'\n' {
            out.push('\n');
        }
        i += 1;
    }
    out
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
