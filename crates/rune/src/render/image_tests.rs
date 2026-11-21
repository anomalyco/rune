use std::sync::Arc;

use crate::pixels::Pixels;

#[test]
fn gpu_pages_interleave_with_video_and_retained_springs() {
    use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceFit};
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::{Cx, El, Env, Runtime, el};
    type State = ([Pixels; 3], NativeSurface);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().size(8.0, 8.0).pixels(&s.0[0]).child(
            el().size(6.0, 6.0).pin(Some(1.0), Some(1.0), None, None).native_surface(&s.1, SurfaceFit::Fill)
                .child(el().size(2.0, 2.0).pin(Some(1.0), Some(1.0), None, None).pixels(&s.0[1]))
                .child(el().size(1.0, 1.0).pin(Some(4.0), Some(4.0), None, None).pixels(&s.0[2])),
        )
    }
    let pixels = [Pixels::new(), Pixels::new(), Pixels::new()];
    pixels[0].set(8, 8, [255, 0, 0, 255].repeat(64));
    pixels[1].set(2, 2, [0, 255, 0, 255].repeat(4));
    pixels[2].set(1024, 1024, [255; 4].repeat(1024 * 1024));
    let surface = NativeSurface::new();
    surface.push_bytes(ByteFrame {
        width: 6,
        height: 6,
        format: Format::Bgra,
        color: ColorInfo::default(),
        planes: [[255, 0, 0, 255].repeat(36), vec![]],
    }, 1).unwrap();
    let mut rt = Runtime::new((pixels, surface), view, (8.0, 8.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.images.pages.len(), 2);
    let gpu = Gpu::headless();
    let zero = crate::gpu::springs::Track::constant(0.0);
    let node = crate::gpu::springs::Node { x: zero, y: zero, opacity: crate::gpu::springs::Track::constant(1.0) };
    let mut renderer = Renderer::with_springs(&gpu.device, OFFSCREEN_FORMAT, &[node], &vec![0; rt.instances.len()]);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (8, 8));
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (8, 8));
    let target = Offscreen::new(&gpu.device, 8, 8);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    let out = target.read(&gpu.device);
    for (x, y, want) in [(0, 0, [255, 0, 0, 255]), (1, 1, [0, 0, 255, 255]), (2, 2, [0, 255, 0, 255]), (5, 5, [255; 4])] {
        assert_eq!(&out[(y * 8 + x) * 4..(y * 8 + x + 1) * 4], &want, "at {x},{y}");
    }
}

/// An emoji in a text element reaches the screen in colour, from the image atlas.
#[test]
fn gpu_emoji_in_text_draws_in_colour() {
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::{Color, Cx, El, Env, Runtime, TextStyle, el, text};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(80.0, 24.0).fill(Color::BLACK).child(text("Hi 🎉").font(TextStyle::new(16.0, 400.0)).color(Color::WHITE))
    }
    let mut rt = Runtime::new((), view, (80.0, 24.0), Env::new(true, 2.0));
    rt.frame(0.0);
    if rt.images.pages.is_empty() {
        eprintln!("skipping: no colour emoji font installed");
        return;
    }
    let (w, h) = (160, 48);
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    let target = Offscreen::new(&gpu.device, w, h);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
    target.copy(&mut enc);
    gpu.queue.submit([enc.finish()]);
    let out = target.read(&gpu.device);
    if let Some(path) = std::env::var_os("RUNE_EMOJI_PNG") {
        crate::render::write_png(std::path::Path::new(&path), w, h, &out).unwrap();
    }
    let coloured = out.chunks(4).filter(|p| p[0].abs_diff(p[1]) > 60 || p[1].abs_diff(p[2]) > 60).count();
    assert!(coloured > 100, "only {coloured} coloured pixels");
}

