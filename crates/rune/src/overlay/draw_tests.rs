use super::layer_resize_scale;
use crate::{Cx, El, Env, Runtime, el};

#[test]
fn layer_resize_scale_keeps_window_scale_under_zoom() {
    fn view(_: &u8, cx: &mut Cx<u8>) -> El {
        cx.zoom(1.5);
        el().w_full().h_full()
    }
    let mut rt = Runtime::new(0u8, view, (400.0, 120.0), Env { scale: 2.0, system_dark: false });
    rt.frame(0.0);
    assert_eq!(rt.window_scale(), 2.0);
    assert_eq!(rt.env.scale, 3.0);

    // Resizing the layer surface without a scale event keeps the display's backing scale
    // instead of feeding `env.scale` back into `set_viewport` and compounding zoom.
    for size in [(500.0, 200.0), (600.0, 250.0)] {
        let scale = layer_resize_scale(rt.core());
        assert_eq!(scale, 2.0);
        rt.set_viewport(size, scale);
        rt.frame(0.0);
        assert_eq!(rt.window_scale(), 2.0);
        assert_eq!(rt.env.scale, 3.0);
        assert_eq!(rt.core().window_viewport(), size);
    }

    // After a display scale change, the next resize uses the updated backing scale.
    rt.set_viewport((600.0, 250.0), 1.0);
    rt.frame(0.0);
    assert_eq!(layer_resize_scale(rt.core()), 1.0);
}

/// AppKit reports a panel ordered in without a fade as occluded until its occlusion update lands
/// some milliseconds later: the shown overlay's first frames draw anyway, instead of waiting out
/// `OCCLUDED_RETRY`. A visible reading, or a hide, ends that; then an occluded reading counts.
#[cfg(target_os = "macos")]
#[test]
fn a_shown_overlay_draws_through_the_occluded_reading_right_after_its_order_in() {
    use super::super::{AppSpec, Overlay, OverlaySpec, Presence, frame};
    fn view(_: &(), _: &mut Cx<()>) -> El { el() }
    let app = AppSpec::new("reveal", (40.0, 20.0), || (), view);
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("reveal", app.size, view), (), &app);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.dirty = true;
    overlay.prime.arm(); // as `show_fading` does once it orders the panel in
    let reading = overlay.occlusion_reading(true);
    overlay.set_occluded(reading);
    assert!(!overlay.presence.occluded(), "the order-in's own occluded reading doesn't count");
    let gate = frame::Gate {
        prewarm: false, occluded: overlay.presence.occluded(), awaiting_frame: false, dirty: true,
        idle: false, retry: overlay.backoff.deadline(), missed_this_turn: false,
    };
    assert_eq!(frame::turn(gate, crate::time::now), frame::Turn::Draw, "the reveal frame draws at once");
    assert!(!overlay.occlusion_reading(false));
    assert!(!overlay.prime.presumes(), "AppKit's visible reading ends the presumption");
    assert!(overlay.occlusion_reading(true), "a panel covered later reads as covered");

    overlay.prime.arm();
    overlay.hide();
    assert!(!overlay.prime.presumes(), "a hide ends the presumption");
    overlay.prime.arm();
    overlay.prime.spend();
    overlay.prime.spend();
    assert!(overlay.occlusion_reading(true), "after its two frames, a covered panel reads as covered");
}
