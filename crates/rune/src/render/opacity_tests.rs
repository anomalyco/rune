//! Regression tests proving layer opacity flattening (`end_layer`) and GPU springs
//! (`vs_springs`) scale only colour/opacity fields on every `Kind`, leaving non-colour payloads
//! (wrapped image placement rects in `border`, Windfoil outline glyph records in `border`,
//! effect marks, layer pivots, etc.) intact.

use super::instance::OpacityFields;
use super::{
    Gpu, Instance, KIND_BACKDROP_BLUR, KIND_DASHED, KIND_EFFECT, KIND_GLYPH, KIND_GRADIENT,
    KIND_IMAGE, KIND_INSET, KIND_LAYER, KIND_RADIAL, KIND_RECT, KIND_SEGMENT, KIND_SHADOW,
    KIND_TRIANGLE, Kind, LayerSpec, NO_CLIP, OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR,
    Offscreen, Renderer, cpu_boot, end_layer,
};

#[test]
fn faded_wrapped_and_tiled_images_keep_exact_placement_in_end_layer_springs_and_cpu_boot() {
    use crate::el::{El, el, row};
    use crate::gpu::springs::{Node, Track};
    use crate::pixels::Pixels;
    use crate::runtime::{Cx, Env, Runtime};
    use crate::{ImageFilter, PixelsWrap};
    use rune_motion::Spring;

    let (w, h) = (96u32, 24u32);
    // 4x4 asymmetric tile: every (x, y) has a distinct (R, G) colour so any shift or shrink
    // changes pixels immediately.
    let tile = Pixels::new();
    let mut rgba = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4u8 {
        for x in 0..4u8 {
            rgba.extend_from_slice(&[x * 75 + 20, y * 75 + 20, 40, 255]);
        }
    }
    tile.set(4, 4, rgba);

    // Four lone wrapped/tiled elements, each inside its own single-child opacity layer so
    // `end_layer` folds the layer opacity directly onto the single `KIND_IMAGE` instance.
    fn view(s: &(Pixels, f32), _: &mut Cx<(Pixels, f32)>) -> El {
        let (px, opacity) = s;
        let wrap_box = |inner: El| el().size(24.0, 24.0).layer_opacity(*opacity).child(inner);
        row()
            .child(wrap_box(el().size(24.0, 24.0).pixels_tile(px, [8.0, 8.0]).image_filter(ImageFilter::Nearest)))
            .child(wrap_box(el().size(24.0, 24.0).pixels_wrap(px, [8.0, 8.0, 8.0, 8.0], PixelsWrap::Repeat).image_filter(ImageFilter::Nearest)))
            .child(wrap_box(el().size(24.0, 24.0).pixels_wrap(px, [8.0, 8.0, 8.0, 8.0], PixelsWrap::Mirror).image_filter(ImageFilter::Nearest)))
            .child(wrap_box(el().size(24.0, 24.0).pixels_wrap(px, [8.0, 8.0, 8.0, 8.0], PixelsWrap::Extend).image_filter(ImageFilter::Nearest)))
    }

    let mut rt_full = Runtime::new((tile.clone(), 1.0), view, (w as f32, h as f32), Env::new(true, 1.0));
    rt_full.frame(0.0);
    let mut rt_half = Runtime::new((tile.clone(), 0.5), view, (w as f32, h as f32), Env::new(true, 1.0));
    rt_half.frame(0.0);

    // Every single-child opacity layer must flatten via `end_layer` with no `KIND_LAYER` left.
    assert!(rt_half.instances.iter().all(|i| i.params[1] != KIND_LAYER), "single-child layers must flatten");
    let full_imgs: Vec<_> = rt_full.instances.iter().filter(|i| i.params[1] == KIND_IMAGE).collect();
    let half_imgs: Vec<_> = rt_half.instances.iter().filter(|i| i.params[1] == KIND_IMAGE).collect();
    assert_eq!(full_imgs.len(), 4);
    assert_eq!(half_imgs.len(), 4);
    for (idx, (a, b)) in full_imgs.iter().zip(&half_imgs).enumerate() {
        assert_eq!(b.border, a.border, "panel {idx}: end_layer corrupted wrapped image border rect");
        assert_eq!(b.params[3], a.params[3], "panel {idx}: end_layer corrupted wrapped image mode");
        assert!((b.fill[3] - a.fill[3] * 0.5).abs() < 1e-6, "panel {idx}: fill.a must scale by 0.5");
    }

    let Some(gpu) = Gpu::test_headless() else { return };
    let clear = [0.0; 4];

    for format in [OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR] {
        let draw_rt = |rt: &mut Runtime<(Pixels, f32)>| -> Vec<u8> {
            let mut r = Renderer::new(&gpu.device, format);
            r.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
            r.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            let target = Offscreen::with_format(&gpu.device, w, h, format);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            r.draw(&mut enc, &target.view, clear);
            target.copy(&mut enc);
            r.submit(&gpu.queue, enc);
            target.read(&gpu.device)
        };
        let px_full = draw_rt(&mut rt_full);
        let px_half = draw_rt(&mut rt_half);

        // Over a transparent clear, fading by 0.5 must scale pixel intensity/alpha only; placement
        // must stay identical to the pixel across Repeat, Mirror, Extend, and pixels_tile.
        let srgb_blend = format == OFFSCREEN_FORMAT;
        for y in 0..h {
            for x in 0..w {
                let at = ((y * w + x) * 4) as usize;
                let a = &px_full[at..at + 4];
                let b = &px_half[at..at + 4];
                let want_a = ((a[3] as f32) * 0.5).round() as u8;
                assert!(b[3].abs_diff(want_a) <= 2, "{format:?} ({x},{y}) alpha: got {} want {want_a}", b[3]);
                for c in 0..3 {
                    let want = if srgb_blend {
                        ((a[c] as f32) * 0.5).round() as u8
                    } else {
                        let lin = crate::color::srgb_decode(a[c] as f32 / 255.0) * 0.5;
                        (crate::color::srgb_encode(lin) * 255.0).round() as u8
                    };
                    assert!(
                        b[c].abs_diff(want) <= 2,
                        "{format:?} ({x},{y}) ch {c}: full={a:?} half={b:?} want={want}",
                    );
                }
            }
        }

        // GPU springs shader path: animating opacity (Spring from 1.0 -> 0.25 sampled mid-flight
        // at t = 0.08) must match CPU end_layer placement to the pixel.
        let zero = Track::constant(0.0);
        let spring = Spring::DEFAULT;
        let anim_t = 0.08;
        let anim_alpha = spring.sample(1.0, 0.0, 0.25, anim_t as f32).0.clamp(0.0, 1.0);
        assert!(anim_alpha > 0.3 && anim_alpha < 0.9, "mid-fade alpha={anim_alpha}");
        let mut rt_anim = Runtime::new((tile.clone(), anim_alpha), view, (w as f32, h as f32), Env::new(true, 1.0));
        rt_anim.frame(0.0);
        let px_cpu_anim = draw_rt(&mut rt_anim);

        let node = Node { x: zero, y: zero, opacity: Track::new(spring, 0.0, 1.0, 0.25, 0.0, 0.001) };
        let ids = vec![0u32; rt_full.instances.len()];
        let mut r_spring = Renderer::with_springs(&gpu.device, format, &[node], &ids);
        r_spring.prepare(&gpu.device, &gpu.queue, &mut rt_full.text.atlas, &rt_full.instances, (w, h));
        r_spring.upload_images(&gpu.device, &gpu.queue, &mut rt_full.images);
        let target = Offscreen::with_format(&gpu.device, w, h, format);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        r_spring.present_at(&gpu.queue, (w, h), anim_t);
        r_spring.draw(&mut enc, &target.view, clear);
        target.copy(&mut enc);
        r_spring.submit(&gpu.queue, enc);
        let px_spring_anim = target.read(&gpu.device);

        for y in 0..h {
            for x in 0..w {
                let at = ((y * w + x) * 4) as usize;
                let a = &px_cpu_anim[at..at + 4];
                let b = &px_spring_anim[at..at + 4];
                let diff = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap();
                assert!(diff <= 2, "GPU springs vs end_layer {format:?} ({x},{y}): cpu={a:?} spring={b:?}");
            }
        }

        // GPU springs translation + opacity together: shifting a wrapped image by (+4, +2) at
        // opacity 0.5 must translate both `rect.xy` and `border.xy` together without shrinking.
        let shifted_instances: Vec<Instance> = rt_half
            .instances
            .iter()
            .map(|inst| {
                let mut out = *inst;
                out.rect[0] += 4.0;
                out.rect[1] += 2.0;
                if out.params[1] == KIND_IMAGE && out.params[3] > 1.5 {
                    out.border[0] += 4.0;
                    out.border[1] += 2.0;
                }
                out
            })
            .collect();
        let (sw, sh) = (w + 8, h + 4);
        let mut r_shifted = Renderer::new(&gpu.device, format);
        r_shifted.prepare(&gpu.device, &gpu.queue, &mut rt_half.text.atlas, &shifted_instances, (sw, sh));
        r_shifted.upload_images(&gpu.device, &gpu.queue, &mut rt_half.images);
        let t_shifted = Offscreen::with_format(&gpu.device, sw, sh, format);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        r_shifted.draw(&mut enc, &t_shifted.view, clear);
        t_shifted.copy(&mut enc);
        r_shifted.submit(&gpu.queue, enc);
        let px_shifted = t_shifted.read(&gpu.device);

        let moved_node = Node {
            x: Track::constant(4.0),
            y: Track::constant(2.0),
            opacity: Track::constant(0.5),
        };
        let mut r_moved = Renderer::with_springs(&gpu.device, format, &[moved_node], &ids);
        r_moved.prepare(&gpu.device, &gpu.queue, &mut rt_full.text.atlas, &rt_full.instances, (sw, sh));
        r_moved.upload_images(&gpu.device, &gpu.queue, &mut rt_full.images);
        let t_moved = Offscreen::with_format(&gpu.device, sw, sh, format);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        r_moved.draw(&mut enc, &t_moved.view, clear);
        t_moved.copy(&mut enc);
        r_moved.submit(&gpu.queue, enc);
        let px_moved = t_moved.read(&gpu.device);

        for y in 0..sh {
            for x in 0..sw {
                let at = ((y * sw + x) * 4) as usize;
                let a = &px_shifted[at..at + 4];
                let b = &px_moved[at..at + 4];
                let diff = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap();
                assert!(diff <= 1, "GPU springs translate+fade {format:?} ({x},{y}): cpu={a:?} spring={b:?}");
            }
        }
    }

    // CPU boot painter parity on the flattened 0.5-opacity wrapped/tiled scene.
    let half = cpu_boot::HalfRounding::of_vendor(gpu.adapter.get_info().vendor);
    let scene = cpu_boot::Scene::new(&rt_half.instances, &rt_half.text.atlas, &rt_half.images, (w, h), clear)
        .srgb_blend(true)
        .half_rounding(half);
    let mut cpu_px = vec![0u8; (w * h * 4) as usize];
    cpu_boot::paint(&scene, &mut cpu_boot::Target::new(&mut cpu_px, w, h), 1).expect("cpu_boot paints");

    let bgra = wgpu::TextureFormat::Bgra8Unorm;
    let mut r_bgra = Renderer::new(&gpu.device, bgra);
    r_bgra.upload_images(&gpu.device, &gpu.queue, &mut rt_half.images);
    r_bgra.prepare(&gpu.device, &gpu.queue, &mut rt_half.text.atlas, &rt_half.instances, (w, h));
    let target = Offscreen::with_format(&gpu.device, w, h, bgra);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    r_bgra.draw(&mut enc, &target.view, clear);
    target.copy(&mut enc);
    r_bgra.submit(&gpu.queue, enc);
    let gpu_px = target.read(&gpu.device);
    let d = cpu_boot::drift(
        cpu_boot::Pixels::bgra(&cpu_px, w, h, w as usize * 4),
        cpu_boot::Pixels::bgra(&gpu_px, w, h, w as usize * 4),
    );
    assert_eq!(d.over_two, 0, "faded wrapped image CPU/GPU drift: {d:?}");
    assert!(d.max <= 1, "faded wrapped image CPU/GPU max diff > 1: {d:?}");
}

