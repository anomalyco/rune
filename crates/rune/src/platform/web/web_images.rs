//! Synchronizes `Runtime::web_images` to the browser's `[data-rune-web-images]` DOM layer on
//! every drawn frame without triggering JS work when nothing changed.

use wasm_bindgen::{JsValue, prelude::*};

#[wasm_bindgen(module = "/src/platform/web/bridge.js")]
extern "C" {
    #[wasm_bindgen(js_name = syncWebImages)]
    fn sync_web_images_js(handle: &JsValue, json: &str);
}

#[derive(Default)]
pub(super) struct WebImagesSync {
    last: Vec<crate::web_image::WebImageDraw>,
}

impl WebImagesSync {
    pub(super) fn sync(&mut self, handle: &JsValue, draws: &[crate::web_image::WebImageDraw]) {
        if self.last == draws {
            return;
        }
        self.last.clear();
        self.last.extend_from_slice(draws);
        let json = crate::web_image::to_json(draws);
        sync_web_images_js(handle, &json);
    }
}
