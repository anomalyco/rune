//! Uncaptured GPU error policy tests across direct, prewarmed, adopted, and headless devices.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use super::super::Gpu;

fn trigger_validation_error(device: &wgpu::Device) -> std::thread::Result<()> {
    catch_unwind(AssertUnwindSafe(|| {
        drop(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uncaptured-error-policy-probe"),
            size: 4,
            usage: wgpu::BufferUsages::empty(),
            mapped_at_creation: false,
        }));
    }))
}

#[test]
fn surface_error_policy_is_tracked_without_suppressing_test_panics() {
    if !Gpu::test_adapter_available() { return; }
    let headless = Gpu::try_new(Gpu::wgpu_instance(), None).expect("headless wgpu device");
    assert!(!headless.has_surface_error_policy());

    let display = Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
        .expect("display wgpu device");
    assert!(display.has_surface_error_policy());
    // Under plain `cargo test`, Rune keeps wgpu's default validation panic active.
    assert!(
        trigger_validation_error(&display.device).is_err(),
        "default test runs must keep wgpu's validation panic",
    );
}

#[test]
#[ignore]
fn surface_error_policy_child() {
    super::enable_uncaptured_logging_for_test();

    // 1. Headless devices keep wgpu's default validation panic.
    let headless = Gpu::try_new(Gpu::wgpu_instance(), None).expect("headless wgpu device");
    assert!(!headless.has_surface_error_policy());
    assert!(
        trigger_validation_error(&headless.device).is_err(),
        "headless wgpu device must panic on uncaptured validation errors",
    );

    // 2. Display / prewarmed devices (warm.rs on Windows/Linux, prewarm.rs on macOS,
    //    embed::LayerHost) get the surface error policy at creation and do not panic.
    let display = Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
        .expect("display wgpu device");
    assert!(display.has_surface_error_policy());
    assert!(
        trigger_validation_error(&display.device).is_ok(),
        "display/prewarmed wgpu device must log uncaptured validation errors without panicking",
    );

    // Calling install_surface_error_policy again on a device that already has it is a no-op
    // and must not overwrite a handler installed later.
    let display_seen = Arc::new(AtomicUsize::new(0));
    let display_seen_cb = Arc::clone(&display_seen);
    display.device.on_uncaptured_error(Arc::new(move |_| {
        display_seen_cb.fetch_add(1, Ordering::SeqCst);
    }));
    display.install_surface_error_policy();
    assert!(trigger_validation_error(&display.device).is_ok());
    assert_eq!(
        display_seen.load(Ordering::SeqCst),
        1,
        "repeat install_surface_error_policy must not reinstall or overwrite the handler",
    );

    // 3. Adopting a device created without a surface installs the policy once.
    let adopted = Gpu::try_new(Gpu::wgpu_instance(), None).expect("unadopted wgpu device");
    assert!(!adopted.has_surface_error_policy());
    adopted.install_surface_error_policy();
    assert!(adopted.has_surface_error_policy());
    assert!(
        trigger_validation_error(&adopted.device).is_ok(),
        "adopted device must log uncaptured validation errors without panicking",
    );
    let adopted_seen = Arc::new(AtomicUsize::new(0));
    let adopted_seen_cb = Arc::clone(&adopted_seen);
    adopted.device.on_uncaptured_error(Arc::new(move |_| {
        adopted_seen_cb.fetch_add(1, Ordering::SeqCst);
    }));
    adopted.install_surface_error_policy();
    assert!(trigger_validation_error(&adopted.device).is_ok());
    assert_eq!(
        adopted_seen.load(Ordering::SeqCst),
        1,
        "second adoption call must be a no-op",
    );

    // 4. Gpu::from_device (external surface device) installs the policy at construction.
    let instance = Gpu::wgpu_instance();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("wgpu adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .expect("wgpu device");
    let external = Gpu::from_device(instance, adapter, device, queue);
    assert!(external.has_surface_error_policy());
    assert!(
        trigger_validation_error(&external.device).is_ok(),
        "Gpu::from_device must log uncaptured validation errors without panicking",
    );
}

#[test]
fn surface_error_policy_matches_across_direct_prewarm_adopted_and_headless_devices() {
    if !Gpu::test_adapter_available() { return; }
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(&exe)
        .args([
            "--exact",
            "render::gpu_loss::tests::surface_error_policy_child",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .expect("spawn child test");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "child test failed: status={:?}\nstdout={}\nstderr={stderr}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        stderr.contains("rune: a GPU error went uncaptured"),
        "expected uncaptured error log in stderr, got:\n{stderr}",
    );
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use super::*;
    use super::super::super::Renderer;
    use objc2::{msg_send, rc::Retained, runtime::{AnyClass, AnyObject}};

    fn layer() -> Retained<AnyObject> {
        // SAFETY: `+[CAMetalLayer new]` allocates and returns a retained `CAMetalLayer`.
        unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] }
    }

    fn surface<'w>(instance: &wgpu::Instance, layer: &Retained<AnyObject>) -> Result<wgpu::Surface<'w>, String> {
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(layer) as *mut std::ffi::c_void);
        // SAFETY: the layer outlives the surface in each test.
        unsafe { instance.create_surface_unsafe(target) }.map_err(|e| e.to_string())
    }

    #[test]
    #[ignore]
    fn macos_prewarmed_wgpu_surface_device_child() {
        super::super::enable_uncaptured_logging_for_test();
        crate::prewarm_gpu_on(crate::GpuBackend::Wgpu);
        let layer = layer();

        // 1. Adopted prewarmed device through the production surface setup path.
        let (adopted_gpu, _surface, took_prewarm) = Gpu::for_surface_setup(
            wgpu::Features::empty(),
            false,
            |i| surface(i, &layer),
            |gpu, _surface, prewarmed| {
                assert!(gpu.has_surface_error_policy(), "policy must be installed before setup runs");
                Ok(prewarmed.is_some())
            },
        )
        .expect("surface setup adopting prewarmed wgpu device");
        assert!(took_prewarm, "expected window setup to adopt the prewarmed device");
        #[cfg(feature = "metal-backend")]
        assert!(crate::gpu::metal::raw_device(&adopted_gpu.device).is_none(), "expected wgpu Metal device");
        assert!(
            trigger_validation_error(&adopted_gpu.device).is_ok(),
            "adopted prewarmed wgpu surface device must log uncaptured validation errors instead of panicking",
        );

        // 2. Freshly created surface device (prewarm slot is now empty).
        let (fresh_gpu, _surface2, took_prewarm_2) = Gpu::for_surface_setup(
            wgpu::Features::empty(),
            false,
            |i| surface(i, &layer),
            |gpu, _surface, prewarmed| {
                assert!(gpu.has_surface_error_policy());
                Ok(prewarmed.is_some())
            },
        )
        .expect("surface setup creating a fresh wgpu device");
        assert!(!took_prewarm_2, "prewarm slot must be empty on the second surface");
        assert!(
            trigger_validation_error(&fresh_gpu.device).is_ok(),
            "fresh wgpu surface device must log uncaptured validation errors instead of panicking",
        );

        // 3. Adopting a device created without a surface installs the policy before setup runs.
        let early = Gpu::try_new(Gpu::wgpu_instance(), None).expect("headless wgpu device");
        assert!(!early.has_surface_error_policy());
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8Unorm);
        let (promoted_gpu, _surface3, kept) = Gpu::for_surface_setup_from(
            Some((early, renderer)),
            wgpu::Features::empty(),
            false,
            |i| surface(i, &layer),
            |gpu, _surface, prewarmed| {
                assert!(gpu.has_surface_error_policy(), "adoption must install policy before setup");
                assert!(
                    trigger_validation_error(&gpu.device).is_ok(),
                    "validation errors during surface setup on an adopted device must not panic",
                );
                Ok(prewarmed.is_some())
            },
        )
        .expect("surface setup adopting headless-created device");
        assert!(kept, "expected early device and renderer to be adopted");
        assert!(
            trigger_validation_error(&promoted_gpu.device).is_ok(),
            "promoted surface device must log uncaptured validation errors instead of panicking",
        );

        // 4. Headless device still panics on uncaptured validation errors.
        let headless = Gpu::try_new(Gpu::wgpu_instance(), None).expect("headless wgpu device");
        assert!(!headless.has_surface_error_policy());
        assert!(
            trigger_validation_error(&headless.device).is_err(),
            "headless wgpu device must keep wgpu's default validation panic",
        );
    }

    #[test]
    fn macos_prewarmed_wgpu_surface_device_logs_uncaptured_validation_error() {
        if !Gpu::test_adapter_available() { return; }
        let exe = std::env::current_exe().expect("test binary path");
        let output = std::process::Command::new(&exe)
            .args([
                "--exact",
                "render::gpu_loss::tests::macos::macos_prewarmed_wgpu_surface_device_child",
                "--ignored",
                "--nocapture",
            ])
            .output()
            .expect("spawn child test");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "child test failed: status={:?}\nstdout={}\nstderr={stderr}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
        );
        assert!(
            stderr.contains("rune: a GPU error went uncaptured"),
            "expected uncaptured error log in stderr, got:\n{stderr}",
        );
    }
}
