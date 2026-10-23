use rune::{Color, Cx, El, Env, Runtime, col, el, text};
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};

fn view(faded: &bool, _: &mut Cx<bool>) -> El {
    let row = || col().size(120.0, 20.0).clip_fade(0.0, 0.0, 8.0, 0.0)
        .child(text("MMMMMMMMMMMMMM").color(Color::WHITE));
    col().size(120.0, 100.0).clip_fade(0.0, if *faded { 16.0 } else { 0.0 }, 0.0, if *faded { 16.0 } else { 0.0 })
        .child(row()).child(el().h(20.0)).child(row()).child(el().h(20.0)).child(row())
}

#[test]
fn nested_row_text_keeps_full_alpha_in_the_middle_of_a_faded_list() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    for scale in [1.0, 2.0] {
        let (w, h) = ((120.0 * scale) as u32, (100.0 * scale) as u32);
        let target = Offscreen::new(&gpu.device, w, h);
        let mut render = |faded, old_bug| {
            let mut rt = Runtime::new(faded, view, (120.0, 100.0), Env { system_dark: true, scale });
            rt.frame(0.0);
            if old_bug {
                for instance in &mut rt.instances { instance.fade_origin = instance.clip; }
            }
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut encoder, &target.view, [0.0; 4]);
            target.copy(&mut encoder);
            gpu.queue.submit([encoder.finish()]);
            target.read(&gpu.device)
        };
        let plain = render(false, false);
        let faded = render(true, false);
        let broken = render(true, true);
        let middle = ((40.0 * scale) as usize * w as usize * 4)..((60.0 * scale) as usize * w as usize * 4);
        assert_eq!(plain[middle.clone()], faded[middle.clone()], "the row's own right fade must not restart top/bottom fades");
        let alpha = |pixels: &[u8]| pixels.chunks_exact(4).map(|p| u64::from(p[3])).sum::<u64>();
        assert!(alpha(&faded[middle.clone()]) > 0);
        assert!(alpha(&broken[middle.clone()]) < alpha(&faded[middle]) / 2, "fixture must catch the old washout");
        let top = 0..(20.0 * scale) as usize * w as usize * 4;
        assert!(alpha(&faded[top.clone()]) < alpha(&plain[top]), "the ancestor fade still acts at the list edge");
    }
}
