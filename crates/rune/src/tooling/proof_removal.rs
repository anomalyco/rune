//! PROOF ONLY, never merged (rune-proof: device-removal). Removes a D3D12 device as a GPU reset
//! does, and only on WARP, the software adapter: on anything else the process ends first.

use std::sync::OnceLock;
use std::time::Instant;

/// When the device was removed, once it was.
pub(crate) static REMOVED: OnceLock<Instant> = OnceLock::new();

/// Whether this run asked for the removal named `what` (`RUNE_PROOF_REMOVE=<what>`).
pub(crate) fn asked(what: &str) -> bool {
    #[allow(clippy::disallowed_methods)]
    let asked = std::env::var("RUNE_PROOF_REMOVE").is_ok_and(|v| v == what);
    asked
}

pub(crate) fn remove(gpu: &crate::render::Gpu, at: &str) {
    let info = gpu.adapter.get_info();
    if info.device_type != wgpu::DeviceType::Cpu || !info.name.contains("Microsoft Basic Render Driver") {
        eprintln!("proof: refusing to remove a device on {:?} ({:?}): proofs remove devices on WARP only", info.name, info.device_type);
        std::process::exit(3);
    }
    #[cfg(target_os = "windows")]
    {
        use windows::core::Interface;
        let hal = unsafe { gpu.device.as_hal::<wgpu::hal::api::Dx12>() }.expect("a D3D12 device");
        let device5: windows::Win32::Graphics::Direct3D12::ID3D12Device5 = hal.raw_device().cast().expect("ID3D12Device5");
        unsafe { device5.RemoveDevice() };
    }
    let _ = REMOVED.set(Instant::now());
    eprintln!("proof: ID3D12Device5::RemoveDevice called on {:?} {at}", info.name);
}

/// Microseconds since the removal, if there was one.
pub(crate) fn since() -> Option<u128> {
    REMOVED.get().map(|at| at.elapsed().as_micros())
}
