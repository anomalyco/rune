//! Error-scope checks and async browser state machine for Rune's primary (uber) shader and
//! pipeline.
//!
//! Native devices and synchronous WebGL2 (`wgpu-core`) answer synchronously inside
//! `Renderer::try_new`, so a rejected last-resort shader returns `ShaderError` immediately and
//! drives the adapter fallback chain (`rune_gpu::fallback::Failure::ShaderRejected`). On WebGL2
//! contexts with `KHR_parallel_shader_compile`, `platform::web::pipelines::warm_webgl2` defers GL
//! program linking across browser turns via `platform::gl_links::GlLinkState`: WGSL/Naga errors
//! still fail synchronously here, while GL compile/link rejections are reported when links finish
//! (falling back from Windfoil to Classic, then to CPU or error display on Classic rejection).
//!
//! Browser WebGPU error scopes (`GPUDevice.popErrorScope`) resolve asynchronously. Rather than
//! blocking launch, `UberCheck` tracks the in-flight check as an explicit state machine:
//! `Renderer::checking_gpu` stays true while `Checking` (holding CPU-first hand-over), and once
//! the browser answers, `Failed` surfaces the error to the web host so the page never stays
//! blank with only a console log.

use std::sync::{Arc, Mutex};

use crate::machine::{Attempt, Effects, Seq};

/// A browser's answer to an async primary-shader error-scope check: `None` while pending,
/// `Some(None)` when accepted, `Some(Some(why))` when rejected.
pub(crate) type Answer = Arc<Mutex<Option<Option<String>>>>;

/// Wake handle invoked once a browser WebGPU primary-shader check resolves.
#[cfg(target_arch = "wasm32")]
pub(crate) type WakeSlot = Arc<Mutex<Option<crate::Waker>>>;

/// Why creating the last-resort uber shader or pipeline failed, separating transient out-of-memory
/// pressure from a driver/validation rejection so the adapter fallback chain knows whether to move
/// on to another adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShaderErrorKind {
    /// Validation or internal driver error compiling the shader or linking the pipeline.
    Rejected,
    /// Transient out-of-memory error (`wgpu::Error::OutOfMemory`).
    OutOfMemory,
}

impl From<ShaderErrorKind> for rune_gpu::fallback::Failure {
    fn from(kind: ShaderErrorKind) -> Self {
        match kind {
            ShaderErrorKind::Rejected => Self::ShaderRejected,
            ShaderErrorKind::OutOfMemory => Self::OutOfMemory,
        }
    }
}

/// Structured failure from building the renderer's last-resort shader or pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShaderError {
    pub kind: ShaderErrorKind,
    pub message: String,
}

impl std::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<ShaderError> for String {
    fn from(error: ShaderError) -> Self {
        error.message
    }
}

/// Classifies the first captured wgpu error scope result, prioritizing out-of-memory over
/// internal or validation shader rejections.
pub(crate) fn classify_scope_error(
    memory: Option<wgpu::Error>,
    internal: Option<wgpu::Error>,
    validation: Option<wgpu::Error>,
) -> Option<ShaderError> {
    if let Some(error) = memory {
        return Some(ShaderError {
            kind: ShaderErrorKind::OutOfMemory,
            message: format!("the GPU ran out of memory building Rune's shader: {error}"),
        });
    }
    if let Some(error) = internal.or(validation) {
        let kind = match error {
            wgpu::Error::OutOfMemory { .. } => ShaderErrorKind::OutOfMemory,
            wgpu::Error::Internal { .. } | wgpu::Error::Validation { .. } => {
                ShaderErrorKind::Rejected
            }
        };
        return Some(ShaderError {
            kind,
            message: format!("the GPU rejected Rune's shader: {error}"),
        });
    }
    None
}

/// Runs `build` inside synchronous error scopes (validation, out-of-memory, internal) on native
/// and WebGL2 (`wgpu-core`, no JS `GPUDevice`) devices, returning `Err(ShaderError)` if the
/// device rejects the shader or pipeline. On browser WebGPU, wgpu's own error scopes panic on
/// `GPUInternalError`, so the outer JS error scopes (`platform::web::health::Scopes`) check the
/// build asynchronously instead.
pub(super) fn checked_sync<T>(
    device: &wgpu::Device,
    build: impl FnOnce() -> T,
) -> Result<T, ShaderError> {
    #[cfg(target_arch = "wasm32")]
    if device.as_webgpu().is_some() {
        return Ok(build());
    }
    let (built, errors) = rune_gpu::health::with_error_scopes(device, build);
    match classify_scope_error(errors.memory, errors.internal, errors.validation) {
        None => Ok(built),
        Some(error) => Err(error),
    }
}

