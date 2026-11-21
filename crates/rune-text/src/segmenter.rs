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
