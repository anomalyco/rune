//! Word boundaries in Thai, Lao, Khmer, Burmese, Chinese and Japanese from Windows' own ICU
//! (`icu.dll`, Windows 10 1903 and later), for selection only: a double-click, word movement and
//! the words Parley marks in those runs (`parley_engine::dictionary`). Line breaks never come
//! from here: Thai, Lao, Khmer and Burmese break lines with ICU4X's baked line dictionaries
//! (`parley_engine`'s `line-dictionaries`), and Chinese and Japanese break lines without one. So
//! the exe leaves out ICU4X's word dictionaries (Chinese and Japanese words above all), and
//! nothing a frame draws ever waits for `icu.dll`.
//!
//! `icu.dll` and its dictionaries load on a thread of their own once the first frame is out (`warm`: about
//! 12,000 µs of that thread and 800,000 B). Until they're ready, and without `icu.dll` (Windows
//! Server Core, Windows 10 before 1903), words fall back to the rule-based segmenter's (per
//! character in Chinese and Japanese, the whole run in Thai): never a wait. The
//! `segmenter-dictionaries` feature links ICU4X's word dictionaries instead.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once `warm` has loaded `icu.dll` and opened each dictionary language once.
static READY: AtomicBool = AtomicBool::new(false);

/// Gives Parley Windows' word boundaries, once per process, and starts loading them (`warm`).
pub(crate) fn install() {
    parley_engine::dictionary::set_dictionary_segmenter(dictionary_words);
    warm();
}

/// Wakes `warm`'s thread: the first frame is on screen (`marks::presented`).
pub(crate) fn first_frame_presented() {
    let (shown, wake) = &*FIRST_FRAME;
    *shown.lock().unwrap_or_else(|e| e.into_inner()) = true;
    wake.notify_all();
}

static FIRST_FRAME: std::sync::LazyLock<(std::sync::Mutex<bool>, std::sync::Condvar)> = std::sync::LazyLock::new(Default::default);

/// Loads `icu.dll` and opens a word iterator in each dictionary language on a thread of its own,
/// once, after the first frame is presented (or a second on, in a run that presents none): loading
/// it during a launch held the loader lock and the disk the window's own DLLs wanted (a cold
/// launch 40,000-100,000 µs later). The first open of a language costs up to ~12,000 µs (ICU
/// reads its dictionary), later ones ~20 µs. Nothing waits for it.
fn warm() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) { return; }
    let _ = std::thread::Builder::new().name("rune-icu".into()).spawn(|| {
        let (shown, wake) = &*FIRST_FRAME;
        let shown = shown.lock().unwrap_or_else(|e| e.into_inner());
        drop(wake.wait_timeout_while(shown, std::time::Duration::from_secs(1), |shown| !*shown));
        for (sample, language) in [("\u{0E20}\u{0E32}\u{0E29}\u{0E32}\u{0E44}\u{0E17}\u{0E22}", "th"), ("\u{0E9E}\u{0EB2}\u{0EAA}\u{0EB2}", "lo"),
            ("\u{1797}\u{17B6}\u{179F}\u{17B6}", "km"), ("\u{1018}\u{102C}\u{101E}\u{102C}", "my"), ("\u{65E5}\u{672C}\u{8A9E}", "ja")] {
            let _ = word_bounds(sample, language);
        }
        READY.store(true, Ordering::Release);
    });
}

/// The word boundaries strictly inside `text` (byte offsets, ascending) for one dictionary
/// language, once `icu.dll` is ready; none before, so Parley's rule-based words stand.
fn dictionary_words(text: &str, language: &str, out: &mut Vec<usize>) {
    if !READY.load(Ordering::Acquire) { return; }
    let Some(bounds) = word_bounds(text, language) else { return };
    // ICU's word iterator ends a `word` where UAX #14 never breaks: after marks with no letter
    // under them (U+0E31 U+0308; LineBreakTest 17.0.0, case 14571), and before a non-starter or a
    // combining mark (LB9, LB21: U+17D6 U+17D6; 16.0.0, cases 16121 and 16123).
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
/// with `text.len()` (as ICU4X's segmenter gives them, past the leading 0). Until `icu.dll` is
/// ready, or without it, the rule-based segmenter's.
pub(crate) fn words(text: &str) -> Vec<usize> {
    let icu = READY.load(Ordering::Acquire).then(|| word_bounds(text, "")).flatten();
    let Some(bounds) = icu else {
        use icu_segmenter::{WordSegmenter, options::WordBreakInvariantOptions};
        return WordSegmenter::new_for_non_complex_scripts(WordBreakInvariantOptions::default()).segment_str(text).filter(|&b| b > 0).collect();
    };
    let mut out: Vec<usize> = bounds.into_iter().filter(|&b| b > 0).collect();
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
