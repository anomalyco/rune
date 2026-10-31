use super::{by_name, by_pci, dev_numbers, pci_address};
use std::path::Path;

fn adapter(name: &str, backend: wgpu::Backend, pci: &str) -> wgpu::AdapterInfo {
    wgpu::AdapterInfo { name: name.into(), backend, device_pci_bus_id: pci.into(), ..wgpu::AdapterInfo::new(wgpu::DeviceType::Other, backend) }
}

/// glibc's `makedev`.
fn makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xffff_f000) << 32) | ((major & 0x0fff) << 8) | ((minor & 0xffff_ff00) << 12) | (minor & 0xff)
}

#[test]
fn dev_t_bytes_split_into_major_and_minor() {
    assert_eq!(dev_numbers(&makedev(226, 128).to_ne_bytes()), Some((226, 128)));
    assert_eq!(dev_numbers(&makedev(226, 1).to_ne_bytes()), Some((226, 1)));
    assert_eq!(dev_numbers(&makedev(0x12345, 0x6789a).to_ne_bytes()), Some((0x12345, 0x6789a)));
    assert_eq!(dev_numbers(&((226u32 << 8) | 128).to_ne_bytes()), Some((226, 128)));
    assert_eq!(dev_numbers(&[1, 2, 3]), None);
}

#[test]
fn drm_device_links_give_their_pci_address() {
    assert_eq!(pci_address(Path::new("../../../0000:01:00.0")).as_deref(), Some("0000:01:00.0"));
    assert_eq!(pci_address(Path::new("../../../0000:C5:00.0")).as_deref(), Some("0000:c5:00.0"));
    assert_eq!(pci_address(Path::new("../../../simple-framebuffer.0")), None);
    assert_eq!(pci_address(Path::new("../../../fd4ab000.gpu")), None);
}

#[test]
fn the_compositor_device_picks_the_adapter_on_it() {
    let adapters = [
        (adapter("AMD Radeon Graphics (RADV RAPHAEL_MENDOCINO)", wgpu::Backend::Vulkan, "0000:0f:00.0"), true),
        (adapter("NVIDIA GeForce RTX 5070", wgpu::Backend::Gl, "0000:01:00.0"), true),
        (adapter("NVIDIA GeForce RTX 5070", wgpu::Backend::Vulkan, "0000:01:00.0"), true),
        (adapter("llvmpipe", wgpu::Backend::Vulkan, ""), true),
    ];
    assert_eq!(by_pci("0000:01:00.0", &adapters), Some(2), "Vulkan before GL");
    assert_eq!(by_pci("0000:0F:00.0", &adapters), Some(0));
    assert_eq!(by_pci("0000:02:00.0", &adapters), None);
    assert_eq!(by_pci("", &adapters), None, "no address matches llvmpipe's empty one");
}

#[test]
fn an_adapter_that_cant_present_is_skipped() {
    let adapters = [
        (adapter("NVIDIA GeForce RTX 5070", wgpu::Backend::Vulkan, "0000:01:00.0"), false),
        (adapter("AMD Radeon Graphics", wgpu::Backend::Vulkan, "0000:0f:00.0"), true),
    ];
    assert_eq!(by_pci("0000:01:00.0", &adapters), None);
    assert_eq!(by_name("nvidia", &adapters), None);
    assert_eq!(by_name("RADEON", &adapters), Some(1));
}
