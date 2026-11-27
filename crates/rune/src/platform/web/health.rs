//! The page's watch on its GPU device (health.js and WebGL2 `wgpu-core` / DOM context events):
//! uncaptured errors logged as `gpu::health::ErrorLog` says, with the adapter they happened on,
//! and the device's or WebGL2 context's loss and restore. The error scopes the renderer checks
//! its lazily built pipelines in (`render::breaker`) are the JS device's on WebGPU (avoiding
//! wgpu's panic on a `GPUInternalError`) and `wgpu-core`'s synchronous scopes on WebGL2.

use std::{cell::RefCell, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};
use wasm_bindgen::{JsCast, prelude::*};
use crate::gpu::health::{ErrorLog, Note};

#[wasm_bindgen(module = "/src/platform/web/health.js")]
extern "C" {
    #[wasm_bindgen(js_name = hasNavigatorGpu)]
    fn has_navigator_gpu_js() -> bool;
    #[wasm_bindgen(js_name = gpuSummary)]
    fn gpu_summary(device: &JsValue) -> String;
    #[wasm_bindgen(js_name = isSoftwareDevice)]
    fn is_software_device_js(device: &JsValue) -> bool;
    #[wasm_bindgen(js_name = dispatchGpuFallback)]
    fn dispatch_gpu_fallback_js(canvas: &web_sys::HtmlCanvasElement, stage: &str, reason: &str);
    #[wasm_bindgen(js_name = watchGpu)]
    fn watch_gpu(device: &JsValue, on_error: &js_sys::Function, on_lost: &js_sys::Function, terminal: bool);
    #[wasm_bindgen(js_name = restoreGpu)]
    fn restore_gpu(device: &JsValue);
    #[wasm_bindgen(js_name = pushScopes)]
    fn push_scopes(device: &JsValue);
    /// Resolves to the first error the scopes caught, as `kind: message`, or null.
    #[wasm_bindgen(js_name = popScopes)]
    fn pop_scopes(device: &JsValue) -> js_sys::Promise;
    #[wasm_bindgen(js_name = watchWebglContext)]
    fn watch_webgl_context(canvas: &web_sys::HtmlCanvasElement, prevent_default: bool, on_lost: &js_sys::Function, on_restored: &js_sys::Function) -> JsValue;
    #[wasm_bindgen(js_name = webglWatchLost)]
    fn webgl_watch_lost(handle: &JsValue) -> bool;
    #[wasm_bindgen(js_name = disposeWebglWatch)]
    fn dispose_webgl_watch(handle: &JsValue);
}

/// Whether `navigator.gpu` is exposed on this page.
pub(super) fn has_navigator_gpu() -> bool {
    has_navigator_gpu_js()
}

/// The JS `GPUDevice` under `device`; none on WebGL2.
fn js_device(device: &wgpu::Device) -> Option<&JsValue> {
    device.as_webgpu().map(|d| d.as_ref())
}

/// The adapter under `device`, as the browser describes it.
pub(crate) fn summary(device: &wgpu::Device) -> String {
    js_device(device).map_or_else(|| "WebGL2".into(), gpu_summary)
}

/// Whether `device`'s WebGPU `GPUDevice.adapterInfo` identifies a software/fallback adapter.
pub(super) fn is_software_device(device: &wgpu::Device) -> bool {
    js_device(device).is_some_and(is_software_device_js)
}

/// Dispatches a bubbling `runegpufallback` DOM `CustomEvent` on `canvas`.
pub(super) fn dispatch_gpu_fallback(canvas: &web_sys::HtmlCanvasElement, stage: &str, reason: &str) {
    dispatch_gpu_fallback_js(canvas, stage, reason);
}

/// Validation, out-of-memory and internal error scopes on the JS device.
pub(crate) struct Scopes<'d>(&'d JsValue);

impl<'d> Scopes<'d> {
    /// None on WebGL2, which uses `wgpu-core` scopes instead of a JS `GPUDevice`.
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

fn log_error_line(log: &mut ErrorLog, adapter: &str, kind: &str, message: &str) {
    let line = match log.note(message) {
        Note::First => format!("rune: GPU error on {adapter}: {kind}: {message}"),
        Note::New => format!("rune: GPU error: {kind}: {message}"),
        Note::Mute => format!("rune: GPU error: {kind}: {message}\n(further GPU errors on this device aren't logged)"),
        Note::Quiet => return,
    };
    web_sys::console::error_1(&line.into());
}

type WebGlLostSlot = Arc<PageOnly<RefCell<Option<Box<dyn FnOnce(String)>>>>>;

/// Restores Rune's web device-lost and uncaptured-error callbacks after an embedded renderer
/// overwrites single-slot handlers on the shared `wgpu::Device`.
#[derive(Clone)]
pub(crate) struct WebRestore(WebRestoreKind);

#[derive(Clone)]
enum WebRestoreKind {
    WebGpu,
    WebGl2 {
        log: Arc<Mutex<ErrorLog>>,
        gpu_lost: Arc<AtomicBool>,
        on_lost: WebGlLostSlot,
        terminal: bool,
    },
}

impl WebRestore {
    pub(crate) fn restore(&self, device: &wgpu::Device) {
        match &self.0 {
            WebRestoreKind::WebGpu => restore_webgpu(device),
            WebRestoreKind::WebGl2 { log, gpu_lost, on_lost, terminal } => {
                install_webgl2_callbacks(device, Arc::clone(log), Arc::clone(gpu_lost), Arc::clone(on_lost), *terminal);
            }
        }
    }
}

/// Clears any foreign JS `device.onuncapturederror` handler on a WebGPU device without touching
/// Rune's `addEventListener('uncapturederror', ...)` or `device.lost.then(...)` watches.
pub(crate) fn restore_webgpu(device: &wgpu::Device) {
    if let Some(js) = js_device(device) {
        restore_gpu(js);
    }
}

fn install_webgl2_callbacks(
    device: &wgpu::Device,
    log: Arc<Mutex<ErrorLog>>,
    gpu_lost: Arc<AtomicBool>,
    on_lost: WebGlLostSlot,
    terminal: bool,
) {
    let on_lost_err = Arc::clone(&on_lost);
    let gpu_lost_err = Arc::clone(&gpu_lost);
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        let kind = match &error {
            wgpu::Error::Validation { .. } => "GPUValidationError",
            wgpu::Error::OutOfMemory { .. } => "GPUOutOfMemoryError",
            wgpu::Error::Internal { .. } => "GPUInternalError",
        };
        let message = error.to_string();
        log_error_line(&mut log.lock().unwrap_or_else(|e| e.into_inner()), "WebGL2", kind, &message);
        if terminal {
            gpu_lost_err.store(true, Ordering::Release);
            let PageOnly(slot) = &*on_lost_err;
            if let Some(lost) = slot.borrow_mut().take() {
                lost(format!("uncaptured GPU error ({kind}): {message}, on WebGL2"));
            }
        }
    }));
    device.set_device_lost_callback(move |reason, message| {
        gpu_lost.store(true, Ordering::Release);
        if !terminal && reason == wgpu::DeviceLostReason::Destroyed { return; }
        let PageOnly(slot) = &*on_lost;
        if let Some(lost) = slot.borrow_mut().take() {
            lost(format!("{reason:?}: {message}, on WebGL2"));
        }
    });
}

