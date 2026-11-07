//! The platform's spell checker for text fields that ask for it (`El::spellcheck`): Windows'
//! spell checking API (`ISpellChecker`), AppKit's `NSSpellChecker` on macOS, none elsewhere.
//! A field's misspelled words get a red marker under them and their suggestions in its menu.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[cfg(target_os = "windows")]
#[path = "spell/windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "spell/macos.rs"]
mod platform;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod platform {
    pub(super) fn misspelled(_: &str) -> Vec<(usize, usize)> { Vec::new() }
    pub(super) fn suggest(_: &str) -> Vec<String> { Vec::new() }
    pub(super) fn learn(_: &str) {}
}

/// The most suggestions a field's menu offers, as Chromium's.
const MAX_SUGGESTIONS: usize = 5;

thread_local! {
    /// Each checked text's misspelled byte ranges, by the text's hash.
    static CHECKED: RefCell<HashMap<u64, Rc<Vec<(usize, usize)>>>> = RefCell::new(HashMap::new());
}

/// The byte ranges of `text`'s misspelled words, checked once per text.
pub(crate) fn misspelled(text: &str) -> Rc<Vec<(usize, usize)>> {
    if text.trim().is_empty() {
        return Rc::default();
    }
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        text.hash(&mut h);
        h.finish()
    };
    CHECKED.with_borrow_mut(|checked| {
        if let Some(found) = checked.get(&key) {
            return found.clone();
        }
        // Typing checks a new text per key: keep the cache to the latest few hundred.
        if checked.len() > 256 {
            checked.clear();
        }
        let found = Rc::new(platform::misspelled(text));
        checked.insert(key, found.clone());
        found
    })
}

/// The misspelled word of `text` at byte `at` (in it or at its end).
pub(crate) fn misspelled_at(text: &str, at: usize) -> Option<(usize, usize)> {
    misspelled(text).iter().copied().find(|&(a, b)| a <= at && at <= b)
}

/// The checker's suggestions for misspelled `word`, best first.
pub(crate) fn suggest(word: &str) -> Vec<String> {
    let mut found = platform::suggest(word);
    found.retain(|s| s != word);
    found.truncate(MAX_SUGGESTIONS);
    found
}

/// Adds `word` to the user's dictionary, so it's no longer marked anywhere.
pub(crate) fn learn(word: &str) {
    platform::learn(word);
    CHECKED.with_borrow_mut(HashMap::clear);
}
