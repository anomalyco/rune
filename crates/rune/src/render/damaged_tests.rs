use super::*;
use crate::{Color, Cx, El, Env, Runtime, WindowBackdrop, el, text};

fn render_with_damage(
    gpu: &Gpu,
    renderer: &mut Renderer,
    rt: &mut Runtime<String>,
    size: (u32, u32),
    arm: bool,
) -> (Vec<u8>, Drawn) {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("damaged-test-target"),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let readback = rune_staging::Readback::new(&gpu.device, "damaged-test-readback", size.0, size.1, 4);
    let clear = rt.clear_color();
    renderer.set_fade_stops(rt.fade_stops());
    renderer.prepare_ui(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    if arm {
        renderer.arm_damage(&gpu.device, &gpu.queue, &[[0, 0, size.0, size.1]], clear, size, OFFSCREEN_FORMAT, true);
    }
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    if !(renderer.armed() && renderer.draw_armed(&mut encoder, &view)) {
        renderer.draw_timed(&mut encoder, &view, clear, None);
    }
    let drawn = renderer.damage_drawn();
    readback.copy_from(&mut encoder, &texture);
    renderer.submit(&gpu.queue, encoder);
    (readback.read(&gpu.device), drawn)
}

#[test]
fn transparent_window_with_active_backdrop_does_not_ghost_previous_text_under_damage() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let size = (240u32, 80u32);
    fn view(label: &String, cx: &mut Cx<String>) -> El {
        let bg = if cx.window_backdrop().is_active() {
            Color::TRANSPARENT
        } else {
            Color { r: 0.1, g: 0.1, b: 0.1, a: 1.0 }
        };
        el().w_full()
            .h_full()
            .fill(bg)
            .child(text(label.as_str()).color(Color::WHITE))
    }

    let mut rt = Runtime::new("MicaAlt".to_owned(), view, (size.0 as f32, size.1 as f32), Env::new(true, 1.0));
    rt.core_mut().transparent = true;
    rt.set_window_backdrop(WindowBackdrop::Mica);
    rt.frame(0.0);
    assert_eq!(rt.clear_color(), [0.0; 4]);

    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    // Frame 1 draws "MicaAlt" over the transparent root.
    let (frame1, drawn1) = render_with_damage(&gpu, &mut renderer, &mut rt, size, true);
    assert!(drawn1.is_err(), "translucent clear must decline partial damage");
    assert!(frame1.chunks_exact(4).any(|px| px[3] > 0), "frame 1 must draw visible glyphs");

    // Frame 2 replaces the text with shorter "Mica" over the transparent root.
    rt.update(|s, tx| {
        *s = "Mica".to_owned();
        tx.instant();
    });
    rt.frame(0.0);
    let (frame2, drawn2) = render_with_damage(&gpu, &mut renderer, &mut rt, size, true);
    assert!(drawn2.is_err(), "translucent clear must decline partial damage on frame 2");

    // Compare against a fresh render of the same "Mica" tree into a clean transparent target.
    let mut fresh_rt = Runtime::new("Mica".to_owned(), view, (size.0 as f32, size.1 as f32), Env::new(true, 1.0));
    fresh_rt.core_mut().transparent = true;
    fresh_rt.set_window_backdrop(WindowBackdrop::Mica);
    fresh_rt.frame(0.0);
    let mut fresh_renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let (fresh, _) = render_with_damage(&gpu, &mut fresh_renderer, &mut fresh_rt, size, false);
    assert!(fresh.chunks_exact(4).any(|px| px[3] > 0), "fresh render must draw visible glyphs");
    assert_ne!(frame1, fresh, "MicaAlt and Mica must differ in rendered pixels");

    assert_eq!(
        frame2, fresh,
        "transparent window with active backdrop must not retain previous frame's text pixels",
    );

    // Opaque window (clear[3] == 1.0) still takes partial damage cleanly and matches a fresh render.
    let mut opaque_rt = Runtime::new("MicaAlt".to_owned(), view, (size.0 as f32, size.1 as f32), Env::new(true, 1.0));
    opaque_rt.frame(0.0);
    assert_eq!(opaque_rt.clear_color()[3], 1.0);
    let mut opaque_renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let (_, op1) = render_with_damage(&gpu, &mut opaque_renderer, &mut opaque_rt, size, true);
    assert_eq!(op1, Ok(KeptDraw::Whole), "first kept frame draws whole and must present whole");
    opaque_rt.update(|s, tx| {
        *s = "Mica".to_owned();
        tx.instant();
    });
    opaque_rt.frame(0.0);
    let (opaque_frame2, op2) = render_with_damage(&gpu, &mut opaque_renderer, &mut opaque_rt, size, true);
    assert_eq!(op2, Ok(KeptDraw::Partial), "opaque partial damage must succeed on frame 2");
    let mut opaque_fresh_rt = Runtime::new("Mica".to_owned(), view, (size.0 as f32, size.1 as f32), Env::new(true, 1.0));
    opaque_fresh_rt.frame(0.0);
    let mut opaque_fresh_renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let (opaque_fresh, _) = render_with_damage(&gpu, &mut opaque_fresh_renderer, &mut opaque_fresh_rt, size, false);
    assert_eq!(opaque_frame2, opaque_fresh, "opaque partial damage must match fresh render");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn surface_to_empty_surface_catchup_requests_whole_present() {
    use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceFit};

    let Some(gpu) = Gpu::test_headless() else { return };
    let size = (120u32, 80u32);
    fn view(surface: &NativeSurface, _: &mut Cx<NativeSurface>) -> El {
        el().w_full()
            .h_full()
            .fill(Color { r: 0.1, g: 0.1, b: 0.1, a: 1.0 })
            .child(el().w_full().h_full().native_surface(surface, SurfaceFit::Fill))
    }

    let loaded = NativeSurface::new();
    let mut bgra = Vec::with_capacity((size.0 * size.1 * 4) as usize);
    for _ in 0..(size.0 * size.1) {
        bgra.extend_from_slice(&[0, 0, 255, 255]); // opaque red in BGRA
    }
    loaded
        .push_bytes(ByteFrame { width: size.0, height: size.1, format: Format::Bgra, color: ColorInfo::default(), planes: [bgra, vec![]] }, 1)
        .unwrap();
    let empty = NativeSurface::new();

    let mut rt = Runtime::new(loaded, view, (size.0 as f32, size.1 as f32), Env::new(true, 1.0));
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);

    let mut step = |rt: &mut Runtime<NativeSurface>, rect: [u32; 4]| -> Drawn {
        rt.frame(0.0);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("damaged-surface-test-target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let clear = rt.clear_color();
        renderer.set_fade_stops(rt.fade_stops());
        renderer.prepare_ui(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, size);
        renderer.arm_damage(&gpu.device, &gpu.queue, &[rect], clear, size, OFFSCREEN_FORMAT, true);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        if !(renderer.armed() && renderer.draw_armed(&mut encoder, &view)) {
            renderer.draw_timed(&mut encoder, &view, clear, None);
        }
        let drawn = renderer.damage_drawn();
        renderer.submit(&gpu.queue, encoder);
        drawn
    };

    // Frame 1: active NativeSurface draws whole and leaves the kept frame behind.
    assert_eq!(step(&mut rt, [0, 0, 16, 16]), Err("surface"));

    // Frame 2: switching to a new tab whose NativeSurface has no frame yet catches up the kept
    // frame by drawing whole; Windows must present the whole window, not just the 16x16 rect.
    rt.update(|s, tx| {
        *s = empty;
        tx.instant();
    });
    assert_eq!(step(&mut rt, [0, 0, 16, 16]), Ok(KeptDraw::Whole));

    // Frame 3: kept frame is now current and synced with the last whole present, so partial
    // redraw and partial present resume.
    assert_eq!(step(&mut rt, [0, 0, 16, 16]), Ok(KeptDraw::Partial));
}

