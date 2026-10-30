//! Shell.

use super::*;

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "select",
    "time", "return", "exit", "export", "local", "readonly", "declare", "unset", "shift", "break", "continue",
];
/// Words after which the next word is a command too.
const SHELL_LEADS: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "time", "function", "!"];
const SHELL_WRAPPERS: &[&str] = &["sudo", "exec", "nohup", "xargs", "env", "command", "builtin"];

pub(super) fn shell(s: &[u8], mut i: usize, end: usize, out: &mut Out, depth: u8) {
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
