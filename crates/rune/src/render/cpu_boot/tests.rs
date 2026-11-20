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

#[test]
fn plus_lighter_overlapping_opaque_rects_match_gpu() {
    use crate::render::Blend;

    let (w, h) = (64u32, 48u32);
    let clear = Color { r: 0.04, g: 0.06, b: 0.1, a: 1.0 }.premul_srgb(1.0);
    let instances = [
        rect([0.0, 0.0, w as f32, h as f32], [0.05, 0.1, 0.2, 1.0], NO_CLIP),
        Instance::begin_layer(
            [4.0, 4.0, 56.0, 40.0],
            NO_CLIP,
            LayerSpec { opacity: 0.7, blend: Blend::PlusLighter, ..Default::default() },
        ),
        // Two overlapping opaque primaries inside a PlusLighter layer: red + blue must sum to
        // magenta [1, 0, 1, 2] -> clamped [1, 0, 1, 1], never let the second rect occlude the first.
        rect([8.0, 8.0, 28.0, 24.0], [1.0, 0.0, 0.0, 1.0], NO_CLIP),
        rect([20.0, 12.0, 28.0, 24.0], [0.0, 0.0, 1.0, 1.0], NO_CLIP),
        // Two overlapping translucent bright rects whose sum saturates past 1.0.
        rect([10.0, 28.0, 24.0, 12.0], [0.8, 0.25, 0.0, 0.8], NO_CLIP),
        rect([18.0, 28.0, 24.0, 12.0], [0.8, 0.25, 0.0, 0.8], NO_CLIP),
        Instance::end_layer(),
    ];

    let atlas = Atlas::new();
    let images = ImageAtlas::default();
    let gpu = Gpu::headless();
    let half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);
    let scene = Scene::new(&instances, &atlas, &images, (w, h), clear).srgb_blend(true).half_rounding(half);
    let mut cpu_px = vec![0u8; (w * h * 4) as usize];
    paint(&scene, &mut Target::new(&mut cpu_px, w, h), 1).expect("CPU paints PlusLighter layers");
    let gpu_px = render_gpu_bgra(&gpu, &instances, (w, h), clear);

    let at = |buf: &[u8], x: u32, y: u32| -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    };
    let mut max_diff = 0u8;
    let mut worst = None;
    for y in 0..h {
        for x in 0..w {
            let (c, g) = (at(&cpu_px, x, y), at(&gpu_px, x, y));
            let d = (0..4).map(|k| c[k].abs_diff(g[k])).max().unwrap();
            if d > max_diff {
                max_diff = d;
                worst = Some((x, y, c, g));
            }
        }
    }
    assert!(
        max_diff <= 1,
        "CPU boot and GPU drifted on PlusLighter layer: max_diff={max_diff} worst={worst:?} (overlap (24,20) cpu={:?} gpu={:?})",
        at(&cpu_px, 24, 20),
        at(&gpu_px, 24, 20),
    );
}

