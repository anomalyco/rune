//! Fetches the faces a page adds once its first frame is up (see `text::full_faces`): the full
//! Inter faces when the app sets text in the embedded Inter, and `App::font_url_later`'s fonts.

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
    if !late.is_empty() {
        load(late, false, share, wake);
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
