//! Search that matches as a person expects in every language:
//! case by Unicode's full case folding (`ß` and `ẞ` as `ss`, final `ς` as `σ`, `İ` as `i`, and
//! in Turkish and Azerbaijani `I` as `ı`), full-width ASCII and half-width katakana as their
//! usual forms (`ＡＰＩ` as `API`, `ﾃｽﾄ` as `テスト`), and accents asymmetrically: a query with
//! no accents matches with or without them (`viet` finds `Việt`, `lodz` finds `Łódź`), a query
//! with one matches only that (`tiếng` skips `tieng`), as Safari's and VS Code's find do.
//! Hiragana and katakana stay apart, as Chrome's find keeps them. Matches are byte ranges of the
//! text as given, whatever folding changed in length.
//!
//! The decomposition is `icu_normalizer`'s NFD, which Parley already links; the rest is the few
//! tables below. No other data. [`markdown::Case::Folded`](crate::markdown::Case::Folded) finds
//! the same way in rendered Markdown.

use std::ops::Range;

use icu_normalizer::DecomposingNormalizerBorrowed;

/// How a search folds: Unicode's case folding, widths and accents (see the module), with
/// Turkish and Azerbaijani's dotted and dotless i where the interface is in one of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Fold {
    /// `I` folds to dotless `ı`, `İ` to `i`.
    turkic: bool,
}

impl Fold {
    /// The folding for interface language `tag` (BCP 47: `tr` and `az` fold `I` to `ı`).
    pub fn for_language(tag: &str) -> Fold {
        Fold { turkic: tag.starts_with("tr") || tag.starts_with("az") }
    }

    /// Non-overlapping matches of `query` in `text`, byte ranges of `text`.
    pub fn find(self, text: &str, query: &str) -> Vec<Range<usize>> {
        if text.is_empty() || query.is_empty() {
            return Vec::new();
        }
        if text.is_ascii() && query.is_ascii() && !self.turkic {
            let (hay, needle) = (text.to_ascii_lowercase(), query.to_ascii_lowercase());
            return hay.match_indices(&needle).map(|(at, _)| at..at + needle.len()).collect();
        }
        let folder = Folder { turkic: self.turkic, nfd: DecomposingNormalizerBorrowed::new_nfd() };
        let strip = !folder.accented(query);
        let (needle, _) = folder.fold(query, strip);
        if needle.is_empty() {
            return Vec::new();
        }
        let (hay, map) = folder.fold(text, strip);
        hay.match_indices(&needle)
            .filter_map(|(f0, _)| {
                let (s0, s1) = (*map.get(f0)?, *map.get(f0 + needle.len())?);
                (s0 != usize::MAX && s1 != usize::MAX && s0 < s1).then_some(s0..s1)
            })
            .collect()
    }

    /// Whether `text` holds `query` (trimmed); an empty query matches everything.
    pub fn contains(self, text: &str, query: &str) -> bool {
        let query = query.trim();
        query.is_empty() || !self.find(text, query).is_empty()
    }
}

struct Folder {
    turkic: bool,
    nfd: DecomposingNormalizerBorrowed<'static>,
}

impl Folder {
    /// Whether `query` names an accent or a letter that folds to plain Latin (`ł`, `ø`, `æ`):
    /// then the text must have it too.
    fn accented(&self, query: &str) -> bool {
        query.chars().filter(|c| !c.is_ascii()).flat_map(char::to_lowercase).any(|c| {
            plain_letter(c).is_some() || self.nfd.normalize(c.encode_utf8(&mut [0; 4])).chars().skip(1).any(is_mark)
        })
    }

