//! The page's watch on its WebGPU device (health.js): uncaptured errors logged as
//! `gpu::health::ErrorLog` says, with the adapter they happened on, and the device's loss.
//! Nothing here goes through wgpu's error conversion, which panics on a `GPUInternalError`.

use std::cell::RefCell;
use wasm_bindgen::{JsCast, prelude::*};
use crate::gpu::health::{ErrorLog, Note};

#[wasm_bindgen(module = "/src/platform/web/health.js")]
extern "C" {
    #[wasm_bindgen(js_name = gpuSummary)]
    fn gpu_summary(device: &JsValue) -> String;
    #[wasm_bindgen(js_name = watchGpu)]
    fn watch_gpu(device: &JsValue, on_error: &js_sys::Function, on_lost: &js_sys::Function);
}

/// The JS `GPUDevice` under `device`; none on WebGL2.
fn js_device(device: &wgpu::Device) -> Option<&JsValue> {
    device.as_webgpu().map(|d| d.as_ref())
}

/// Logs `device`'s uncaptured errors, and calls `lost` with why and on what adapter if the
/// browser loses it. Does nothing on WebGL2.
pub(super) fn watch(device: &wgpu::Device, lost: impl FnOnce(String) + 'static) {
    let Some(js) = js_device(device) else { return };
    let adapter = gpu_summary(js);
    let log = RefCell::new(ErrorLog::default());
    let on_error = {
        let adapter = adapter.clone();
        Closure::<dyn FnMut(String, String)>::new(move |kind: String, message: String| {
            let line = match log.borrow_mut().note(&message) {
                Note::First => format!("rune: GPU error on {adapter}: {kind}: {message}"),
                Note::New => format!("rune: GPU error: {kind}: {message}"),
                Note::Mute => format!("rune: GPU error: {kind}: {message}\n(further GPU errors on this device aren't logged)"),
                Note::Quiet => return,
            };
            web_sys::console::error_1(&line.into());
        })
    };
    let on_lost = Closure::once_into_js(move |reason: String, message: String| {
        lost(format!("{reason}: {message}, on {adapter}"));
    });
    watch_gpu(js, on_error.as_ref().unchecked_ref(), on_lost.unchecked_ref());
    on_error.forget();
}
