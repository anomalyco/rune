use crate::gpu::{DEFAULT_TOLERANCE, composite_tolerance};

fn adapter(backend: wgpu::Backend, vendor: u32, name: &str) -> wgpu::AdapterInfo {
    wgpu::AdapterInfo {
        name: name.into(),
        vendor,
        ..wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, backend)
    }
}

#[test]
fn composite_tolerance_is_three_only_for_nvidia_dx12() {
    assert_eq!(composite_tolerance(&adapter(wgpu::Backend::Dx12, 0x10DE, "NVIDIA GeForce")), 3);
    assert_eq!(
        composite_tolerance(&adapter(wgpu::Backend::Vulkan, 0x10DE, "NVIDIA GeForce")),
        DEFAULT_TOLERANCE,
        "NVIDIA under Vulkan is not measured and stays at 2"
    );
    assert_eq!(
        composite_tolerance(&adapter(wgpu::Backend::Gl, 0x10DE, "NVIDIA GeForce")),
        DEFAULT_TOLERANCE
    );
    assert_eq!(
        composite_tolerance(&adapter(wgpu::Backend::Dx12, 0x1002, "AMD Radeon Graphics")),
        DEFAULT_TOLERANCE
    );
    assert_eq!(
        composite_tolerance(&adapter(wgpu::Backend::Dx12, 0x1414, "Microsoft Basic Render Driver")),
        DEFAULT_TOLERANCE
    );
    assert_eq!(
        composite_tolerance(&adapter(wgpu::Backend::Dx12, 0x8086, "Intel Iris Xe")),
        DEFAULT_TOLERANCE
    );
    assert_eq!(
        composite_tolerance(&adapter(wgpu::Backend::Metal, 0x106B, "Apple M4 Pro")),
        DEFAULT_TOLERANCE
    );
}
