use super::{Overlay, OverlayHost};
use crate::compose::{Compositor, Frame};
use crate::overlay::OverlaySpec;
use crate::render::KIND_EFFECT;
use crate::{App, AppSpec, Cx, El, FontSmoothing, WindowSpec, col, effect};

#[test]
fn overlay_runtime_tracks_marks_only_when_compositor_is_attached() {
    struct NoopCompositor;
    impl Compositor<()> for NoopCompositor {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            f.draw_ui();
        }
    }

    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(120.0, 80.0).child(effect(42).size(60.0, 20.0))
    }

    let app = AppSpec::new("overlay-marks", (120.0, 80.0), || (), view);

    let with_spec = OverlaySpec::new("overlay-marks-on", app.size, view)
        .options(Default::default())
        .compositor(NoopCompositor);
    let (mut overlay_with, _) = Overlay::new(with_spec, (), &app);
    assert!(overlay_with.compositor.is_some());
    assert!(overlay_with.rt.track_marks, "overlay runtime with a compositor must enable track_marks");
    overlay_with.rt.frame(0.0);
    assert_eq!(overlay_with.rt.marks.len(), overlay_with.rt.instances.len());
    let effect_marks: Vec<u32> = overlay_with
        .rt
        .instances
        .iter()
        .enumerate()
        .filter(|(_, inst)| inst.params[1] == KIND_EFFECT)
        .map(|(i, _)| overlay_with.rt.marks[i])
        .collect();
    assert_eq!(effect_marks, [42]);

    let without_spec = OverlaySpec::new("overlay-marks-off", app.size, view).options(Default::default());
    let (mut overlay_without, _) = Overlay::new(without_spec, (), &app);
    assert!(overlay_without.compositor.is_none());
    assert!(!overlay_without.rt.track_marks, "overlay runtime without a compositor keeps track_marks off");
    overlay_without.rt.frame(0.0);
    assert!(overlay_without.rt.marks.is_empty());

    let host_spec = OverlaySpec::new("overlay-host-marks-on", app.size, view)
        .options(Default::default())
        .compositor(NoopCompositor);
    let (mut host_with, _) = OverlayHost::new(host_spec, (), &app, FontSmoothing::default());
    assert!(host_with.app.rt().track_marks);
    host_with.app.frame(0.0);
    let host_marks: Vec<u32> = host_with
        .app
        .rt()
        .instances
        .iter()
        .enumerate()
        .filter(|(_, inst)| inst.params[1] == KIND_EFFECT)
        .map(|(i, _)| host_with.app.rt().marks[i])
        .collect();
    assert_eq!(host_marks, [42]);

    let host_without_spec = OverlaySpec::new("overlay-host-marks-off", app.size, view).options(Default::default());
    let (mut host_without, _) = OverlayHost::new(host_without_spec, (), &app, FontSmoothing::default());
    assert!(!host_without.app.rt().track_marks);
    host_without.app.frame(0.0);
    assert!(host_without.app.rt().marks.is_empty());
}

#[test]
fn overlay_inherits_app_font_smoothing() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    let spec = AppSpec::new("overlay-smoothing", (120.0, 80.0), || (), view);

    // App::font_smoothing set before App::overlay.
    let mut app = App::new(spec).font_smoothing(FontSmoothing::Off);
    let _ = app.overlay(OverlaySpec::new("before", (120.0, 80.0), view), ());
    assert_eq!(app.overlays[0].font_smoothing(), FontSmoothing::Off);

    // App::font_smoothing set after App::overlay (and via WindowSpec::font_smoothing).
    let mut app = App::new(AppSpec::new("overlay-smoothing-after", (120.0, 80.0), || (), view));
    let _ = app.overlay(OverlaySpec::new("after", (120.0, 80.0), view), ());
    let window = WindowSpec::from_app("main", app).font_smoothing(FontSmoothing::Off);
    assert_eq!(window.app.overlays[0].font_smoothing(), FontSmoothing::Off);
}

