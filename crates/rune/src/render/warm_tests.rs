//! A device that died while it waited for a surface (Windows' Modern Standby, a driver reset) is
//! not handed to the next one: a lost device configures nothing, and wgpu panics on the drawable
//! its surface is asked for.

#[cfg(all(target_os = "linux", feature = "test-gpu-noop"))]
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

#[cfg(not(target_arch = "wasm32"))]
mod warm_renderer {
    use crate::render::{Gpu, Renderer};

    #[test]
    fn warm_renderer_transfers_across_threads_and_rejects_surfaces() {
        let Ok(gpu) = Gpu::try_new(Gpu::instance(), None) else { return eprintln!("skipped: no GPU") };
        let device = gpu.device.clone();
        let warm = std::thread::spawn(move || Renderer::new_warm(&device, wgpu::TextureFormat::Bgra8Unorm))
            .join()
            .expect("warm thread");
        let renderer = Renderer::from_warm(warm);
        assert_eq!(renderer.format(), wgpu::TextureFormat::Bgra8Unorm);

        #[cfg(not(target_os = "macos"))]
        {
            let warm = renderer.into_warm().expect("new renderer has no native surface renderer");
            let mut renderer = std::thread::spawn(move || Renderer::from_warm(warm))
                .join()
                .expect("attach thread");
            let draw = crate::native_surface::SurfaceDraw {
                surface: crate::native_surface::NativeSurface::new(),
                fit: crate::native_surface::SurfaceFit::Fill,
                before_instance: 0,
                rect: [0.0, 0.0, 16.0, 16.0],
                radii: [0.0; 4],
                clip: crate::render::NO_CLIP,
                fade: [0; 4],
                opacity: 1.0,
            };
            renderer.prepare_surfaces(&gpu.device, &gpu.queue, &[draw], (16, 16));
            assert!(renderer.into_warm().is_none(), "renderer with native surfaces must not convert to WarmRenderer");
        }
    }
}
