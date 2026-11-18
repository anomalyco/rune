use rune::render::Gpu;
use rune_fx::*;

#[test]
fn external_struct_update_and_emitted_rust_compile() {
    let _literal = FxLayer {
        fx: Fx::distort(DistortParams::chroma_split([0.0; 2])),
        shape: Shape::rounded(0.0, 0.0, 16.0, 16.0, 0.0),
        place: Place::Over, time: 0.0, seed: 1, reduced_motion: false,
        source: None, opacity: 1.0,
    };
    let card = Shape::rounded(10.0, 10.0, 120.0, 80.0, 12.0);
    let layers = [
        Fx::fill(LiquidMetalParams { flow: 0.9, scale: 80.0, ..LiquidMetalParams::rose() }.border(4.0).intensity(0.8)),
        Fx::fill(MeshGradientParams { colors: Stops::hex(&[0x1b1f3b, 0xff6f91, 0xffc75f, 0x845ec2]), drift: 0.15, ..MeshGradientParams::dusk() }),
        Fx::fill(DitherParams { pattern: DitherPattern::BlueNoise, pixel: 2.0, ..DitherParams::backdrop() }),
        Fx::fill(GrainGradientParams { kind: GradientKind::Conic, mono: false, amount: 0.3, ..GrainGradientParams::sweep() }),
        Fx::fill(FlutedGlassParams { rib: 10.0, refraction: 7.0, frost: 0.4, ..FlutedGlassParams::reeded() }),
        Fx::fill(MetaballParams { count: 4, gloss: 0.7, ..MetaballParams::mint() }),
        Fx::fill(SwirlParams { twist: -0.8, warp: 1.2, ..SwirlParams::marble() }),
        Fx::fill(VoronoiParams { scale: 24.0, edge: 2.0, ..VoronoiParams::stained() }),
        Fx::fill(WaveParams { amplitude: 0.06, softness: 0.01, ..WaveParams::dunes() }),
        Fx::fill(HalftoneParams { input: FillInput::Linear, cell: 5.0, ..HalftoneParams::print() }),
        Fx::fill(PlasticParams { roughness: 0.6, dome: -0.4, ..PlasticParams::glossy() }.border(5.0)),
        Fx::fill(LiquidMetalParams::chrome().anchor(FillAnchor::Element)),
        Fx::fill(LiquidMetalParams { anchor: FillAnchor::Element, ..LiquidMetalParams::gold() }),
    ];
    for fx in layers {
        let Fx::Fill(p) = fx else { unreachable!() };
        assert!(!p.to_rust().is_empty());
        let layer = fx.on(card).at(1.0);
        assert_eq!(layer.place, if p.needs_backdrop() { Place::Over } else { Place::Under });
    }
}

#[test]
fn linear_backdrop_views_are_not_double_decoded_in_srgb_frames() {
    use rune::render::Offscreen;
    let Gpu { device, queue, .. } = Gpu::headless();
    let mut fx = FxRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, FxOptions::default());
    fx.set_srgb_blend(true);
    let layers = [Fx::distort(DistortParams::chroma_split([1e-6, 0.0]))
        .on(Shape::rounded(0.0, 0.0, 32.0, 32.0, 0.0))];
    for format in [wgpu::TextureFormat::Rgba8UnormSrgb, wgpu::TextureFormat::Rgba16Float] {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        // Direct frame input retains the old linear-samples contract; a hook can also
        // override the pass-copy convention for its own linear or hardware-decoded view.
        for per_slot in [false, true] {
            let target = Offscreen::with_format(&device, 32, 32, wgpu::TextureFormat::Rgba8Unorm);
            let frame = FxFrame { viewport: [32, 32], scale: 1.0, layers: &layers,
                backdrop: (!per_slot).then_some(&view), sources: &[] };
            let slots = if per_slot { vec![FxSlot::default().backdrop_srgb(false)] } else { vec![] };
            assert_eq!(fx.prepare_with_slots(&device, &queue, &frame, &slots).drawn, 1);
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view, depth_slice: None, resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.25, g: 0.125, b: 0.0625, a: 1.0 }), store: wgpu::StoreOp::Store },
                    })], ..Default::default()
                });
            }
            fx.blur_backdrop(&device, &queue, &mut encoder, &view, 0);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.view, depth_slice: None, resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })], ..Default::default()
                });
                fx.draw_layer(&mut pass, 0);
            }
            target.copy(&mut encoder);
            queue.submit([encoder.finish()]);
            let pixels = target.read(&device);
            let at = (16 * 32 + 16) * 4;
            let expected = [0.25, 0.125, 0.0625].map(|c| (rune::color::srgb_encode(c) * 255.0).round() as u8);
            for (got, want) in pixels[at..at + 3].iter().zip(expected) {
                assert!(got.abs_diff(want) <= 2, "{format:?} per_slot={per_slot}: {got} != {want}");
            }
        }
    }
}

