//! Uncaptured GPU error policy tests across direct, prewarmed, adopted, and headless devices.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
use super::super::{Gpu, OFFSCREEN_FORMAT, Renderer};

fn trigger_validation_error(device: &wgpu::Device) -> std::thread::Result<()> {
    trigger_validation_error_labeled(device, "uncaptured-error-policy-probe")
}

fn trigger_validation_error_labeled(device: &wgpu::Device, label: &str) -> std::thread::Result<()> {
    catch_unwind(AssertUnwindSafe(|| {
        drop(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
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

#[test]
#[ignore]
fn restore_device_callbacks_surface_and_headless_child() {
    super::enable_uncaptured_logging_for_test();

    // 1. Surface device: restore re-installs both device-lost and bounded deduplicated
    //    uncaptured-error logging, preserving Printed deduplication across the restore.
    let surface = Gpu::try_for_display(Gpu::wgpu_instance(), wgpu::Features::empty())
        .expect("display wgpu device");
    let mut surface_renderer = Renderer::new(&surface.device, OFFSCREEN_FORMAT);
    surface_renderer.attach_gpu(&surface);
    assert!(surface.has_surface_error_policy());

    // Log probe-a once before foreign code overwrites the callbacks.
    assert!(trigger_validation_error_labeled(&surface.device, "restore-dedup-probe-a").is_ok());

    let foreign_lost = Arc::new(AtomicBool::new(false));
    let foreign_lost_cb = Arc::clone(&foreign_lost);
    surface.device.set_device_lost_callback(move |_, _| {
        foreign_lost_cb.store(true, Ordering::SeqCst);
    });
    let foreign_errors = Arc::new(AtomicUsize::new(0));
    let foreign_errors_cb = Arc::clone(&foreign_errors);
    surface.device.on_uncaptured_error(Arc::new(move |_| {
        foreign_errors_cb.fetch_add(1, Ordering::SeqCst);
    }));

    // Confirm the foreign error handler took over the single slot before restoration.
    assert!(trigger_validation_error_labeled(&surface.device, "restore-foreign-only").is_ok());
    assert_eq!(foreign_errors.load(Ordering::SeqCst), 1);

    // Restore Rune's callbacks through Renderer (the path Frame::restore_device_callbacks uses).
    surface_renderer.restore_device_callbacks(&surface.device);

    // Re-triggering probe-a must not panic, must not hit the foreign callback, and must be
    // deduplicated by the shared Printed log (parent asserts count == 1).
    assert!(trigger_validation_error_labeled(&surface.device, "restore-dedup-probe-a").is_ok());
    // A distinct error probe-b must be logged once by the restored Rune handler without panicking.
    assert!(trigger_validation_error_labeled(&surface.device, "restore-dedup-probe-b").is_ok());
    assert_eq!(
        foreign_errors.load(Ordering::SeqCst),
        1,
        "foreign uncaptured-error callback must not run after restore_device_callbacks",
    );

    surface.device.destroy();
    rune_gpu::health::poll(&surface.device, wgpu::PollType::Poll).ok();
    assert!(surface.is_lost(), "surface Gpu::is_lost must flip after restore + destroy");
    assert!(
        !foreign_lost.load(Ordering::SeqCst),
        "foreign device-lost callback must not run after restore_device_callbacks",
    );

    // 2. Headless device (draws_surfaces == false): restore re-installs device-lost but keeps
    //    wgpu's default validation panic intact.
    let headless = Gpu::try_new(Gpu::wgpu_instance(), None).expect("headless wgpu device");
    let mut headless_renderer = Renderer::new(&headless.device, OFFSCREEN_FORMAT);
    headless_renderer.attach_gpu(&headless);
    assert!(!headless.has_surface_error_policy());

    let headless_foreign_lost = Arc::new(AtomicBool::new(false));
    let headless_foreign_lost_cb = Arc::clone(&headless_foreign_lost);
    headless.device.set_device_lost_callback(move |_, _| {
        headless_foreign_lost_cb.store(true, Ordering::SeqCst);
    });
    headless_renderer.restore_device_callbacks(&headless.device);
    assert!(
        trigger_validation_error_labeled(&headless.device, "restore-headless-panic").is_err(),
        "headless device must keep wgpu's validation panic after restore_device_callbacks",
    );
    headless.device.destroy();
    rune_gpu::health::poll(&headless.device, wgpu::PollType::Poll).ok();
    assert!(headless.is_lost(), "headless Gpu::is_lost must flip after restore + destroy");
    assert!(!headless_foreign_lost.load(Ordering::SeqCst));
}

#[test]
fn restore_device_callbacks_restores_loss_and_deduplicated_error_logging() {
    if !Gpu::test_adapter_available() { return; }
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(&exe)
        .args([
            "--exact",
            "render::gpu_loss::tests::restore_device_callbacks_surface_and_headless_child",
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
    assert_eq!(
        stderr.matches("rune: a GPU error went uncaptured").count(),
        2,
        "expected exactly two uncaptured error headers (probe-a once, probe-b once), got:\n{stderr}",
    );
    assert_eq!(
        stderr.matches("restore-dedup-probe-a").count(),
        1,
        "expected restore-dedup-probe-a logged exactly once (deduplicated across restore), got:\n{stderr}",
    );
    assert_eq!(
        stderr.matches("restore-dedup-probe-b").count(),
        1,
        "expected restore-dedup-probe-b logged once by restored handler, got:\n{stderr}",
    );
    assert!(
        !stderr.contains("restore-foreign-only"),
        "foreign error while overwritten must not be logged by Rune, got:\n{stderr}",
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

    #[cfg(feature = "metal-backend")]
    #[test]
    fn macos_native_metal_restore_device_lost_callback() {
        let gpu = Gpu::try_new(crate::gpu::metal::instance(), None).expect("native Metal device");
        assert!(
            crate::gpu::metal::raw_device(&gpu.device).is_some(),
            "expected Rune's native Metal backend device",
        );
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.attach_gpu(&gpu);
        assert!(renderer.gpu_instance().is_some());
        assert_eq!(
            renderer.gpu_adapter().map(wgpu::Adapter::get_info),
            Some(gpu.device.adapter_info()),
        );

        let foreign_lost = Arc::new(AtomicBool::new(false));
        let foreign_lost_cb = Arc::clone(&foreign_lost);
        gpu.device.set_device_lost_callback(move |_, _| {
            foreign_lost_cb.store(true, Ordering::SeqCst);
        });
        renderer.restore_device_callbacks(&gpu.device);
        assert!(!gpu.is_lost());
        gpu.device.destroy();
        assert!(
            gpu.is_lost(),
            "native Metal Gpu::is_lost must flip to true on device.destroy() after restore_device_callbacks",
        );
        assert!(
            !foreign_lost.load(Ordering::SeqCst),
            "foreign device-lost callback must not run after restore_device_callbacks",
        );
    }
}
