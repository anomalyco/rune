use super::{Overlay, OverlayHost};
use crate::compose::{Compositor, Frame};
use crate::overlay::OverlaySpec;
use crate::render::KIND_EFFECT;
use crate::{AppSpec, Cx, El, col, effect};

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
    let (mut host_with, _) = OverlayHost::new(host_spec, (), &app);
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
    let (mut host_without, _) = OverlayHost::new(host_without_spec, (), &app);
    assert!(!host_without.app.rt().track_marks);
    host_without.app.frame(0.0);
    assert!(host_without.app.rt().marks.is_empty());
}
