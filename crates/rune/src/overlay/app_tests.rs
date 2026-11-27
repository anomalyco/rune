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
    let (mut host_with, _) = OverlayHost::new(host_spec, (), &app, Default::default());
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
    let (mut host_without, _) = OverlayHost::new(host_without_spec, (), &app, Default::default());
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
fn overlay_inherits_app_wheel_settings_before_and_after_registration() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    let spec = AppSpec::new("overlay-wheel", (120.0, 80.0), || (), view);

    // App::wheel_line and App::wheel_glide set before App::overlay.
    let mut app = App::new(spec).wheel_line(60.0).wheel_glide(false);
    let _ = app.overlay(OverlaySpec::new("before", (120.0, 80.0), view), ());
    assert_eq!(app.overlays[0].wheel_line(), Some(60.0));
    assert!(!app.overlays[0].wheel_glide());

    // App::wheel_line and App::wheel_glide set after App::overlay (and via WindowSpec).
    let mut app = App::new(AppSpec::new("overlay-wheel-after", (120.0, 80.0), || (), view));
    let _ = app.overlay(OverlaySpec::new("after", (120.0, 80.0), view), ());
    assert_eq!(app.overlays[0].wheel_line(), None);
    assert!(app.overlays[0].wheel_glide());
    let window = WindowSpec::from_app("main", app).wheel_line(60.0).wheel_glide(false);
    assert_eq!(window.app.overlays[0].wheel_line(), Some(60.0));
    assert!(!window.app.overlays[0].wheel_glide());
}

#[test]
fn overlay_wheel_settings_override_app_in_either_order() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }

    // Explicit overrides beat App settings set before App::overlay, including restoring defaults.
    let mut app = App::new(AppSpec::new("overlay-wheel-override-before", (120.0, 80.0), || (), view))
        .wheel_line(60.0)
        .wheel_glide(false);
    let _ = app.overlay(
        OverlaySpec::new("override-custom", (120.0, 80.0), view)
            .options(Default::default())
            .wheel_line(90.0)
            .wheel_glide(true),
        (),
    );
    let _ = app.overlay(
        OverlaySpec::new("override-platform-default", (120.0, 80.0), view)
            .options(Default::default())
            .wheel_line(0.0),
        (),
    );
    assert_eq!(app.overlays[0].wheel_line(), Some(90.0));
    assert!(app.overlays[0].wheel_glide());
    assert_eq!(app.overlays[1].wheel_line(), None);
    assert!(!app.overlays[1].wheel_glide());

    // Explicit overrides also survive App::wheel_line and App::wheel_glide called after App::overlay.
    let mut app = App::new(AppSpec::new("overlay-wheel-override-after", (120.0, 80.0), || (), view));
    let _ = app.overlay(
        OverlaySpec::new("explicit", (120.0, 80.0), view)
            .options(Default::default())
            .wheel_line(0.0)
            .wheel_glide(true),
        (),
    );
    let _ = app.overlay(OverlaySpec::new("inherit", (120.0, 80.0), view), ());
    let app = app.wheel_line(60.0).wheel_glide(false);
    assert_eq!(app.overlays[0].wheel_line(), None);
    assert!(app.overlays[0].wheel_glide());
    assert_eq!(app.overlays[1].wheel_line(), Some(60.0));
    assert!(!app.overlays[1].wheel_glide());
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
    let (mut host, _) = OverlayHost::new(spec, (), &app, Default::default());
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
    let (host, _) = OverlayHost::new(host_cfg, (), &app, Default::default());
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
        let (host, _) = OverlayHost::new(host_cfg, (), &app, Default::default());
        assert_eq!(host.core.frame_latency, Some(expected), "OverlayHost input {input}");
    }
}