#[test]
fn gpu_fills_are_finite_at_extremes_degenerate_params_and_large_time() {
    let Gpu { device, queue, .. } = Gpu::headless();
    let fmt = wgpu::TextureFormat::Rgba16Float;
    let mut fx = FxRenderer::new(&device, fmt, FxOptions { dither: true });
    let (w, h) = (32u32, 32u32);
    let row_bytes = w * 8; // 256 bytes: aligned to COPY_BYTES_PER_ROW_ALIGNMENT
    let mk_tex = |usage: wgpu::TextureUsages| {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage,
            view_formats: &[],
        });
        let v = t.create_view(&Default::default());
        (t, v)
    };
    let (target, target_view) = mk_tex(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let (_bd, bd_view) = mk_tex(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let (_src, src_view) = mk_tex(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row_bytes * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let shape = Shape::rounded(2.0, 2.0, 28.0, 28.0, 6.0);
    let mut draw = |layer: FxLayer| -> Vec<u16> {
        let layers = [layer];
        let sources = [&src_view];
        let stats = fx.prepare(&device, &queue, &FxFrame {
            viewport: [w, h],
            scale: 1.0,
            layers: &layers,
            backdrop: Some(&bd_view),
            sources: &sources,
        });
        assert_eq!(stats.drawn, 1);
        let mut enc = device.create_command_encoder(&Default::default());
        fx.blur_backdrop(&device, &queue, &mut enc, &bd_view, 0);
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            fx.draw_layer(&mut pass, 0);
        }
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row_bytes), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let words: Vec<u16> = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
        readback.unmap();
        for &word in &words {
            assert_ne!(word & 0x7c00, 0x7c00, "non-finite f16 word {word:#06x}");
        }
        words
    };

    // Every style at catalog min, catalog max, and t = 1e5 s.
    for style in fill_styles() {
        let mut min_fill = style.default_fill();
        let mut max_fill = style.default_fill();
        for p in style.params {
            match p.kind {
                ParamKind::Number { min, max, .. } => {
                    min_fill.set(p.id, Value::Number(min));
                    max_fill.set(p.id, Value::Number(max));
                }
                ParamKind::Count { min, max } => {
                    min_fill.set(p.id, Value::Count(min));
                    max_fill.set(p.id, Value::Count(max));
                }
                ParamKind::Choice(c) => {
                    min_fill.set(p.id, Value::Choice(0));
                    max_fill.set(p.id, Value::Choice(c.len() - 1));
                }
                ParamKind::Colors { min, max } => {
                    let palette = [0x112233, 0x445566, 0x778899, 0xaabbcc, 0xddeeff, 0xff8800];
                    min_fill.set(p.id, Value::Colors(Stops::hex(&palette[..min])));
                    max_fill.set(p.id, Value::Colors(Stops::hex(&palette[..max])));
                }
                _ => {}
            }
        }
        draw(Fx::fill(min_fill).on(shape).source(0, Rect::new(2.0, 2.0, 28.0, 28.0)).at(0.5));
        draw(Fx::fill(max_fill).on(shape).source(0, Rect::new(2.0, 2.0, 28.0, 28.0)).at(0.5));

        let preset = style.default_fill();
        let f0 = draw(Fx::fill(preset).on(shape).at(1e5));
        let f1 = draw(Fx::fill(preset).on(shape).at(1e5 + 0.25));
        if Fx::fill(preset).on(shape).is_animating() {
            assert_ne!(f0, f1, "{} still animates at t = 1e5", style.id);
        }
    }

    // Degenerate cases: conic centre on an exact pixel centre, zero-radius metaballs, extreme mesh sharpness, zero-size source rect,
    // slab extremes (roughness 0/1, dome ±1, bevel and border larger than half the shape, tiny shapes, non-finite fields).
    let pixel_center = Shape::rounded(0.5, 0.5, 20.0, 20.0, 0.0);
    draw(Fx::fill(GrainGradientParams { kind: GradientKind::Conic, center: [0.5, 0.5], ..GrainGradientParams::sweep() }).on(pixel_center).at(1.0));
    draw(Fx::fill(MetaballParams { size: 0.0, ..MetaballParams::lava() }).on(shape).at(1.0));
    draw(Fx::fill(MeshGradientParams { sharpness: 50.0, ..MeshGradientParams::dusk() }).on(shape).at(1.0));
    draw(Fx::fill(DitherParams { input: FillInput::Image, ..DitherParams::bayer() }).on(shape).source(0, Rect::new(0.0, 0.0, 0.0, 0.0)).at(1.0));
    for tiny in [Shape::rounded(2.0, 2.0, 4.0, 4.0, 2.0), Shape::rounded(2.5, 2.5, 1.0, 1.0, 0.5)] {
        draw(Fx::fill(PlasticParams { roughness: 0.0, dome: 1.0, bevel: 48.0, ..PlasticParams::glossy() }).on(tiny));
        draw(Fx::fill(PlasticParams { roughness: 1.0, dome: -1.0, bevel: 48.0, ..PlasticParams::dished() }.border(20.0)).on(tiny));
        draw(Fx::fill(LiquidMetalParams { bevel: 48.0, dome: -1.0, anchor: FillAnchor::Element, ..LiquidMetalParams::chrome() }).on(tiny).at(1.0));
    }
    draw(Fx::fill(PlasticParams { roughness: f32::NAN, dome: f32::INFINITY, bevel: f32::NAN, angle: f32::NEG_INFINITY, ..PlasticParams::glossy() }).on(shape));
    draw(Fx::fill(LiquidMetalParams { flow: f32::NAN, scale: f32::NAN, bevel: f32::INFINITY, dome: f32::NAN, contrast: f32::NAN, ..LiquidMetalParams::chrome() }).on(shape).at(1.0));
}

