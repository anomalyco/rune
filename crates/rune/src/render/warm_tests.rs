//! A device that died while it waited for a surface (Windows' Modern Standby, a driver reset) is
//! not handed to the next one: a lost device configures nothing, and wgpu panics on the drawable
//! its surface is asked for.

#[cfg(target_os = "linux")]
mod lost {
    use crate::render::prewarm::{PARKED, discard, park, take};
    use crate::render::{Gpu, Renderer};

    /// A GPU on wgpu's noop backend: it takes anything and draws nothing.
    fn noop_gpu() -> Gpu {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::NOOP;
        descriptor.backend_options.noop.enable = true;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        Gpu::from_device(instance, adapter, device, queue)
    }

    #[test]
    fn a_parked_device_that_died_is_not_handed_to_the_next_surface() {
        discard();
        let gpu = noop_gpu();
        let renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
        assert!(!gpu.is_lost());
        gpu.device.destroy();
        // wgpu loses a destroyed device once its queues drain, when the device next maintains itself.
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        assert!(gpu.is_lost(), "the device's lost callback marks it");
        park(gpu, renderer, PARKED);
        assert!(take().is_none(), "a lost device waits for no surface");
    }
}