#[test]
fn overlay_setup_measures_use_overlay_text_system_and_stay_warm_for_first_frame() {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    const LABEL: &str = "Overlay setup measurement";
    const STYLE: crate::TextStyle = crate::TextStyle::new(14.0, 400.0);

    #[derive(Default)]
    struct State {
        setup_width: f32,
        view_width: Cell<f32>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        s.view_width.set(crate::text::measure(LABEL, &[], STYLE, None).0);
        col().size(240.0, 80.0).child(crate::text(LABEL).font(STYLE))
    }

    // Run on a clean thread so no ambient `with_families` override bypasses `lend`.
    std::thread::spawn(|| {
        let app = AppSpec::new("overlay-setup-measure", (240.0, 80.0), State::default, view);
        let cfg = OverlaySpec::new("overlay-setup-measure", app.size, view)
            .options(Default::default())
            .setup(|s, _| {
                s.setup_width = crate::text::measure(LABEL, &[], STYLE, None).0;
            });

        // Test wrapper path (`BorrowOverlayApp::run_setup`).
        let (mut overlay, _) = Overlay::new(cfg, State::default(), &app);
        assert_eq!(overlay.rt.text.shaped, 0);
        overlay.connect(Arc::new(|| {}), Arc::new(|| {}), Arc::new(|| {}), Rc::default());
        let shaped_after_setup = overlay.rt.text.shaped;
        assert!(shaped_after_setup >= 1, "overlay setup must shape into the overlay's TextSystem");
        assert!(overlay.rt.text.cached_layout_count() >= 1);
        overlay.rt.frame(0.0);
        assert_eq!(
            overlay.rt.text.shaped, shaped_after_setup,
            "overlay's first frame must reuse the layout shaped during setup",
        );
        assert_eq!(overlay.rt.state.setup_width, overlay.rt.state.view_width.get());

        // Live host path (`TypedOverlayApp::run_setup`).
        let host_cfg = OverlaySpec::new("overlay-host-setup-measure", app.size, view)
            .options(Default::default())
            .setup(|s, _| {
                s.setup_width = crate::text::measure(LABEL, &[], STYLE, None).0;
            });
        let (mut host, _) = OverlayHost::new(host_cfg, State::default(), &app, Default::default());
        assert_eq!(host.app.rt().text.shaped, 0);
        host.app.run_setup(Arc::new(|| {}));
        let host_shaped = host.app.rt().text.shaped;
        assert!(host_shaped >= 1, "TypedOverlayApp::run_setup must lend the overlay's TextSystem");
        assert!(host.app.rt().text.cached_layout_count() >= 1);
        host.app.frame(0.0);
        assert_eq!(host.app.rt().text.shaped, host_shaped);
    })
    .join()
    .unwrap();
}

#[test]
fn an_overlay_added_through_the_handle_is_built_later_for_the_handle_already_given() {
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    fn bar_view(_: &usize, _: &mut Cx<usize>) -> El { col().size(100.0, 24.0) }

    let spec = AppSpec::new("dynamic-overlay", (120.0, 80.0), || (), view);
    let app = App::new(spec).font_smoothing(FontSmoothing::Off);
    let app_wakes = Arc::new(AtomicUsize::new(0));
    let app_wakes_cb = app_wakes.clone();
    app.shared.core.connect(Arc::new(move || {
        app_wakes_cb.fetch_add(1, Ordering::Relaxed);
    }));
    let wakes_after_connect = app_wakes.load(Ordering::Relaxed);

    let built = Arc::new(AtomicBool::new(false));
    let built_cb = built.clone();
    let handle = app.handle();
    // Queue from a background thread to verify Send + deferred UI-thread construction.
    let ov_handle = std::thread::spawn(move || {
        let ov = handle.overlay(move || {
            built_cb.store(true, Ordering::Release);
            (OverlaySpec::new("bar", (100.0, 24.0), bar_view), 7usize)
        });
        // Requests queued immediately on the returned handle before build runs.
        ov.show();
        ov.update(|n, _| *n = 99);
        ov
    })
    .join()
    .unwrap();

    assert!(!built.load(Ordering::Acquire), "builder must wait for the UI thread turn");
    assert!(app.shared.core.has_overlays());
    assert!(app_wakes.load(Ordering::Relaxed) > wakes_after_connect, "AppHandle::overlay wakes the loop");

    let defaults = super::OverlayDefaults::from_spec(
        &app.spec,
        super::AppOverlayDefaults {
            font_smoothing: app.runtime.font_smoothing,
            wheel_glide: app.runtime.wheel_glide,
            wheel_line: app.runtime.wheel_line,
        },
        app.runtime.emoji_family.clone(),
        app.prewarm_effects,
        app.gpu_loss_latch.clone(),
    );
    let builders = app.shared.core.take_overlays();
    assert_eq!(builders.len(), 1);
    let mut overlay = builders
        .into_iter()
        .next()
        .unwrap()(&defaults)
        .expect("builder constructs overlay");
    assert!(built.load(Ordering::Acquire));
    assert_eq!(overlay.font_smoothing(), FontSmoothing::Off, "inherits app font_smoothing");
    assert!(!overlay.finished());

    let ov_wakes = Arc::new(AtomicUsize::new(0));
    let ov_wakes_cb = ov_wakes.clone();
    overlay.connect(
        Arc::new(move || { ov_wakes_cb.fetch_add(1, Ordering::Relaxed); }),
        Arc::new(|| {}),
        Arc::new(|| {}),
        Rc::default(),
    );
    let after_connect = ov_wakes.load(Ordering::Relaxed);
    assert!(after_connect >= 1, "connect wakes the loop to drain pre-build requests");

    // Further calls on the already-given handle wake the connected overlay.
    ov_handle.resize((200.0, 32.0));
    assert_eq!(ov_wakes.load(Ordering::Relaxed), after_connect + 1);
}