#[test]
fn overlay_font_smoothing_builder_overrides_app() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    let default = FontSmoothing::default();

    // Explicit override beats App::font_smoothing set before App::overlay, even when overriding
    // back to FontSmoothing::default().
    let mut app = App::new(AppSpec::new("overlay-override-before", (120.0, 80.0), || (), view))
        .font_smoothing(FontSmoothing::Off);
    let _ = app.overlay(
        OverlaySpec::new("override-default", (120.0, 80.0), view).options(Default::default()).font_smoothing(default),
        (),
    );
    let _ = app.overlay(
        OverlaySpec::new("override-windfoil", (120.0, 80.0), view).options(Default::default()).font_smoothing(FontSmoothing::Windfoil),
        (),
    );
    assert_eq!(app.overlays[0].font_smoothing(), default);
    assert_eq!(app.overlays[1].font_smoothing(), FontSmoothing::Windfoil);

    // Explicit override also survives App::font_smoothing called after App::overlay.
    let mut app = App::new(AppSpec::new("overlay-override-after", (120.0, 80.0), || (), view));
    let _ = app.overlay(
        OverlaySpec::new("override-later", (120.0, 80.0), view).options(Default::default()).font_smoothing(default),
        (),
    );
    let _ = app.overlay(OverlaySpec::new("inherit-later", (120.0, 80.0), view), ());
    let app = app.font_smoothing(FontSmoothing::Off);
    assert_eq!(app.overlays[0].font_smoothing(), default);
    assert_eq!(app.overlays[1].font_smoothing(), FontSmoothing::Off);
}

#[test]
fn overlay_app_forwards_release_compositor_gpu() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingComp(Arc<AtomicUsize>);
    impl Compositor<()> for CountingComp {
        fn render(&mut self, f: &mut Frame<'_, ()>) { f.draw_ui(); }
        fn release_gpu(&mut self) { self.0.fetch_add(1, Ordering::Relaxed); }
    }

    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    let app = AppSpec::new("overlay-release-gpu", (120.0, 80.0), || (), view);
    let releases = Arc::new(AtomicUsize::new(0));
    let spec = OverlaySpec::new("overlay-release-gpu", app.size, view)
        .options(Default::default())
        .compositor(CountingComp(releases.clone()));
    let (mut host, _) = OverlayHost::new(spec, (), &app, FontSmoothing::default());
    host.app.release_compositor_gpu();
    assert_eq!(releases.load(Ordering::Relaxed), 1);
}

