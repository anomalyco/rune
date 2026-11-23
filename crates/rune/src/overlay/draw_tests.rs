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
