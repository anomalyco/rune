//! Shared Hunspell dictionary parsing, word filtering, checking and suggestions via `spellbook`,
//! used by both the Linux (`linux.rs`) and web (`web.rs`) spellcheck backends.

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Iso8859_1,
    Iso8859_15,
    Other,
}

/// Misspelled word byte ranges in `text`: a word is misspelled when every active dictionary
/// rejects it, matching Chromium's multilingual spell checking.
pub(crate) fn misspelled(dicts: &[&spellbook::Dictionary], text: &str) -> Vec<(usize, usize)> {
    if dicts.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (chunk_at, chunk) in text.split_whitespace().map(|c| (c.as_ptr() as usize - text.as_ptr() as usize, c)) {
        // Paths, URLs, addresses and code are not prose.
        if chunk.contains(['/', '@', '`', '_', '\\', '=', '<', '>', '{', '}']) || chunk.contains("::") {
            continue;
        }
        for (at, word) in chunk.split_word_bound_indices() {
            if !checked(word) {
                continue;
            }
            if dicts.iter().all(|d| !d.check(word)) {
                out.push((chunk_at + at, chunk_at + at + word.len()));
            }
        }
    }
    out
}

/// A word worth checking: letters (an apostrophe inside), at least two, no digits, and no
/// capital after its first letter (an acronym or an identifier).
pub(crate) fn checked(word: &str) -> bool {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    letters >= 2
        && word.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '\u{2019}')
        && !word.chars().skip(1).any(char::is_uppercase)
}

/// Suggestions for `word` across `dicts`, deduplicated in dictionary order.
pub(crate) fn suggest(dicts: &[&spellbook::Dictionary], word: &str) -> Vec<String> {
    let mut out = Vec::new();
    for d in dicts {
        let mut found = Vec::new();
        d.suggest(word, &mut found);
        for s in found {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// Escapes a plain word for `spellbook::Dictionary::add`, which parses `.dic` lines where `/`
/// starts flags and `\\` escapes.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn escape_dic_word(word: &str) -> Cow<'_, str> {
    if !word.contains(['/', '\\']) {
        return Cow::Borrowed(word);
    }
    let mut escaped = String::with_capacity(word.len() + 2);
    for ch in word.chars() {
        if ch == '/' || ch == '\\' {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    Cow::Owned(escaped)
}

/// Decodes a Hunspell `.aff` / `.dic` byte pair honoring the `.aff` `SET` directive
/// (UTF-8 with optional BOM, ISO-8859-1, and ISO-8859-15).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn decode_pair(aff: &[u8], dic: &[u8]) -> Option<(String, String)> {
    let enc = detect_encoding(aff);
    Some((decode_bytes(aff, enc)?, decode_bytes(dic, enc)?))
}

/// Decodes and parses a Hunspell `.aff` / `.dic` byte pair into a `spellbook::Dictionary`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn parse(aff: &[u8], dic: &[u8]) -> Option<spellbook::Dictionary> {
    let (aff_str, dic_str) = decode_pair(aff, dic)?;
    spellbook::Dictionary::new(&aff_str, &dic_str).ok()
}

fn detect_encoding(aff: &[u8]) -> Encoding {
    let aff = aff.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(aff);
    for line in aff.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let start = line.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(line.len());
        let trimmed = &line[start..];
        if trimmed.is_empty() || trimmed[0] == b'#' {
            continue;
        }
        let mut parts = trimmed.split(|b| b.is_ascii_whitespace()).filter(|s| !s.is_empty());
        let Some(keyword) = parts.next() else { continue };
        if keyword.eq_ignore_ascii_case(b"SET") {
            let Some(enc) = parts.next() else { return Encoding::Utf8 };
            let enc = std::str::from_utf8(enc).unwrap_or("");
            if enc.eq_ignore_ascii_case("UTF-8") || enc.eq_ignore_ascii_case("UTF8") {
                return Encoding::Utf8;
            }
            if enc.eq_ignore_ascii_case("ISO8859-1")
                || enc.eq_ignore_ascii_case("ISO-8859-1")
                || enc.eq_ignore_ascii_case("ISO_8859-1")
                || enc.eq_ignore_ascii_case("LATIN1")
            {
                return Encoding::Iso8859_1;
            }
            if enc.eq_ignore_ascii_case("ISO8859-15")
                || enc.eq_ignore_ascii_case("ISO-8859-15")
                || enc.eq_ignore_ascii_case("ISO_8859-15")
                || enc.eq_ignore_ascii_case("LATIN9")
            {
                return Encoding::Iso8859_15;
            }
            return Encoding::Other;
        }
    }
    Encoding::Utf8
}

fn decode_bytes(bytes: &[u8], enc: Encoding) -> Option<String> {
    match enc {
        Encoding::Utf8 | Encoding::Other => {
            let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
            std::str::from_utf8(bytes).ok().map(str::to_owned)
        }
        Encoding::Iso8859_1 => Some(bytes.iter().map(|&b| b as char).collect()),
        Encoding::Iso8859_15 => Some(
            bytes
                .iter()
                .map(|&b| match b {
                    0xA4 => '\u{20AC}',
                    0xA6 => '\u{0160}',
                    0xA8 => '\u{0161}',
                    0xB4 => '\u{017D}',
                    0xB8 => '\u{017E}',
                    0xBC => '\u{0152}',
                    0xBD => '\u{0153}',
                    0xBE => '\u{0178}',
                    _ => b as char,
                })
                .collect(),
        ),
    }
}
