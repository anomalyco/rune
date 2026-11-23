//! Platform word segmentation for Parley dictionaries and double-click / accessibility word
//! boundaries: `icu.dll` on Windows (unless `segmenter-dictionaries` is enabled), `Intl.Segmenter`
//! on web, and ICU4X's compiled dictionaries on macOS and Linux.

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(all(target_os = "windows", not(feature = "segmenter-dictionaries")))]
mod win32;

/// Installs the platform dictionary word segmenter into Parley.
#[cfg(all(target_os = "windows", not(feature = "segmenter-dictionaries")))]
#[doc(hidden)]
pub fn install() {
    win32::install();
}

/// Wakes Windows' deferred `icu.dll` warmup thread once the first frame is on screen.
#[cfg(all(target_os = "windows", not(feature = "segmenter-dictionaries")))]
#[doc(hidden)]
pub fn first_frame_presented() {
    win32::first_frame_presented();
}

/// Installs the browser's `Intl.Segmenter` word segmenter into Parley.
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub fn install() {
    web::install();
}

/// Where each of `text`'s words ends, in order, the last at `text.len()`: ICU4X's dictionaries
/// natively, the browser's (`Intl.Segmenter`) in a page and Windows' own (`icu.dll`) on Windows,
/// which link none.
#[doc(hidden)]
pub fn words(text: &str) -> Vec<usize> {
    #[cfg(all(target_os = "windows", not(feature = "segmenter-dictionaries")))]
    {
        win32::words(text)
    }
    #[cfg(target_arch = "wasm32")]
    {
        web::words(text)
    }
    #[cfg(not(any(target_arch = "wasm32", all(target_os = "windows", not(feature = "segmenter-dictionaries")))))]
    {
        use icu_segmenter::{WordSegmenter, options::WordBreakInvariantOptions};
        WordSegmenter::new_dictionary(WordBreakInvariantOptions::default())
            .segment_str(text)
            .filter(|&b| b > 0)
            .collect()
    }
}

/// Where each of `text`'s words ends, as a browser's selection and word motion take them: the
/// words of [`words`], with Chromium's tailoring of Unicode's rules. A full stop or a colon between
/// letters ends a word (`foo.bar` is `foo`, `.`, `bar`; `a:b` is `a`, `:`, `b`), while a full stop
/// between digits doesn't (`1.5`, `1,000.50`) and an apostrophe still joins (`can't`).
#[doc(hidden)]
pub fn word_ends(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut start = 0;
    for end in words(text) {
        for (i, c) in text[start..end].char_indices().filter(|&(i, c)| i > 0 && splits(c)) {
            let at = start + i;
            let next = at + c.len_utf8();
            let digits = text[..at].chars().next_back().is_some_and(|b| b.is_numeric())
                && text[next..].chars().next().is_some_and(|a| a.is_numeric());
            if full_stop(c) && digits {
                continue;
            }
            out.push(at);
            out.push(next);
        }
        out.push(end);
        start = end;
    }
    out.dedup();
    out
}

/// A full stop or colon Unicode joins letters across (`MidNumLet`, `MidLetter`) and Chromium
/// doesn't; apostrophes, the middle dot (Catalan `l·l`) and the other joiners are left alone.
fn splits(c: char) -> bool {
    full_stop(c) || matches!(c, ':' | '\u{FE13}' | '\u{FE55}' | '\u{FF1A}')
}

fn full_stop(c: char) -> bool {
    matches!(c, '.' | '\u{2024}' | '\u{FE52}' | '\u{FF0E}')
}

/// Non-complex-script word starts strictly inside `words` below `256` bytes, for waiting ASCII
/// accessibility text runs (`a11y::text`).
#[doc(hidden)]
pub fn non_complex_word_starts(words: &str, max_byte: usize) -> Vec<u8> {
    let wb_seg = const {
        icu_segmenter::WordSegmenter::new_for_non_complex_scripts(
            icu_segmenter::options::WordBreakInvariantOptions::default(),
        )
    };
    wb_seg
        .segment_str(words)
        .filter(|&w| w > 0 && w < max_byte && w < 256)
        .map(|w| w as u8)
        .collect()
}