fn render_target_with_usage(
    gpu: &Gpu,
    renderer: &mut Renderer,
    rt: &mut Runtime<u8>,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    rects: Option<&[[u32; 4]]>,
    copy_dst: bool,
) -> (Vec<u8>, Drawn) {
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::COPY_SRC
        | if copy_dst { wgpu::TextureUsages::COPY_DST } else { wgpu::TextureUsages::empty() };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("damaged-mode-test-target"),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let readback = rune_staging::Readback::new(&gpu.device, "damaged-mode-test-readback", size.0, size.1, 4);
    let clear = rt.clear_color();
    renderer.set_fade_stops(rt.fade_stops());
    renderer.prepare_ui(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    if let Some(rects) = rects {
        renderer.arm_damage(&gpu.device, &gpu.queue, rects, clear, size, format, copy_dst);
    }
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &view, clear);
    let drawn = renderer.damage_drawn();
    readback.copy_from(&mut encoder, &texture);
    renderer.submit(&gpu.queue, encoder);
    (readback.read(&gpu.device), drawn)
}

#[test]
fn non_copy_dst_blit_fallback_and_copy_path_match_fresh_and_clip_to_damage_rect() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let size = (160u32, 80u32);
    fn view(phase: &u8, _: &mut Cx<u8>) -> El {
        let left = if *phase == 0 {
            Color { r: 0.9, g: 0.1, b: 0.1, a: 1.0 }
        } else {
            Color { r: 0.1, g: 0.9, b: 0.1, a: 1.0 }
        };
        let right = if *phase == 0 {
            Color { r: 0.1, g: 0.2, b: 0.9, a: 1.0 }
        } else {
            Color { r: 0.95, g: 0.85, b: 0.1, a: 1.0 }
        };
        el().w_full()
            .h_full()
            .fill(Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 })
            .child(el().absolute(10.0, 16.0).size(40.0, 40.0).fill(left))
            .child(el().absolute(100.0, 16.0).size(40.0, 40.0).fill(right))
    }

    for (format, srgb_blend) in [(OFFSCREEN_FORMAT, true), (OFFSCREEN_FORMAT_LINEAR, false)] {
        for copy_dst in [true, false] {
            let mut rt = Runtime::new(0u8, view, (size.0 as f32, size.1 as f32), Env::new(true, 1.0));
            rt.core_mut().set_srgb_blend(srgb_blend);
            rt.frame(0.0);

            let mut renderer = Renderer::new(&gpu.device, format);
            // Frame 1: first armed frame catches up whole and copies or blits to target.
            let (frame1, d1) = render_target_with_usage(&gpu, &mut renderer, &mut rt, size, format, Some(&[[100, 16, 40, 40]]), copy_dst);
            assert_eq!(d1, Ok(KeptDraw::Whole), "first frame must catch up whole ({format:?}, copy_dst={copy_dst})");

            let mut fresh1_rt = Runtime::new(0u8, view, (size.0 as f32, size.1 as f32), Env::new(true, 1.0));
            fresh1_rt.core_mut().set_srgb_blend(srgb_blend);
            fresh1_rt.frame(0.0);
            let mut fresh_renderer = Renderer::new(&gpu.device, format);
            let (fresh1, _) = render_target_with_usage(&gpu, &mut fresh_renderer, &mut fresh1_rt, size, format, None, copy_dst);
            assert_eq!(frame1, fresh1, "frame 1 must match fresh render ({format:?}, copy_dst={copy_dst})");

            // Frame 2: change both boxes in the scene, but arm only the right box.
            // Scissor clipping must update the right box and keep the left box from frame 1.
            rt.update(|p, tx| {
                *p = 1;
                tx.instant();
            });
            rt.frame(0.0);
            let (clipped, d2) = render_target_with_usage(&gpu, &mut renderer, &mut rt, size, format, Some(&[[96, 12, 48, 48]]), copy_dst);
            assert_eq!(d2, Ok(KeptDraw::Partial), "frame 2 must redraw partially ({format:?}, copy_dst={copy_dst})");

            let px = |buf: &[u8], x: u32, y: u32| -> [u8; 4] {
                let at = ((y * size.0 + x) * 4) as usize;
                [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]
            };
            assert_eq!(
                px(&clipped, 30, 36),
                px(&frame1, 30, 36),
                "pixels outside the armed rect must keep frame 1 content ({format:?}, copy_dst={copy_dst})",
            );
            assert_ne!(
                px(&clipped, 120, 36),
                px(&frame1, 120, 36),
                "pixels inside the armed rect must update to frame 2 content ({format:?}, copy_dst={copy_dst})",
            );

            // Frame 3: arm both boxes so retained matches a fresh phase-1 render byte-for-byte.
            let (frame3, d3) = render_target_with_usage(
                &gpu,
                &mut renderer,
                &mut rt,
                size,
                format,
                Some(&[[8, 12, 48, 48], [96, 12, 48, 48]]),
                copy_dst,
            );
            assert_eq!(d3, Ok(KeptDraw::Partial));
            let mut fresh2_rt = Runtime::new(1u8, view, (size.0 as f32, size.1 as f32), Env::new(true, 1.0));
            fresh2_rt.core_mut().set_srgb_blend(srgb_blend);
            fresh2_rt.frame(0.0);
            let (fresh2, _) = render_target_with_usage(&gpu, &mut fresh_renderer, &mut fresh2_rt, size, format, None, copy_dst);
            assert_eq!(frame3, fresh2, "full damage update must match fresh render ({format:?}, copy_dst={copy_dst})");
        }
    }
}

