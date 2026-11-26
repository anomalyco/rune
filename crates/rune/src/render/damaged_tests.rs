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
        renderer.arm_damage(&gpu.device, &gpu.queue, &[[0, 0, size.0, size.1]], clear, size, OFFSCREEN_FORMAT);
    }
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    if !(renderer.armed() && renderer.draw_armed(&mut encoder, &texture)) {
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
        renderer.arm_damage(&gpu.device, &gpu.queue, &[rect], clear, size, OFFSCREEN_FORMAT);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        if !(renderer.armed() && renderer.draw_armed(&mut encoder, &texture)) {
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
