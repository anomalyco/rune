//! GPU adapter selection, backend dispatch, device error logging, analytic GPU springs and
//! Rune's native Apple Metal backend. On macOS and iOS, `metal-backend` (on by default) provides
//! the native Metal backend (`metal`) alongside wgpu's: a Mac app draws with the native one
//! unless `GpuBackend::Wgpu` or `RUNE_GPU_BACKEND=wgpu` selects wgpu's, and an iOS app draws with
//! wgpu's unless it chooses `GpuBackend::Metal`. Apps normally reach this crate through `rune`.

pub mod backend;
pub mod health;
#[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
pub mod metal;
#[cfg(not(target_arch = "wasm32"))]
pub mod pick;
pub mod springs;

pub use backend::GpuBackend;
pub use backend::describe as describe_gpu;

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