#[test]
fn format_switch_and_resize_invalidate_retained_damage_on_blit_path() {
    let Some(gpu) = Gpu::test_headless() else { return };
    fn view(phase: &u8, _: &mut Cx<u8>) -> El {
        let color = if *phase == 0 {
            Color { r: 0.2, g: 0.6, b: 0.9, a: 1.0 }
        } else {
            Color { r: 0.9, g: 0.3, b: 0.2, a: 1.0 }
        };
        el().w_full()
            .h_full()
            .fill(Color { r: 0.12, g: 0.12, b: 0.14, a: 1.0 })
            .child(el().absolute(12.0, 12.0).size(48.0, 32.0).fill(color))
    }

    let size1 = (120u32, 64u32);
    let mut rt = Runtime::new(0u8, view, (size1.0 as f32, size1.1 as f32), Env::new(true, 1.0));
    rt.frame(0.0);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);

    // Frame 1 & 2 on Rgba8Unorm without COPY_DST: catch-up whole, then partial.
    let (_, d1) = render_target_with_usage(&gpu, &mut renderer, &mut rt, size1, OFFSCREEN_FORMAT, Some(&[[10, 10, 52, 36]]), false);
    assert_eq!(d1, Ok(KeptDraw::Whole));
    let (_, d2) = render_target_with_usage(&gpu, &mut renderer, &mut rt, size1, OFFSCREEN_FORMAT, Some(&[[10, 10, 52, 36]]), false);
    assert_eq!(d2, Ok(KeptDraw::Partial));

    // Blend/format switch invalidates retained texture and blit pipeline -> must catch up whole.
    renderer.set_format(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
    rt.core_mut().set_srgb_blend(false);
    rt.frame(0.0);
    let (after_fmt, d3) = render_target_with_usage(&gpu, &mut renderer, &mut rt, size1, OFFSCREEN_FORMAT_LINEAR, Some(&[[10, 10, 52, 36]]), false);
    assert_eq!(d3, Ok(KeptDraw::Whole), "format switch must invalidate retained damage texture");
    let (_, d4) = render_target_with_usage(&gpu, &mut renderer, &mut rt, size1, OFFSCREEN_FORMAT_LINEAR, Some(&[[10, 10, 52, 36]]), false);
    assert_eq!(d4, Ok(KeptDraw::Partial), "partial redraw resumes after format-switch catch-up");

    let mut fresh_rt = Runtime::new(0u8, view, (size1.0 as f32, size1.1 as f32), Env::new(true, 1.0));
    fresh_rt.core_mut().set_srgb_blend(false);
    fresh_rt.frame(0.0);
    let mut fresh_renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
    let (fresh_fmt, _) = render_target_with_usage(&gpu, &mut fresh_renderer, &mut fresh_rt, size1, OFFSCREEN_FORMAT_LINEAR, None, false);
    assert_eq!(after_fmt, fresh_fmt, "format-switch catch-up must match fresh render");

    // Resize invalidates retained texture -> must catch up whole, then resume partial.
    let size2 = (160u32, 96u32);
    rt.set_viewport((size2.0 as f32, size2.1 as f32), 1.0);
    rt.update(|p, tx| {
        *p = 1;
        tx.instant();
    });
    rt.frame(0.0);
    let (after_resize, d5) = render_target_with_usage(&gpu, &mut renderer, &mut rt, size2, OFFSCREEN_FORMAT_LINEAR, Some(&[[10, 10, 52, 36]]), false);
    assert_eq!(d5, Ok(KeptDraw::Whole), "resize must invalidate retained damage texture");
    let (_, d6) = render_target_with_usage(&gpu, &mut renderer, &mut rt, size2, OFFSCREEN_FORMAT_LINEAR, Some(&[[10, 10, 52, 36]]), false);
    assert_eq!(d6, Ok(KeptDraw::Partial), "partial redraw resumes at new size");

    let mut fresh_resized = Runtime::new(1u8, view, (size2.0 as f32, size2.1 as f32), Env::new(true, 1.0));
    fresh_resized.core_mut().set_srgb_blend(false);
    fresh_resized.frame(0.0);
    let (fresh_r, _) = render_target_with_usage(&gpu, &mut fresh_renderer, &mut fresh_resized, size2, OFFSCREEN_FORMAT_LINEAR, None, false);
    assert_eq!(after_resize, fresh_r, "resized catch-up must match fresh render");
}
