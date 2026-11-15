//! The renderer's pipelines compiled ahead, off the browser's GPU thread (pipelines.js), where
//! the browser compiles pipelines asynchronously. A stand-in renderer is made while the device's
//! pipeline creation is diverted to `createRenderPipelineAsync`; the real one, made once those
//! are in, finds them compiled.

use wasm_bindgen::prelude::*;
use crate::render::Renderer;

#[wasm_bindgen(module = "/src/platform/web/pipelines.js")]
extern "C" {
    #[wasm_bindgen(js_name = compilesAsync)]
    fn compiles_async() -> bool;
    #[wasm_bindgen(js_name = canDivert)]
    fn can_divert(device: &JsValue) -> bool;
    fn divert(device: &JsValue) -> JsValue;
    fn settle(diverted: &JsValue) -> js_sys::Promise;
}

/// Kept until the real renderer is made: the stand-in renderer and the compiled pipelines, which
/// the browser's cache holds only while they live.
pub(super) struct Warmed {
    _renderer: Renderer,
    _pipelines: JsValue,
}

/// Whether this browser's WebGPU compiles pipelines asynchronously, so asking for the GPU early
/// doesn't hold the page's frames.
pub(super) fn browser_compiles_async() -> bool { compiles_async() }

/// A renderer for `format` made with the device's pipeline creation diverted, once the browser
/// has compiled every pipeline it asked for. None where it can't (WebGL2, no async compile).
pub(super) async fn warm(device: &wgpu::Device, format: wgpu::TextureFormat) -> Option<Warmed> {
    let js: &JsValue = device.as_webgpu()?.as_ref();
    if !can_divert(js) { return None; }
    let diverted = divert(js);
    let renderer = Renderer::new(device, format);
    let pipelines = wasm_bindgen_futures::JsFuture::from(settle(&diverted)).await.ok()?;
    Some(Warmed { _renderer: renderer, _pipelines: pipelines })
}
