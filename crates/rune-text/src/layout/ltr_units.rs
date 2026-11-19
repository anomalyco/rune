//! The runs of a prose text that read as one left-to-right unit whatever language is around
//! them: a path, a URL, a file name, `renderView()`, `--no-verify`, `#693`, `+12`, `v2.14.0`,
//! `C++`. Isolating each (`El::isolate(range, Direction::Ltr)`) keeps its characters in the
//! order they were typed in an Arabic or Hebrew sentence, where its leading or trailing
//! punctuation would otherwise take the sentence's direction (`.gitignore` drawn as
//! `gitignore.`, `--no-verify` as `no-verify--`), and keeps it out of the paragraph's
//! direction, so a paragraph that starts with a path still reads the way its words do.

use std::ops::Range;

/// The left-to-right units of `text`, in order: each word (a run between spaces), less the
/// sentence punctuation around it, that is ASCII letters or digits joined by symbols, a signed
/// number, or a path (which may hold other scripts' folder names). A plain word, a word in
/// another script and punctuation alone are not units, nor is a pair like `و/أو`.
pub fn ltr_units(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0;
    for word in text.split(char::is_whitespace) {
        let start = at;
        at += word.len() + text[start + word.len()..].chars().next().map_or(0, char::len_utf8);
        if let Some(r) = unit(word) {
            out.push(start + r.start..start + r.end);
        }
    }
    out
}

/// The unit in `word`, if it holds one, as a range of `word`.
fn unit(word: &str) -> Option<Range<usize>> {
    let (mut a, mut b) = (0, word.len());
    // Opening brackets and quotes come off the front, and closing ones and sentence
    // punctuation off the end, unless the word opened them itself (`renderView()`, `a[0]`).
    while let Some(c) = word[a..b].chars().next().filter(|c| OPEN.contains(c) && !closes_inside(&word[a..b], *c)) {
        a += c.len_utf8();
    }
    while let Some(c) = word[a..b].chars().next_back().filter(|&c| END.contains(&c) && !(CLOSE.contains(&c) && opens_inside(&word[a..b], c))) {
        b -= c.len_utf8();
    }
    let core = &word[a..b];
    let ascii = core.is_ascii();
    let alnum = core.bytes().any(|c| c.is_ascii_alphanumeric());
    let symbol = core.bytes().any(|c| !c.is_ascii_alphanumeric());
    let path = core.starts_with(['/', '~', '.']) && core.contains(['/', '\\']) || drive(core);
    let named = core.contains(['/', '\\']) && extension(core);
    let ok = alnum && ((ascii && symbol) || (!ascii && (path || named)));
    ok.then_some(a..b)
}

const OPEN: &[char] = &['(', '[', '{', '"', '\'', '«', '“', '‘', '<'];
const CLOSE: &[char] = &[')', ']', '}', '>'];
const END: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '»', '”', '’', '،', '؛', '؟', '۔', '…', '>'];

fn pair(c: char) -> char {
    match c {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        ')' => '(',
        ']' => '[',
        '}' => '{',
        '>' => '<',
        other => other,
    }
}

/// `s` (starting with opening `c`) closes it again after its first character.
fn closes_inside(s: &str, c: char) -> bool {
    let close = pair(c);
    close != c && s[c.len_utf8()..].contains(close)
}

/// `s` (ending with closing `c`) opened it before.
fn opens_inside(s: &str, c: char) -> bool {
    s[..s.len() - c.len_utf8()].contains(pair(c))
}

/// `C:\…` or `C:/…`.
fn drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// It ends in a dot and one to eight ASCII letters or digits, as a file name does.
fn extension(s: &str) -> bool {
    s.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && (1..=8).contains(&ext.len()) && ext.bytes().all(|c| c.is_ascii_alphanumeric()))
}