/// Logs `device`'s uncaptured errors, and calls `lost` with why and on what adapter if the
/// browser loses it. Handles both WebGPU (through `health.js`) and WebGL2 (through `wgpu-core`
/// callbacks logged to `console.error`), returning the state needed to restore them later.
pub(super) fn watch(
    device: &wgpu::Device,
    gpu_lost: Arc<AtomicBool>,
    terminal: bool,
    lost: impl FnOnce(String) + 'static,
) -> WebRestore {
    if let Some(js) = js_device(device) {
        let adapter = gpu_summary(js);
        let log = RefCell::new(ErrorLog::default());
        let lost_slot: std::rc::Rc<RefCell<Option<Box<dyn FnOnce(String)>>>> = std::rc::Rc::new(RefCell::new(Some(Box::new(lost))));
        let on_error = {
            let adapter = adapter.clone();
            let lost_slot = std::rc::Rc::clone(&lost_slot);
            let gpu_lost = Arc::clone(&gpu_lost);
            Closure::<dyn FnMut(String, String)>::new(move |kind: String, message: String| {
                log_error_line(&mut log.borrow_mut(), &adapter, &kind, &message);
                if terminal {
                    gpu_lost.store(true, Ordering::Release);
                    if let Some(lost) = lost_slot.borrow_mut().take() {
                        lost(format!("uncaptured GPU error ({kind}): {message}, on {adapter}"));
                    }
                }
            })
        };
        let on_lost = Closure::once_into_js(move |reason: String, message: String| {
            gpu_lost.store(true, Ordering::Release);
            if let Some(lost) = lost_slot.borrow_mut().take() {
                lost(format!("{reason}: {message}, on {adapter}"));
            }
        });
        watch_gpu(js, on_error.as_ref().unchecked_ref(), on_lost.unchecked_ref(), terminal);
        on_error.forget();
        return WebRestore(WebRestoreKind::WebGpu);
    }
    let log = Arc::new(Mutex::new(ErrorLog::default()));
    let on_lost: WebGlLostSlot = Arc::new(PageOnly(RefCell::new(Some(Box::new(lost)))));
    install_webgl2_callbacks(device, Arc::clone(&log), Arc::clone(&gpu_lost), Arc::clone(&on_lost), terminal);
    WebRestore(WebRestoreKind::WebGl2 { log, gpu_lost, on_lost, terminal })
}

/// Active `webglcontextlost` / `webglcontextrestored` DOM listeners on a canvas; removed on drop.
pub(super) struct WebglWatch {
    handle: JsValue,
    _on_lost: Closure<dyn FnMut(String)>,
    _on_restored: Closure<dyn FnMut()>,
}

impl WebglWatch {
    pub(super) fn new(
        canvas: &web_sys::HtmlCanvasElement,
        prevent_default: bool,
        on_lost: impl FnMut(String) + 'static,
        on_restored: impl FnMut() + 'static,
    ) -> Self {
        let on_lost = Closure::<dyn FnMut(String)>::new(on_lost);
        let on_restored = Closure::<dyn FnMut()>::new(on_restored);
        let handle = watch_webgl_context(
            canvas,
            prevent_default,
            on_lost.as_ref().unchecked_ref(),
            on_restored.as_ref().unchecked_ref(),
        );
        Self { handle, _on_lost: on_lost, _on_restored: on_restored }
    }

    pub(super) fn is_lost(&self) -> bool {
        webgl_watch_lost(&self.handle)
    }
}

impl Drop for WebglWatch {
    fn drop(&mut self) {
        dispose_webgl_watch(&self.handle);
    }
}

struct PageOnly<T>(T);
// SAFETY: wasm32 without threads: there is one thread, and every callback runs on it.
#[allow(unsafe_code)]
unsafe impl<T> Send for PageOnly<T> {}
// SAFETY: wasm32 without threads: there is one thread, and every callback runs on it.
#[allow(unsafe_code)]
unsafe impl<T> Sync for PageOnly<T> {}
