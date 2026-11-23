use super::radius_in_device_pixels;
use crate::{Cx, El, Env, Runtime, el};

#[test]
fn wayland_corner_radius_uses_window_scale_under_zoom() {
    fn view(zoom: &f32, cx: &mut Cx<f32>) -> El {
        cx.zoom(*zoom);
        el().w_full().h_full()
    }
    let mut rt = Runtime::new(1.5f32, view, (800.0, 600.0), Env { scale: 2.0, system_dark: false });
    rt.frame(0.0);
    assert_eq!(rt.window_scale(), 2.0);
    assert_eq!(rt.env.scale, 3.0);
    assert_eq!(radius_in_device_pixels(12.0, rt.core()), 24.0);
    assert_eq!(radius_in_device_pixels(0.0, rt.core()), 0.0);

    // Zooming out leaves the frame's corner arc at the window's backing scale too.
    rt.update(|z, _| *z = 0.75);
    rt.frame(0.0);
    assert_eq!(rt.window_scale(), 2.0);
    assert_eq!(rt.env.scale, 1.5);
    assert_eq!(radius_in_device_pixels(12.0, rt.core()), 24.0);

    // A display scale change updates the corner radius while zoom stays active.
    rt.set_viewport((800.0, 600.0), 1.5);
    rt.frame(0.0);
    assert_eq!(rt.window_scale(), 1.5);
    assert_eq!(radius_in_device_pixels(12.0, rt.core()), 18.0);
}
