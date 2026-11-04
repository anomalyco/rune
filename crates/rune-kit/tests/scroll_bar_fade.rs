//! A scroll bar draws above its viewport's clip and fade, set on the viewport or on what
//! `scroll_bar` wraps it in: with the rows fading out at the bottom, the thumb at the end of the
//! track is drawn whole, as the reference renderer rasterises the frame.
use rune::{Color, Cx, El, Env, Runtime, ScrollBars, col, el};
use rune_kit::{ScrollBar, ScrollBarStyle};
use rune_ref::{Atlas, Frame, Primitive, Scene};

const W: u32 = 200;
const H: u32 = 100;

/// The fade set on the viewport (`inside`), or on what `scroll_bar` wraps it in.
fn pane(inside: &bool, _: &mut Cx<bool>) -> El {
    let fade = |e: El| e.clip_fade_linear(0.0, 0.0, 0.0, 40.0);
    let viewport = col()
        .key("scroll")
        .w_full()
        .h(H as f32)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .child(col().children((0..40).map(|i| el().key(format!("row{i}")).w(100.0).h(20.0).fill(Color::WHITE))));
    let style = ScrollBarStyle::neutral(true).thumb(Color::WHITE);
    let area = if *inside { fade(viewport).scroll_bar_styled(style) } else { fade(viewport.scroll_bar_styled(style)) };
    col().size(W as f32, H as f32).child(area)
}

fn thumb<S>(rt: &Runtime<S>) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("thumb")).unwrap().rect
}

#[test]
fn the_thumb_draws_whole_over_a_viewport_that_fades_out_at_the_bottom() {
    for inside in [true, false] {
        fades_the_rows_but_not_the_thumb(inside);
    }
}

fn fades_the_rows_but_not_the_thumb(inside: bool) {
    let mut rt = Runtime::new(inside, pane, (W as f32, H as f32), Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.frame(0.0);
    rt.wheel(100.0, 50.0, -10_000.0);
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
    let [x, y, w, h] = thumb(&rt);
    assert!(y + h > H as f32 - 10.0, "scrolled to the end, the thumb sits in the fade: {:?}", [x, y, w, h]);

    let scene = Scene {
        width: W,
        height: H,
        clear: [0.0, 0.0, 0.0, 1.0],
        primitives: rt
            .instances
            .iter()
            .enumerate()
            .map(|(i, p)| Primitive::from_instance(p, i as u32 + 1, format!("scroll-bar/{i}")))
            .collect(),
        atlas: Atlas { width: rune::text::ATLAS_SIZE, height: rune::text::ATLAS_SIZE, data: rt.text.atlas.data.clone() },
    };
    scene.validate().unwrap();
    let mut frame = Frame::new(W, H, false);
    frame.render(&scene);
    let px = |x: u32, y: u32| frame.rgba[(y * W + x) as usize * 4];
    assert!(px(50, H - 2) < 64, "the rows fade out at the bottom ({inside}): {}", px(50, H - 2));
    let cx = (x + w / 2.0) as u32;
    for row in [y + h / 2.0, y + h - 4.0, y + h - 2.0] {
        assert_eq!(px(cx, row as u32), 255, "the thumb is whole at {row}, in the fade ({inside})");
    }
}
