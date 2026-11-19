use super::bin::{Bins, Show};
use super::{CpuPainter, HalfRounding, Pixels, Scene, Target, drift, paint};
use crate::color::Color;
use crate::pixels::ImageAtlas;
use crate::render::{
    Gpu, Instance, KIND_BACKDROP_BLUR, KIND_RECT, LayerSpec, MAX_BACKDROP_BLURS, NO_CLIP, Offscreen,
    Renderer, pack_clip_radii, pack_fade,
};
use crate::text::Atlas;

fn rect(bounds: [f32; 4], fill: [f32; 4], clip: [f32; 4]) -> Instance {
    Instance {
        rect: bounds,
        radii: [0.0; 4],
        fill,
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip,
        fade: [0; 4],
    }
}

fn render_gpu_bgra(gpu: &Gpu, instances: &[Instance], size: (u32, u32), clear: [f32; 4]) -> Vec<u8> {
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(true);
    let mut atlas = Atlas::new();
    renderer.prepare_ui(&gpu.device, &gpu.queue, &mut atlas, instances, size);
    let target = Offscreen::with_format(&gpu.device, size.0, size.1, format);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, clear);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    target.read(&gpu.device)
}

#[test]
fn backdrop_blur_cap_matches_gpu() {
    let (w, h) = (256u32, 64u32);
    let clear = Color { r: 0.05, g: 0.08, b: 0.12, a: 1.0 }.premul_srgb(1.0);
    let mut instances = Vec::new();
    // High-contrast vertical stripes so blurring vs not blurring produces a clear pixel delta.
    for x in (0..w).step_by(8) {
        let color = if (x / 8) % 2 == 0 {
            Color { r: 0.9, g: 0.15, b: 0.1, a: 1.0 }.premul_srgb(1.0)
        } else {
            Color { r: 0.1, g: 0.85, b: 0.25, a: 1.0 }.premul_srgb(1.0)
        };
        instances.push(rect([x as f32, 0.0, 8.0, h as f32], color, NO_CLIP));
    }
    // Emit MAX_BACKDROP_BLURS + 2 backdrop blurs; the GPU caps at MAX_BACKDROP_BLURS.
    for k in 0..(MAX_BACKDROP_BLURS + 2) {
        let x = 4.0 + k as f32 * 40.0;
        instances.push(Instance {
            rect: [x, 8.0, 32.0, 48.0],
            radii: [4.0; 4],
            fill: [0.0, 0.0, 0.0, 1.0],
            border: [0.0; 4],
            params: [8.0, KIND_BACKDROP_BLUR, 0.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        });
    }

    let atlas = Atlas::new();
    let images = ImageAtlas::default();
    let gpu = Gpu::headless();
    let half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);
    let scene = Scene::new(&instances, &atlas, &images, (w, h), clear).srgb_blend(true).half_rounding(half);
    let bins = Bins::of(&scene);
    assert_eq!(bins.blurs.len(), MAX_BACKDROP_BLURS, "CPU boot must cap backdrop blurs at MAX_BACKDROP_BLURS");

    let mut cpu_px = vec![0u8; (w * h * 4) as usize];
    paint(&scene, &mut Target::new(&mut cpu_px, w, h), 1).expect("CPU paints backdrop blurs");
    let gpu_px = render_gpu_bgra(&gpu, &instances, (w, h), clear);
    let d = drift(
        Pixels::bgra(&cpu_px, w, h, w as usize * 4),
        Pixels::bgra(&gpu_px, w, h, w as usize * 4),
    );
    assert_eq!(d.over_two, 0, "CPU and GPU backdrop blur frames drifted: {d:?}");
}

