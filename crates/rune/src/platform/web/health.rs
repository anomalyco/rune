//! The page's watch on its WebGPU device (health.js): uncaptured errors logged as
//! `gpu::health::ErrorLog` says, with the adapter they happened on, and the device's loss.
//! The error scopes the renderer checks its lazily built pipelines in (`render::breaker`) are
//! the JS device's too. Nothing here goes through wgpu's error conversion, which panics on a
//! `GPUInternalError`.

use std::cell::RefCell;
use wasm_bindgen::{JsCast, prelude::*};
use crate::gpu::health::{ErrorLog, Note};

#[wasm_bindgen(module = "/src/platform/web/health.js")]
extern "C" {
    #[wasm_bindgen(js_name = gpuSummary)]
    fn gpu_summary(device: &JsValue) -> String;
    #[wasm_bindgen(js_name = watchGpu)]
    fn watch_gpu(device: &JsValue, on_error: &js_sys::Function, on_lost: &js_sys::Function);
    #[wasm_bindgen(js_name = pushScopes)]
    fn push_scopes(device: &JsValue);
    /// Resolves to the first error the scopes caught, as `kind: message`, or null.
    #[wasm_bindgen(js_name = popScopes)]
    fn pop_scopes(device: &JsValue) -> js_sys::Promise;
}

/// The JS `GPUDevice` under `device`; none on WebGL2.
fn js_device(device: &wgpu::Device) -> Option<&JsValue> {
    device.as_webgpu().map(|d| d.as_ref())
}

/// The adapter under `device`, as the browser describes it.
pub(crate) fn summary(device: &wgpu::Device) -> String {
    js_device(device).map_or_else(|| "WebGL2".into(), gpu_summary)
}

/// Validation, out-of-memory and internal error scopes on the JS device.
pub(crate) struct Scopes<'d>(&'d JsValue);

impl<'d> Scopes<'d> {
    /// None on WebGL2, which has no error scopes.
    pub(crate) fn push(device: &'d wgpu::Device) -> Option<Self> {
        let js = js_device(device)?;
        push_scopes(js);
        Some(Self(js))
    }

    /// Pops the scopes now, and calls `answer` with what they caught when the browser says.
    pub(crate) fn pop(self, answer: impl FnOnce(Option<String>) + 'static) {
        let caught = wasm_bindgen_futures::JsFuture::from(pop_scopes(self.0));
        wasm_bindgen_futures::spawn_local(async move {
            let error = match caught.await {
                Ok(value) => value.as_string(),
                Err(e) => Some(format!("{e:?}")),
            };
            answer(error);
        });
    }
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
