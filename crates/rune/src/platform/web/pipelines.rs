//! Async pipeline preparation on the web:
//! - WebGPU (`pipelines.js`): a stand-in renderer is made while the device's pipeline creation is
//!   diverted to `createRenderPipelineAsync`; the real one, made once those are in, finds them
//!   compiled.
//! - WebGL2 (`KHR_parallel_shader_compile`): enables deferred linking on `wgpu_hal::gles::Device`,
//!   constructs the real `Renderer` once (because `wgpu-hal::gles` keys `program_cache` on
//!   monotonic `ShaderId`), turns deferred linking back off, polls `COMPLETION_STATUS_KHR` across
//!   browser turns through `gl_links::GlLinkState`, and hands the verified `Renderer` to `gpu_side`.

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
    let renderer = Renderer::try_new(device, format).ok()?;
    let pipelines = wasm_bindgen_futures::JsFuture::from(settle(&diverted)).await.ok()?;
    Some(Warmed { _renderer: renderer, _pipelines: pipelines })
}

/// Result of preparing a WebGL2 `Renderer` with `KHR_parallel_shader_compile`.
#[cfg(feature = "webgl")]
pub(super) enum WebGlWarm {
    /// Extension unavailable: caller should create the renderer synchronously in `gpu_side`.
    Unsupported,
    /// Deferred links completed and verified; hand this renderer directly into `gpu_side`.
    Ready(Renderer),
    /// Context lost, last-resort shader rejected, or compilation timed out.
    Failed(String),
    /// Page quit while deferred links were polling.
    Aborted,
}

#[cfg(feature = "webgl")]
#[allow(unsafe_code)]
fn supports_parallel_compile(device: &wgpu::Device) -> bool {
    // SAFETY: `as_hal` borrows the underlying GLES HAL device without modifying ownership.
    unsafe {
        device
            .as_hal::<wgpu::hal::api::Gles>()
            .is_some_and(|hal| hal.supports_parallel_shader_compile())
    }
}

#[cfg(feature = "webgl")]
#[allow(unsafe_code)]
fn set_defer_links(device: &wgpu::Device, defer: bool) {
    // SAFETY: toggles the atomic deferred-link flag on the GLES HAL device.
    unsafe {
        if let Some(hal) = device.as_hal::<wgpu::hal::api::Gles>() {
            hal.set_defer_program_links(defer);
        }
    }
}

#[cfg(feature = "webgl")]
#[allow(unsafe_code)]
fn pending_links(device: &wgpu::Device) -> usize {
    // SAFETY: reads the count of pending deferred links without issuing GL queries.
    unsafe {
        device
            .as_hal::<wgpu::hal::api::Gles>()
            .map_or(0, |hal| hal.pending_deferred_links())
    }
}

#[cfg(feature = "webgl")]
#[allow(unsafe_code)]
fn poll_links(device: &wgpu::Device) -> super::super::gl_links::LinkPollOutcome {
    // SAFETY: non-blocking `COMPLETION_STATUS_KHR` poll on the current thread's GLES device.
    let status = unsafe {
        device
            .as_hal::<wgpu::hal::api::Gles>()
            .map(|hal| hal.poll_deferred_links())
            .unwrap_or_default()
    };
    super::super::gl_links::LinkPollOutcome {
        pending: status.pending,
        error: status.error,
        context_lost: status.context_lost,
    }
}

#[cfg(feature = "webgl")]
fn build_deferred(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    classic: bool,
) -> Result<(Renderer, usize), String> {
    struct DeferGuard<'a>(&'a wgpu::Device);
    impl Drop for DeferGuard<'_> {
        fn drop(&mut self) {
            set_defer_links(self.0, false);
        }
    }
    set_defer_links(device, true);
    let guard = DeferGuard(device);
    let built = if classic {
        Renderer::try_new_classic(device, format)
    } else {
        Renderer::try_new(device, format)
    };
    drop(guard);
    match built {
        Ok(renderer) => Ok((renderer, pending_links(device))),
        Err(err) => Err(err),
    }
}

/// Compiles and links the real WebGL2 `Renderer` for `format` without blocking the browser main
/// thread when `KHR_parallel_shader_compile` is supported.
#[cfg(feature = "webgl")]
pub(super) async fn warm_webgl2(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    quitting: &std::sync::atomic::AtomicBool,
) -> WebGlWarm {
    use std::sync::atomic::Ordering;
    use crate::platform::gl_links::{GlLinkEffect, GlLinkEvent, GlLinkState, LinkPhase};
    use crate::time::Instant;

    if !supports_parallel_compile(device) {
        return WebGlWarm::Unsupported;
    }
    // Separate shader submission from `request_device` so the browser can paint and handle input.
    super::yield_turn().await;
    if quitting.load(Ordering::Acquire) {
        return WebGlWarm::Aborted;
    }

    let mut seq = crate::machine::Seq::default();
    let mut state = GlLinkState::default();
    let mut renderer: Option<Renderer> = None;

    let first_event = match build_deferred(device, format, false) {
        Ok((built, pending)) => {
            let phase = if built.is_windfoil() {
                LinkPhase::Windfoil
            } else {
                LinkPhase::Classic
            };
            renderer = Some(built);
            GlLinkEvent::Submitted { phase, pending, now: Instant::now() }
        }
        Err(reason) => GlLinkEvent::SubmitFailed {
            phase: LinkPhase::Classic,
            reason,
        },
    };
    let mut effects = state.step(&mut seq, first_event);

    loop {
        let mut next_event = None;
        for effect in effects {
            match effect {
                GlLinkEffect::SchedulePoll { attempt, delay } => {
                    super::sleep_turn(delay).await;
                    if quitting.load(Ordering::Acquire) {
                        next_event = Some(GlLinkEvent::Quit);
                        break;
                    }
                    let outcome = poll_links(device);
                    next_event = Some(GlLinkEvent::PollDue {
                        attempt,
                        now: Instant::now(),
                        outcome,
                    });
                }
                GlLinkEffect::BuildClassic => {
                    web_sys::console::warn_1(
                        &"rune: WebGL2 rejected Windfoil shader; falling back to Classic".into(),
                    );
                    renderer = None;
                    if quitting.load(Ordering::Acquire) {
                        next_event = Some(GlLinkEvent::Quit);
                        break;
                    }
                    next_event = Some(match build_deferred(device, format, true) {
                        Ok((built, pending)) => {
                            renderer = Some(built);
                            GlLinkEvent::Submitted {
                                phase: LinkPhase::Classic,
                                pending,
                                now: Instant::now(),
                            }
                        }
                        Err(reason) => GlLinkEvent::SubmitFailed {
                            phase: LinkPhase::Classic,
                            reason,
                        },
                    });
                }
                GlLinkEffect::Deliver => {
                    crate::marks::mark("web.gpu.warmed");
                    return WebGlWarm::Ready(renderer.expect("deferred WebGL2 renderer"));
                }
                GlLinkEffect::Fail(reason) => {
                    drop(renderer);
                    return WebGlWarm::Failed(reason);
                }
                GlLinkEffect::Abort => {
                    drop(renderer);
                    return WebGlWarm::Aborted;
                }
            }
        }
        let Some(event) = next_event else {
            drop(renderer);
            return WebGlWarm::Aborted;
        };
        effects = state.step(&mut seq, event);
    }
}
