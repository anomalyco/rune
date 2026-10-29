//! Picking a GPU that composites translucent pixels on a surface. Some surfaces are opaque only:
//! a Win32 Vulkan surface on AMD's integrated GPUs, and any DXGI swapchain made from an HWND. A
//! transparent window or overlay drawn there shows black where it should be clear. Rune makes
//! DX12 surfaces through DirectComposition (`Gpu::instance`), which composites alpha on every
//! GPU, and gives a translucent surface the lowest-power adapter that composites it.

/// The alpha mode that composites translucent pixels on `surface` through `adapter`, if any.
pub(crate) fn mode(surface: &wgpu::Surface<'_>, adapter: &wgpu::Adapter) -> Option<wgpu::CompositeAlphaMode> {
    let caps = surface.get_capabilities(adapter);
    [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied, wgpu::CompositeAlphaMode::Inherit]
        .into_iter()
        .find(|mode| caps.alpha_modes.contains(mode))
}

// Windows Vulkan can report Inherit while still presenting opaque black pixels. Try an
// explicit-alpha backend first, but preserve Inherit if there is no better choice.
fn reliable_alpha(backend: wgpu::Backend, mode: Option<wgpu::CompositeAlphaMode>, windows: bool) -> bool {
    mode.is_some_and(|mode| !(windows && backend == wgpu::Backend::Vulkan && mode == wgpu::CompositeAlphaMode::Inherit))
}

/// Whether `gpu` can draw `surface` with `wanted`'s features its adapter supports, and, for a
/// `translucent` surface, composite translucent pixels on it.
pub(crate) fn serves(gpu: &crate::render::Gpu, surface: &wgpu::Surface<'_>, wanted: wgpu::Features, translucent: bool) -> bool {
    gpu.adapter.is_surface_supported(surface)
        && gpu.device.features().contains(wanted & gpu.adapter.features())
        && (!translucent || reliable_alpha(gpu.adapter.get_info().backend, mode(surface, &gpu.adapter), cfg!(target_os = "windows")))
}

/// `adapter` when it composites translucent pixels on `surface`; otherwise the lowest-power
/// adapter on `instance` that does, or `adapter` when none can.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn prefer(instance: &wgpu::Instance, surface: &wgpu::Surface<'_>, adapter: wgpu::Adapter) -> wgpu::Adapter {
    let original_mode = mode(surface, &adapter);
    if reliable_alpha(adapter.get_info().backend, original_mode, cfg!(target_os = "windows")) {
        return adapter;
    }
    let mut able: Vec<_> = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .into_iter()
        .filter(|candidate| candidate.is_surface_supported(surface) && mode(surface, candidate).is_some())
        .collect();
    able.sort_by_key(|candidate| (!reliable_alpha(candidate.get_info().backend, mode(surface, candidate), cfg!(target_os = "windows")), power_rank(candidate.get_info().device_type)));
    if let Some(candidate) = able.into_iter().next() {
        if reliable_alpha(candidate.get_info().backend, mode(surface, &candidate), cfg!(target_os = "windows")) || original_mode.is_none() {
            return candidate;
        }
    }
    adapter
}

/// `PowerPreference::LowPower`'s order; a software adapter comes last.
#[cfg(not(target_arch = "wasm32"))]
fn power_rank(device: wgpu::DeviceType) -> u8 {
    match device {
        wgpu::DeviceType::IntegratedGpu => 0,
        wgpu::DeviceType::DiscreteGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_vulkan_inherit_is_not_evidence_of_transparency() {
        use wgpu::{Backend, CompositeAlphaMode as Alpha};
        assert!(!super::reliable_alpha(Backend::Vulkan, Some(Alpha::Inherit), true));
        assert!(!super::reliable_alpha(Backend::Dx12, None, true));
        assert!(super::reliable_alpha(Backend::Vulkan, Some(Alpha::PreMultiplied), true));
        assert!(super::reliable_alpha(Backend::Dx12, Some(Alpha::PreMultiplied), true));
        assert!(super::reliable_alpha(Backend::Vulkan, Some(Alpha::Inherit), false));
    }

    #[test]
    fn low_power_order_puts_integrated_first_and_software_last() {
        use wgpu::DeviceType::*;
        let mut devices = [Cpu, DiscreteGpu, Other, IntegratedGpu, VirtualGpu];
        devices.sort_by_key(|d| super::power_rank(*d));
        assert_eq!(devices, [IntegratedGpu, DiscreteGpu, VirtualGpu, Other, Cpu]);
    }
}
