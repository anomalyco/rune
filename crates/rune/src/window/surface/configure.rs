//! Configuring a window's or overlay's surface, and saying so when it can't.

use crate::render::Gpu;

/// Why `Surface::configure` failed, separating transient resource pressure and device loss from
/// a persistent driver/surface refusal so the adapter fallback chain knows whether to move on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigureErrorKind {
    DeviceLost,
    OutOfMemory,
    DirectCompositionUnavailable,
    Refused,
}

impl From<ConfigureErrorKind> for rune_gpu::fallback::Failure {
    fn from(kind: ConfigureErrorKind) -> Self {
        match kind {
            ConfigureErrorKind::DeviceLost => Self::DeviceLost,
            ConfigureErrorKind::OutOfMemory => Self::OutOfMemory,
            ConfigureErrorKind::DirectCompositionUnavailable => {
                Self::DirectCompositionUnavailable
            }
            ConfigureErrorKind::Refused => Self::SurfaceRefused,
        }
    }
}

/// Structured error from [`configure_checked`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigureError {
    pub kind: ConfigureErrorKind,
    pub message: String,
}

/// Classifies the first captured wgpu error scope result, prioritizing out-of-memory over
/// DirectComposition unavailability and generic internal/validation refusals.
pub(crate) fn classify_scope_error(
    memory: Option<wgpu::Error>,
    internal: Option<wgpu::Error>,
    validation: Option<wgpu::Error>,
    dcomp_unavailable: bool,
) -> Option<(ConfigureErrorKind, wgpu::Error)> {
    if let Some(error) = memory {
        return Some((ConfigureErrorKind::OutOfMemory, error));
    }
    if let Some(error) = internal.or(validation) {
        let kind = match error {
            wgpu::Error::OutOfMemory { .. } => ConfigureErrorKind::OutOfMemory,
            wgpu::Error::Internal { .. } | wgpu::Error::Validation { .. }
                if dcomp_unavailable =>
            {
                ConfigureErrorKind::DirectCompositionUnavailable
            }
            wgpu::Error::Internal { .. } | wgpu::Error::Validation { .. } => {
                ConfigureErrorKind::Refused
            }
        };
        return Some((kind, error));
    }
    None
}

/// Configures `surface`, returning a classified [`ConfigureError`] instead of panicking in wgpu's
/// default handler, so a surface the driver refuses (a broken DirectComposition path in a VM, or
/// an overlay's XWayland window on NVIDIA, hex#979) can trigger adapter fallback or cost only its
/// owner. The message names the configuration for logs.
///
/// A lost device fails here even when wgpu reports nothing: it drops the `DeviceLost` that
/// `Surface::configure` returns (wgpu-30.0.1 `handle_error_inner`, "will be surfaced via
/// callback") without recording the surface's error sink, so the surface has no presentation
/// and the next `get_current_texture` panics ("Surface is not configured for presentation").
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn configure_checked(surface: &wgpu::Surface, gpu: &Gpu, config: &wgpu::SurfaceConfiguration) -> Result<(), ConfigureError> {
    if gpu.is_lost() {
        return Err(ConfigureError { kind: ConfigureErrorKind::DeviceLost, message: lost_device(config) });
    }
    let validation = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let memory = gpu.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = gpu.device.push_error_scope(wgpu::ErrorFilter::Internal);
    // No other thread of Rune's submits while wgpu waits for the queue to empty: a submission
    // that lands inside the wait makes it refuse the configuration (`rune_gpu::configure`).
    rune_gpu::configure::configuring(|| surface.configure(&gpu.device, config));
    // `Device::destroy` only marks the device invalid: its lost callback runs when the device
    // next maintains itself, at the soonest a poll. Without this, a device destroyed before it
    // reported itself configures nothing while `is_lost` still reads false, and the drawable
    // its surface is asked for panics.
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
    let [internal, memory, validation] = [internal.pop(), memory.pop(), validation.pop()].map(pollster::block_on);
    if gpu.is_lost() {
        return Err(ConfigureError { kind: ConfigureErrorKind::DeviceLost, message: lost_device(config) });
    }
    let has_error = memory.is_some() || internal.is_some() || validation.is_some();
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    let dcomp_unavailable = has_error
        && unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }
            .is_some_and(|hal| hal.dcomp_unavailable());
    #[cfg(not(target_os = "windows"))]
    let dcomp_unavailable = { let _ = has_error; false };
    match classify_scope_error(memory, internal, validation, dcomp_unavailable) {
        None => { configured(surface); Ok(()) }
        Some((kind, error)) => {
            let text = if kind == ConfigureErrorKind::DirectCompositionUnavailable {
                format!("{error}: DirectComposition unavailable")
            } else {
                error.to_string()
            };
            Err(ConfigureError {
                kind,
                message: format_error(&text, config.width, config.height, config.format, config.alpha_mode, config.present_mode),
            })
        }
    }
}

