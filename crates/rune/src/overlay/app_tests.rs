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