#[test]
fn an_image_over_its_fill_fades_into_a_soft_edge_like_a_flat_fill() {
    use crate::color::hex;
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::{Cx, El, Env, Runtime, ScrollSpec, col, el, image::ImageCache, row};
    // A dark image on a lighter placeholder with a border, as a chat thumbnail, beside a
    // flat fill of the image's colour, under a scroll's 16 pt top fade.
    fn view(s: &Pixels, _: &mut Cx<Pixels>) -> El {
        let thumb = el().size(40.0, 60.0).radius(8.0).fill(hex("#5a5a5a").unwrap())
            .border(0.5, hex("#ffffff33").unwrap()).pixels_fit(s, crate::SurfaceFit::Fill);
        el().size(100.0, 60.0).fill(hex("#141414").unwrap()).child(
            col().w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).clip_fade(0.0, 16.0, 0.0, 8.0)
                .child(row().gap(10.0).child(thumb).child(el().size(40.0, 60.0).fill(hex("#282828").unwrap())))
                .child(el().size(4.0, 200.0)),
        )
    }
    let cache = ImageCache::new(Arc::new(|| {}));
    let image = cache.rgba(80, 120, [40u8, 40, 40, 255].repeat(80 * 120));
    image.wait().unwrap();
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 100, 60);
    let mut rt = Runtime::new(image.pixels().clone(), view, (100.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (100, 60));
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
    target.copy(&mut enc);
    gpu.queue.submit([enc.finish()]);
    let out = target.read(&gpu.device);
    let px = |x: usize, y: usize| out[(y * 100 + x) * 4];
    let (image, flat): (Vec<u8>, Vec<u8>) = (0..24).map(|y| (px(20, y), px(70, y))).unzip();
    assert!(flat[0] < flat[20], "the band fades: {flat:?}");
    assert!(image.iter().zip(&flat).all(|(a, b)| a.abs_diff(*b) <= 1), "the image fades like the fill\nimage {image:?}\nflat  {flat:?}");
}

#[test]
fn gpu_static_mips_alpha_clip_and_page_order_match_pixels() {
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::{Cx, El, Env, Runtime, el, image::ImageCache};
    type State = (Pixels, Pixels, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let base = el().size(8.0, 8.0).pixels(&s.0);
        if s.2 {
            base.child(el().size(4.0, 4.0).pin(Some(2.0), Some(2.0), None, None).pixels(&s.1))
        } else {
            base.radius(4.0).clip_fade(2.0, 0.0, 0.0, 0.0)
        }
    }
    let cache = ImageCache::new(Arc::new(|| {}));
    let checker: Vec<_> = (0..64 * 64).flat_map(|i| {
        let v = if (i % 64 + i / 64) % 2 == 0 { 0 } else { 255 };
        [v, v, v, 255]
    }).collect();
    let image = cache.rgba(64, 64, checker);
    image.wait().unwrap();
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 8, 8);
    let mut rt = Runtime::new((image.pixels().clone(), Pixels::new(), false), view, (8.0, 8.0), Env::new(true, 1.0));
    let mut draw = |rt: &mut Runtime<State>| {
        rt.frame(0.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (8, 8));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0, 0.0, 1.0, 1.0]);
        target.copy(&mut enc);
        gpu.queue.submit([enc.finish()]);
        target.read(&gpu.device)
    };
    let pixel = |out: &[u8], x: usize, y: usize| <[u8; 4]>::try_from(&out[(y * 8 + x) * 4..(y * 8 + x + 1) * 4]).unwrap();
    let near = |got: [u8; 4], want: [u8; 4]| assert!(got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2), "{got:?} != {want:?}");
    let out = draw(&mut rt);
    near(pixel(&out, 4, 4), [188, 188, 188, 255]);
    near(pixel(&out, 0, 0), [0, 0, 255, 255]);
    assert!(pixel(&out, 0, 4)[0] < pixel(&out, 3, 4)[0], "edge fade clips the image");
    assert!(rt.images.has_slot(image.pixels(), 3), "64px source uses its 8px mip");
    let alpha: Vec<_> = (0..64 * 64).flat_map(|i| if i % 2 == 0 { [255, 0, 0, 128] } else { [0, 255, 0, 0] }).collect();
    let transparent = cache.rgba(64, 64, alpha);
    transparent.wait().unwrap();
    rt.state.0 = transparent.pixels().clone();
    rt.invalidate();
    let out = draw(&mut rt);
    near(pixel(&out, 4, 4), [137, 0, 224, 255]);
    let big = Pixels::new();
    big.set(2048, 1, [255, 0, 0, 255].repeat(2048));
    let full = Pixels::new();
    full.set(1024, 1024, [0, 255, 0, 255].repeat(1024 * 1024));
    rt.state = (big, full, true);
    rt.invalidate();
    rt.settle_all();
    let out = draw(&mut rt);
    near(pixel(&out, 0, 0), [255, 0, 0, 255]);
    near(pixel(&out, 4, 4), [0, 255, 0, 255]);
    assert!(rt.images.pages.len() >= 2, "large images and full pages share painter order");
    rt.state.0.set(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    let out = draw(&mut rt);
    near(pixel(&out, 0, 0), [255, 0, 0, 255]);
    near(pixel(&out, 7, 0), [0, 0, 255, 255]);
    assert!(rt.images.pages.iter().any(|p| p.data.is_empty()), "the old large page is released between live pages");
}