#[test]
fn plus_lighter_turned_and_scaled_layers_saturate_before_interpolation() {
    use crate::render::{Blend, OFFSCREEN_FORMAT, pack_u64_f32};

    let (w, h) = (64u32, 48u32);
    let gpu = Gpu::headless();
    let render = |renderer: &mut Renderer, instances: &[Instance]| -> Vec<u8> {
        let mut atlas = Atlas::new();
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, instances, (w, h));
        let target = Offscreen::with_format(&gpu.device, w, h, OFFSCREEN_FORMAT);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0; 4]);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        target.read(&gpu.device)
    };
    let max_diff = |a: &[u8], b: &[u8]| -> (u8, Option<(u32, u32, [u8; 4], [u8; 4])>) {
        let mut max_d = 0u8;
        let mut worst = None;
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let pa = [a[i], a[i + 1], a[i + 2], a[i + 3]];
                let pb = [b[i], b[i + 1], b[i + 2], b[i + 3]];
                let d = (0..4).map(|k| pa[k].abs_diff(pb[k])).max().unwrap();
                if d > max_d {
                    max_d = d;
                    worst = Some((x, y, pa, pb));
                }
            }
        }
        (max_d, worst)
    };

    // 1. Turned PlusLighter layer: two overlapping [0.8, 0.2, 0.0, 0.8] rects saturate to
    //    [1.0, 0.4, 0.0, 1.0], so rotating the layer must match a single [1.0, 0.4, 0.0, 1.0] rect.
    let box_rect = [16.0, 12.0, 24.0, 24.0];
    let turned_open = Instance::begin_layer(
        box_rect,
        NO_CLIP,
        LayerSpec { opacity: 0.8, blend: Blend::PlusLighter, ..Default::default() },
    )
    .turned(0.4, [28.0, 24.0]);
    let single_turned = [turned_open, rect(box_rect, [1.0, 0.4, 0.0, 1.0], NO_CLIP), Instance::end_layer()];
    let doubled_turned = [
        turned_open,
        rect(box_rect, [0.8, 0.2, 0.0, 0.8], NO_CLIP),
        rect(box_rect, [0.8, 0.2, 0.0, 0.8], NO_CLIP),
        Instance::end_layer(),
    ];
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let a = render(&mut renderer, &single_turned);
    let b = render(&mut renderer, &doubled_turned);
    let (diff_turned, worst_turned) = max_diff(&a, &b);

    // 2. Scaled cached PlusLighter layer (`uv.z > 0`): frame 1 renders a 20x20 cache with a 12x12
    //    inner saturated shape and transparent border; frame 2 composites the cached target scaled
    //    to 36x36 so bilinear resampling crosses the shape's boundary.
    let cache_frame = |node: u64, bounds: [f32; 4], children: &[Instance]| -> Vec<Instance> {
        let mut open = Instance::begin_layer(
            bounds,
            NO_CLIP,
            LayerSpec { opacity: 0.8, blend: Blend::PlusLighter, ..Default::default() },
        );
        open.params[3] = 1.0;
        open.radii = pack_u64_f32(node);
        open.uv = pack_u64_f32(1);
        let mut out = vec![open];
        out.extend_from_slice(children);
        out.push(Instance::end_layer());
        out
    };
    let inner = [12.0, 12.0, 12.0, 12.0];
    let base_bounds = [8.0, 8.0, 20.0, 20.0];
    let scaled_bounds = [8.0, 8.0, 36.0, 36.0];
    let mut r_single = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let _ = render(&mut r_single, &cache_frame(1, base_bounds, &[rect(inner, [1.0, 0.4, 0.0, 1.0], NO_CLIP)]));
    let scaled_single = render(&mut r_single, &cache_frame(1, scaled_bounds, &[]));

    let mut r_doubled = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let _ = render(
        &mut r_doubled,
        &cache_frame(
            2,
            base_bounds,
            &[rect(inner, [0.8, 0.2, 0.0, 0.8], NO_CLIP), rect(inner, [0.8, 0.2, 0.0, 0.8], NO_CLIP)],
        ),
    );
    let scaled_doubled = render(&mut r_doubled, &cache_frame(2, scaled_bounds, &[]));
    let (diff_scaled, worst_scaled) = max_diff(&scaled_single, &scaled_doubled);
    assert!(
        diff_turned <= 1 && diff_scaled <= 1,
        "PlusLighter layer interpolated unclamped texels: turned max={diff_turned} worst={worst_turned:?}; scaled max={diff_scaled} worst={worst_scaled:?}",
    );
}

