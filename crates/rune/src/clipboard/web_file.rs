//! [`WebFile`]: a browser `File` a paste or drop offered the focused field on the web.
use std::sync::Arc;

/// The most files one paste or drop offers; the rest are left out.
pub const MAX_WEB_FILES: usize = 32;

/// A browser `File` that a paste or drop offered the focused field on the web, from
/// [`ClipboardContent::web_files`](crate::ClipboardContent::web_files). It holds the file's
/// name, type and size, not its bytes and never a path: nothing is read until the app calls
/// `read`, and the page keeps the handle for as long as any clone lives, letting go of it when
/// the last one drops. There is no cap on handles an app keeps: the app bounds what it holds.
/// Native platforms offer none (their pastes and drops carry paths).
///
/// Debug output shows only its type and size.
#[derive(Clone)]
pub struct WebFile(Arc<Inner>);
struct Inner { id: u32, name: String, mime: String, size: u64 }

impl WebFile {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn new(id: u32, name: String, mime: String, size: u64) -> Self {
        Self(Arc::new(Inner { id, name, mime, size }))
    }
    /// The file's name as the browser gives it, without any folder.
    pub fn name(&self) -> &str { &self.0.name }
    /// Its MIME type as the browser guessed it, empty when unknown.
    pub fn mime(&self) -> &str { &self.0.mime }
    /// Its size in bytes.
    pub fn size(&self) -> u64 { self.0.size }

    /// Reads the file's bytes, or `None` when it is larger than `max` bytes or the browser
    /// can't read it (the file was moved or deleted since it was dropped, say).
    #[cfg(target_arch = "wasm32")]
    pub async fn read(&self, max: usize) -> Option<Vec<u8>> {
        use wasm_bindgen::JsCast;
        let promise = js::read_web_file(self.0.id, u32::try_from(max).unwrap_or(u32::MAX));
        let bytes = wasm_bindgen_futures::JsFuture::from(promise).await.ok()?.dyn_into::<js_sys::Uint8Array>().ok()?;
        (bytes.length() as usize <= max).then(|| bytes.to_vec())
    }
    /// The browser's `File` object, to upload or hand to page JavaScript without copying it
    /// into wasm memory. The page holds it while this `WebFile` (or a clone) lives.
    #[cfg(target_arch = "wasm32")]
    pub fn js_file(&self) -> Option<wasm_bindgen::JsValue> {
        Some(js::web_file(self.0.id)).filter(|f| !f.is_undefined())
    }
}
impl PartialEq for WebFile {
    fn eq(&self, other: &Self) -> bool { self.0.id == other.0.id }
}
impl Eq for WebFile {}
impl std::fmt::Debug for WebFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebFile").field("mime", &self.0.mime).field("size", &self.0.size).finish()
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        #[cfg(target_arch = "wasm32")]
        js::release_web_file(self.id);
    }
}

#[cfg(target_arch = "wasm32")]
mod js {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(module = "/src/platform/web/bridge.js")]
    extern "C" {
        #[wasm_bindgen(js_name = claimWebFile)]
        pub(super) fn claim_web_file(id: u32) -> bool;
        #[wasm_bindgen(js_name = webFile)]
        pub(super) fn web_file(id: u32) -> JsValue;
        #[wasm_bindgen(js_name = releaseWebFile)]
        pub(super) fn release_web_file(id: u32);
        #[wasm_bindgen(js_name = readWebFile)]
        pub(super) fn read_web_file(id: u32, max: u32) -> js_sys::Promise;
    }
}

/// The handles a page's paste or drop event listed: `[{ id, name, type, size }]`, at most
/// [`MAX_WEB_FILES`].
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(crate) fn from_event(files: &serde_json::Value) -> Vec<WebFile> {
    let Some(files) = files.as_array() else { return Vec::new() };
    files.iter().take(MAX_WEB_FILES).filter_map(|f| {
        let id = u32::try_from(f["id"].as_u64()?).ok()?;
        // Take the page's handle now, while the event is handled: one Rust doesn't claim is
        // let go of as soon as the event returns.
        #[cfg(target_arch = "wasm32")]
        if !js::claim_web_file(id) { return None; }
        let text = |k: &str| f[k].as_str().unwrap_or_default().to_owned();
        Some(WebFile::new(id, text("name"), text("type"), f["size"].as_u64().unwrap_or(0)))
    }).collect()
}
