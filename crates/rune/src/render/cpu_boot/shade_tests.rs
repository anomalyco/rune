use super::super::{HalfRounding, Pixels, Scene, Target, drift, paint};
use super::*;
use crate::color::Color;
use crate::el::{Direction, El, Shadow, el};
use crate::pixels::ImageAtlas;
use crate::render::{Gpu, Instance, KIND_INSET, KIND_SHADOW, NO_CLIP, Offscreen, Renderer};
use crate::runtime::{Cx, Env, Runtime};
use crate::text::Atlas;

fn outer_shadow(rect: [f32; 4], radii: [f32; 4], sigma: f32) -> Instance {
    Instance {
        rect,
        radii,
        fill: [1.0, 1.0, 1.0, 1.0],
        border: [0.0; 4],
        params: [0.0, KIND_SHADOW, sigma, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

fn inset_shadow(rect: [f32; 4], radii: [f32; 4], spread: f32, sigma: f32, offset: [f32; 2]) -> Instance {
    Instance {
        rect,
        radii,
        fill: [1.0, 1.0, 1.0, 1.0],
        border: [0.0; 4],
        params: [spread, KIND_INSET, sigma, 1.0],
        uv: [offset[0], offset[1], 0.0, 0.0],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

/// Brute-force 2D Gaussian blur of a rounded box centered at the origin, evaluated at `p`.
fn gaussian_blur_box(p: [f32; 2], half: [f32; 2], radii: [f32; 4], sigma: f32) -> f32 {
    let reach = (4.0 * sigma).ceil() as i32;
    let sub = 4;
    let h = 1.0 / sub as f32;
    let inv_two_s2 = 1.0 / (2.0 * sigma * sigma);
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for dy in (-reach * sub)..=(reach * sub) {
        let oy = dy as f32 * h;
        let wy = (-f64::from(oy * oy * inv_two_s2)).exp();
        for dx in (-reach * sub)..=(reach * sub) {
            let ox = dx as f32 * h;
            let w = wy * (-f64::from(ox * ox * inv_two_s2)).exp();
            den += w;
            if sd_rounded([p[0] - ox, p[1] - oy], half, radii) <= 0.0 {
                num += w;
            }
        }
    }
    (num / den) as f32
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
fn mixed_corner_shadow_is_mirror_symmetric() {
    let rect = [20.0, 20.0, 80.0, 32.0];
    let cx = rect[0] + rect[2] * 0.5;
    let cy = rect[1] + rect[3] * 0.5;

    // Outer shadow: [4, 12, 12, 12] vs its horizontal mirror [12, 4, 12, 12].
    let left_tail = outer_shadow(rect, [4.0, 12.0, 12.0, 12.0], 2.5);
    let right_tail = outer_shadow(rect, [12.0, 4.0, 12.0, 12.0], 2.5);
    for dy in [-18.0f32, -15.0, -12.0, 0.0, 12.0, 15.0, 18.0] {
        for dx in [-43.0f32, -39.0, -35.0, -20.0, 0.0, 20.0, 35.0, 39.0, 43.0] {
            let a = boxes(&left_tail, Kind::Shadow, [cx + dx, cy + dy])[3];
            let b = boxes(&right_tail, Kind::Shadow, [cx - dx, cy + dy])[3];
            assert!(
                (a - b).abs() <= 1e-5,
                "outer shadow not mirror-symmetric at offset ({dx}, {dy}): left_tail={a}, mirrored right_tail={b}",
            );
        }
    }
    // Just outside the tight 4 px top-left corner must be darker than outside the 12 px top-right corner.
    let near_small = boxes(&left_tail, Kind::Shadow, [cx - 38.0, cy - 14.0])[3];
    let near_large = boxes(&left_tail, Kind::Shadow, [cx + 38.0, cy - 14.0])[3];
    assert!(
        near_small > near_large + 0.2,
        "tight corner must cast a fuller shadow than rounded corner: small={near_small}, large={near_large}",
    );

    // Inset shadow: [4, 12, 12, 12] vs its horizontal mirror [12, 4, 12, 12].
    let inset_left = inset_shadow(rect, [4.0, 12.0, 12.0, 12.0], 2.0, 2.5, [0.0, 2.0]);
    let inset_right = inset_shadow(rect, [12.0, 4.0, 12.0, 12.0], 2.0, 2.5, [0.0, 2.0]);
    for dy in [-14.0f32, -10.0, 0.0, 10.0, 14.0] {
        for dx in [-36.0f32, -32.0, -20.0, 0.0, 20.0, 32.0, 36.0] {
            let a = boxes(&inset_left, Kind::Inset, [cx + dx, cy + dy])[3];
            let b = boxes(&inset_right, Kind::Inset, [cx - dx, cy + dy])[3];
            assert!(
                (a - b).abs() <= 1e-5,
                "inset shadow not mirror-symmetric at offset ({dx}, {dy}): inset_left={a}, mirrored inset_right={b}",
            );
        }
    }
}

#[test]
fn mixed_corner_shadow_matches_gaussian_blur_reference() {
    let rect = [20.0, 20.0, 80.0, 32.0];
    let half = [40.0, 16.0];
    let cx = rect[0] + half[0];
    let cy = rect[1] + half[1];
    let sigma = 2.0;
    let diag = std::f32::consts::FRAC_1_SQRT_2;

    for radii in [[12.0; 4], [4.0, 12.0, 12.0, 12.0], [0.0, 12.0, 4.0, 8.0]] {
        let inst = outer_shadow(rect, radii, sigma);
        let corners = [
            (-1.0f32, -1.0f32, radii[0]),
            (1.0, -1.0, radii[1]),
            (1.0, 1.0, radii[2]),
            (-1.0, 1.0, radii[3]),
        ];
        for (sx, sy, r) in corners {
            for out in [1.0f32, 2.0, 4.0] {
                let d = r * (1.0 - diag) - out * diag;
                let p = [sx * (half[0] - d), sy * (half[1] - d)];
                let got = boxes(&inst, Kind::Shadow, [cx + p[0], cy + p[1]])[3];
                let want = gaussian_blur_box(p, half, radii, sigma);
                assert!(
                    (got - want).abs() <= 0.02,
                    "radii {radii:?}, {out} px outside corner ({sx}, {sy}) at {p:?}: got {got}, reference {want}",
                );
            }
        }
    }
}

#[test]
fn shadow_row_matches_per_pixel_shadow_for_mixed_and_equal_corners() {
    let rect = [24.0, 24.0, 96.0, 48.0];
    let cx = rect[0] + rect[2] * 0.5;
    let cases = [
        outer_shadow(rect, [12.0; 4], 3.0),
        outer_shadow(rect, [4.0, 12.0, 12.0, 12.0], 2.5),
        outer_shadow(rect, [0.0, 14.0, 6.0, 10.0], 1.5),
        Instance {
            radii: [10.0, 10.0, 10.0, 6.0],
            uv: [8.0, 8.0, 80.0, 32.0],
            ..outer_shadow(rect, [10.0; 4], 3.0)
        },
    ];
    for inst in &cases {
        for y in 12..84 {
            let py = y as f32 + 0.5;
            let row = shadow_row(inst, py);
            for x in 12..132 {
                let px = x as f32 + 0.5;
                let dx = px - cx;
                let cov = if row.inside(dx) { SHADOW_INSIDE } else { row.sum(dx) };
                let from_row = shadow_cov(inst, &row, px, cov);
                let per_px = boxes(inst, Kind::Shadow, [px, py]);
                assert_eq!(
                    from_row.map(f32::to_bits),
                    per_px.map(f32::to_bits),
                    "ShadowRow diverged from boxes at ({px}, {py}) for radii {:?} uv {:?}",
                    inst.radii,
                    inst.uv,
                );
                if inst.uv[2] <= 0.0
                    && let Some((reach, flat_cov)) = row.flat()
                    && dx.abs() <= reach
                {
                    assert_eq!(cov.to_bits(), flat_cov.to_bits(), "flat span mismatch at ({px}, {py})");
                }
            }
        }
    }
}

#[test]
fn mixed_and_rtl_mirrored_shadows_match_gpu_and_mirror_pixels() {
    let (w, h) = (180u32, 96u32);
    let clear = Color::WHITE.premul_srgb(1.0);
    let gpu = Gpu::headless();
    let half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);

    fn view(dir: &Direction, _: &mut Cx<Direction>) -> El {
        let outer = Shadow { x: 0.0, y: 3.0, blur: 10.0, spread: 0.0, color: Color { r: 0.06, g: 0.09, b: 0.16, a: 0.55 }, inset: false };
        let inner = Shadow { x: 0.0, y: 2.0, blur: 8.0, spread: 0.0, color: Color { r: 0.06, g: 0.09, b: 0.16, a: 0.45 }, inset: true };
        el().size(180.0, 96.0).dir(*dir).child(
            el().absolute(30.0, 24.0)
                .size(120.0, 48.0)
                .corners([12.0, 4.0, 12.0, 12.0])
                .fill(Color::WHITE)
                .box_shadow(outer)
                .box_shadow(inner),
        )
    }

    let capture = |dir: Direction| -> (Vec<u8>, Vec<u8>) {
        let mut rt = Runtime::new(dir, view, (w as f32, h as f32), Env { scale: 1.0, system_dark: false });
        rt.set_srgb_blend(true);
        rt.frame(0.0);
        let atlas = Atlas::new();
        let images = ImageAtlas::default();
        let scene = Scene::new(&rt.instances, &atlas, &images, (w, h), clear).srgb_blend(true).half_rounding(half);
        let mut cpu_px = vec![0u8; (w * h * 4) as usize];
        paint(&scene, &mut Target::new(&mut cpu_px, w, h), 1).expect("CPU paints mixed-corner shadows");
        let gpu_px = render_gpu_bgra(&gpu, &rt.instances, (w, h), clear);
        (cpu_px, gpu_px)
    };

    let (ltr_cpu, ltr_gpu) = capture(Direction::Ltr);
    let (rtl_cpu, rtl_gpu) = capture(Direction::Rtl);

    // CPU first frame and GPU frame must agree for both LTR and RTL.
    let d_ltr = drift(Pixels::bgra(&ltr_cpu, w, h, w as usize * 4), Pixels::bgra(&ltr_gpu, w, h, w as usize * 4));
    let d_rtl = drift(Pixels::bgra(&rtl_cpu, w, h, w as usize * 4), Pixels::bgra(&rtl_gpu, w, h, w as usize * 4));
    assert_eq!(d_ltr.over_two, 0, "LTR CPU/GPU drift: {d_ltr:?}");
    assert_eq!(d_rtl.over_two, 0, "RTL CPU/GPU drift: {d_rtl:?}");

    // The centered card's RTL render must be the horizontal mirror of its LTR render on both CPU and GPU.
    let px_at = |buf: &[u8], x: u32, y: u32| -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    };
    let mut max_gpu_mirror = 0u8;
    let mut max_cpu_mirror = 0u8;
    for y in 0..h {
        for x in 0..w {
            let mx = w - 1 - x;
            let (gl, gr) = (px_at(&ltr_gpu, x, y), px_at(&rtl_gpu, mx, y));
            let (cl, cr) = (px_at(&ltr_cpu, x, y), px_at(&rtl_cpu, mx, y));
            max_gpu_mirror = max_gpu_mirror.max((0..4).map(|k| gl[k].abs_diff(gr[k])).max().unwrap());
            max_cpu_mirror = max_cpu_mirror.max((0..4).map(|k| cl[k].abs_diff(cr[k])).max().unwrap());
        }
    }
    assert!(max_gpu_mirror <= 1, "GPU RTL shadow is not the horizontal mirror of LTR: max diff {max_gpu_mirror}");
    assert!(max_cpu_mirror <= 1, "CPU RTL shadow is not the horizontal mirror of LTR: max diff {max_cpu_mirror}");
}

#[test]
fn sub_texel_and_degenerate_image_never_panics() {
    use crate::render::{KIND_IMAGE, KIND_SEGMENT};

    let (w, h) = (32u32, 32u32);
    let clear = [0.0, 0.0, 0.0, 1.0];
    let atlas = Atlas::new();

    // Build an image atlas page with a distinct neighbour colour (red) around a blue 4x4 slot at
    // (4, 4) so any sub-texel clamp that reads `lo - 1` would visibly bleed red instead of blue.
    let mut images = ImageAtlas::default();
    let probe = crate::pixels::Pixels::new();
    probe.set(16, 16, vec![255, 0, 0, 255].repeat(16 * 16));
    let placed = images.place(&probe, [16.0, 16.0]).expect("places image");
    let page = &mut images.pages[placed.page];
    for y in 4..8 {
        for x in 4..8 {
            let at = ((placed.rect[1] + y) * page.width + (placed.rect[0] + x)) as usize * 4;
            page.data[at..at + 4].copy_from_slice(&[0, 64, 240, 255]);
        }
    }
    let ox = (placed.rect[0] + 4) as f32;
    let oy = (placed.rect[1] + 4) as f32;

    let image_inst = |rect: [f32; 4], uv: [f32; 4]| Instance {
        rect,
        radii: [0.0; 4],
        fill: [0.0, 0.0, 0.0, 1.0],
        border: [0.0; 4],
        params: [placed.page as f32, KIND_IMAGE, 0.0, 1.0],
        uv,
        clip: NO_CLIP,
        fade: [0; 4],
    };

    // Sub-texel crops (0 < uv < 1) must not panic and must sample texel `lo` (blue), never
    // bleeding the red neighbour at `lo - 1`.
    for uv in [
        [ox, oy, 0.5, 4.0],
        [ox, oy, 4.0, 0.5],
        [ox, oy, 0.5, 0.5],
        [ox, oy, 1e-4, 4.0],
        [ox, oy, 4.0, 1e-4],
        [ox, oy, 1e-6, 1e-6],
    ] {
        let inst = image_inst([4.0, 4.0, 8.0, 8.0], uv);
        let scene = Scene::new(std::slice::from_ref(&inst), &atlas, &images, (w, h), clear).srgb_blend(true);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        paint(&scene, &mut Target::new(&mut buf, w, h), 1).expect("CPU paints sub-texel image");
        // Centre pixel (8, 8) in BGRA must be the slot's blue [240, 64, 0, 255], not red [0, 0, 255, 255].
        let at = ((8 * w + 8) * 4) as usize;
        assert_eq!(&buf[at..at + 4], &[240, 64, 0, 255], "sub-texel uv={uv:?} sampled wrong texel");
    }

    // Zero, negative, NaN and infinite UV spans, origins, or box sizes must not panic and must
    // draw nothing (leaving every pixel at the clear colour [0, 0, 0, 255]).
    let mut clear_buf = vec![0u8; (w * h * 4) as usize];
    for px in clear_buf.chunks_exact_mut(4) { px.copy_from_slice(&[0, 0, 0, 255]); }

    for (rect, uv) in [
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, 0.0, 4.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, 4.0, 0.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, 0.0, 0.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, -2.0, 4.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, 4.0, -2.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, -0.5, -0.5]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, f32::NAN, 4.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, 4.0, f32::NAN]),
        ([4.0, 4.0, 8.0, 8.0], [f32::NAN, oy, 4.0, 4.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, f32::INFINITY, 4.0, 4.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, f32::INFINITY, 4.0]),
        ([4.0, 4.0, 8.0, 8.0], [ox, oy, f32::NEG_INFINITY, 4.0]),
        ([4.0, 4.0, 8.0, 8.0], [-4.0, -4.0, 2.0, 2.0]),
        ([4.0, 4.0, 8.0, 8.0], [1e20, 1e20, 4.0, 4.0]),
        ([4.0, 4.0, 0.0, 8.0], [ox, oy, 4.0, 4.0]),
        ([4.0, 4.0, 8.0, 0.0], [ox, oy, 4.0, 4.0]),
        ([4.0, 4.0, -4.0, 8.0], [ox, oy, 4.0, 4.0]),
        ([4.0, 4.0, f32::NAN, 8.0], [ox, oy, 4.0, 4.0]),
    ] {
        let inst = image_inst(rect, uv);
        let scene = Scene::new(std::slice::from_ref(&inst), &atlas, &images, (w, h), clear).srgb_blend(true);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        paint(&scene, &mut Target::new(&mut buf, w, h), 1).expect("CPU paints degenerate image");
        assert_eq!(buf, clear_buf, "degenerate image rect={rect:?} uv={uv:?} should draw nothing");
        // Also exercise `shade::image` directly for inputs `pixel_bounds` skips (like zero rect).
        let img = super::super::Image { width: images.pages[placed.page].width, data: &images.pages[placed.page].data, dirty: None };
        assert_eq!(image(&inst, &img, [8.5, 8.5]), [0.0; 4], "shade::image rect={rect:?} uv={uv:?}");
    }

    // End-to-end: SurfaceFit::Fill (or unscaled in a sub-pixel box) emits a sub-texel KIND_IMAGE
    // crop and paints through cpu_boot without panicking.
    for (bw, bh, unscaled) in [(0.4f32, 24.0f32, false), (24.0, 0.4, false), (0.4, 0.4, true)] {
        let img = crate::pixels::Pixels::new();
        img.set(8, 8, vec![20, 120, 220, 255].repeat(8 * 8));
        let mut rt = Runtime::new(
            (img, bw, bh, unscaled),
            |(img, bw, bh, unscaled), _| {
                el().size(32.0, 32.0).child(
                    el().absolute(4.0, 4.0)
                        .size(*bw, *bh)
                        .pixels_fit(img, crate::SurfaceFit::Fill)
                        .surface_unscaled(*unscaled),
                )
            },
            (32.0, 32.0),
            Env { scale: 1.0, system_dark: false },
        );
        rt.set_srgb_blend(true);
        rt.frame(0.0);
        assert!(
            rt.instances.iter().any(|i| i.kind() == Some(Kind::Image) && (i.uv[2] < 1.0 || i.uv[3] < 1.0)),
            "expected a sub-texel KIND_IMAGE crop for ({bw}, {bh})",
        );
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), clear).srgb_blend(true);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        paint(&scene, &mut Target::new(&mut buf, w, h), 1).expect("CPU paints sub-pixel SurfaceFit::Fill");
    }

    // Degenerate shadow, inset shadow and round-cap segment clamps must never panic either.
    let degenerate_boxes = [
        // Negative-height shadow still has non-empty pixel_bounds once blur expands the quad.
        outer_shadow([10.0, 10.0, 12.0, -2.0], [0.0; 4], 4.0),
        outer_shadow([10.0, 10.0, 12.0, f32::NAN], [0.0; 4], 4.0),
        // Inset shadow with NaN offset passes pixel_bounds and feeds NaN into shadow().
        inset_shadow([6.0, 6.0, 20.0, 20.0], [4.0; 4], 0.0, 4.0, [0.0, f32::NAN]),
        // Round-cap segment with NaN endpoint passes pixel_bounds and feeds NaN length into clamp.
        Instance {
            rect: [6.0, 6.0, 20.0, 20.0],
            radii: [8.0, 8.0, f32::NAN, 16.0],
            fill: [1.0, 1.0, 1.0, 1.0],
            border: [0.0; 4],
            params: [2.0, KIND_SEGMENT, 1.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        },
    ];
    for inst in &degenerate_boxes {
        let scene = Scene::new(std::slice::from_ref(inst), &atlas, &images, (w, h), clear).srgb_blend(true);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        paint(&scene, &mut Target::new(&mut buf, w, h), 1).expect("CPU paints degenerate box");
    }
}