#[test]
fn gpu_draws_pixels_texel_for_texel_and_follows_new_frames() {
    use crate::el::{El, el};
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::runtime::{Cx, Env, Runtime};
    fn view(px: &Pixels, _: &mut Cx<Pixels>) -> El {
        el().size(4.0, 4.0).pixels(px)
    }
    let px = Pixels::new();
    let mut rgba = Vec::new();
    for i in 0..16u8 {
        rgba.extend_from_slice(&[i * 16, 255 - i * 16, 40, 255]);
    }
    px.set(4, 4, rgba.clone());
    let mut rt = Runtime::new(px.clone(), view, (4.0, 4.0), Env::new(true, 1.0));
    let gpu = Gpu::new(Gpu::instance(), None);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 4, 4);
    let mut draw = |rt: &mut Runtime<Pixels>| {
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (4, 4));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
        target.copy(&mut enc);
        gpu.queue.submit([enc.finish()]);
        target.read(&gpu.device)
    };
    let out = draw(&mut rt);
    for (got, want) in out.chunks_exact(4).zip(rgba.chunks_exact(4)) {
        assert!(got.iter().zip(want).all(|(a, b)| a.abs_diff(*b) <= 1), "drew {got:?}, image {want:?}");
    }
    px.set(4, 4, [10, 20, 30, 255].repeat(16));
    let out = draw(&mut rt);
    assert!(out.chunks_exact(4).all(|p| p.iter().zip([10u8, 20, 30, 255]).all(|(a, b)| a.abs_diff(b) <= 1)), "a new frame shows without re-running the view: {:?}", &out[..4]);
}

#[test]
fn gpu_pixels_fit_letterboxes_and_fill_crops() {
    use crate::el::{El, el};
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::runtime::{Cx, Env, Runtime};
    type State = (Pixels, crate::SurfaceFit);
    fn view(s: &State, _: &mut Cx<State>) -> El { el().size(4.0, 4.0).pixels_fit(&s.0, s.1) }
    // Four columns, red green blue white, two rows tall.
    let px = Pixels::new();
    px.set(4, 2, [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255; 4]].concat().repeat(2));
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 4, 4);
    let mut draw = |fit| {
        let mut rt = Runtime::new((px.clone(), fit), view, (4.0, 4.0), Env::new(true, 1.0));
        rt.frame(0.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (4, 4));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
        target.copy(&mut enc);
        gpu.queue.submit([enc.finish()]);
        target.read(&gpu.device)
    };
    let at = |out: &[u8], x: usize, y: usize| <[u8; 4]>::try_from(&out[(y * 4 + x) * 4..(y * 4 + x + 1) * 4]).unwrap();
    let out = draw(crate::SurfaceFit::Fit);
    for x in 0..4 {
        assert_eq!(at(&out, x, 0), [0, 0, 0, 255], "letterbox above column {x}");
        assert_eq!(at(&out, x, 3), [0, 0, 0, 255], "letterbox below column {x}");
    }
    assert_eq!([at(&out, 0, 1), at(&out, 1, 2), at(&out, 2, 1), at(&out, 3, 2)],
        [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255; 4]], "the image draws texel for texel");
    let out = draw(crate::SurfaceFit::Fill);
    for y in 0..4 {
        assert_eq!(at(&out, 0, y), [0, 255, 0, 255], "row {y} starts on green");
        assert_eq!(at(&out, 3, y), [0, 0, 255, 255], "row {y} ends on blue");
    }
    assert!(out.as_chunks::<4>().0.iter().all(|p| p[0] < 200), "red and white columns are cropped");
}