/// Health of the renderer's primary uber shader and pipeline.
#[derive(Default)]
pub(crate) enum UberCheck {
    /// Accepted synchronously (native / WebGL2), or the browser answered with no error.
    #[default]
    Ready,
    /// Waiting for browser WebGPU's `popErrorScope()` promise for `attempt`.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    Checking { attempt: Attempt, answer: Answer },
    /// Browser WebGPU rejected the primary uber shader or pipeline.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    Failed { reason: String },
}

#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum UberCheckEvent {
    /// Started an async browser error-scope check for the primary shader/pipeline.
    Started(Answer),
    /// The browser answered check `attempt` (`None` = accepted, `Some` = rejected).
    Answered { attempt: Attempt, error: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum UberCheckEffect {
    /// Surface the shader rejection to the web host.
    Fail(String),
}

impl UberCheck {
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub(crate) fn step(
        &mut self,
        seq: &mut Seq,
        event: UberCheckEvent,
    ) -> Effects<UberCheckEffect> {
        match (&*self, event) {
            (Self::Failed { .. }, _) => Effects::none(),
            (_, UberCheckEvent::Started(answer)) => {
                *self = Self::Checking { attempt: seq.next(), answer };
                Effects::none()
            }
            (Self::Checking { attempt, .. }, UberCheckEvent::Answered { attempt: answered, error })
                if *attempt == answered =>
            {
                match error {
                    None => {
                        *self = Self::Ready;
                        Effects::none()
                    }
                    Some(reason) => {
                        *self = Self::Failed { reason: reason.clone() };
                        Effects::one(UberCheckEffect::Fail(reason))
                    }
                }
            }
            _ => Effects::none(),
        }
    }

    pub(crate) fn checking(&self) -> bool {
        matches!(self, Self::Checking { .. })
    }

    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub(crate) fn answered(&self) -> Option<UberCheckEvent> {
        let Self::Checking { attempt, answer } = self else { return None };
        let error = answer.lock().unwrap_or_else(|e| e.into_inner()).take()?;
        Some(UberCheckEvent::Answered { attempt: *attempt, error })
    }

    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub(crate) fn failed_reason(&self) -> Option<&str> {
        match self {
            Self::Failed { reason } => Some(reason),
            Self::Ready | Self::Checking { .. } => None,
        }
    }
}

#[cfg(test)]
pub(super) const INVALID_WGSL: &str =
    "@vertex fn vs() -> @builtin(position) vec4<f32> { return missing_symbol; }";

#[cfg(test)]
std::thread_local! {
    static REFUSE_CLASSIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static REFUSE_WINDFOIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(super) fn refuse_classic() -> bool {
    REFUSE_CLASSIC.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(super) fn refuse_windfoil() -> bool {
    REFUSE_WINDFOIL.with(std::cell::Cell::get)
}

/// Tests: makes Classic uber shader creation compile invalid WGSL inside the error scope, so a
/// real GPU rejects it instead of compiling the built-in shader.
#[cfg(test)]
pub(crate) fn with_classic_shader_refused<R>(refuse: bool, f: impl FnOnce() -> R) -> R {
    struct Guard(bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            REFUSE_CLASSIC.with(|c| c.set(self.0));
        }
    }
    let _guard = Guard(REFUSE_CLASSIC.with(|c| c.replace(refuse)));
    f()
}

/// Tests: forces both Windfoil and Classic uber shader creation to fail so `Renderer::try_new`
/// exercises the full fallback-to-Classic and Classic-rejection path on any platform.
#[cfg(test)]
pub(crate) fn with_uber_shaders_refused<R>(refuse: bool, f: impl FnOnce() -> R) -> R {
    struct Guard(bool, bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            REFUSE_WINDFOIL.with(|c| c.set(self.0));
            REFUSE_CLASSIC.with(|c| c.set(self.1));
        }
    }
    let prev_windfoil = REFUSE_WINDFOIL.with(|c| c.replace(refuse));
    let prev_classic = REFUSE_CLASSIC.with(|c| c.replace(refuse));
    let _guard = Guard(prev_windfoil, prev_classic);
    f()
}

#[cfg(test)]
#[path = "shader_check_tests.rs"]
mod tests;
