//! Making a GPU device ahead of a surface (`warm_gpu`) and looking at the GPUs once at launch
//! (`probe_gpu`), so a first surface waits less for its device.

/// A window's or overlay's surface draws with `gpu`, which `warm_gpu` then leaves alone.
pub(crate) fn surface_gpu(gpu: &std::sync::Arc<super::Gpu>) {
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    super::prewarm::surface_gpu(gpu);
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    let _ = gpu;
}

/// A hidden overlay's device and renderer, its surface dropped, for the next window's or
/// overlay's surface to take (`warm::park`). macOS drops them: its prewarmed panels keep theirs.
pub(crate) fn park_gpu(gpu: super::Gpu, renderer: super::Renderer) {
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    super::prewarm::park(gpu, renderer, super::prewarm::PARKED);
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    drop((renderer, gpu));
}

/// Starts making, on a thread, the GPU device and Rune's renderer the next window or overlay
/// surface would make, so a show soon after (a tray app's HUD, on its shortcut's first key)
/// doesn't wait for them on the UI thread. That surface takes them, waiting for the thread if
/// it's still working; unclaimed, they go half a minute after the last call. Does nothing while a
/// surface's device lives. `features` are the ones that surface's compositor asks for
/// (`Compositor::features`); a device without them isn't taken. Windows and Linux; on macOS
/// use `prewarm_gpu`. Cheap to call often, from any thread.
pub fn warm_gpu(features: wgpu::Features) {
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    super::prewarm::warm(features, None);
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    let _ = features;
}

/// `warm_gpu`, then `prepare` on that thread with the device, new or the one a surface draws
/// with: an app makes its own pipelines there, off the UI thread, for its compositor to find.
pub fn warm_gpu_with(features: wgpu::Features, prepare: impl FnOnce(&wgpu::Device) + Send + 'static) {
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    super::prewarm::warm(features, Some(Box::new(prepare)));
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    let _ = (features, prepare);
}

/// Looks at the GPUs once, on a thread, as the event loop starts: whether a DX12 adapter
/// exists, which loads the DX12 runtime and drivers. It leaves no device (8 MB, no driver
/// threads), and the first surface then enumerates the adapters once instead of twice.
pub(crate) fn probe_gpu() {
    #[cfg(target_os = "windows")]
    let _ = std::thread::Builder::new().name("rune-gpu-probe".into()).spawn(|| drop(super::Gpu::wgpu_instance()));
}