    /// `s` folded, and for each byte of the result where a source character's folding starts
    /// (and one past the end) the source byte it came from; `usize::MAX` inside one character's
    /// folding, so no match starts or ends there.
    fn fold(&self, s: &str, strip: bool) -> (String, Vec<usize>) {
        let mut out = String::with_capacity(s.len());
        let mut map = Vec::with_capacity(s.len() + 1);
        for (at, c) in s.char_indices() {
            let before = out.len();
            self.push(&mut out, c, strip);
            // A half-width voiced mark joined the kana before it: that kana's folding now ends
            // where the next character starts.
            if out.len() == before { continue; }
            map.resize(before, usize::MAX);
            map.push(at);
        }
        map.resize(out.len(), usize::MAX);
        map.push(s.len());
        (out, map)
    }

    fn push(&self, out: &mut String, c: char, strip: bool) {
        let c = match c as u32 {
            // Full-width ASCII and the ideographic space.
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            0x3000 => ' ',
            // Half-width katakana, and their voiced marks joined to the kana before them.
            0xFF61..=0xFF9F => {
                if let Some(voiced) = voice(out.chars().last(), c) {
                    out.pop();
                    out.push(voiced);
                    return;
                }
                halfwidth_kana(c)
            }
            _ => c,
        };
        let lowered = match c {
            'ß' | 'ẞ' => return out.push_str("ss"),
            'ς' => return out.push('σ'),
            'İ' => return out.push('i'),
            'I' if self.turkic => return out.push('ı'),
            _ if c.is_ascii() => return out.push(c.to_ascii_lowercase()),
            _ => c.to_lowercase(),
        };
        for c in lowered {
            if !strip {
                out.push(c);
                continue;
            }
            if let Some(plain) = plain_letter(c) {
                out.push_str(plain);
                continue;
            }
            // Only Latin, Greek and Cyrillic letters carry the marks that go; the rest (CJK,
            // Hangul, kana) stay as they are without a lookup.
            if !matches!(c as u32, 0xC0..=0x24F | 0x370..=0x52F | 0x1E00..=0x1FFF) {
                out.push(c);
                continue;
            }
            let mut buf = [0; 4];
            let decomposed = self.nfd.normalize(c.encode_utf8(&mut buf));
            let mut chars = decomposed.chars();
            match chars.next() {
                Some(base) if chars.clone().all(is_mark) && decomposed.chars().count() > 1 => out.push(base),
                _ => out.push(c),
            }
        }
    }
}

/// Combining marks folded away from a query with none: Latin, Greek and Cyrillic diacritics.
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// Letters with no decomposition that a reader treats as a plain one (lowercase forms).
fn plain_letter(c: char) -> Option<&'static str> {
    Some(match c {
        'ł' => "l",
        'ø' => "o",
        'đ' | 'ð' => "d",
        'ħ' => "h",
        'ŧ' => "t",
        'æ' => "ae",
        'œ' => "oe",
        'þ' => "th",
        _ => return None,
    })
}

/// Half-width katakana U+FF61-FF9F as their full-width forms.
fn halfwidth_kana(c: char) -> char {
    const KANA: &str = "。「」、・ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン゛゜";
    KANA.chars().nth(c as usize - 0xFF61).unwrap_or(c)
}

/// The voiced form of full-width kana `before` with half-width mark `mark` (ﾞ U+FF9E, ﾟ U+FF9F).
fn voice(before: Option<char>, mark: char) -> Option<char> {
    let before = before? as u32;
    let voiced = match (mark as u32, before) {
        (0xFF9E, 0x30A6) => 0x30F4, // ウ゛ = ヴ
        (0xFF9E, 0x30AB..=0x30C1) if before % 2 == 1 => before + 1, // カ..チ: ガ..ヂ
        (0xFF9E, 0x30C4 | 0x30C6 | 0x30C8) => before + 1, // ツテト: ヅデド
        (0xFF9E, 0x30CF..=0x30DB) if (before - 0x30CF) % 3 == 0 => before + 1, // ハ..ホ: バ..ボ
        (0xFF9F, 0x30CF..=0x30DB) if (before - 0x30CF) % 3 == 0 => before + 2, // ハ..ホ: パ..ポ
        _ => return None,
    };
    char::from_u32(voiced)
}