#[test]
fn valid_image_matches_gpu_byte_for_byte() {
    use crate::render::KIND_IMAGE;

    let (w, h) = (96u32, 64u32);
    let clear = [0.06, 0.08, 0.12, 1.0];
    let gpu = Gpu::headless();
    let half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);

    let mut images = ImageAtlas::default();
    let px = crate::pixels::Pixels::new();
    let mut raw = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u8 {
        for x in 0..16u8 {
            raw.extend_from_slice(&[x * 16, y * 16, 200u8.saturating_sub(x * 8), if x == 0 { 128 } else { 255 }]);
        }
    }
    px.set(16, 16, raw);
    let placed = images.place(&px, [16.0, 16.0]).expect("places image");

    let [ux, uy, uw, uh] = placed.rect.map(|v| v as f32);
    let instances = [
        // Whole image, scaled up with rounded corners.
        Instance {
            rect: [8.0, 8.0, 32.0, 32.0],
            radii: [6.0; 4],
            fill: [0.0, 0.0, 0.0, 0.9],
            border: [0.0; 4],
            params: [placed.page as f32, KIND_IMAGE, 0.0, 1.0],
            uv: [ux, uy, uw, uh],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // Cropped fractional span (>= 1 texel).
        Instance {
            rect: [48.0, 12.0, 36.0, 40.0],
            radii: [0.0; 4],
            fill: [0.0, 0.0, 0.0, 1.0],
            border: [0.0; 4],
            params: [placed.page as f32, KIND_IMAGE, 0.0, 1.0],
            uv: [ux + 3.0, uy + 2.0, 7.5, 9.25],
            clip: NO_CLIP,
            fade: [0; 4],
        },
    ];

    let mut atlas = Atlas::new();
    let scene = Scene::new(&instances, &atlas, &images, (w, h), clear).srgb_blend(true).half_rounding(half);
    let mut cpu_px = vec![0u8; (w * h * 4) as usize];
    paint(&scene, &mut Target::new(&mut cpu_px, w, h), 1).expect("CPU paints valid images");

    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(true);
    renderer.upload_images(&gpu.device, &gpu.queue, &mut images);
    renderer.prepare_ui(&gpu.device, &gpu.queue, &mut atlas, &instances, (w, h));
    let target = Offscreen::with_format(&gpu.device, w, h, format);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, clear);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    let gpu_px = target.read(&gpu.device);

    let d = drift(Pixels::bgra(&cpu_px, w, h, w as usize * 4), Pixels::bgra(&gpu_px, w, h, w as usize * 4));
    assert_eq!(d.over_two, 0, "valid image CPU/GPU drift: {d:?}");
    assert!(d.max <= 1, "valid image CPU/GPU max diff > 1: {d:?}");
}
