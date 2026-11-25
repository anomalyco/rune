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
    let Some(gpu) = Gpu::test_headless() else { return };
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
    let Some(gpu) = Gpu::test_headless() else { return };
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
    let Some(gpu) = Gpu::test_headless() else { return };
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
    let Some(gpu) = Gpu::test_headless() else { return };
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
    let Some(gpu) = Gpu::test_headless() else { return };
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
    let Some(gpu) = Gpu::test_headless() else { return };
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

#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
#[test]
fn clear_for_park_drops_image_textures_and_reuploads_cleanly() {
    use crate::el::{El, el};
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::runtime::{Cx, Env, Runtime};
    fn view(px: &Pixels, _: &mut Cx<Pixels>) -> El { el().size(4.0, 4.0).pixels(px) }
    let px = Pixels::new();
    px.set(4, 4, [240, 30, 80, 255].repeat(16));
    let mut rt = Runtime::new(px, view, (4.0, 4.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (4, 4));
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    assert!(renderer.has_images());
    renderer.clear_for_park();
    assert!(!renderer.has_images(), "parking a renderer drops its image textures");
    rt.images.invalidate();
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (4, 4));
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    let target = Offscreen::new(&gpu.device, 4, 4);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0; 4]);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    let out = target.read(&gpu.device);
    for (got, want) in out[..4].iter().zip([240u8, 30, 80, 255]) {
        assert!(got.abs_diff(want) <= 1, "re-uploaded pixel {got} != {want}");
    }
}

#[test]
fn gpu_sub_texel_image_crop_samples_own_texel_not_atlas_neighbour() {
    use crate::pixels::ImageAtlas;
    use crate::render::{
        Gpu, Instance, KIND_IMAGE, NO_CLIP, Offscreen, Renderer,
        cpu_boot::{HalfRounding, Pixels as BootPixels, Scene, Target, drift, paint},
    };
    use crate::text::Atlas;

    let (w, h) = (32u32, 32u32);
    let clear = [0.0, 0.0, 0.0, 1.0];
    let Some(gpu) = Gpu::test_headless() else { return };
    let half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);

    // Place a 16x16 red slot, then paint a 4x4 blue sub-region at (4, 4) inside it.
    // A sub-texel crop rooted at (ox, oy) whose clamp sets hi = lo - 1 bleeds the red
    // neighbour at lo - 1 instead of sampling the blue texel at lo.
    let mut images = ImageAtlas::default();
    let probe = Pixels::new();
    probe.set(16, 16, [255, 0, 0, 255].repeat(16 * 16));
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

    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(true);
    renderer.upload_images(&gpu.device, &gpu.queue, &mut images);
    let target = Offscreen::with_format(&gpu.device, w, h, format);
    let mut atlas = Atlas::new();

    for uv in [[ox, oy, 0.5, 4.0], [ox, oy, 4.0, 0.5], [ox, oy, 0.5, 0.5]] {
        let inst = Instance {
            rect: [4.0, 4.0, 8.0, 8.0],
            radii: [0.0; 4],
            fill: [0.0, 0.0, 0.0, 1.0],
            border: [0.0; 4],
            params: [placed.page as f32, KIND_IMAGE, 0.0, 1.0],
            uv,
            clip: NO_CLIP,
            fade: [0; 4],
        };
        renderer.prepare_ui(&gpu.device, &gpu.queue, &mut atlas, std::slice::from_ref(&inst), (w, h));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, clear);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        let gpu_px = target.read(&gpu.device);

        // Centre pixel (8, 8) in BGRA must be the slot's blue [240, 64, 0, 255], not red [0, 0, 255, 255].
        let at = ((8 * w + 8) * 4) as usize;
        assert_eq!(
            &gpu_px[at..at + 4],
            &[240, 64, 0, 255],
            "GPU sub-texel crop uv={uv:?} sampled wrong texel",
        );

        let scene = Scene::new(std::slice::from_ref(&inst), &atlas, &images, (w, h), clear)
            .srgb_blend(true)
            .half_rounding(half);
        let mut cpu_px = vec![0u8; (w * h * 4) as usize];
        paint(&scene, &mut Target::new(&mut cpu_px, w, h), 1).expect("CPU paints sub-texel image");
        let d = drift(
            BootPixels::bgra(&cpu_px, w, h, w as usize * 4),
            BootPixels::bgra(&gpu_px, w, h, w as usize * 4),
        );
        assert_eq!(d.over_two, 0, "sub-texel uv={uv:?} CPU/GPU drift: {d:?}");
        assert!(d.max <= 1, "sub-texel uv={uv:?} CPU/GPU max diff > 1: {d:?}");
    }
}