#[test]
fn backdrop_blur_multithreaded_matches_single_thread() {
    let (w, h) = (320u32, 173u32);
    let clear = Color { r: 0.05, g: 0.08, b: 0.12, a: 1.0 }.premul_srgb(1.0);
    let mut instances = Vec::new();
    for y in (0..h).step_by(12) {
        for x in (0..w).step_by(12) {
            let color = if ((x / 12) + (y / 12)) % 2 == 0 {
                Color { r: 0.9, g: 0.2, b: 0.1, a: 1.0 }.premul_srgb(1.0)
            } else {
                Color { r: 0.1, g: 0.75, b: 0.85, a: 1.0 }.premul_srgb(1.0)
            };
            instances.push(rect([x as f32, y as f32, 12.0, 12.0], color, NO_CLIP));
        }
    }
    for (k, sigma) in [2.5f32, 8.0, 24.0].into_iter().enumerate() {
        let x = 10.0 + k as f32 * 96.0;
        instances.push(Instance {
            rect: [x, 12.0, 84.0, 149.0],
            radii: [6.0; 4],
            fill: [0.0, 0.0, 0.0, 1.0],
            border: [0.0; 4],
            params: [sigma, KIND_BACKDROP_BLUR, 0.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        });
    }
    let atlas = Atlas::new();
    let images = ImageAtlas::default();
    let scene = Scene::new(&instances, &atlas, &images, (w, h), clear).srgb_blend(true);
    let mut single = vec![0u8; (w * h * 4) as usize];
    let mut multi = vec![0u8; (w * h * 4) as usize];
    paint(&scene, &mut Target::new(&mut single, w, h), 1).expect("single-thread paint");
    paint(&scene, &mut Target::new(&mut multi, w, h), 4).expect("multi-thread paint");
    assert_eq!(single, multi, "multi-threaded backdrop blur must match single-threaded pixels bit-for-bit");
}

#[test]
fn move_tiles_matches_reference_across_shifts_strides_and_threads() {
    use super::bin::{TILE_H, TILE_W};
    let size = (275u32, 210u32);
    let cols = size.0.div_ceil(TILE_W);
    let tile_rows = size.1.div_ceil(TILE_H);
    let row_bytes = size.0 as usize * 4;

    for (stride, extra_tail) in [(row_bytes, 0usize), (row_bytes + 48, 0usize), (row_bytes + 48, (row_bytes + 48) * 3 + 12)] {
        let buf_len = stride * (size.1 as usize - 1) + row_bytes + extra_tail;
        let initial: Vec<u8> = (0..buf_len).map(|i| ((i.wrapping_mul(131) ^ (i >> 5)) & 0xff) as u8).collect();
        for dy in [-130i32, -64, -33, -12, -1, 1, 12, 33, 64, 130] {
            let mut clean = vec![false; (cols * tile_rows) as usize];
            for ty in 0..tile_rows {
                let y0 = ty * TILE_H;
                let y1 = ((ty + 1) * TILE_H).min(size.1);
                let in_bounds = y0 as i32 - dy >= 0 && y1 as i32 - dy <= size.1 as i32;
                if !in_bounds { continue; }
                for tx in 0..cols {
                    // Mix gaps (static sidebar at tx=0, dirty tile at tx=2 on odd rows) with contiguous runs.
                    if tx > 0 && !(tx == 2 && ty % 2 == 1) {
                        clean[(ty * cols + tx) as usize] = true;
                    }
                }
            }
            let mut expected = initial.clone();
            for ty in 0..tile_rows {
                let y0 = ty * TILE_H;
                let y1 = ((ty + 1) * TILE_H).min(size.1);
                for tx in 0..cols {
                    if !clean[(ty * cols + tx) as usize] { continue; }
                    let x0 = (tx * TILE_W) as usize * 4;
                    let x1 = ((tx + 1) * TILE_W).min(size.0) as usize * 4;
                    for y in y0..y1 {
                        let src_y = (y as i32 - dy) as usize;
                        let dst_y = y as usize;
                        expected[dst_y * stride + x0..dst_y * stride + x1]
                            .copy_from_slice(&initial[src_y * stride + x0..src_y * stride + x1]);
                    }
                }
            }
            for threads in [1usize, 4] {
                let mut actual = initial.clone();
                let mut target = Target::with_stride(&mut actual, size.0, size.1, stride);
                super::painter::move_tiles(&super::raster::Spawn(threads), &mut target, size, cols, &clean, dy);
                assert_eq!(actual, expected, "mismatch for stride={stride} tail={extra_tail} dy={dy} threads={threads}");
            }
        }
    }
}
