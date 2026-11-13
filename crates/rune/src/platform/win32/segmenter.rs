//! Words in Thai, Lao, Khmer, Burmese, Chinese and Japanese from Windows' own ICU (`icu.dll`,
//! Windows 10 1903 and later): the Windows build links none of ICU4X's dictionaries (about 3 MB),
//! and Parley takes these boundaries for those runs' words and, in Thai, Lao, Khmer and Burmese,
//! their line breaks (`parley_engine::dictionary`), as the web build takes the browser's. A
//! double-click takes its words from here too. `icu.dll` and its Thai and Japanese dictionaries
//! load on a thread of their own as the app starts (`warm`: about 12,000 µs of that thread's CPU and
//! 800,000 B), so the UI thread never pays for them, a Thai first frame or a pasted line included;
//! without it (Windows Server Core, older Windows 10) those runs fall back to
//! what the rule-based segmenter does, and the `segmenter-dictionaries` feature links ICU4X's
//! instead.

use std::ffi::c_void;
use std::sync::OnceLock;

/// Gives Parley Windows' word boundaries, once per process, and starts loading them (`warm`).
pub(crate) fn install() {
    parley_engine::dictionary::set_dictionary_segmenter(dictionary_words);
    warm();
}

/// Loads `icu.dll` and opens a Thai and a Japanese word iterator on a thread of its own, once:
/// the first open of a dictionary language costs ~12,000 µs (ICU reads its dictionary), the next
/// ~20 µs. A layout that needs them before this is done waits for it rather than loading twice.
fn warm() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    let _ = std::thread::Builder::new().name("rune-icu".into()).spawn(|| {
        let _ = word_bounds("\u{0E20}\u{0E32}\u{0E29}\u{0E32}\u{0E44}\u{0E17}\u{0E22}", "th");
        let _ = word_bounds("\u{65E5}\u{672C}\u{8A9E}", "ja");
    });
}

/// The boundaries strictly inside `text` (byte offsets, ascending) for one dictionary language.
fn dictionary_words(text: &str, language: &str, out: &mut Vec<usize>) {
    let Some(bounds) = word_bounds(text, language) else { return };
    // ICU's word iterator ends a `word` where UAX #14 never breaks a line: after marks with no
    // letter under them (a run that starts U+0E31 U+0308; LineBreakTest 17.0.0, case 14571), and
    // before a non-starter or a combining mark (LB9, LB21: U+17D6 U+17D6; 16.0.0, cases 16121
    // and 16123). ICU4X's line segmenter keeps both together, and so does this.
    out.extend(bounds.into_iter().filter(|&b| b > 0 && b < text.len() && !marks_only(&text[..b]) && !no_break_before(&text[b..])));
}

fn no_break_before(rest: &str) -> bool {
    use icu_properties::{CodePointMapData, props::LineBreak};
    rest.chars().next().is_some_and(|c| matches!(CodePointMapData::<LineBreak>::new().get(c), LineBreak::Nonstarter | LineBreak::CombiningMark | LineBreak::ZWJ))
}

fn marks_only(s: &str) -> bool {
    use icu_properties::{CodePointMapData, props::GeneralCategory};
    let category = CodePointMapData::<GeneralCategory>::new();
    s.chars().all(|c| matches!(category.get(c), GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark))
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
