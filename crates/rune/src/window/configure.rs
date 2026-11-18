//! Configuring a window's or overlay's surface, and saying so when it can't.

use crate::render::Gpu;

/// Configures `surface`, returning wgpu's error instead of panicking in its default handler,
/// so a surface the driver refuses (an overlay's XWayland window on NVIDIA, hex#979) costs
/// only its owner. The message names the configuration for logs.
///
/// A lost device fails here even when wgpu reports nothing: it drops the `DeviceLost` that
/// `Surface::configure` returns (wgpu-30.0.1 `handle_error_inner`, "will be surfaced via
/// callback") without recording the surface's error sink, so the surface has no presentation
/// and the next `get_current_texture` panics ("Surface is not configured for presentation").
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn configure(surface: &wgpu::Surface, gpu: &Gpu, config: &wgpu::SurfaceConfiguration) -> Result<(), String> {
    if gpu.is_lost() { return Err(lost_device(config)); }
    let validation = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = gpu.device.push_error_scope(wgpu::ErrorFilter::Internal);
    surface.configure(&gpu.device, config);
    // `Device::destroy` only marks the device invalid: its lost callback runs when the device
    // next maintains itself, at the soonest a poll. Without this, a device destroyed before it
    // reported itself configures nothing while `is_lost` still reads false, and the drawable
    // its surface is asked for panics.
    gpu.device.poll(wgpu::PollType::Poll).ok();
    let error = pollster::block_on(internal.pop()).or(pollster::block_on(validation.pop()));
    if gpu.is_lost() { return Err(lost_device(config)); }
    match error {
        None => { configured(surface); Ok(()) }
        Some(error) => {
            let text = error.to_string();
            let cause = text.lines().map(str::trim).filter(|line| !line.is_empty() && *line != "Caused by:").collect::<Vec<_>>().join(": ");
            Err(format!("{cause} ({}x{}, {:?}, {:?}, {:?})", config.width, config.height, config.format, config.alpha_mode, config.present_mode))
        }
    }
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

/// The browser reports surface errors asynchronously; there is nothing to catch here.
#[cfg(target_arch = "wasm32")]
pub(crate) fn configure(surface: &wgpu::Surface, gpu: &Gpu, config: &wgpu::SurfaceConfiguration) -> Result<(), String> {
    surface.configure(&gpu.device, config);
    Ok(())
}
