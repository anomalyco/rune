use crate::color::hex;
use crate::el::Shadow;
use crate::render::cpu_boot::{self, Scene, Target};
use crate::render::{Gpu, Offscreen, Renderer};
use crate::{Cx, El, Env, Pixels, Runtime, SurfaceFit, col, el, row, rule};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn render_gpu<S: 'static>(rt: &mut Runtime<S>, w: u32, h: u32) -> Vec<u8> {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, FORMAT);
    let target = Offscreen::with_format(&gpu.device, w, h, FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0; 4]);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    target.read(&gpu.device)
}

fn render_cpu<S: 'static>(rt: &Runtime<S>, w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), [0.0; 4]);
    cpu_boot::paint(&scene, &mut Target::new(&mut px, w, h), 1).expect("cpu_boot paints");
    px
}

fn all_same_pixel(px: &[u8]) -> Option<(usize, [u8; 4], [u8; 4])> {
    let first: [u8; 4] = px[0..4].try_into().unwrap();
    for (idx, chunk) in px.chunks_exact(4).enumerate() {
        let got: [u8; 4] = chunk.try_into().unwrap();
        if got != first {
            return Some((idx, first, got));
        }
    }
    None
}

#[test]
fn zero_height_or_width_filled_elements_paint_nothing_at_fractional_offsets() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let green = || hex("#4ADE96").unwrap();
        let lanes = [0.0, 0.1, 0.2, 0.25, 0.3, 0.4, 0.45, 0.5].map(|f: f32| {
            row().w_full().h(20.0).child(el().w_full().h(0.0).mt(10.0 + f).fill(green()))
        });
        let column = row().w_full().h(40.0).child(el().w(0.0).h_full().ml(100.3).fill(green()));
        col().w_full().h_full().fill(hex("#101118").unwrap()).children(lanes).child(column)
    }

    for scale in [1.0f32, 2.0] {
        let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, scale));
        rt.frame(0.0);
        assert_eq!(rt.instances.len(), 1, "only the background should be emitted at scale {scale}");
        let (dw, dh) = ((200.0 * scale) as u32, (200.0 * scale) as u32);
        let cpu_px = render_cpu(&rt, dw, dh);
        assert_eq!(all_same_pixel(&cpu_px), None, "cpu_boot drew a non-background pixel at scale {scale}");
        let gpu_px = render_gpu(&mut rt, dw, dh);
        assert_eq!(all_same_pixel(&gpu_px), None, "GPU drew a non-background pixel at scale {scale}");
    }
}

#[test]
fn zero_area_gradients_borders_and_shadows_emit_nothing() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let bg = hex("#101118").unwrap();
        let green = hex("#4ADE96").unwrap();
        let blue = hex("#38BDF8").unwrap();
        let inset = Shadow { x: 0.0, y: 1.0, blur: 2.0, spread: 0.0, color: green, inset: true };
        let crisp_offset = Shadow { x: 2.0, y: 2.0, blur: 0.0, spread: 0.0, color: green, inset: false };
        col()
            .w_full()
            .h_full()
            .fill(bg)
            .child(el().absolute(0.0, 10.5).w(200.0).h(0.0).gradient(green, blue))
            .child(el().absolute(0.0, 20.5).w(200.0).h(0.0).fill(green).gradient(green, blue).opacity(0.6))
            .child(el().absolute(0.0, 30.5).w(200.0).h(0.0).radial_gradient([0.5, 0.5], 40.0, [(0.0, green), (1.0, blue)]))
            .child(el().absolute(0.0, 40.5).w(200.0).h(0.0).linear_gradient(1.0, [(0.0, green), (1.0, blue)]))
            .child(el().absolute(0.0, 50.5).w(200.0).h(0.0).border(1.0, green))
            .child(el().absolute(0.0, 60.5).w(200.0).h(0.0).border(1.0, green).border_dash(4.0, 2.0))
            .child(el().absolute(0.0, 70.5).w(200.0).h(0.0).fill(green).border(1.0, blue).box_shadow(inset))
            .child(el().absolute(0.0, 80.5).w(200.0).h(0.0).radius(8.0).box_shadow(crisp_offset))
            .child(el().absolute(100.5, 0.0).w(0.0).h(120.0).gradient(green, blue).border(1.0, blue))
    }

    let mut rt = Runtime::new((), view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.instances.len(), 1, "only the background should be emitted: {:?}", rt.instances);
    let cpu_px = render_cpu(&rt, 200, 120);
    assert_eq!(all_same_pixel(&cpu_px), None, "cpu_boot drew a non-background pixel");
}

#[test]
fn zero_area_guard_keeps_children_rules_outlines_and_spread_shadows() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let bg = hex("#101118").unwrap();
        let green = hex("#4ADE96").unwrap();
        let spread = Shadow { x: 0.0, y: 0.0, blur: 0.0, spread: 4.0, color: green, inset: false };
        col()
            .w_full()
            .h_full()
            .fill(bg)
            // Zero-height filled parent still paints its non-zero child.
            .child(col().w_full().h(0.0).fill(hex("#FF0000").unwrap()).child(el().size(20.0, 10.0).fill(green)))
            // Border-layout rule has positive height and still paints.
            .child(rule(0.5, green).w_full().mt(20.0))
            // Zero-size element with positive-spread shadow or outline still paints those.
            .child(el().absolute(50.0, 50.0).size(0.0, 0.0).box_shadow(spread))
            .child(el().absolute(80.0, 50.0).size(0.0, 0.0).outline(2.0, 0.0, green))
    }

    let mut rt = Runtime::new((), view, (120.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Background + child fill + rule + spread shadow + outline = 5 instances (no red parent fill).
    assert_eq!(rt.instances.len(), 5, "unexpected instances: {:?}", rt.instances);
    assert!(rt.instances.iter().all(|i| i.rect[2] > 0.0 && i.rect[3] > 0.0));
}

#[test]
fn zero_area_images_emit_nothing_and_paint_nothing() {
    fn view(px: &Pixels, _: &mut Cx<Pixels>) -> El {
        let bg = hex("#101118").unwrap();
        col()
            .w_full()
            .h_full()
            .fill(bg)
            .child(el().absolute(0.0, 10.5).w(100.0).h(0.0).pixels(px))
            .child(el().absolute(50.5, 0.0).w(0.0).h(100.0).pixels(px))
            .child(el().absolute(20.5, 20.5).size(0.0, 0.0).pixels(px))
            .child(el().absolute(0.0, 30.5).w(100.0).h(0.0).pixels_fit(px, SurfaceFit::Fit))
            .child(el().absolute(0.0, 40.5).w(100.0).h(0.0).pixels_fit(px, SurfaceFit::Fill))
    }

    let px = Pixels::new();
    px.set(4, 4, [74u8, 222, 150, 255].repeat(16));
    let mut rt = Runtime::new(px, view, (120.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.instances.len(), 1, "only the background should be emitted: {:?}", rt.instances);
    assert!(rt.instances.iter().all(|i| i.rect[2] > 0.0 && i.rect[3] > 0.0));
    let cpu_px = render_cpu(&rt, 120, 120);
    assert_eq!(all_same_pixel(&cpu_px), None, "cpu_boot drew a non-background pixel");
    let gpu_px = render_gpu(&mut rt, 120, 120);
    assert_eq!(all_same_pixel(&gpu_px), None, "GPU drew a non-background pixel");
}