#[test]
fn overlay_compositor_exposes_instance_and_adapter_when_gpu_is_attached() {
    use std::sync::{Arc, Mutex};
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};

    #[derive(Default)]
    struct Seen {
        has_instance: bool,
        adapter_info: Option<wgpu::AdapterInfo>,
        device_info: Option<wgpu::AdapterInfo>,
    }
    struct Probe(Arc<Mutex<Seen>>);
    impl Compositor<()> for Probe {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            let mut seen = self.0.lock().unwrap();
            seen.has_instance = f.instance().is_some();
            seen.adapter_info = f.adapter().map(wgpu::Adapter::get_info);
            seen.device_info = Some(f.device.adapter_info());
            f.restore_device_callbacks();
            f.draw_ui();
        }
    }

    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(32.0, 16.0) }
    let Some(gpu) = Gpu::test_headless().map(Arc::new) else { return };
    let app = AppSpec::new("overlay-host-handles", (32.0, 16.0), || (), view);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let spec = OverlaySpec::new("overlay-host-handles", app.size, view)
        .options(Default::default())
        .compositor(Probe(Arc::clone(&seen)));
    let (mut overlay, _) = Overlay::new(spec, (), &app);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.attach_gpu(&gpu);
    overlay.renderer = Some(renderer);
    overlay.gpu = Some(Arc::clone(&gpu));
    overlay.rt.transparent = true;
    overlay.rt.frame(0.0);

    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    overlay.render_offscreen(&gpu, &mut enc, &target.view, OFFSCREEN_FORMAT, (32, 16), 0.0);
    overlay.renderer.as_mut().unwrap().submit(&gpu.queue, enc);

    let seen = seen.lock().unwrap();
    assert!(seen.has_instance, "overlay Frame::instance must be Some when gpu is attached");
    assert_eq!(
        seen.adapter_info,
        seen.device_info,
        "overlay Frame::adapter info must match Frame::device.adapter_info()",
    );
}
#[test]
fn overlay_macos_color_space_defaults_to_srgb_and_retains_unmanaged() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    let app = AppSpec::new("overlay-color-space", (120.0, 80.0), || (), view);

    let default_cfg = OverlaySpec::new("default", app.size, view).options(Default::default());
    assert_eq!(default_cfg.macos_color_space, crate::MacColorSpace::Srgb);
    let (default_overlay, _) = Overlay::new(default_cfg, (), &app);
    assert_eq!(default_overlay.core.macos_color_space, crate::MacColorSpace::Srgb);

    let from_spec: crate::ConfiguredOverlay<()> = OverlaySpec::new("from-spec", app.size, view).into();
    assert_eq!(from_spec.macos_color_space, crate::MacColorSpace::Srgb);

    let unmanaged_cfg = OverlaySpec::new("unmanaged", app.size, view)
        .options(Default::default())
        .macos_color_space(crate::MacColorSpace::Unmanaged);
    assert_eq!(unmanaged_cfg.macos_color_space, crate::MacColorSpace::Unmanaged);
    let (unmanaged_overlay, _) = Overlay::new(unmanaged_cfg, (), &app);
    assert_eq!(unmanaged_overlay.core.macos_color_space, crate::MacColorSpace::Unmanaged);

    let host_cfg = OverlaySpec::new("host-unmanaged", app.size, view)
        .options(Default::default())
        .macos_color_space(crate::MacColorSpace::Unmanaged);
    let (host, _) = OverlayHost::new(host_cfg, (), &app, FontSmoothing::default());
    assert_eq!(host.core.macos_color_space, crate::MacColorSpace::Unmanaged);

    let back_to_srgb = OverlaySpec::new("toggle", app.size, view)
        .options(Default::default())
        .macos_color_space(crate::MacColorSpace::Unmanaged)
        .macos_color_space(crate::MacColorSpace::Srgb);
    let (toggled, _) = Overlay::new(back_to_srgb, (), &app);
    assert_eq!(toggled.core.macos_color_space, crate::MacColorSpace::Srgb);
}

#[test]
fn an_overlay_keeps_the_system_credential_store() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(20.0, 20.0) }
    // A thread of its own: the flag is per thread in tests, and other tests make headless runs.
    let noted = std::thread::spawn(|| {
        let app = AppSpec::new("overlay-credentials", (20.0, 20.0), || (), view);
        let spec = OverlaySpec::new("overlay-credentials", app.size, view).options(Default::default());
        let _ = Overlay::new(spec, (), &app);
        crate::credentials::headless_runtime_noted()
    });
    assert!(!noted.join().unwrap(), "a real app's overlay must not switch credentials to memory");
}
#[test]
fn overlay_frame_latency_defaults_to_none_clamps_and_retains() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    let app = AppSpec::new("overlay-latency", (120.0, 80.0), || (), view);

    let default_cfg = OverlaySpec::new("default", app.size, view).options(Default::default());
    assert_eq!(default_cfg.frame_latency, None);
    let (default_overlay, _) = Overlay::new(default_cfg, (), &app);
    assert_eq!(default_overlay.core.frame_latency, None);

    for (input, expected) in [(0, 1), (1, 1), (2, 2), (3, 2), (u32::MAX, 2)] {
        let cfg = OverlaySpec::new("clamped", app.size, view)
            .options(Default::default())
            .frame_latency(input);
        assert_eq!(cfg.frame_latency, Some(expected), "input {input}");
        let (overlay, _) = Overlay::new(cfg, (), &app);
        assert_eq!(overlay.core.frame_latency, Some(expected), "OverlayCore input {input}");

        let host_cfg = OverlaySpec::new("host-clamped", app.size, view)
            .options(Default::default())
            .frame_latency(input);
        let (host, _) = OverlayHost::new(host_cfg, (), &app, FontSmoothing::default());
        assert_eq!(host.core.frame_latency, Some(expected), "OverlayHost input {input}");
    }
}
