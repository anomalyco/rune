//! Windows' spell checking API: a checker for the user's language (`GetUserDefaultLocaleName`),
//! else US English, made once per thread.
use windows::Win32::Globalization::{
    CORRECTIVE_ACTION_GET_SUGGESTIONS, CORRECTIVE_ACTION_REPLACE, GetUserDefaultLocaleName, ISpellChecker,
    ISpellCheckerFactory, ISpellingError, SpellCheckerFactory,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
use windows::core::{HSTRING, PWSTR};

thread_local! {
    // Never released: releasing the checker's out-of-process provider as the thread exits can
    // wait on it forever, holding the process open.
    static CHECKER: std::mem::ManuallyDrop<Option<ISpellChecker>> = std::mem::ManuallyDrop::new(create());
}

fn create() -> Option<ISpellChecker> {
    // SAFETY: COM calls on this thread's apartment, with buffers that outlive them.
    unsafe {
        // The window's thread already has an apartment; a headless one may not.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let factory: ISpellCheckerFactory = CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut name = [0u16; 85];
        let len = GetUserDefaultLocaleName(&mut name);
        let user = (len > 1).then(|| String::from_utf16_lossy(&name[..len as usize - 1]));
        let base = user.as_deref().and_then(|u| u.split('-').next()).map(str::to_owned);
        [user, base, Some("en-US".to_owned())].into_iter().flatten().find_map(|tag| {
            let tag = HSTRING::from(tag);
            if !factory.IsSupported(&tag).is_ok_and(|s| s.as_bool()) {
                return None;
            }
            factory.CreateSpellChecker(&tag).ok()
        })
    }
}

pub(super) fn misspelled(text: &str) -> Vec<(usize, usize)> {
    CHECKER.with(|checker| {
        let Some(checker) = &**checker else { return Vec::new() };
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
    })
}

pub(super) fn suggest(word: &str) -> Vec<String> {
    CHECKER.with(|checker| {
        let Some(checker) = &**checker else { return Vec::new() };
        let mut found = Vec::new();
        // SAFETY: as in `misspelled`; each string `Next` hands over is freed once read.
        unsafe {
            let Ok(list) = checker.Suggest(&HSTRING::from(word)) else { return found };
            loop {
                let mut one = [PWSTR::null()];
                let mut fetched = 0u32;
                if list.Next(&mut one, Some(&mut fetched)).is_err() || fetched == 0 || one[0].is_null() {
                    break;
                }
                found.extend(one[0].to_string().ok());
                CoTaskMemFree(Some(one[0].0 as *const _));
            }
        }
        found
    })
}

pub(super) fn learn(word: &str) {
    CHECKER.with(|checker| {
        if let Some(checker) = &**checker {
            // SAFETY: the checker is this thread's, and the word outlives the call.
            let _ = unsafe { checker.Add(&HSTRING::from(word)) };
        }
    });
}
