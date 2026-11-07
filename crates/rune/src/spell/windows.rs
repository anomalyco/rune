//! Windows' spell checking API: one checker per chosen dictionary (`super::languages`), made on
//! each thread that checks and made again when the choice changes.
use std::cell::RefCell;

use windows::Win32::Globalization::{
    CORRECTIVE_ACTION_GET_SUGGESTIONS, CORRECTIVE_ACTION_REPLACE, GetLocaleInfoEx, GetUserPreferredUILanguages, ISpellChecker,
    ISpellCheckerFactory, ISpellingError, LOCALE_SENGLISHDISPLAYNAME, MUI_LANGUAGE_NAME, SpellCheckerFactory,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, IEnumString};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// A thread's checkers, for the generation they were made in.
struct Checkers {
    generation: u64,
    list: Vec<ISpellChecker>,
}

thread_local! {
    // Never released: releasing the checkers' out-of-process provider as the thread exits can
    // wait on it forever, holding the process open. A changed choice makes new ones while the
    // thread runs, where releasing the old ones is safe.
    static FACTORY: std::mem::ManuallyDrop<Option<ISpellCheckerFactory>> = std::mem::ManuallyDrop::new(factory());
    static CHECKERS: std::mem::ManuallyDrop<RefCell<Checkers>> =
        std::mem::ManuallyDrop::new(RefCell::new(Checkers { generation: u64::MAX, list: Vec::new() }));
}

fn factory() -> Option<ISpellCheckerFactory> {
    // SAFETY: COM calls on this thread's apartment.
    unsafe {
        // The window's thread already has an apartment; a headless one may not.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()
    }
}

/// Runs `f` over this thread's checkers for the chosen dictionaries.
fn with_checkers<T>(f: impl FnOnce(&[ISpellChecker]) -> T) -> T {
    CHECKERS.with(|checkers| {
        let mut checkers = checkers.borrow_mut();
        let generation = super::generation();
        if checkers.generation != generation {
            let list = FACTORY.with(|factory| {
                let Some(factory) = &**factory else { return Vec::new() };
                // SAFETY: the factory is this thread's, and each tag outlives its call.
                super::languages().into_iter().filter_map(|tag| unsafe { factory.CreateSpellChecker(&HSTRING::from(tag)).ok() }).collect()
            });
            *checkers = Checkers { generation, list };
        }
        f(&checkers.list)
    })
}

/// The dictionaries Windows has: those of the languages added in Settings › Time & language.
pub(super) fn available() -> Vec<String> {
    FACTORY.with(|factory| {
        let Some(factory) = &**factory else { return Vec::new() };
        // SAFETY: the factory is this thread's; each string `Next` hands over is freed once read.
        unsafe { factory.SupportedLanguages().map(|list| strings(&list)).unwrap_or_default() }
    })
}

/// The user's display languages, most preferred first.
pub(super) fn preferred() -> Vec<String> {
    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: the first call sizes the buffer the second fills; the buffer outlives both.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut len).is_err() || len == 0 {
            return Vec::new();
        }
        let mut buffer = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, Some(PWSTR(buffer.as_mut_ptr())), &mut len).is_err() {
            return Vec::new();
        }
        buffer.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// The language's English name, "French (France)".
pub(super) fn english_name(tag: &str) -> String {
    let name = HSTRING::from(tag);
    let mut buffer = [0u16; 128];
    // SAFETY: `name` and `buffer` outlive the call, which writes at most the buffer's length.
    let len = unsafe { GetLocaleInfoEx(PCWSTR(name.as_ptr()), LOCALE_SENGLISHDISPLAYNAME, Some(&mut buffer)) };
    if len <= 1 {
        return tag.to_owned();
    }
    String::from_utf16_lossy(&buffer[..len as usize - 1])
}

/// Reads a COM string list to its end.
///
/// # Safety
/// `list` must be a live enumerator on this thread.
unsafe fn strings(list: &IEnumString) -> Vec<String> {
    let mut found = Vec::new();
    loop {
        let mut one = [PWSTR::null()];
        let mut fetched = 0u32;
        // SAFETY: per this function's contract; each string handed over is freed once read.
        unsafe {
            if list.Next(&mut one, Some(&mut fetched)).is_err() || fetched == 0 || one[0].is_null() {
                break;
            }
            found.extend(one[0].to_string().ok());
            CoTaskMemFree(Some(one[0].0 as *const _));
        }
    }
    found
}

pub(super) fn misspelled(text: &str) -> Vec<(usize, usize)> {
    with_checkers(|checkers| {
        // A word is misspelled when every dictionary says so, as Chromium's multilingual checking.
        let mut lists = checkers.iter().map(|checker| check(checker, text));
        let Some(first) = lists.next() else { return Vec::new() };
        lists.fold(first, |found, other| found.into_iter().filter(|range| other.contains(range)).collect())
    })
}

/// One dictionary's misspelled words in `text`, as byte ranges.
fn check(checker: &ISpellChecker, text: &str) -> Vec<(usize, usize)> {
    // Each UTF-16 unit's byte offset, and the text's end.
    let mut bytes = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        bytes.extend(std::iter::repeat_n(i, c.len_utf16()));
    }
    bytes.push(text.len());
    let mut found = Vec::new();
    // SAFETY: the checker is this thread's, and the text outlives the call.
    unsafe {
        let Ok(errors) = checker.Check(&HSTRING::from(text)) else { return found };
        loop {
            let mut error: Option<ISpellingError> = None;
            if errors.Next(&mut error).is_err() {
                break;
            }
            let Some(error) = error else { break };
            // A repeated word ("the the") is a delete: Chromium leaves it unmarked.
            let action = error.CorrectiveAction().unwrap_or_default();
            if action != CORRECTIVE_ACTION_GET_SUGGESTIONS && action != CORRECTIVE_ACTION_REPLACE {
                continue;
            }
            let (Ok(start), Ok(len)) = (error.StartIndex(), error.Length()) else { continue };
            let (a, b) = (start as usize, start as usize + len as usize);
            if let (Some(&a), Some(&b)) = (bytes.get(a), bytes.get(b))
                && a < b
            {
                found.push((a, b));
            }
        }
    }
    found
}

pub(super) fn suggest(word: &str) -> Vec<String> {
    with_checkers(|checkers| {
        let mut found: Vec<String> = Vec::new();
        for checker in checkers {
            // SAFETY: as in `check`; the list is read to its end on this thread.
            let more = unsafe { checker.Suggest(&HSTRING::from(word)).map(|list| strings(&list)).unwrap_or_default() };
            for s in more {
                if !found.contains(&s) {
                    found.push(s);
                }
            }
        }
        found
    })
}

pub(super) fn learn(word: &str) {
    with_checkers(|checkers| {
        for checker in checkers {
            // SAFETY: the checker is this thread's, and the word outlives the call.
            let _ = unsafe { checker.Add(&HSTRING::from(word)) };
        }
    });
}