#[test]
fn gpu_image_filter_nearest_keeps_crisp_blocks_and_mip_zero() {
    use crate::el::{El, el, row};
    use crate::render::{Gpu, KIND_IMAGE, OFFSCREEN_FORMAT, Offscreen, Renderer, instance::shader_consts as sc};
    use crate::runtime::{Cx, Env, Runtime};
    use crate::ImageFilter;

    // 4x4 black/white checkerboard drawn at 64x64: Nearest produces 2 levels in hard 16 px blocks,
    // while default Linear smooths the transitions into many intermediate grey levels.
    let px = Pixels::new();
    let mut rgba = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4 {
        for x in 0..4 {
            let v = if (x + y) % 2 == 0 { 0u8 } else { 255u8 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    px.set(4, 4, rgba);

    let draw = |filter: ImageFilter| -> (Vec<u8>, [f32; 4]) {
        fn view(s: &(Pixels, ImageFilter), _: &mut Cx<(Pixels, ImageFilter)>) -> El {
            el().size(64.0, 64.0).pixels(&s.0).image_filter(s.1)
        }
        let mut rt = Runtime::new((px.clone(), filter), view, (64.0, 64.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let inst = rt.instances.iter().find(|i| i.params[1] == KIND_IMAGE).expect("image instance");
        let params = inst.params;
        let Some(gpu) = Gpu::test_headless() else { return (Vec::new(), params) };
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 64));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let target = Offscreen::new(&gpu.device, 64, 64);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        (target.read(&gpu.device), params)
    };

    let (near_px, near_params) = draw(ImageFilter::Nearest);
    let (lin_px, lin_params) = draw(ImageFilter::Linear);
    assert_eq!(near_params[2] as u32, sc::IMAGE_FLAG_NEAREST);
    assert_eq!(near_params[3], 1.0, "params[3] stays untouched for wrap modes");
    assert_eq!(lin_params[2] as u32, 0);
    assert_eq!(lin_params[3], 1.0);

    if !near_px.is_empty() {
        let mut near_levels = std::collections::BTreeSet::new();
        for y in 0..64usize {
            for x in 0..64usize {
                let v = near_px[(y * 64 + x) * 4];
                let expected = if ((x / 16) + (y / 16)) % 2 == 0 { 0u8 } else { 255u8 };
                assert_eq!(v, expected, "Nearest checkerboard mismatch at ({x}, {y})");
                near_levels.insert(v);
            }
        }
        assert_eq!(near_levels.len(), 2, "Nearest must produce exactly 2 grey levels");

        let lin_levels: std::collections::BTreeSet<u8> =
            lin_px.chunks_exact(4).map(|p| p[0]).collect();
        assert!(lin_levels.len() > 2, "default Linear must stay bilinear-smoothed, got {lin_levels:?}");
    }

    // Mip selection: a 64x64 mipmapped Image drawn at 16x16 uses mip 0 under Nearest and a
    // downsampled mip under Linear.
    let cache = crate::ImageCache::new(std::sync::Arc::new(|| {}));
    let big = cache.rgba(64, 64, [255, 0, 0, 255].repeat(64 * 64));
    big.wait().expect("decodes mipmapped image");
    fn mip_view(p: &Pixels, _: &mut Cx<Pixels>) -> El {
        row()
            .child(el().size(16.0, 16.0).pixels(p))
            .child(el().size(16.0, 16.0).pixels(p).image_filter(ImageFilter::Nearest))
    }
    let mut rt = Runtime::new(big.pixels().clone(), mip_view, (32.0, 16.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let imgs: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_IMAGE).collect();
    assert_eq!(imgs.len(), 2);
    assert_eq!([imgs[0].uv[2], imgs[0].uv[3]], [16.0, 16.0], "Linear uses 16x16 mip");
    assert_eq!([imgs[1].uv[2], imgs[1].uv[3]], [64.0, 64.0], "Nearest requests 64x64 mip 0");
    assert_eq!(imgs[0].params[2] as u32, sc::IMAGE_FLAG_PREMULTIPLIED);
    assert_eq!(imgs[1].params[2] as u32, sc::IMAGE_FLAG_PREMULTIPLIED | sc::IMAGE_FLAG_NEAREST);
    assert!(rt.images.has_slot(big.pixels(), 0));
    assert!(rt.images.has_slot(big.pixels(), 2));
}

#[test]
fn gpu_pixels_tint_recolors_alpha_mask_in_linear_and_srgb_blend() {
    use crate::color::Color;
    use crate::el::{El, el, row};
    use crate::render::{Gpu, KIND_IMAGE, OFFSCREEN_FORMAT, Offscreen, Renderer, instance::shader_consts as sc};
    use crate::runtime::{Cx, Env, Runtime};
    use crate::ImageFilter;

    // 4x4 dark-grey (#2e3436) symbolic icon: left half opaque, top-right half-alpha, bottom-right transparent.
    let mut icon_rgba = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4 {
        for x in 0..4 {
            let a = if x < 2 { 255 } else if y < 2 { 128 } else { 0 };
            icon_rgba.extend_from_slice(&[0x2e, 0x34, 0x36, a]);
        }
    }
    let straight = Pixels::new();
    straight.set(4, 4, icon_rgba.clone());
    let cache = crate::ImageCache::new(std::sync::Arc::new(|| {}));
    let decoded = cache.rgba(4, 4, icon_rgba);
    decoded.wait().expect("decodes icon");

    let tint_red = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    let tint_sheen = Color::WHITE.alpha(0.6);

    fn view(s: &(Pixels, Pixels, Color, Color), _: &mut Cx<(Pixels, Pixels, Color, Color)>) -> El {
        row()
            // 0..16: untinted (must stay dark grey #2e3436 on opaque pixels)
            .child(el().size(16.0, 16.0).pixels(&s.0).image_filter(ImageFilter::Nearest))
            // 16..32: straight-alpha tinted pure red
            .child(el().size(16.0, 16.0).pixels(&s.0).pixels_tint(s.2).image_filter(ImageFilter::Nearest))
            // 32..48: premultiplied mip tinted translucent white sheen
            .child(el().size(16.0, 16.0).pixels(&s.1).pixels_tint(s.3).image_filter(ImageFilter::Nearest))
            // 48..64: Color::TRANSPARENT tint draws nothing
            .child(el().size(16.0, 16.0).pixels(&s.0).pixels_tint(Color::TRANSPARENT).image_filter(ImageFilter::Nearest))
    }

    for srgb_blend in [false, true] {
        let mut rt = Runtime::new(
            (straight.clone(), decoded.pixels().clone(), tint_red, tint_sheen),
            view,
            (64.0, 16.0),
            Env::new(true, 1.0),
        );
        rt.set_srgb_blend(srgb_blend);
        rt.frame(0.0);

        let imgs: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_IMAGE).collect();
        assert_eq!(imgs.len(), 4);
        assert_eq!(imgs[0].params[2] as u32, sc::IMAGE_FLAG_NEAREST);
        assert_eq!(imgs[1].params[2] as u32, sc::IMAGE_FLAG_TINT | sc::IMAGE_FLAG_NEAREST);
        assert_eq!(imgs[2].params[2] as u32, sc::IMAGE_FLAG_PREMULTIPLIED | sc::IMAGE_FLAG_TINT | sc::IMAGE_FLAG_NEAREST);
        assert_eq!(imgs[3].params[2] as u32, sc::IMAGE_FLAG_TINT | sc::IMAGE_FLAG_NEAREST);

        let Some(gpu) = Gpu::test_headless() else { continue };
        let format = if srgb_blend { wgpu::TextureFormat::Bgra8Unorm } else { OFFSCREEN_FORMAT };
        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.set_srgb_blend(srgb_blend);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 16));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let target = Offscreen::with_format(&gpu.device, 64, 16, format);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        let out = target.read(&gpu.device);
        let rgb = |x: usize, y: usize| -> [u8; 3] {
            let p = &out[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
            if srgb_blend { [p[2], p[1], p[0]] } else { [p[0], p[1], p[2]] }
        };

        // Untinted opaque pixel keeps dark grey (#2e3436).
        let u = rgb(2, 2);
        assert!(u[0].abs_diff(0x2e) <= 2 && u[1].abs_diff(0x34) <= 2 && u[2].abs_diff(0x36) <= 2,
            "srgb_blend={srgb_blend}: untinted icon changed: {u:?}");
        // Red-tinted opaque pixel is pure red, ignoring source dark grey.
        assert_eq!(rgb(18, 2), [255, 0, 0], "srgb_blend={srgb_blend}: red tint on opaque mask");
        // Red-tinted transparent corner stays black clear colour.
        assert_eq!(rgb(28, 12), [0, 0, 0], "srgb_blend={srgb_blend}: red tint must respect 0 alpha");
        // Red-tinted half-alpha texel is strictly dimmer than opaque and strictly brighter than 0.
        let half_red = rgb(28, 2);
        assert!(half_red[0] > 80 && half_red[0] < 240 && half_red[1] == 0 && half_red[2] == 0,
            "srgb_blend={srgb_blend}: red tint on half-alpha mask: {half_red:?}");
        // Translucent white sheen (alpha 0.6) over premultiplied opaque pixel is grey, brighter than half-alpha.
        let sheen_full = rgb(34, 2);
        let sheen_half = rgb(44, 2);
        assert!(sheen_full[0] > 100 && sheen_full[0] < 230 && sheen_full[0] == sheen_full[1],
            "srgb_blend={srgb_blend}: translucent white sheen on full alpha: {sheen_full:?}");
        assert!(sheen_half[0] > 40 && sheen_half[0] < sheen_full[0],
            "srgb_blend={srgb_blend}: translucent sheen multiplies image alpha: full={sheen_full:?} half={sheen_half:?}");
        // Transparent tint draws nothing even on opaque mask texels.
        assert_eq!(rgb(50, 2), [0, 0, 0], "srgb_blend={srgb_blend}: transparent tint must draw nothing");
    }
}

#[test]
fn memo_reuse_invalidates_when_pixels_tint_or_image_filter_changes() {
    use crate::color::Color;
    use crate::el::{El, el};
    use crate::memo::memo;
    use crate::render::{KIND_IMAGE, instance::shader_consts as sc};
    use crate::runtime::{Cx, Env, Runtime};
    use crate::ImageFilter;

    struct State {
        px: Pixels,
        tint: Option<Color>,
        filter: ImageFilter,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut e = memo("img", (), || el().size(16.0, 16.0).pixels(&s.px));
        if let Some(c) = s.tint {
            e = e.pixels_tint(c);
        }
        e.image_filter(s.filter)
    }

    let px = Pixels::new();
    px.set(4, 4, vec![255; 4 * 4 * 4]);
    let mut rt = Runtime::new(
        State { px, tint: None, filter: ImageFilter::Linear },
        view,
        (16.0, 16.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let flags = |rt: &Runtime<State>| -> u32 {
        rt.instances.iter().find(|i| i.params[1] == KIND_IMAGE).unwrap().params[2] as u32
    };
    assert_eq!(flags(&rt), 0);

    rt.update(|s, _| s.filter = ImageFilter::Nearest);
    rt.frame(0.0);
    assert_eq!(flags(&rt), sc::IMAGE_FLAG_NEAREST);

    rt.update(|s, _| s.tint = Some(Color::WHITE));
    rt.frame(0.0);
    assert_eq!(flags(&rt), sc::IMAGE_FLAG_TINT | sc::IMAGE_FLAG_NEAREST);

    rt.update(|s, _| {
        s.tint = None;
        s.filter = ImageFilter::Linear;
    });
    rt.frame(0.0);
    assert_eq!(flags(&rt), 0);
}

#[test]
fn gpu_pixels_wrap_and_tile_modes_and_atlas_isolation() {
    use crate::color::Color;
    use crate::el::{El, el, row};
    use crate::render::{Gpu, KIND_IMAGE, OFFSCREEN_FORMAT, Offscreen, Renderer, instance::shader_consts as sc};
    use crate::runtime::{Cx, Env, Runtime};
    use crate::{ImageFilter, PixelsWrap};

    // 4x4 asymmetric tile: distinct (R, G) per texel, blue = 40.
    let tile = Pixels::new();
    let mut rgba = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4u8 {
        for x in 0..4u8 {
            rgba.extend_from_slice(&[x * 75 + 20, y * 75 + 20, 40, 255]);
        }
    }
    tile.set(4, 4, rgba);
    // Bright magenta neighbour to prove wrapped taps never bleed across atlas slots.
    let guard = Pixels::new();
    guard.set(4, 4, [255, 0, 255, 255].repeat(16));
    // 4x4 alpha mask for tinted tiling.
    let mask = Pixels::new();
    let mut mask_rgba = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4u8 {
        for x in 0..4u8 {
            let a = if x < 2 && y < 2 { 255 } else { 0 };
            mask_rgba.extend_from_slice(&[0x2e, 0x34, 0x36, a]);
        }
    }
    mask.set(4, 4, mask_rgba);

    fn view(s: &(Pixels, Pixels, Pixels), _: &mut Cx<(Pixels, Pixels, Pixels)>) -> El {
        row()
            // 0..24: Repeat via pixels_tile (8x8 tiles -> 3x3 copies)
            .child(el().size(24.0, 24.0).pixels_tile(&s.0, [8.0, 8.0]).image_filter(ImageFilter::Nearest))
            // 24..48: Mirror around inset [8, 8, 8, 8]
            .child(el().size(24.0, 24.0).pixels_wrap(&s.0, [8.0, 8.0, 8.0, 8.0], PixelsWrap::Mirror).image_filter(ImageFilter::Nearest))
            // 48..72: Extend around inset [8, 8, 8, 8]
            .child(el().size(24.0, 24.0).pixels_wrap(&s.0, [8.0, 8.0, 8.0, 8.0], PixelsWrap::Extend).image_filter(ImageFilter::Nearest))
            // 72..96: Negative offset Repeat + rounded corners
            .child(el().size(24.0, 24.0).radius(6.0).pixels_wrap(&s.0, [-4.0, -4.0, 8.0, 8.0], PixelsWrap::Repeat).image_filter(ImageFilter::Nearest))
            // 96..120: Tinted pixels_tile
            .child(el().size(24.0, 24.0).pixels_tile(&s.2, [8.0, 8.0]).pixels_tint(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }).image_filter(ImageFilter::Nearest))
            // 120..124: Plain untiled pixels (default path + atlas guard)
            .child(el().size(4.0, 24.0).pixels(&s.1))
    }

    let mut rt = Runtime::new((tile, guard, mask), view, (124.0, 24.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let imgs: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_IMAGE).collect();
    assert_eq!(imgs.len(), 6);
    assert_eq!(imgs[0].params[3] as u32, sc::IMAGE_WRAP_REPEAT);
    assert_eq!(imgs[0].border, [0.0, 0.0, 8.0, 8.0]);
    assert_eq!(imgs[1].params[3] as u32, sc::IMAGE_WRAP_MIRROR);
    assert_eq!(imgs[1].border, [32.0, 8.0, 8.0, 8.0]);
    assert_eq!(imgs[2].params[3] as u32, sc::IMAGE_WRAP_EXTEND);
    assert_eq!(imgs[2].border, [56.0, 8.0, 8.0, 8.0]);
    assert_eq!(imgs[3].params[3] as u32, sc::IMAGE_WRAP_REPEAT);
    assert_eq!(imgs[3].border, [68.0, -4.0, 8.0, 8.0]);
    assert_eq!(imgs[4].params[2] as u32, sc::IMAGE_FLAG_TINT | sc::IMAGE_FLAG_NEAREST);
    assert_eq!(imgs[4].params[3] as u32, sc::IMAGE_WRAP_REPEAT);
    // Default untiled image keeps border == [0; 4] and params[3] == 1.0.
    assert_eq!(imgs[5].border, [0.0; 4]);
    assert_eq!(imgs[5].params[3], 1.0);

    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (124, 24));
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    let target = Offscreen::new(&gpu.device, 124, 24);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    let out = target.read(&gpu.device);
    let px = |x: usize, y: usize| -> [u8; 3] {
        let p = &out[(y * 124 + x) * 4..(y * 124 + x) * 4 + 4];
        [p[0], p[1], p[2]]
    };
    let expected_texel = |tx: i32, ty: i32| -> [u8; 3] {
        [(tx as u8) * 75 + 20, (ty as u8) * 75 + 20, 40]
    };

    // Panel 0 (pixels_tile 8x8 -> 2 px per texel): check all 3x3 tiles match exact repeat.
    for y in 0..24usize {
        for x in 0..24usize {
            let exp = expected_texel(((x / 2) % 4) as i32, ((y / 2) % 4) as i32);
            let got = px(x, y);
            assert!(got[0].abs_diff(exp[0]) <= 2 && got[1].abs_diff(exp[1]) <= 2 && got[2].abs_diff(exp[2]) <= 2,
                "Repeat tile ({x},{y}): got {got:?}, expected {exp:?}");
        }
    }
    // Panel 1 (Mirror around inset [8, 8, 8, 8]): left/right/top/bottom reflect symmetrically.
    let mirror_idx = |k: i32| -> i32 {
        let m = k.rem_euclid(8);
        if m >= 4 { 7 - m } else { m }
    };
    for y in 0..24usize {
        for x in 0..24usize {
            let tx = mirror_idx((x as i32 - 8).div_euclid(2));
            let ty = mirror_idx((y as i32 - 8).div_euclid(2));
            let exp = expected_texel(tx, ty);
            let got = px(24 + x, y);
            assert!(got[0].abs_diff(exp[0]) <= 2 && got[1].abs_diff(exp[1]) <= 2 && got[2].abs_diff(exp[2]) <= 2,
                "Mirror ({x},{y}): got {got:?}, expected {exp:?}");
        }
    }
    // Panel 2 (Extend around inset [8, 8, 8, 8]): corners clamp to (0,0),(3,0),(0,3),(3,3).
    for y in 0..24usize {
        for x in 0..24usize {
            let tx = (x as i32 - 8).div_euclid(2).clamp(0, 3);
            let ty = (y as i32 - 8).div_euclid(2).clamp(0, 3);
            let exp = expected_texel(tx, ty);
            let got = px(48 + x, y);
            assert!(got[0].abs_diff(exp[0]) <= 2 && got[1].abs_diff(exp[1]) <= 2 && got[2].abs_diff(exp[2]) <= 2,
                "Extend ({x},{y}): got {got:?}, expected {exp:?}");
        }
    }
    // Panel 3 (negative offset [-4, -4, 8, 8] + radius(6.0)): corner (0,0) clipped to clear,
    // interior (12, 12) samples texel ((12 + 4)/2 % 4) = (0, 0).
    assert_eq!(px(72, 0), [0, 0, 0], "rounded corner must clip wrapped image");
    let p3_mid = px(72 + 12, 12);
    let exp00 = expected_texel(0, 0);
    assert!(p3_mid[0].abs_diff(exp00[0]) <= 2 && p3_mid[1].abs_diff(exp00[1]) <= 2,
        "negative offset Repeat interior: got {p3_mid:?}, expected {exp00:?}");
    // Panel 4 (tinted pixels_tile): top-left quadrant of every 8x8 tile is pure red, bottom-right is black.
    for ty in 0..3usize {
        for tx in 0..3usize {
            assert_eq!(px(96 + tx * 8 + 1, ty * 8 + 1), [255, 0, 0], "tinted tile ({tx},{ty}) opaque quad");
            assert_eq!(px(96 + tx * 8 + 6, ty * 8 + 6), [0, 0, 0], "tinted tile ({tx},{ty}) transparent quad");
        }
    }
}

#[test]
fn pixels_wrap_and_tile_layout_mip_edges_and_memo() {
    use crate::el::{El, col, el};
    use crate::memo::memo;
    use crate::render::{KIND_IMAGE, instance::shader_consts as sc};
    use crate::runtime::{Cx, Env, Runtime};
    use crate::{ImageFilter, PixelsWrap};

    // 1. Layout: pixels_wrap and pixels_tile must NOT supply an intrinsic aspect ratio,
    // while El::pixels still does.
    let wide = Pixels::new();
    wide.set(20, 10, vec![255; 20 * 10 * 4]);
    fn layout_view(p: &Pixels, _: &mut Cx<Pixels>) -> El {
        col()
            .child(el().key("plain").w(100.0).pixels(p))
            .child(el().key("tiled").w(100.0).pixels_tile(p, [20.0, 20.0]))
            .child(el().key("wrapped").w(100.0).pixels_wrap(p, [0.0, 0.0, 20.0, 20.0], PixelsWrap::Extend))
            .child(el().key("zero").size(40.0, 40.0).pixels_tile(p, [0.0, 10.0]))
            .child(el().key("neg").size(40.0, 40.0).pixels_wrap(p, [0.0, 0.0, -4.0, 8.0], PixelsWrap::Repeat))
            .child(el().key("nan").size(40.0, 40.0).pixels_wrap(p, [f32::NAN, 0.0, 8.0, 8.0], PixelsWrap::Mirror))
    }
    let mut rt = Runtime::new(wide, layout_view, (200.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let rect_of = |key: &str| {
        let idx = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(key)).expect(key);
        rt.tree.nodes[idx].target
    };
    let r_plain = rect_of("plain");
    let r_tiled = rect_of("tiled");
    let r_wrapped = rect_of("wrapped");
    assert_eq!(r_plain[3] - r_plain[1], 50.0, "El::pixels keeps intrinsic 2:1 height");
    assert_eq!(r_tiled[3] - r_tiled[1], 0.0, "El::pixels_tile must not supply intrinsic aspect ratio");
    assert_eq!(r_wrapped[3] - r_wrapped[1], 0.0, "El::pixels_wrap must not supply intrinsic aspect ratio");
    // Only the plain image emitted a KIND_IMAGE instance (zero/neg/NaN rects draw nothing).
    let imgs: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_IMAGE).collect();
    assert_eq!(imgs.len(), 1, "degenerate wrap rects must emit no KIND_IMAGE instance");

    // 2. HiDPI & mip selection: 64x64 mipmapped image tiled at 8x8 pt at scale 2.0 (16x16 dev px)
    // chooses 16x16 mip under Linear and 64x64 mip 0 under Nearest.
    let cache = crate::ImageCache::new(std::sync::Arc::new(|| {}));
    let big = cache.rgba(64, 64, [255, 0, 0, 255].repeat(64 * 64));
    big.wait().expect("decodes 64x64");
    fn mip_view(p: &Pixels, _: &mut Cx<Pixels>) -> El {
        col()
            .child(el().size(40.0, 40.0).pixels_tile(p, [8.0, 8.0]))
            .child(el().size(40.0, 40.0).pixels_tile(p, [8.0, 8.0]).image_filter(ImageFilter::Nearest))
    }
    let mut rt_mip = Runtime::new(big.pixels().clone(), mip_view, (80.0, 100.0), Env::new(true, 2.0));
    rt_mip.frame(0.0);
    let mimgs: Vec<_> = rt_mip.instances.iter().filter(|i| i.params[1] == KIND_IMAGE).collect();
    assert_eq!(mimgs.len(), 2);
    assert_eq!([mimgs[0].uv[2], mimgs[0].uv[3]], [16.0, 16.0], "Linear selects 16x16 mip for 8pt*2x tile");
    assert_eq!([mimgs[1].uv[2], mimgs[1].uv[3]], [64.0, 64.0], "Nearest keeps 64x64 mip 0");
    assert_eq!([mimgs[0].border[2], mimgs[0].border[3]], [16.0, 16.0], "tile size scales by HiDPI");

    // 3. Memo reuse invalidates when pixels_wrap rect or wrap mode changes.
    struct MemoState {
        px: Pixels,
        wrap: Option<([f32; 4], PixelsWrap)>,
    }
    fn memo_view(s: &MemoState, _: &mut Cx<MemoState>) -> El {
        let base = memo("w", (), || el().size(32.0, 32.0).pixels(&s.px));
        match s.wrap {
            Some((r, w)) => base.pixels_wrap(&s.px, r, w),
            None => base,
        }
    }
    let px = Pixels::new();
    px.set(4, 4, vec![255; 4 * 4 * 4]);
    let mut rt_m = Runtime::new(MemoState { px, wrap: None }, memo_view, (32.0, 32.0), Env::new(true, 1.0));
    rt_m.frame(0.0);
    let img0 = rt_m.instances.iter().find(|i| i.params[1] == KIND_IMAGE).unwrap();
    assert_eq!((img0.params[3], img0.border), (1.0, [0.0; 4]));

    rt_m.update(|s, _| s.wrap = Some(([2.0, 4.0, 8.0, 8.0], PixelsWrap::Repeat)));
    rt_m.frame(0.0);
    let img1 = rt_m.instances.iter().find(|i| i.params[1] == KIND_IMAGE).unwrap();
    assert_eq!((img1.params[3] as u32, img1.border), (sc::IMAGE_WRAP_REPEAT, [2.0, 4.0, 8.0, 8.0]));

    rt_m.update(|s, _| s.wrap = Some(([2.0, 4.0, 8.0, 8.0], PixelsWrap::Mirror)));
    rt_m.frame(0.0);
    let img2 = rt_m.instances.iter().find(|i| i.params[1] == KIND_IMAGE).unwrap();
    assert_eq!(img2.params[3] as u32, sc::IMAGE_WRAP_MIRROR);

    rt_m.update(|s, _| s.wrap = Some(([6.0, 4.0, 12.0, 8.0], PixelsWrap::Extend)));
    rt_m.frame(0.0);
    let img3 = rt_m.instances.iter().find(|i| i.params[1] == KIND_IMAGE).unwrap();
    assert_eq!((img3.params[3] as u32, img3.border), (sc::IMAGE_WRAP_EXTEND, [6.0, 4.0, 12.0, 8.0]));
}