#[test]
fn every_kind_declares_opacity_fields_and_end_layer_preserves_non_colour_payloads() {
    // Exhaustive contract table: adding a new `Kind` variant causes a compile error here and in
    // `Kind::opacity_fields` until its colour/opacity fields are explicitly declared.
    let expected = |kind: Kind| -> OpacityFields {
        match kind {
            Kind::Rect => OpacityFields { fill: true, border: true, uv: false, params_w: true },
            Kind::Gradient => OpacityFields { fill: true, border: true, uv: true, params_w: true },
            Kind::Dashed | Kind::Radial => OpacityFields { fill: true, border: true, uv: false, params_w: false },
            Kind::Shadow
            | Kind::Glyph
            | Kind::Inset
            | Kind::Segment
            | Kind::Triangle
            | Kind::Image
            | Kind::Layer
            | Kind::BackdropBlur => OpacityFields { fill: true, border: false, uv: false, params_w: false },
            Kind::LayerEnd | Kind::Effect => OpacityFields { fill: false, border: false, uv: false, params_w: false },
        }
    };

    assert_eq!(Kind::from_u8(Kind::COUNT as u8), None, "Kind::COUNT must match the enum");
    for raw in 0..Kind::COUNT as u8 {
        let kind = Kind::from_u8(raw).expect("every discriminant in 0..Kind::COUNT is valid");
        let fields = kind.opacity_fields();
        assert_eq!(fields, expected(kind), "opacity_fields mismatch for {kind:?}");

        let canary = Instance {
            rect: [10.0, 20.0, 30.0, 40.0],
            radii: [4.0, 5.0, 6.0, 7.0],
            fill: [0.2, 0.4, 0.6, 0.8],
            border: [4172.0, 128.0, 64.0, 48.0],
            params: [2.0, kind.as_f32(), 5.0, 3.0],
            uv: [16.0, 24.0, 32.0, 40.0],
            clip: NO_CLIP,
            fade: [0; 4],
        };
        let alpha = 0.5f32;
        let mut scaled = canary;
        scaled.scale_opacity(alpha);

        assert_eq!(scaled.rect, canary.rect, "{kind:?}: rect must never scale");
        assert_eq!(scaled.radii, canary.radii, "{kind:?}: radii must never scale");
        assert_eq!(scaled.params[0], canary.params[0], "{kind:?}: params[0] must never scale");
        assert_eq!(scaled.params[1], canary.params[1], "{kind:?}: kind must never scale");
        assert_eq!(scaled.params[2], canary.params[2], "{kind:?}: params[2] must never scale");
        let want_fill = if fields.fill { canary.fill.map(|v| v * alpha) } else { canary.fill };
        let want_border = if fields.border { canary.border.map(|v| v * alpha) } else { canary.border };
        let want_uv = if fields.uv { canary.uv.map(|v| v * alpha) } else { canary.uv };
        let want_pw = if fields.params_w { canary.params[3] * alpha } else { canary.params[3] };
        assert_eq!(scaled.fill, want_fill, "{kind:?}: fill mismatch");
        assert_eq!(scaled.border, want_border, "{kind:?}: border mismatch");
        assert_eq!(scaled.uv, want_uv, "{kind:?}: uv mismatch");
        assert_eq!(scaled.params[3], want_pw, "{kind:?}: params[3] mismatch");

        // Also check `end_layer` on every single-primitive scope.
        let mut list = vec![
            Instance::begin_layer(canary.rect, NO_CLIP, LayerSpec { opacity: alpha, ..Default::default() }),
            canary,
        ];
        let mut marks = Vec::new();
        end_layer(&mut list, &mut [], &mut marks, 0, 0);
        if matches!(kind, Kind::Effect | Kind::BackdropBlur) {
            // Faded effect slots and backdrop blurs stay inside isolated layers.
            assert_eq!(list.len(), 3, "{kind:?} must keep an isolated layer when opacity < 1");
            assert_eq!(bytemuck::bytes_of(&list[1]), bytemuck::bytes_of(&canary), "{kind:?} inside isolated layer must stay unmodified");
        } else {
            assert_eq!(list.len(), 1, "{kind:?} must flatten cleanly via end_layer");
            assert_eq!(bytemuck::bytes_of(&list[0]), bytemuck::bytes_of(&scaled), "{kind:?}: end_layer must match Instance::scale_opacity");
        }
    }

    // Faded single video surface folds cleanly through `end_layer` without corrupting geometry.
    let mut video_list = vec![Instance::begin_layer(
        [0.0, 0.0, 160.0, 90.0],
        [4.0, 6.0, 156.0, 84.0],
        LayerSpec { opacity: 0.5, ..Default::default() },
    )];
    let mut surfaces = [crate::native_surface::SurfaceDraw {
        surface: crate::NativeSurface::new(),
        rect: [10.0, 12.0, 140.0, 70.0],
        radii: [8.0; 4],
        fit: crate::SurfaceFit::Fit,
        clip: NO_CLIP,
        fade: [0; 4],
        opacity: 0.8,
        before_instance: 1,
    }];
    let mut marks = Vec::new();
    end_layer(&mut video_list, &mut surfaces, &mut marks, 0, 0);
    assert!(video_list.is_empty(), "single-surface layer header must fold away");
    assert!((surfaces[0].opacity - 0.4).abs() < 1e-6, "video surface opacity must multiply by 0.5");
    assert_eq!(surfaces[0].before_instance, 0);
    assert_eq!(surfaces[0].rect, [10.0, 12.0, 140.0, 70.0]);
    assert_eq!(surfaces[0].clip, [4.0, 6.0, 156.0, 84.0]);
}

