use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, col, el};

fn pixels<S: 'static>(gpu: &Gpu, rt: &mut Runtime<S>, size: (u32, u32)) -> Vec<u8> {
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    target.read(&gpu.device)
}

fn alpha(px: &[u8], width: u32, x: u32, y: u32) -> u8 {
    px[((y * width + x) * 4 + 3) as usize]
}

#[derive(Default)]
struct S {
    pressed: bool,
    stick: bool,
    soft: bool,
}

/// A 100×100 viewport at y 40 whose 140 pt of content overflows it by 40 pt.
fn view(s: &S, _: &mut Cx<S>) -> El {
    let mut scroll = col().key("scroll").size(100.0, 100.0).scroll(ScrollSpec { stick_bottom: s.stick })
        .child(el().size(100.0, 60.0).fill(Color::WHITE))
        .child(el().key("under").size(100.0, 80.0).fill(Color::WHITE).on_press(|s: &mut S| s.pressed = true));
    if s.soft {
        scroll = scroll.clip_fade(0.0, 0.0, 0.0, 16.0);
    }
    col().size(100.0, 200.0).child(el().h(40.0)).child(scroll)
}

#[test]
fn overflowing_scroll_content_stays_inside_the_box_as_it_scrolls() {
    let gpu = Gpu::headless();
    let size = (100, 200);
    let mut rt = Runtime::new(S::default(), view, (100.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let px = pixels(&gpu, &mut rt, size);
    assert_eq!(alpha(&px, 100, 50, 130), 255, "inside the box");
    assert_eq!(alpha(&px, 100, 50, 160), 0, "overflow below the box isn't drawn");
    assert!(rt.instances.iter().all(|i| i.fade[3] == 0), "clip-radius lane stays reserved");

    // Scrolled to the end, the content moves under a clip that stays put.
    rt.wheel(50.0, 90.0, -40.0);
    rt.frame(1.0 / 120.0);
    let px = pixels(&gpu, &mut rt, size);
    assert_eq!(alpha(&px, 100, 50, 45), 255, "top of the box");
    assert_eq!(alpha(&px, 100, 50, 135), 255, "bottom of the box");
    assert_eq!(alpha(&px, 100, 50, 20), 0, "content scrolled above the box isn't drawn");

    // Opened at the bottom, the top of the content is clipped too.
    let mut rt = Runtime::new(S { stick: true, ..S::default() }, view, (100.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let px = pixels(&gpu, &mut rt, size);
    assert_eq!(alpha(&px, 100, 50, 45), 255);
    assert_eq!(alpha(&px, 100, 50, 20), 0, "stick_bottom content above the box isn't drawn");
}

#[test]
fn a_press_outside_the_box_misses_clipped_content() {
    let mut rt = Runtime::new(S::default(), view, (100.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_down(50.0, 160.0);
    rt.pointer_up(50.0, 160.0);
    rt.frame(1.0 / 120.0);
    assert!(!rt.state.pressed, "the overflowing part of the child isn't hit");
    rt.pointer_down(50.0, 130.0);
    rt.pointer_up(50.0, 130.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.state.pressed, "the visible part still is");
}

#[test]
fn an_explicit_soft_clip_still_wins() {
    let gpu = Gpu::headless();
    let mut rt = Runtime::new(S { soft: true, ..S::default() }, view, (100.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let px = pixels(&gpu, &mut rt, (100, 200));
    let edge = alpha(&px, 100, 50, 136);
    assert!(edge > 0 && edge < 128, "16 pt fade at the bottom edge: {edge}");
    assert_eq!(alpha(&px, 100, 50, 160), 0);
    assert!(rt.instances.iter().all(|i| i.fade[3] == 0));
}
