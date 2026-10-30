//! The Metal backend under a surface: wgpu's own, or with `metal-backend` Rune's native one
//! (`metal`), which `RUNE_GPU_BACKEND=metal` selects.

#[cfg(feature = "metal-backend")]
pub(crate) mod metal;

use objc2_video::runtime::AnyObject;

/// Runs `f` on the CAMetalLayer `surface` presents into, under wgpu-hal's lock on wgpu's Metal
/// backend. `None` when the surface is on neither backend.
pub(crate) fn with_layer<R>(surface: &wgpu::Surface<'_>, f: impl FnOnce(&AnyObject) -> R) -> Option<R> {
    // SAFETY: the layer is only borrowed, under wgpu's lock.
    if let Some(hal) = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() } {
        let layer = hal.render_layer().lock();
        // SAFETY: a live CAMetalLayer, which the lock keeps; wgpu's objc2 may be another version
        // than ours, the object is the same.
        return Some(f(unsafe { &*objc2_video::rc::Retained::as_ptr(&*layer).cast::<AnyObject>() }));
    }
    #[cfg(feature = "metal-backend")]
    if let Some(layer) = metal::layer(surface) {
        return Some(f(layer));
    }
    None
}

/// Says once, on `RUNE_GPU_BACKEND=metal`, that `what` needs wgpu-hal and does nothing there.
pub(crate) fn inert(what: &'static str) {
    #[cfg(feature = "metal-backend")]
    if metal::enabled() {
        metal::inert(what);
    }
    #[cfg(not(feature = "metal-backend"))]
    let _ = what;
}