#[test]
fn a_closed_overlay_is_finished_for_the_loop_to_drop_and_its_handle_wakes_nothing() {
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use crate::overlay::{Availability, PendingOverlay};
    use crate::window::AnyHost;

    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    let app = AppSpec::new("close-overlay", (120.0, 80.0), || (), view);
    let (mut host, handle) = OverlayHost::new(OverlaySpec::new("bar", app.size, view), (), &app, super::AppOverlayDefaults::default());

    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = wakes.clone();
    host.connect(
        Arc::new(move || { wakes_cb.fetch_add(1, Ordering::Relaxed); }),
        Arc::new(|| {}),
        Arc::new(|| {}),
        Rc::default(),
    );
    let wakes_before_close = wakes.load(Ordering::Relaxed);

    let token_answer = Arc::new(Mutex::new(Some(Some("pending".to_owned()))));
    let token_cb = token_answer.clone();
    handle.activation_token(move |tok| *token_cb.lock().unwrap() = Some(tok));

    let after_hidden_ran = Arc::new(AtomicBool::new(false));
    let after_hidden_cb = after_hidden_ran.clone();
    handle.after_hidden(move |_, _| after_hidden_cb.store(true, Ordering::Release));

    let wakes_before = wakes.load(Ordering::Relaxed);
    assert!(wakes_before > wakes_before_close);

    // Closing wakes the loop once so the host can finish, answers pending tokens None,
    // and drops pending after_hidden callbacks unrun.
    handle.close();
    assert_eq!(wakes.load(Ordering::Relaxed), wakes_before + 1);
    assert_eq!(*token_answer.lock().unwrap(), Some(None));
    assert!(!after_hidden_ran.load(Ordering::Acquire));

    // Further calls on the closed handle (or clones) wake nothing.
    let wakes_after_close = wakes.load(Ordering::Relaxed);
    let clone = handle.clone();
    clone.close();
    clone.show();
    clone.hide();
    clone.resize((300.0, 40.0));
    clone.update(|_, _| {});
    clone.after_hidden(|_, _| panic!("after_hidden on closed handle must not run"));
    let late_token = Arc::new(AtomicBool::new(false));
    let late_token_cb = late_token.clone();
    clone.activation_token(move |tok| {
        assert!(tok.is_none());
        late_token_cb.store(true, Ordering::Release);
    });
    assert!(late_token.load(Ordering::Acquire));
    assert_eq!(wakes.load(Ordering::Relaxed), wakes_after_close, "closed handle must wake nothing");

    // Processing the close on the UI thread finishes the host for cleanup to drop.
    assert!(!host.finished());
    host.core.close_host(&mut *host.app);
    assert!(host.finished());
    assert_eq!(host.wake_deadline(), None);
    assert_eq!(handle.availability(), Availability::Unavailable);
}

#[test]
fn an_overlay_closed_before_build_is_skipped_and_drops_its_builder() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) { self.0.store(true, Ordering::Release); }
    }

    fn view(_: &(), _: &mut Cx<()>) -> El { col().size(120.0, 80.0) }
    let app = App::new(AppSpec::new("close-before-build", (120.0, 80.0), || (), view));
    let handle = app.handle();
    let ran = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let ran_cb = ran.clone();
    let guard = DropFlag(dropped.clone());

    let ov = handle.overlay(move || {
        let _keep = guard;
        ran_cb.store(true, Ordering::Release);
        (OverlaySpec::new("gone", (100.0, 20.0), view), ())
    });
    ov.close();

    let defaults = super::OverlayDefaults::from_spec(
        &app.spec,
        super::AppOverlayDefaults {
            font_smoothing: app.runtime.font_smoothing,
            wheel_glide: app.runtime.wheel_glide,
            wheel_line: app.runtime.wheel_line,
        },
        app.runtime.emoji_family.clone(),
        app.prewarm_effects,
        app.gpu_loss_latch.clone(),
    );
    let builders = app.shared.core.take_overlays();
    assert_eq!(builders.len(), 1);
    assert!(builders.into_iter().next().unwrap()(&defaults).is_none());
    assert!(!ran.load(Ordering::Acquire), "builder closure must not run after close");
    assert!(dropped.load(Ordering::Acquire), "builder closure must be dropped cleanly");
}