/// Draws `layers` at `scale` into a transparent `w`×`h` Rgba16Float target with no dither, and
/// reads it back as premultiplied linear RGBA (or sRGB-encoded with `srgb`).
fn render(w: u32, h: u32, scale: f32, srgb: bool, layers: &[FxLayer]) -> Vec<[f32; 4]> {
    render_at(w, h, scale, srgb, [0.0, 0.0], layers)
}

fn render_at(w: u32, h: u32, scale: f32, srgb: bool, origin: [f32; 2], layers: &[FxLayer]) -> Vec<[f32; 4]> {
    static CTX: std::sync::Mutex<Option<(Gpu, FxRenderer)>> = std::sync::Mutex::new(None);
    let fmt = wgpu::TextureFormat::Rgba16Float;
    let mut guard = CTX.lock().unwrap();
    let (gpu, fx) = guard.get_or_insert_with(|| {
        let gpu = Gpu::headless();
        let fx = FxRenderer::new(&gpu.device, fmt, FxOptions { dither: false });
        (gpu, fx)
    });
    let (device, queue) = (&gpu.device, &gpu.queue);
    fx.set_srgb_blend(srgb);
    assert_eq!(w * 8 % 256, 0, "row alignment");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (w * 8 * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let vw = w + origin[0].ceil() as u32;
    let vh = h + origin[1].ceil() as u32;
    let stats = fx.prepare(&device, &queue, &FxFrame { viewport: [vw, vh], scale, layers, backdrop: None, sources: &[] });
    assert_eq!(stats.drawn as usize, layers.len());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for k in 0..layers.len() {
            fx.draw_layer_in(&mut pass, fmt, origin, k);
        }
    }
    enc.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 8), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let words: Vec<u16> = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
    words.chunks(4).map(|p| std::array::from_fn(|c| f16(p[c]))).collect()
}

fn f16(h: u16) -> f32 {
    let (s, e, m) = ((h >> 15) as u32, ((h >> 10) & 0x1f) as i32, (h & 0x3ff) as f32);
    let v = match e {
        0 => m * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    };
    if s == 1 { -v } else { v }
}