#[test]
fn springs_wgsl_opacity_rules_match_kind_opacity_fields_and_gpu_parity() {
    use crate::gpu::springs::{Node, Track, SHADER as SPRINGS_SHADER};

    // Guard against reintroducing a blocklist `border *= alpha` fallback in `springs.wgsl`.
    let border_scales = SPRINGS_SHADER.matches("i.border *= alpha;").count();
    assert_eq!(
        border_scales, 3,
        "springs.wgsl must allowlist border scaling only on Rect, Gradient, and Dashed/Radial",
    );
    assert!(
        SPRINGS_SHADER.contains("else if (kind > 9.5 && kind < 11.5)"),
        "springs.wgsl must positively allowlist Dashed (10) and Radial (11) for border scaling",
    );

    // Real-GPU parity: draw every rasterized primitive kind with non-zero sentinel `border`
    // values (including wrapped Repeat/Mirror/Extend images and outline-like glyph `border`)
    // through both `end_layer` and `Renderer::with_springs` at opacity 0.5.
    let Some(gpu) = Gpu::test_headless() else { return };
    let (w, h) = (192u32, 128u32);
    let mut atlas = crate::text::Atlas::new();
    atlas.data.resize((crate::text::ATLAS_SIZE * 16) as usize, 0);
    for y in 0..16usize {
        for x in 0..16usize {
            atlas.data[y * crate::text::ATLAS_SIZE as usize + x] = ((x + y) * 8).min(255) as u8;
        }
    }
    let mut images = crate::pixels::ImageAtlas::default();
    let tile = crate::pixels::Pixels::new();
    let mut tile_rgba = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4u8 {
        for x in 0..4u8 {
            tile_rgba.extend_from_slice(&[x * 75 + 20, y * 75 + 20, 40, 255]);
        }
    }
    tile.set(4, 4, tile_rgba);
    let placed = images.place(&tile, [4.0, 4.0]).expect("places tile");

    let base = [
        // Kinds whose `border` IS a colour: Rect, Gradient, Dashed, Radial.
        Instance {
            rect: [4.0, 4.0, 32.0, 24.0], radii: [4.0; 4],
            fill: [0.8, 0.2, 0.2, 0.9], border: [0.2, 0.9, 0.3, 1.0],
            params: [2.0, KIND_RECT, 0.5, 1.0], uv: [0.0; 4], clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [40.0, 4.0, 32.0, 24.0], radii: [4.0; 4],
            fill: [0.1, 0.2, 0.7, 0.8], border: [0.9, 0.8, 0.2, 1.0],
            params: [2.0, KIND_GRADIENT, 0.5, 1.0], uv: [0.7, 0.2, 0.9, 0.9], clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [76.0, 4.0, 32.0, 24.0], radii: [4.0; 4],
            fill: [0.9, 0.7, 0.1, 1.0], border: [0.2, 0.8, 0.9, 1.0],
            params: [3.0, KIND_DASHED, 0.0, 1.0], uv: [6.0, 4.0, 0.0, 0.0], clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [112.0, 4.0, 32.0, 24.0], radii: [4.0; 4],
            fill: [0.9, 0.3, 0.2, 1.0], border: [0.1, 0.4, 0.9, 0.8],
            params: [0.0, KIND_RADIAL, 0.0, 1.0], uv: [16.0, 12.0, 16.0, 12.0], clip: NO_CLIP, fade: [0; 4],
        },
        // Non-colour `border` kinds carrying non-zero sentinels: Shadow, Inset, Segment, Triangle, Glyph.
        Instance {
            rect: [4.0, 36.0, 32.0, 24.0], radii: [4.0; 4],
            fill: [0.1, 0.1, 0.2, 0.8], border: [99.0, 88.0, 77.0, 66.0],
            params: [0.0, KIND_SHADOW, 4.0, 0.0], uv: [0.0; 4], clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [40.0, 36.0, 32.0, 24.0], radii: [4.0; 4],
            fill: [0.1, 0.2, 0.4, 0.8], border: [99.0, 88.0, 77.0, 66.0],
            params: [2.0, KIND_INSET, 3.0, 0.0], uv: [1.0, 2.0, 0.0, 0.0], clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [76.0, 36.0, 32.0, 24.0], radii: [80.0, 40.0, 104.0, 56.0],
            fill: [0.9, 0.5, 0.2, 1.0], border: [99.0, 88.0, 77.0, 66.0],
            params: [2.5, KIND_SEGMENT, 1.0, 0.0], uv: [0.0; 4], clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [112.0, 36.0, 32.0, 24.0], radii: [116.0, 56.0, 136.0, 40.0],
            fill: [0.5, 0.3, 0.9, 0.9], border: [99.0, 88.0, 77.0, 66.0],
            params: [7.0, KIND_TRIANGLE, 0.0, 0.0], uv: [140.0, 56.0, 0.0, 0.0], clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [152.0, 36.0, 16.0, 16.0], radii: [0.0; 4],
            fill: [0.95, 0.95, 0.85, 1.0], border: [4172.0, 128.0, 0.25, 1.0],
            params: [0.0, KIND_GLYPH, 0.0, 1.45], uv: [0.0, 0.0, 16.0, 16.0], clip: NO_CLIP, fade: [0; 4],
        },
        // Wrapped images (Repeat, Mirror, Extend) + non-rasterized Effect slot sentinel.
        Instance {
            rect: [4.0, 72.0, 40.0, 40.0], radii: [4.0; 4],
            fill: [0.0, 0.0, 0.0, 1.0], border: [12.0, 80.0, 16.0, 16.0],
            params: [placed.page as f32, KIND_IMAGE, 4.0, 3.0],
            uv: placed.rect.map(|v| v as f32), clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [52.0, 72.0, 40.0, 40.0], radii: [4.0; 4],
            fill: [0.0, 0.0, 0.0, 1.0], border: [60.0, 80.0, 16.0, 16.0],
            params: [placed.page as f32, KIND_IMAGE, 4.0, 4.0],
            uv: placed.rect.map(|v| v as f32), clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [100.0, 72.0, 40.0, 40.0], radii: [4.0; 4],
            fill: [0.0, 0.0, 0.0, 1.0], border: [108.0, 80.0, 16.0, 16.0],
            params: [placed.page as f32, KIND_IMAGE, 4.0, 2.0],
            uv: placed.rect.map(|v| v as f32), clip: NO_CLIP, fade: [0; 4],
        },
        Instance {
            rect: [148.0, 72.0, 32.0, 32.0], radii: [0.0; 4],
            fill: [0.0, 0.0, 0.0, 1.0], border: [123.0, 456.0, 0.0, 0.0],
            params: [0.0, KIND_EFFECT, 0.0, 2.0], uv: [1.0, 2.0, 3.0, 4.0], clip: NO_CLIP, fade: [0; 4],
        },
        // Non-rasterized BackdropBlur placeholder (size 0 in uber shader when drawn directly) is
        // exercised by `every_kind_declares_opacity_fields_and_end_layer_preserves_non_colour_payloads`.
        Instance {
            rect: [0.0; 4], radii: [0.0; 4],
            fill: [0.0, 0.0, 0.0, 0.0], border: [0.0; 4],
            params: [0.0, KIND_BACKDROP_BLUR, 0.0, 1.0], uv: [0.0; 4], clip: NO_CLIP, fade: [0; 4],
        },
    ];

    let alpha = 0.5f32;
    let folded: Vec<Instance> = base
        .iter()
        .map(|&inst| {
            let mut out = inst;
            out.scale_opacity(alpha);
            out
        })
        .collect();
    let clear = [0.05, 0.07, 0.11, 1.0];
    let zero = Track::constant(0.0);
    let node = Node { x: zero, y: zero, opacity: Track::constant(alpha) };
    let ids = vec![0u32; base.len()];

    for format in [OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR] {
        atlas.dirty = Some((0, 16));
        let mut r_cpu = Renderer::new(&gpu.device, format);
        r_cpu.upload_images(&gpu.device, &gpu.queue, &mut images);
        r_cpu.prepare(&gpu.device, &gpu.queue, &mut atlas, &folded, (w, h));
        let t_cpu = Offscreen::with_format(&gpu.device, w, h, format);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        r_cpu.draw(&mut enc, &t_cpu.view, clear);
        t_cpu.copy(&mut enc);
        r_cpu.submit(&gpu.queue, enc);
        let px_cpu = t_cpu.read(&gpu.device);

        atlas.dirty = Some((0, 16));
        let mut r_spring = Renderer::with_springs(&gpu.device, format, &[node], &ids);
        r_spring.upload_images(&gpu.device, &gpu.queue, &mut images);
        r_spring.prepare(&gpu.device, &gpu.queue, &mut atlas, &base, (w, h));
        let t_spring = Offscreen::with_format(&gpu.device, w, h, format);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        r_spring.draw(&mut enc, &t_spring.view, clear);
        t_spring.copy(&mut enc);
        r_spring.submit(&gpu.queue, enc);
        let px_spring = t_spring.read(&gpu.device);

        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let a = &px_cpu[i..i + 4];
                let b = &px_spring[i..i + 4];
                let diff = (0..4).map(|k| a[k].abs_diff(b[k])).max().unwrap();
                assert!(diff <= 1, "all-kind spring parity {format:?} ({x},{y}): cpu={a:?} spring={b:?}");
            }
        }
    }
}
