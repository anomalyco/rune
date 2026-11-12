//! A page's spell checker. No browser lets a page ask its own checker, so the page checks with
//! Hunspell dictionaries (spellbook, in the module) it fetches the first time a field asks:
//! `spell/<tag>.aff` and `spell/<tag>.dic` beside the page, for the tags the app named
//! ([`super::set_web_dictionaries`]). Until a dictionary is in, nothing is marked; when it
//! arrives the window checks again. A word is misspelled when every dictionary loaded says so.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

pub(super) const OFF_THREAD: bool = false;

enum Dict {
    Loading,
    Ready(Box<spellbook::Dictionary>),
    Failed,
}

#[derive(Default)]
struct State {
    /// The tags the page has dictionaries for.
    tags: Vec<String>,
    dicts: Vec<(String, Dict)>,
    /// Windows that asked while a dictionary was on its way.
    waiting: Vec<Weak<crate::live::Wake>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

pub(super) fn set_tags(tags: Vec<String>) {
    STATE.with_borrow_mut(|s| s.tags = tags);
}

/// Whether every dictionary checking text now is in; one not asked for yet is fetched, and the
/// window asking (`wake`) is woken once it arrives.
pub(super) fn prepare(wake: Option<&Arc<crate::live::Wake>>) -> bool {
    let tags = super::languages();
    STATE.with_borrow_mut(|s| {
        let mut ready = true;
        for tag in &tags {
            match s.dicts.iter().find(|(t, _)| t == tag) {
                Some((_, Dict::Loading)) => ready = false,
                Some(_) => {}
                None => {
                    s.dicts.push((tag.clone(), Dict::Loading));
                    load(tag.clone());
                    ready = false;
                }
            }
        }
        if !ready && let Some(w) = wake { s.waiting.push(Arc::downgrade(w)); }
        ready
    })
}

/// Fetches `tag`'s dictionary; once it's in, every text is checked again.
fn load(tag: String) {
    let aff = crate::platform::web::platform::fetch_bytes(&format!("spell/{tag}.aff"));
    let dic = crate::platform::web::platform::fetch_bytes(&format!("spell/{tag}.dic"));
    wasm_bindgen_futures::spawn_local(async move {
        let text = |v: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| v.ok().and_then(|b| String::from_utf8(js_sys::Uint8Array::new(&b).to_vec()).ok());
        let aff = text(wasm_bindgen_futures::JsFuture::from(aff).await);
        let dic = text(wasm_bindgen_futures::JsFuture::from(dic).await);
        let dict = match (aff, dic) {
            (Some(aff), Some(dic)) => match spellbook::Dictionary::new(&aff, &dic) {
                Ok(d) => Dict::Ready(Box::new(d)),
                Err(e) => { web_sys::console::warn_1(&format!("rune: the {tag} dictionary didn't parse: {e}").into()); Dict::Failed }
            },
            _ => { web_sys::console::warn_1(&format!("rune: the {tag} dictionary didn't load").into()); Dict::Failed }
        };
        let waiting = STATE.with_borrow_mut(|s| {
            if let Some((_, slot)) = s.dicts.iter_mut().find(|(t, _)| *t == tag) { *slot = dict; }
            std::mem::take(&mut s.waiting)
        });
        super::GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        for w in waiting.iter().filter_map(Weak::upgrade) { w.notify(); }
    });
}

/// Calls `f` with the dictionaries loaded, if any.
fn with_ready<R>(f: impl FnOnce(&[&spellbook::Dictionary]) -> R) -> Option<R> {
    STATE.with_borrow(|s| {
        let ready: Vec<&spellbook::Dictionary> = s.dicts.iter().filter_map(|(_, d)| match d { Dict::Ready(d) => Some(&**d), _ => None }).collect();
        (!ready.is_empty()).then(|| f(&ready))
    })
}

pub(super) fn misspelled(text: &str) -> Vec<(usize, usize)> {
    use unicode_segmentation::UnicodeSegmentation;
    with_ready(|dicts| {
        let mut out = Vec::new();
        for (chunk_at, chunk) in text.split_whitespace().map(|c| (c.as_ptr() as usize - text.as_ptr() as usize, c)) {
            // Paths, URLs, addresses and code are not prose.
            if chunk.contains(['/', '@', '`', '_', '\\', '=', '<', '>', '{', '}']) || chunk.contains("::") { continue; }
            for (at, word) in chunk.split_word_bound_indices() {
                if !checked(word) { continue; }
                if dicts.iter().all(|d| !d.check(word)) { out.push((chunk_at + at, chunk_at + at + word.len())); }
            }
        }
        out
    }).unwrap_or_default()
}

/// A word worth checking: letters (an apostrophe inside), at least two, no digits, and no
/// capital after its first letter (an acronym or an identifier).
fn checked(word: &str) -> bool {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    letters >= 2
        && word.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '\u{2019}')
        && !word.chars().skip(1).any(char::is_uppercase)
}

pub(super) fn suggest(word: &str) -> Vec<String> {
    with_ready(|dicts| {
        let mut out = Vec::new();
        for d in dicts {
            let mut found = Vec::new();
            d.suggest(word, &mut found);
            for s in found {
                if !out.contains(&s) { out.push(s); }
            }
        }
        out
    }).unwrap_or_default()
}

pub(super) fn learn(word: &str) {
    STATE.with_borrow_mut(|s| {
        for (_, d) in &mut s.dicts {
            if let Dict::Ready(d) = d { let _ = d.add(word); }
        }
    });
}

pub(super) fn available() -> Vec<String> {
    STATE.with_borrow(|s| s.tags.clone())
}

/// The browser's preferred languages (`navigator.languages`).
pub(super) fn preferred() -> Vec<String> {
    let navigator = js_sys::Reflect::get(&js_sys::global(), &"navigator".into()).ok();
    let languages = navigator.and_then(|n| js_sys::Reflect::get(&n, &"languages".into()).ok());
    languages.filter(js_sys::Array::is_array).map(|l| js_sys::Array::from(&l).iter().filter_map(|v| v.as_string()).collect()).unwrap_or_default()
}

pub(super) fn english_name(tag: &str) -> String { tag.to_owned() }
pub(super) fn name_in(_: &str, _: &str) -> Option<String> { None }
pub(super) fn sort_in<T>(_: &mut [(String, T)], _: &str) -> bool { false }
