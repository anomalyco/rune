//! GPU adapter selection, backend dispatch, device error logging, analytic GPU springs and
//! Rune's native Apple Metal backend. On macOS and iOS, `metal-backend` (on by default) provides
//! the native Metal backend (`metal`) alongside wgpu's: a Mac app draws with the native one
//! unless `GpuBackend::Wgpu` or `RUNE_GPU_BACKEND=wgpu` selects wgpu's, and an iOS app draws with
//! wgpu's unless it chooses `GpuBackend::Metal`. Apps normally reach this crate through `rune`.

pub mod backend;
pub mod configure;
pub mod fallback;
pub mod health;
#[cfg(any(test, all(target_vendor = "apple", feature = "metal-backend")))]
mod lazy_init;
pub mod memory;
#[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
pub mod metal;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod naga_override_tests;
#[cfg(not(target_arch = "wasm32"))]
pub mod pick;
pub mod springs;

pub use backend::GpuBackend;
pub use backend::describe as describe_gpu;
pub use memory::GpuMemoryPolicy;

/// Opt-in environment switch for GPU-less test hosts (such as a headless Linux container without
/// Vulkan): when set to `"1"`, `"true"`, `"TRUE"`, or `"yes"`, GPU test helpers skip with one
/// clear line (or use wgpu's noop backend when `test-gpu-noop` is enabled for non-pixel tests)
/// instead of failing when no adapter is found. Unset by default so a host with a GPU fails loudly
/// if its adapter disappears.
#[doc(hidden)]
pub const ALLOW_NO_GPU_ENV: &str = "RUNE_ALLOW_NO_GPU";

/// Whether the caller explicitly opted into skipping GPU tests when no adapter is available.
#[doc(hidden)]
pub fn allow_no_gpu_tests() -> bool {
    rune_core::env::flag(ALLOW_NO_GPU_ENV)
}

/// Whether `error` reports that no GPU adapter exists on the host (as opposed to a device error,
/// shader rejection, or an unmatched `WGPU_ADAPTER_NAME`).
#[doc(hidden)]
pub fn is_missing_adapter_error(error: &str) -> bool {
    error.starts_with("no GPU adapter:")
}

/// Handles a GPU initialization error in a test: returns `true` (after printing one clear skip
/// line) only when [`ALLOW_NO_GPU_ENV`] is set and `error` reports a missing adapter. Otherwise
/// panics with `error` so real GPU failures or missing adapters without the opt-in switch fail
/// loudly.
#[doc(hidden)]
pub fn skip_test_without_gpu(error: &str) -> bool {
    skip_test_without_gpu_with(allow_no_gpu_tests(), error)
}

#[doc(hidden)]
pub fn skip_test_without_gpu_with(allow: bool, error: &str) -> bool {
    if allow && is_missing_adapter_error(error) {
        eprintln!("skipped: {error} ({ALLOW_NO_GPU_ENV}=1)");
        return true;
    }
    panic!("{error}");
}

#[cfg(target_os = "macos")]
use objc2::runtime::AnyObject;

/// Runs `f` on the CAMetalLayer `surface` presents into, under wgpu-hal's lock on wgpu's Metal
/// backend. `None` when the surface is on neither backend.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn with_layer<R>(surface: &wgpu::Surface<'_>, f: impl FnOnce(&AnyObject) -> R) -> Option<R> {
    // SAFETY: the layer is only borrowed, under wgpu's lock.
    if let Some(hal) = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() } {
        let layer = hal.render_layer().lock();
        // SAFETY: a live CAMetalLayer, which the lock keeps; wgpu's objc2 may be another version
        // than ours, the object is the same.
        return Some(f(unsafe { &*objc2::rc::Retained::as_ptr(&*layer).cast::<AnyObject>() }));
    }
    #[cfg(feature = "metal-backend")]
    if let Some(layer) = metal::layer(surface) {
        return Some(f(layer));
    }
    None
}

/// Whether `queue` is Rune's native Metal backend's, where a frame's submit and present commit
/// one command buffer, too cheap to hand to a present thread.
#[cfg(target_os = "macos")]
pub fn native(queue: &wgpu::Queue) -> bool {
    #[cfg(feature = "metal-backend")]
    return metal::owns(queue);
    #[cfg(not(feature = "metal-backend"))]
    {
        let _ = queue;
        false
    }
}
