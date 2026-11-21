//! `prewarm_gpu` ordering and macOS Metal prewarm tests.

#[test]
fn prewarm_gpu_on_chooses_before_it_starts() {
    let log = std::cell::RefCell::new(Vec::new());
    super::device::choose_then_start(crate::GpuBackend::Metal, |b| log.borrow_mut().push(format!("choose {b:?}")), || log.borrow_mut().push("start".into()));
    assert_eq!(*log.borrow(), ["choose Metal", "start"]);
}

/// `prewarm_gpu` and how a window picks it up, on a bare Metal layer (no window server needed).
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use super::super::{Gpu, Renderer};
    use objc2::{rc::Retained, runtime::{AnyClass, AnyObject}, msg_send};

    fn layer() -> Retained<AnyObject> {
        // SAFETY: `+[CAMetalLayer new]` allocates and returns a retained `CAMetalLayer`.
        unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] }
    }

    fn surface<'w>(instance: &wgpu::Instance, layer: &Retained<AnyObject>) -> Result<wgpu::Surface<'w>, String> {
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(layer) as *mut std::ffi::c_void);
        // SAFETY: the layer outlives the surface in each test.
        unsafe { instance.create_surface_unsafe(target) }.map_err(|e| e.to_string())
    }

    /// The window gets the thread's device and its compiled pipeline, once, in the Mac window format.
    #[test]
    fn a_window_takes_the_prewarmed_gpu_and_renderer_once() {
        crate::prewarm_gpu();
        crate::prewarm_gpu();
        let layer = layer();
        let (gpu, _surface, renderer) = match Gpu::for_surface(wgpu::Features::empty(), false, |i| surface(i, &layer)) {
            Ok(taken) => taken,
            Err(error) => return eprintln!("skipped: {error}"),
        };
        let Some(renderer) = renderer else { return eprintln!("skipped: no GPU for the prewarm thread") };
        // Rune's Metal backend loads its metallib without advertising passthrough.
        #[cfg(feature = "metal-backend")]
        let native = crate::gpu::metal::raw_device(&gpu.device).is_some();
        #[cfg(not(feature = "metal-backend"))]
        let native = false;
        assert!(native || gpu.device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS));
        assert_eq!(renderer.format, wgpu::TextureFormat::Bgra8Unorm);
        drop((renderer, gpu));
        let (_, _surface, again) = Gpu::for_surface(wgpu::Features::empty(), false, |i| surface(i, &layer)).expect("a second GPU");
        assert!(again.is_none(), "the prewarmed GPU goes to one window");

        // A slot that already took or discarded a GPU ignores a later `start`, so a late
        // `prewarm_gpu()` call cannot leave an unclaimed GPU in memory.
        let taken = super::super::prewarm::Slot::new();
        assert!(taken.take().is_none());
        taken.start();
        assert!(taken.take().is_none());

        let discarded = super::super::prewarm::Slot::new();
        discarded.start();
        discarded.discard();
        assert!(discarded.take().is_none());
        discarded.start();
        assert!(discarded.take().is_none());

        // `restart` replaces a working thread and leaves an idle or finished slot alone.
        let restarted = super::super::prewarm::Slot::new();
        restarted.restart();
        restarted.start();
        restarted.restart();
        assert!(restarted.take().is_some(), "the new thread's GPU");
        restarted.restart();
        assert!(restarted.take().is_none(), "a taken slot stays taken");
    }

    /// A prewarmed device without a feature the window wants (and the adapter has) is replaced on
    /// the same instance, with the surface made once: a second surface would add a second layer.
    #[test]
    fn a_prewarmed_gpu_missing_wanted_features_is_redone_on_its_instance() {
        let Ok(early) = Gpu::try_new(Gpu::instance(), None) else { return eprintln!("skipped: no GPU") };
        let plain = wgpu::Features::FLOAT32_FILTERABLE | wgpu::Features::DEPTH32FLOAT_STENCIL8 | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        let missing = (early.adapter.features() - early.device.features()) & plain;
        let Some(wanted) = missing.iter().next() else { return eprintln!("skipped: the adapter has none of the test's features") };
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let mut made = 0;
        let (gpu, _surface, renderer) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| { made += 1; surface(i, &layer) })
            .expect("a GPU with the wanted feature");
        assert_eq!(made, 1);
        assert!(renderer.is_none(), "the prewarmed renderer belongs to the dropped device");
        assert!(gpu.device.features().contains(wanted));

        let Ok(early) = Gpu::try_new(Gpu::instance(), None) else { return };
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let (_, _surface, kept) = Gpu::for_surface_from(Some((early, renderer)), wgpu::Features::empty(), false, |i| surface(i, &layer)).expect("a GPU");
        assert!(kept.is_some(), "a suitable prewarmed device keeps its renderer");
    }

    /// A compositor wanting a feature the native Metal backend lacks gets wgpu's device, when
    /// Metal is only the default, with the prewarmed native GPU dropped before any surface.
    #[cfg(feature = "metal-backend")]
    #[test]
    fn a_compositor_wanting_what_the_native_backend_lacks_draws_with_wgpu() {
        if !crate::gpu::metal::enabled() || std::env::var_os("RUNE_GPU_BACKEND").is_some() {
            return eprintln!("skipped: the native backend isn't the default here");
        }
        let early = Gpu::headless();
        assert!(crate::gpu::metal::raw_device(&early.device).is_some());
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let mut made = 0;
        let wanted = wgpu::Features::POLYGON_MODE_LINE;
        let (gpu, _surface, renderer) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| { made += 1; surface(i, &layer) })
            .expect("a wgpu GPU");
        assert_eq!(made, 1);
        assert!(renderer.is_none());
        assert!(crate::gpu::metal::raw_device(&gpu.device).is_none(), "wgpu's device");
        assert!(gpu.device.features().contains(wanted));
        let described = crate::gpu::backend::describe(&gpu.device);
        assert!(described.starts_with("wgpu Metal on ") && described.contains("which Rune's Metal backend lacks"), "{described}");
        assert!(crate::gpu::backend::describe(&Gpu::headless().device).ends_with("(the default)"));
        assert!(!crate::gpu::backend::serves(&Gpu::headless().device, wanted));
        assert!(crate::gpu::backend::serves(&Gpu::headless().device, wgpu::Features::TIMESTAMP_QUERY));
    }

    /// A compositor wanting immediates keeps the native backend and the prewarmed device, which
    /// has them whether or not anything asked, with the adapter's immediate size.
    #[cfg(feature = "metal-backend")]
    #[test]
    fn a_compositor_wanting_immediates_keeps_the_native_backend_and_the_prewarmed_gpu() {
        if !crate::gpu::metal::enabled() || std::env::var_os("RUNE_GPU_BACKEND").is_some() {
            return eprintln!("skipped: the native backend isn't the default here");
        }
        let early = Gpu::headless();
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let wanted = wgpu::Features::IMMEDIATES;
        let (gpu, _surface, kept) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| surface(i, &layer)).expect("a GPU");
        assert!(kept.is_some(), "the prewarmed renderer");
        assert!(crate::gpu::metal::raw_device(&gpu.device).is_some(), "the native backend's device");
        assert!(gpu.device.features().contains(wanted));
        assert_eq!(gpu.device.limits().max_immediate_size, 4096);
        let described = crate::gpu::backend::describe(&gpu.device);
        assert!(described.starts_with("Rune's Metal backend on ") && described.ends_with("(the default)"), "{described}");
    }
}
