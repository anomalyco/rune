//! Public offscreen rendering helper: `Renderer::render_runtime_to_offscreen`.
use rune::native_surface::{ByteFrame, ColorInfo, Format};
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{
    Color, Cx, El, Env, NativeSurface, Pixels, Runtime, ScrollFade, ScrollSpec, SurfaceFit,
    TextStyle, col, el, text,
};

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * width + x) * 4) as usize;
    [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
}

#[test]
fn renders_boxes_text_and_pixels_to_offscreen() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (w, h) = (120u32, 80u32);
    let swatch = Pixels::new();
    swatch.set(16, 16, [0, 255, 0, 255].repeat(16 * 16));

    fn view(swatch: &Pixels, _: &mut Cx<Pixels>) -> El {
        col()
            .size(120.0, 80.0)
            .fill(Color::BLACK)
            .padding(8.0)
            .gap(6.0)
            .child(el().size(24.0, 16.0).fill(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }))
            .child(el().size(16.0, 16.0).pixels(swatch))
            .child(text("Hello").font(TextStyle::new(16.0, 600.0)).color(Color::WHITE))
    }

    let mut rt = Runtime::new(swatch, view, (w as f32, h as f32), Env::new(true, 1.0));
    rt.frame(0.0);

    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, w, h);
    renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    let rgba = target.read(&gpu.device);
    assert_eq!(rgba.len(), (w * h * 4) as usize);

    // Background corner is black.
    assert_eq!(pixel(&rgba, w, 1, 1), [0, 0, 0, 255]);
    // Red box sits at (8..32, 8..24).
    assert_eq!(pixel(&rgba, w, 16, 16), [255, 0, 0, 255]);
    // Green El::pixels swatch sits at (8..24, 30..46).
    assert_eq!(pixel(&rgba, w, 16, 38), [0, 255, 0, 255]);
    // Text row sits below y = 52; verify bright white glyph ink was rasterised and drawn.
    let mut white_ink = 0usize;
    for y in 52..h {
        for x in 8..w {
            let p = pixel(&rgba, w, x, y);
            if p[0] > 180 && p[1] > 180 && p[2] > 180 {
                white_ink += 1;
            }
        }
    }
    assert!(white_ink > 10, "expected visible text ink pixels, found {white_ink}");
}

#[test]
fn renders_native_surface_and_syncs_fade_stops() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (w, h) = (64u32, 64u32);
    let surface = NativeSurface::new();
    surface
        .push_bytes(
            ByteFrame {
                width: 16,
                height: 16,
                format: Format::Bgra,
                color: ColorInfo::default(),
                // BGRA [255, 0, 0, 255] is pure blue in RGBA.
                planes: [[255, 0, 0, 255].repeat(16 * 16), vec![]],
            },
            1,
        )
        .unwrap();

    fn view(surface: &NativeSurface, _: &mut Cx<NativeSurface>) -> El {
        col()
            .size(64.0, 64.0)
            .fill(Color::BLACK)
            .child(el().size(16.0, 16.0).native_surface(surface, SurfaceFit::Fill))
            .child(
                col()
                    .size(64.0, 48.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .scroll_fade(ScrollFade::strength(12.0).stops(&[(0.25, 0.16), (0.5, 0.5), (0.75, 0.84)]))
                    .child(el().size(64.0, 120.0).fill(Color::WHITE)),
            )
    }

    let mut rt = Runtime::new(surface, view, (w as f32, h as f32), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_ne!(rt.fade_stops(), [0.0, 0.0], "scroll fade stops were recorded by the runtime");

    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, w, h);
    renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    let rgba = target.read(&gpu.device);

    assert_eq!(renderer.fade_stops(), rt.fade_stops(), "renderer fade stops match runtime");
    assert_eq!(pixel(&rgba, w, 8, 8), [0, 0, 255, 255], "native surface rendered blue");
}

#[test]
fn supports_srgb_blend_and_transparent_clear() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (w, h) = (40u32, 40u32);
    const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };

    fn blend_view(_: &(), _: &mut Cx<()>) -> El {
        el().size(40.0, 40.0)
            .fill(RED)
            .child(el().size(20.0, 20.0).fill(Color::WHITE.alpha(0.5)))
    }

    let near = |got: [u8; 4], want: [u8; 4]| {
        assert!(
            got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1),
            "{got:?} != {want:?}"
        );
    };

    for (srgb, want) in [(true, [255, 128, 128, 255]), (false, [255, 188, 188, 255])] {
        let format = if srgb { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
        let text = TextSystem::new(&[], ("Inter", "Inter"));
        let mut rt = Runtime::with_text((), blend_view, (w as f32, h as f32), Env::new(true, 1.0), text);
        rt.set_srgb_blend(srgb);
        rt.frame(0.0);

        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.set_srgb_blend(srgb);
        let target = Offscreen::with_format(&gpu.device, w, h, format);
        renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
        let rgba = target.read(&gpu.device);
        near(pixel(&rgba, w, 10, 10), want);
    }

    // Transparent runtime clears untouched pixels to transparent black while opaque children keep alpha 255.
    fn transparent_view(_: &(), _: &mut Cx<()>) -> El {
        el().size(40.0, 40.0)
            .child(el().size(16.0, 16.0).fill(Color::WHITE))
    }
    let text = TextSystem::new(&[], ("Inter", "Inter"));
    let mut rt = Runtime::with_text((), transparent_view, (w as f32, h as f32), Env::new(true, 1.0), text);
    rt.transparent = true;
    rt.frame(0.0);

    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, w, h);
    renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    let rgba = target.read(&gpu.device);
    assert_eq!(pixel(&rgba, w, 8, 8), [255, 255, 255, 255]);
    assert_eq!(pixel(&rgba, w, 30, 30), [0, 0, 0, 0]);
}

#[test]
fn handles_empty_runtime_and_consecutive_frame_updates() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (w, h) = (32u32, 32u32);

    fn view(blue: &bool, _: &mut Cx<bool>) -> El {
        let color = if *blue {
            Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }
        } else {
            Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }
        };
        el().size(32.0, 32.0).fill(color)
    }

    let text = TextSystem::new(&[], ("Inter", "Inter"));
    let mut rt = Runtime::with_text(false, view, (w as f32, h as f32), Env::new(true, 1.0), text);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, w, h);

    // Before the first frame, instances are empty; rendering clears cleanly without panicking.
    assert!(rt.instances.is_empty());
    renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    let empty = target.read(&gpu.device);
    assert_eq!(pixel(&empty, w, 16, 16), [0, 0, 0, 255]);

    // First real frame renders red.
    rt.frame(0.0);
    renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    let red = target.read(&gpu.device);
    assert_eq!(pixel(&red, w, 16, 16), [255, 0, 0, 255]);

    // Subsequent state update on the same renderer + target renders blue.
    rt.update(|blue, tx| {
        *blue = true;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    let blue = target.read(&gpu.device);
    assert_eq!(pixel(&blue, w, 16, 16), [0, 0, 255, 255]);
}
