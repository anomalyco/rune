//! Words in Thai, Lao, Khmer, Burmese, Chinese and Japanese from the browser's `Intl.Segmenter`
//! (segmenter.js): the web build links none of ICU4X's dictionaries (about 3 MB), and Parley takes
//! these boundaries for those runs' words and, in Thai, Lao, Khmer and Burmese, their line breaks
//! (`parley_engine::dictionary`). A double-click takes its words from here too.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/platform/web/segmenter.js")]
extern "C" {
    #[wasm_bindgen(js_name = wordBounds)]
    fn word_bounds(text: &str, language: &str) -> Vec<u32>;
}

/// Gives Parley the browser's word boundaries, once per page.
pub(crate) fn install() {
    parley_engine::dictionary::set_dictionary_segmenter(dictionary_words);
}

/// The boundaries strictly inside `text` (byte offsets, ascending) for one dictionary language.
fn dictionary_words(text: &str, language: &str, out: &mut Vec<usize>) {
    out.extend(bytes(text, &word_bounds(text, language)));
}

/// `text`'s word boundaries by the browser's rules and dictionaries, as byte offsets, ascending,
/// ending with `text.len()` (as ICU4X's segmenter gives them, past the leading 0).
pub(crate) fn words(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = bytes(text, &word_bounds(text, "und")).collect();
    if out.last() != Some(&text.len()) { out.push(text.len()); }
    out
}

/// UTF-16 offsets into `text` as byte offsets.
fn bytes<'a>(text: &'a str, utf16: &'a [u32]) -> impl Iterator<Item = usize> + 'a {
    let mut chars = text.char_indices().peekable();
    let mut unit = 0u32;
    utf16.iter().filter_map(move |&want| {
        while unit < want {
            let (_, c) = chars.next()?;
            unit += c.len_utf16() as u32;
        }
        (unit == want).then(|| chars.peek().map_or(text.len(), |&(at, _)| at))
    })
}
