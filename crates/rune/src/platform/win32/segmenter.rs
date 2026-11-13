//! Words in Thai, Lao, Khmer, Burmese, Chinese and Japanese from Windows' own ICU (`icu.dll`,
//! Windows 10 1903 and later): the Windows build links none of ICU4X's dictionaries (about 3 MB),
//! and Parley takes these boundaries for those runs' words and, in Thai, Lao, Khmer and Burmese,
//! their line breaks (`parley_engine::dictionary`), as the web build takes the browser's. A
//! double-click takes its words from here too. `icu.dll` loads the first time a run needs it,
//! never at launch; without it (Windows Server Core, older Windows 10) those runs fall back to
//! what the rule-based segmenter does, and the `segmenter-dictionaries` feature links ICU4X's
//! instead.

use std::ffi::c_void;
use std::sync::OnceLock;

/// Gives Parley Windows' word boundaries, once per process. Loads nothing yet.
pub(crate) fn install() {
    parley_engine::dictionary::set_dictionary_segmenter(dictionary_words);
}

/// The boundaries strictly inside `text` (byte offsets, ascending) for one dictionary language.
fn dictionary_words(text: &str, language: &str, out: &mut Vec<usize>) {
    let Some(bounds) = word_bounds(text, language) else { return };
    out.extend(bounds.into_iter().filter(|&b| b > 0 && b < text.len()));
}

/// `text`'s word boundaries by ICU's rules and dictionaries, as byte offsets, ascending, ending
/// with `text.len()` (as ICU4X's segmenter gives them, past the leading 0). Without `icu.dll`,
/// `text` is one word.
pub(crate) fn words(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = word_bounds(text, "").unwrap_or_default().into_iter().filter(|&b| b > 0).collect();
    if out.last() != Some(&text.len()) { out.push(text.len()); }
    out
}

type UBreakIterator = c_void;
struct Icu {
    open: unsafe extern "C" fn(kind: i32, locale: *const u8, text: *const u16, len: i32, status: *mut i32) -> *mut UBreakIterator,
    next: unsafe extern "C" fn(*mut UBreakIterator) -> i32,
    close: unsafe extern "C" fn(*mut UBreakIterator),
}

/// `icu.dll`'s break iterator functions, looked up once from System32.
fn icu() -> Option<&'static Icu> {
    static ICU: OnceLock<Option<Icu>> = OnceLock::new();
    ICU.get_or_init(|| {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
        let name: Vec<u16> = "icu.dll\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated name; System32 only, so no other icu.dll can stand in.
        let module = unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if module.is_null() { return None; }
        // SAFETY: each name is NUL-terminated, and each export has the C signature ICU documents
        // for it (unicode/ubrk.h), which the transmutes give it.
        unsafe {
            let open = GetProcAddress(module, c"ubrk_open".as_ptr().cast())?;
            let next = GetProcAddress(module, c"ubrk_next".as_ptr().cast())?;
            let close = GetProcAddress(module, c"ubrk_close".as_ptr().cast())?;
            Some(Icu { open: std::mem::transmute(open), next: std::mem::transmute(next), close: std::mem::transmute(close) })
        }
    })
    .as_ref()
}

/// ICU's word boundaries in `text` for `language` (empty: the root locale), as byte offsets
/// past 0, the last `text.len()`.
fn word_bounds(text: &str, language: &str) -> Option<Vec<usize>> {
    const UBRK_WORD: i32 = 1;
    const UBRK_DONE: i32 = -1;
    let icu = icu()?;
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let locale: Vec<u8> = language.bytes().chain([0]).collect();
    let mut status = 0i32;
    // SAFETY: `locale` is NUL-terminated and `utf16` outlives the iterator, which is closed below.
    let it = unsafe { (icu.open)(UBRK_WORD, locale.as_ptr(), utf16.as_ptr(), utf16.len() as i32, &mut status) };
    // ICU's warnings are negative (a fallback or default locale, as for every dictionary
    // language) and its failures positive (`U_FAILURE`).
    if it.is_null() || status > 0 {
        // SAFETY: an iterator ICU made despite the failure is still its to close.
        if !it.is_null() { unsafe { (icu.close)(it) }; }
        return None;
    }
    let mut units = Vec::new();
    loop {
        // SAFETY: `it` is a live iterator over `utf16`.
        let at = unsafe { (icu.next)(it) };
        if at == UBRK_DONE { break; }
        units.push(at as usize);
    }
    // SAFETY: opened above, closed once.
    unsafe { (icu.close)(it) };
    Some(bytes(text, &units))
}

/// UTF-16 offsets into `text` (ascending) as byte offsets.
fn bytes(text: &str, units: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(units.len());
    let (mut unit, mut wanted) = (0usize, units.iter().peekable());
    for (at, c) in text.char_indices().chain([(text.len(), '\0')]) {
        while wanted.peek().is_some_and(|&&w| w <= unit) {
            if wanted.next() == Some(&unit) { out.push(at); }
        }
        unit += c.len_utf16();
    }
    out
}
