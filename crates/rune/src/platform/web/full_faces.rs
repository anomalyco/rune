//! Fetches the faces a page adds once its first frame is up (see `text::full_faces`): the full
//! Inter faces when the app sets text in the embedded Inter, and `App::font_url_later`'s fonts.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "embedded-fonts")]
use wasm_bindgen::prelude::*;

#[cfg(feature = "embedded-fonts")]
#[wasm_bindgen]
extern "C" {
    /// The URL of the app's `app.js`, which a web build serves the faces beside.
    #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
    static MODULE_URL: String;
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// Starts the fetches, once per page; `wake` runs as each set arrives. The full Inter faces
/// are one set, as a page that fetches only some would set text in two cuts; the app's late
/// fonts are another, each skipped alone if it fails.
/// `share` adds the late fonts to the page's own too (`page_fonts`).
pub(super) fn fetch(late: Vec<String>, share: bool, wake: impl Fn() + Clone + 'static) {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(feature = "embedded-fonts")]
    if crate::text::full_faces::embedded_inter() {
        let base = MODULE_URL.with(String::clone);
        let urls = crate::text::full_faces::FILES.iter().map(|file| super::platform::resolve_url(file, &base)).collect();
        load(urls, true, false, wake.clone());
    }
    let wake: Rc<dyn Fn()> = Rc::new(wake);
    let later = LATER.with_borrow_mut(|later| {
        later.ready = Some((share, wake.clone()));
        later.seen.extend(late.iter().cloned());
        std::mem::take(&mut later.asked)
    });
    let late: Vec<String> = late.into_iter().chain(later).collect();
    if !late.is_empty() {
        load(late, false, share, move || wake());
    }
}

/// [`now`]'s fonts, and, once the first frame is up, how [`fetch`] loads them.
#[derive(Default)]
struct Later {
    /// Asked for before the first frame: they go with the app's late fonts.
    asked: Vec<String>,
    /// Every URL asked for, so each loads once.
    seen: Vec<String>,
    ready: Option<(bool, Rc<dyn Fn()>)>,
}

thread_local! {
    static LATER: RefCell<Later> = RefCell::default();
}

/// Fetches `url`'s font now, as `App::font_url_later`'s fonts are fetched: added to the app's
/// fonts when it arrives (every window takes it up on its next frame), reported and skipped if
/// it fails. Asked before the first frame, it waits for it. Each URL loads once.
pub(super) fn now(url: &str) {
    let ready = LATER.with_borrow_mut(|later| {
        if later.seen.iter().any(|u| u == url) {
            return None;
        }
        later.seen.push(url.to_owned());
        if later.ready.is_none() {
            later.asked.push(url.to_owned());
        }
        later.ready.clone()
    });
    if let Some((share, wake)) = ready {
        load(vec![url.to_owned()], false, share, move || wake());
    }
}
fn load(urls: Vec<String>, all_or_none: bool, share: bool, wake: impl FnOnce() + 'static) {
    let files: Vec<_> = urls.into_iter().map(|url| {
        let bytes = super::platform::fetch_bytes(&url);
        (url, bytes)
    }).collect();
    wasm_bindgen_futures::spawn_local(async move {
        let mut faces = Vec::new();
        for (url, bytes) in files {
            match wasm_bindgen_futures::JsFuture::from(bytes).await {
                Ok(bytes) => faces.push(js_sys::Uint8Array::new(&bytes).to_vec()),
                Err(e) if all_or_none => {
                    let note = format!("rune: {url} did not load, so text outside Latin, Greek and Cyrillic draws in fallback fonts:");
                    web_sys::console::warn_2(&note.into(), &e);
                    return;
                }
                Err(e) => web_sys::console::warn_2(&format!("rune: font {url} did not load:").into(), &e),
            }
        }
        if faces.is_empty() {
            return;
        }
        if share { super::page_fonts::share(faces.iter().map(Vec::as_slice)); }
        crate::text::full_faces::arrive(faces);
        wake();
    });
}