/// Configures `surface`, returning the formatted error string on failure.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn configure(surface: &wgpu::Surface, gpu: &Gpu, config: &wgpu::SurfaceConfiguration) -> Result<(), String> {
    configure_checked(surface, gpu, config).map_err(|error| error.message)
}

/// Each surface's last successful configuration, numbered across all of them: a configure makes a
/// new swap chain (new back buffers), so a frame that drew partly into the last one must draw
/// whole into this one (`Damage`). Wherever the configure came from: a resize, a blend switch,
/// a frame-latency change, or `acquire` retrying an outdated surface inside a frame.
#[cfg(not(target_arch = "wasm32"))]
static CONFIGURED: std::sync::Mutex<Vec<(usize, u64)>> = std::sync::Mutex::new(Vec::new());
#[cfg(not(target_arch = "wasm32"))]
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(not(target_arch = "wasm32"))]
fn configured(surface: &wgpu::Surface) {
    let key = std::ptr::from_ref(surface) as usize;
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut all = CONFIGURED.lock().unwrap();
    match all.iter_mut().find(|(k, _)| *k == key) {
        Some(e) => e.1 = n,
        None => all.push((key, n)),
    }
    // Surfaces long gone: their numbers are never read again.
    if all.len() > 64 { all.remove(0); }
}

/// The number of `surface`'s last successful configuration (0 before any). A surface dropped and
/// another made at its address configures before it draws, so it never reads the old number.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn generation(surface: &wgpu::Surface) -> u64 {
    let key = std::ptr::from_ref(surface) as usize;
    CONFIGURED.lock().unwrap().iter().find(|(k, _)| *k == key).map_or(0, |e| e.1)
}

/// A configuration that never reached the surface because its device is gone.
#[cfg(not(target_arch = "wasm32"))]
fn lost_device(config: &wgpu::SurfaceConfiguration) -> String {
    format!("the GPU device was lost ({}x{}, {:?}, {:?}, {:?})", config.width, config.height, config.format, config.alpha_mode, config.present_mode)
}

/// wgpu's error text on one line, with the configuration it refused.
fn format_error(text: &str, width: u32, height: u32, format: wgpu::TextureFormat, alpha: wgpu::CompositeAlphaMode, present: wgpu::PresentMode) -> String {
    let cause = text.lines().map(str::trim).filter(|line| !line.is_empty() && *line != "Caused by:").collect::<Vec<_>>().join(": ");
    format!("{cause} ({width}x{height}, {format:?}, {alpha:?}, {present:?})")
}

#[cfg(target_arch = "wasm32")]
fn poll_scope_now<F: std::future::Future<Output = Option<wgpu::Error>>>(fut: F) -> Option<wgpu::Error> {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match std::pin::pin!(fut).poll(&mut cx) {
        std::task::Poll::Ready(err) => err,
        std::task::Poll::Pending => None,
    }
}

/// Configures `surface` on WebGL2, whose `wgpu-core` error scopes answer at once. A WebGPU
/// surface goes through `configure_then`: wgpu's own scopes there panic on a `GPUInternalError`.
#[cfg(target_arch = "wasm32")]
pub(crate) fn configure_checked(surface: &wgpu::Surface, gpu: &Gpu, config: &wgpu::SurfaceConfiguration) -> Result<(), ConfigureError> {
    let validation = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let memory = gpu.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = gpu.device.push_error_scope(wgpu::ErrorFilter::Internal);
    surface.configure(&gpu.device, config);
    match classify_scope_error(poll_scope_now(memory.pop()), poll_scope_now(internal.pop()), poll_scope_now(validation.pop()), false) {
        None => Ok(()),
        Some((kind, error)) => Err(ConfigureError {
            kind,
            message: format_error(&error.to_string(), config.width, config.height, config.format, config.alpha_mode, config.present_mode),
        }),
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn configure(surface: &wgpu::Surface, gpu: &Gpu, config: &wgpu::SurfaceConfiguration) -> Result<(), String> {
    configure_checked(surface, gpu, config).map_err(|error| error.message)
}

/// Configures `surface` on the web and delivers the outcome to `on_result`: asynchronously via
/// the JS `GPUDevice`'s error scopes on WebGPU, or synchronously through `wgpu-core`'s scopes on
/// WebGL2.
#[cfg(target_arch = "wasm32")]
pub(crate) fn configure_then(
    surface: &wgpu::Surface,
    gpu: &Gpu,
    config: &wgpu::SurfaceConfiguration,
    on_result: impl FnOnce(Result<(), String>) + 'static,
) {
    if let Some(scopes) = crate::platform::web::health::Scopes::push(&gpu.device) {
        let (w, h, fmt, alpha, present) = (config.width, config.height, config.format, config.alpha_mode, config.present_mode);
        surface.configure(&gpu.device, config);
        scopes.pop(move |error| {
            let res = match error {
                None => Ok(()),
                Some(err) => Err(format_error(&err, w, h, fmt, alpha, present)),
            };
            on_result(res);
        });
        return;
    }
    on_result(configure(surface, gpu, config));
}