fn lum(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// A 56 pt key at 2× in a 128×128 px target.
const KEY: (u32, u32, f32) = (128, 128, 2.0);

fn key() -> Shape {
    Shape::rounded(4.0, 4.0, 56.0, 56.0, 8.0)
}

/// Plastic's colour is coverage: the highlight lightens toward white inside the shape's alpha and
/// never adds light past it, at any roughness, intensity or blend mode.
#[test]
fn plastic_never_paints_past_its_coverage() {
    let (w, h, s) = KEY;
    for srgb in [false, true] {
        for roughness in [0.0, 0.05, 0.45, 1.0] {
            for (color, intensity) in [(rgb(0xffffff), 1.0), (rgb(0xffd60a), 1.0), (rgb(0xd7263d), 0.5), (rgb(0x202226), 1.0)] {
                let p = PlasticParams { color, roughness, ..PlasticParams::glossy() };
                for layer in [Fx::fill(p.intensity(intensity)).on(key()), Fx::fill(p.border(6.0)).on(key())] {
                    let img = render(w, h, s, srgb, &[layer]);
                    let worst = img.iter().map(|p| p[0].max(p[1]).max(p[2]) - p[3]).fold(f32::MIN, f32::max);
                    assert!(worst <= 2e-3, "srgb {srgb} roughness {roughness} {color:?} x{intensity}: rgb exceeds alpha by {worst}");
                    let centre = img[(h / 2 * w + w / 2) as usize];
                    assert!(layer_is_border(&layer) || (centre[3] - intensity).abs() < 2e-3, "centre alpha {centre:?}");
                }
            }
        }
        // Extreme dome (±1), bevel wider than half the shape, and a tiny 4 pt shape.
        for dome in [-1.0, 1.0] {
            let p = PlasticParams { color: rgb(0xffd60a), roughness: 0.0, dome, bevel: 40.0, angle: 0.8 };
            for sh in [key(), Shape::rounded(4.0, 4.0, 4.0, 4.0, 1.5)] {
                let img = render(w, h, s, srgb, &[Fx::fill(p).on(sh)]);
                let worst = img.iter().map(|p| p[0].max(p[1]).max(p[2]) - p[3]).fold(f32::MIN, f32::max);
                assert!(worst <= 2e-3, "srgb {srgb} dome {dome}: rgb exceeds alpha by {worst}");
            }
        }
    }
}

fn layer_is_border(l: &FxLayer) -> bool {
    matches!(l.fx, Fx::Fill(p) if p.border > 0.0)
}

/// The flat centre shows the chosen colour (a satin or matte highlight lays a faint sheen over it); glossy plastic has a brighter, tighter highlight than
/// matte; the rim facing the light is lighter than the rim facing away.
#[test]
fn plastic_highlight_follows_roughness_and_light() {
    let (w, h, s) = KEY;
    let base = rgb(0x3d6fd6);
    let mut peaks = Vec::new();
    for roughness in [0.05, 0.45, 1.0] {
        let img = render(w, h, s, false, &[Fx::fill(PlasticParams { color: base, roughness, ..PlasticParams::glossy() }).on(key())]);
        let at = |x: u32, y: u32| img[(y * w + x) as usize];
        let centre = at(w / 2, h / 2);
        // The colour, under at most a faint sheen from a broad highlight.
        for c in 0..3 {
            let lift = centre[c] - base.0[c];
            assert!((-0.01..0.07).contains(&lift), "roughness {roughness}: centre {centre:?} vs {base:?}");
        }
        let face: Vec<f32> = img.iter().filter(|p| p[3] > 0.999).map(|p| lum(*p) - lum(centre)).collect();
        let peak = face.iter().copied().fold(0.0f32, f32::max);
        let spread = face.iter().filter(|v| **v > 0.5 * peak).count();
        // Light from above and a little left: the top rim is lit, the bottom rim is in shade.
        let (lit, shade) = (lum(at(64, 14)), lum(at(64, 114)));
        eprintln!("roughness {roughness}: peak +{peak:.3}, {spread} px above half peak, rims {lit:.3} / {shade:.3}");
        assert!(lit > shade * 1.3, "roughness {roughness}: lit rim {lit} vs shaded rim {shade}");
        peaks.push((peak, spread));
    }
    assert!(peaks[0].0 > peaks[1].0 && peaks[1].0 > peaks[2].0, "gloss is brighter: {peaks:?}");
    assert!(peaks[0].1 < peaks[2].1, "gloss is tighter: {peaks:?}");
}

/// Eight 24 pt keys in a row at 2×: element-anchored liquid metal paints every key the same,
/// pixel for pixel, at any time (including folded large times); window-anchored does not.
#[test]
fn element_anchored_metal_keys_are_identical() {
    let keys_at = |anchor: FillAnchor, t: f32, ox: f32, oy: f32| -> Vec<FxLayer> {
        (0..8).map(|k| Fx::fill(LiquidMetalParams::chrome().anchor(anchor)).on(Shape::rounded(ox + k as f32 * 28.0, oy, 24.0, 24.0, 5.0)).at(t)).collect()
    };
    let (w, h) = (512u32, 64u32);
    // Each key's quad is its rect plus 1 pt: 26 pt = 52 px from (3 + 28k) pt.
    let tile = |img: &[[f32; 4]], k: u32| -> Vec<[u32; 4]> {
        let x0 = 6 + 56 * k;
        (6..58).flat_map(|y| (x0..x0 + 52).map(move |x| (x, y))).map(|(x, y)| img[(y * w + x) as usize].map(f32::to_bits)).collect()
    };
    for t in [1.3, 1e5 + 0.37] {
        let img = render(w, h, 2.0, false, &keys_at(FillAnchor::Element, t, 4.0, 4.0));
        let first = tile(&img, 0);
        assert!(first.iter().any(|p| f32::from_bits(p[3]) > 0.99), "the key is drawn");
        for k in 1..8 {
            assert!(tile(&img, k) == first, "t = {t}: key {k} differs from key 0");
        }
        let world = render(w, h, 2.0, false, &keys_at(FillAnchor::World, t, 4.0, 4.0));
        assert!((1..8).any(|k| tile(&world, k) != tile(&world, 0)), "t = {t}: window-anchored keys should differ");
    }

    // Sub-pixel / live_offset positions, isolated-layer viewport shift, and non-power-of-two scale (1.5×).
    let subpx = render(w, h, 2.0, false, &keys_at(FillAnchor::Element, 1.3, 4.25, 4.375));
    let subpx_tile = |img: &[[f32; 4]], k: u32| -> Vec<[u32; 4]> {
        let x0 = 6 + 56 * k;
        (6..59).flat_map(|y| (x0..x0 + 53).map(move |x| (x, y))).map(|(x, y)| img[(y * w + x) as usize].map(f32::to_bits)).collect()
    };
    let sub0 = subpx_tile(&subpx, 0);
    for k in 1..8 {
        assert!(subpx_tile(&subpx, k) == sub0, "fractional offset: key {k} differs from key 0");
    }

    let in_layer = render_at(w, h, 2.0, false, [16.0, 12.0], &keys_at(FillAnchor::Element, 1.3, 12.25, 10.375));
    for k in 0..8 {
        assert!(subpx_tile(&in_layer, k) == sub0, "in-layer origin shift: key {k} differs from direct draw");
    }

    let scale15 = render(w, h, 1.5, false, &keys_at(FillAnchor::Element, 1.3, 4.0, 4.0));
    let tile15 = |img: &[[f32; 4]], k: u32| -> Vec<[u32; 4]> {
        let x0 = 4 + 42 * k;
        (4..44).flat_map(|y| (x0..x0 + 40).map(move |x| (x, y))).map(|(x, y)| img[(y * w + x) as usize].map(f32::to_bits)).collect()
    };
    let s15_0 = tile15(&scale15, 0);
    for k in 1..8 {
        assert!(tile15(&scale15, k) == s15_0, "scale 1.5: key {k} differs from key 0");
    }

    // Time folding at 512 s and 1024 s turns smoothly without a frame jump.
    for fold in [512.0f32, 1024.0] {
        let dt = 1.0 / 64.0;
        let before = render(128, 64, 2.0, false, &[Fx::fill(LiquidMetalParams::chrome()).on(Shape::rounded(4.0, 4.0, 24.0, 24.0, 5.0)).at(fold - dt)]);
        let at_fold = render(128, 64, 2.0, false, &[Fx::fill(LiquidMetalParams::chrome()).on(Shape::rounded(4.0, 4.0, 24.0, 24.0, 5.0)).at(fold)]);
        let after = render(128, 64, 2.0, false, &[Fx::fill(LiquidMetalParams::chrome()).on(Shape::rounded(4.0, 4.0, 24.0, 24.0, 5.0)).at(fold + dt)]);
        assert_eq!(before, after, "triangle wave is symmetric across {fold}");
        let step = before.iter().zip(&at_fold).flat_map(|(a, b)| (0..4).map(move |c| (a[c] - b[c]).abs())).fold(0.0f32, f32::max);
        assert!(step < 0.2, "no frame jump at {fold}: max step {step}");
    }
}
