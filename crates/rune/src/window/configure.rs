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
        None => Ok(()),
        Some(error) => {
            let text = error.to_string();
            let cause = text.lines().map(str::trim).filter(|line| !line.is_empty() && *line != "Caused by:").collect::<Vec<_>>().join(": ");
            Err(format!("{cause} ({}x{}, {:?}, {:?}, {:?})", config.width, config.height, config.format, config.alpha_mode, config.present_mode))
        }
    }
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
