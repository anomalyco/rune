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

/// Whether `gpu` can draw `surface` with `wanted`'s features its adapter supports, and, for a
/// `translucent` surface, composite translucent pixels on it.
pub(crate) fn serves(gpu: &crate::render::Gpu, surface: &wgpu::Surface<'_>, wanted: wgpu::Features, translucent: bool) -> bool {
    gpu.adapter.is_surface_supported(surface)
        && gpu.device.features().contains(wanted & gpu.adapter.features())
        && (!translucent || mode(surface, &gpu.adapter).is_some())
}

/// `pick`'s adapter when it composites translucent pixels on `surface`; otherwise the adapter
/// on `instance` that does on the compositor's GPU or Windows' card (`gpu::pick`), then the
/// lowest-power one that does, or `pick`'s adapter when none can.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn prefer(instance: &wgpu::Instance, surface: &wgpu::Surface<'_>, pick: &crate::gpu::pick::Pick) -> wgpu::Adapter {
    if mode(surface, &pick.adapter).is_some() {
        return pick.adapter.clone();
    }
    let mut able: Vec<_> = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .into_iter()
        .filter(|candidate| candidate.is_surface_supported(surface) && mode(surface, candidate).is_some())
        .collect();
    able.sort_by_key(|candidate| {
        let info = candidate.get_info();
        (!pick.on_preferred_device(&info), power_rank(info.device_type), crate::gpu::pick::backend_rank(info.backend))
    });
    able.into_iter().next().unwrap_or_else(|| pick.adapter.clone())
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
    fn low_power_order_puts_integrated_first_and_software_last() {
        use wgpu::DeviceType::*;
        let mut devices = [Cpu, DiscreteGpu, Other, IntegratedGpu, VirtualGpu];
        devices.sort_by_key(|d| super::power_rank(*d));
        assert_eq!(devices, [IntegratedGpu, DiscreteGpu, VirtualGpu, Other, Cpu]);
    }
}
