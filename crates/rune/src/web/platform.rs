use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/web/platform.js")]
extern "C" {
    #[wasm_bindgen(js_name = writeClipboard)]
    pub(crate) fn write_clipboard(text: &str, html: &str, png: &[u8]) -> js_sys::Promise;
    #[wasm_bindgen(js_name = readClipboard)]
    pub(crate) fn read_clipboard(rich: bool, image: bool, max_image: u32) -> js_sys::Promise;
    #[wasm_bindgen(catch, js_name = openLink)]
    pub(crate) fn open_link(url: &str) -> Result<(), JsValue>;
    pub(super) fn viewport(canvas: &web_sys::HtmlCanvasElement, callback: &js_sys::Function, embedded: bool) -> js_sys::Function;
}