#[test]
fn scissor_clipped_layer_flattens_and_matches_gpu() {
    let (w, h) = (128u32, 64u32);
    let clear = Color { r: 0.08, g: 0.1, b: 0.14, a: 1.0 }.premul_srgb(1.0);
    let atlas = Atlas::new();
    let images = ImageAtlas::default();

    // 1. Planning rules match GPU `layers/plan.rs` `is_scissor_clip`:
    //    - integer-aligned square clip with zero fade flattens (`Show::Flat`)
    //    - fractional clip isolates (`Show::Render`)
    //    - soft-faded or rounded clip isolates (`Show::Render`)
    let child = rect([0.0, 0.0, 128.0, 64.0], Color { r: 0.8, g: 0.25, b: 0.15, a: 1.0 }.premul_srgb(1.0), NO_CLIP);
    let int_clip = [16.0, 16.0, 80.0, 48.0];
    let mut soft_layer = Instance::begin_layer([0.0, 0.0, 128.0, 64.0], int_clip, LayerSpec::default());
    soft_layer.fade = pack_fade([8.0, 0.0, 0.0, 0.0], int_clip, int_clip);
    let mut rounded_layer = Instance::begin_layer([0.0, 0.0, 128.0, 64.0], int_clip, LayerSpec::default());
    rounded_layer.fade[3] = pack_clip_radii(int_clip, [6.0; 4]);
    let plan_instances = [
        Instance::begin_layer([0.0, 0.0, 128.0, 64.0], int_clip, LayerSpec::default()),
        child,
        Instance::end_layer(),
        Instance::begin_layer([0.0, 0.0, 128.0, 64.0], [16.5, 16.0, 80.0, 48.0], LayerSpec::default()),
        child,
        Instance::end_layer(),
        soft_layer,
        child,
        Instance::end_layer(),
        rounded_layer,
        child,
        Instance::end_layer(),
    ];
    let plan_scene = Scene::new(&plan_instances, &atlas, &images, (w, h), clear).srgb_blend(true);
    let plan_bins = Bins::of(&plan_scene);
    assert_eq!(plan_bins.layers[0].show, Show::Flat, "integer square clip with no fade must flatten");
    assert_eq!(plan_bins.layers[1].show, Show::Render, "fractional clip must isolate");
    assert_eq!(plan_bins.layers[2].show, Show::Render, "soft-faded clip must isolate");
    assert_eq!(plan_bins.layers[3].show, Show::Render, "rounded clip must isolate");

    // 2. Flattened scissor-clipped layers (including nested) clip children accurately and match GPU.
    let instances = [
        // Outer scissor-clipped flat layer: [16, 16, 80, 48]
        Instance::begin_layer([0.0, 0.0, 128.0, 64.0], [16.0, 16.0, 80.0, 48.0], LayerSpec::default()),
        // Unclipped child larger than the layer's clip: must stay strictly inside [16, 16, 80, 48].
        rect([0.0, 0.0, 128.0, 64.0], Color { r: 0.75, g: 0.2, b: 0.15, a: 0.8 }.premul_srgb(1.0), NO_CLIP),
        // Nested scissor-clipped flat layer: [32, 8, 96, 40] -> effective intersection [32, 16, 80, 40].
        Instance::begin_layer([0.0, 0.0, 128.0, 64.0], [32.0, 8.0, 96.0, 40.0], LayerSpec::default()),
        rect([0.0, 0.0, 128.0, 64.0], Color { r: 0.15, g: 0.7, b: 0.9, a: 0.65 }.premul_srgb(1.0), NO_CLIP),
        Instance::end_layer(),
        Instance::end_layer(),
    ];
    let gpu = Gpu::headless();
    let half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);
    let scene = Scene::new(&instances, &atlas, &images, (w, h), clear).srgb_blend(true).half_rounding(half);
    let mut cpu_px = vec![0u8; (w * h * 4) as usize];
    paint(&scene, &mut Target::new(&mut cpu_px, w, h), 1).expect("CPU paints flat scissor layers");
    let gpu_px = render_gpu_bgra(&gpu, &instances, (w, h), clear);
    let d = drift(
        Pixels::bgra(&cpu_px, w, h, w as usize * 4),
        Pixels::bgra(&gpu_px, w, h, w as usize * 4),
    );
    assert_eq!(d.over_two, 0, "CPU and GPU scissor-clipped flat layers drifted: {d:?}");
    // Pixel (4, 4) is outside the outer scissor: must keep the clear colour.
    let at = |buf: &[u8], x: u32, y: u32| -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    };
    let bg_px = at(&cpu_px, 4, 4);
    assert_eq!(bg_px, at(&gpu_px, 4, 4));
    // Pixel (85, 30) is outside outer [16..80] even though inside nested [32..96]: must be clear.
    assert_eq!(at(&cpu_px, 85, 30), bg_px, "outer scissor must clip nested flat layer");
    // Pixel (40, 24) is inside both clips: must differ from clear.
    assert_ne!(at(&cpu_px, 40, 24), bg_px);

    // 3. Incremental `CpuPainter` repaints tiles when a flattened layer's scissor shrinks even if
    //    the child instance itself didn't change.
    let mut painter = CpuPainter::new(1);
    let mut retained = vec![0u8; (w * h * 4) as usize];
    let frame1 = [
        Instance::begin_layer([0.0, 0.0, 128.0, 64.0], [16.0, 16.0, 80.0, 48.0], LayerSpec::default()),
        child,
        Instance::end_layer(),
    ];
    let frame2 = [
        Instance::begin_layer([0.0, 0.0, 128.0, 64.0], [16.0, 16.0, 48.0, 48.0], LayerSpec::default()),
        child,
        Instance::end_layer(),
    ];
    painter.paint(&Scene::new(&frame1, &atlas, &images, (w, h), clear).srgb_blend(true), &mut Target::new(&mut retained, w, h)).unwrap();
    assert_ne!(at(&retained, 60, 30), bg_px, "frame 1 paints inside [16..80]");
    painter.paint(&Scene::new(&frame2, &atlas, &images, (w, h), clear).srgb_blend(true), &mut Target::new(&mut retained, w, h)).unwrap();
    assert_eq!(at(&retained, 60, 30), bg_px, "frame 2 must repaint pixel exposed when flat scissor shrinks");
}
