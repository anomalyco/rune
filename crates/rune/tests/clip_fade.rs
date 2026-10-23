use rune::{Color, Cx, El, Env, Runtime, col, el, text};
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};

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

#[test]
fn narrower_nested_edge_and_streaming_mask_survive_outer_fades() {
    use rune::render::unpack_fade;
    let gpu = Gpu::headless();
    for scale in [1.0, 2.0] {
        fn nested(outer: &bool, _: &mut Cx<bool>) -> El {
            col().size(80.0, 200.0).clip_fade(0.0, if *outer { 24.0 } else { 0.0 }, 0.0, 0.0)
                .child(el().h(40.0))
                .child(col().size(80.0, 80.0).clip_fade(0.0, 8.0, 0.0, 0.0)
                    .child(el().size(80.0, 80.0).fill(Color::WHITE)))
        }
        let size = ((80.0 * scale) as u32, (200.0 * scale) as u32);
        let env = Env { scale, system_dark: true };
        let mut plain = Runtime::new(false, nested, (80.0, 200.0), env);
        let mut outer = Runtime::new(true, nested, (80.0, 200.0), env);
        plain.frame(0.0); outer.frame(0.0);
        let a = pixels(&gpu, &mut plain, size);
        let b = pixels(&gpu, &mut outer, size);
        assert_eq!(a, b, "8px inner edge remains inside the finished 24px ancestor ramp");
        assert!(b[(((40.0 * scale) as u32 * size.0 + 20) * 4 + 3) as usize] < 30);

        fn streaming(s: &(bool, f32), _: &mut Cx<(bool, f32)>) -> El {
            col().size(80.0, 200.0).clip_fade(0.0, 0.0, 0.0, if s.0 { 24.0 } else { 0.0 })
                .child(col().w_full().growth(rune::el::GrowthSpec { edge: rune::growth::edge::FAST_FEATHER, ..Default::default() })
                    .child(col().key("row").w_full().streaming(true)
                        .child(el().size(80.0, s.1).fill(Color::WHITE))))
        }
        let render = |faded| {
            let mut rt = Runtime::new((faded, 20.0), streaming, (80.0, 200.0), env);
            rt.frame(0.0);
            rt.update(|s, _| s.1 = 80.0);
            rt.frame(1.0 / 60.0);
            let instance = rt.instances.iter().find(|i| i.fill[3] == 1.0).unwrap();
            let (width, offset) = unpack_fade(instance.fade);
            assert!(width[3] > 0.0 && width[3] < 24.0 * scale);
            assert_eq!(offset[3], 0.0, "row_mask belongs to the growing row's bottom");
            pixels(&gpu, &mut rt, size)
        };
        assert_eq!(render(false), render(true), "streaming row mask survives a transcript's larger bottom fade");
    }
}

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
                for instance in &mut rt.instances { instance.fade[2] = 0; }
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
