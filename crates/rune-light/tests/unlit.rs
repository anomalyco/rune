//! Tests for `Prim::unlit` (`FLAG_UNLIT`): flag composition, hue preservation under direct light,
//! GI emission to neighbours, and unlit text immunity to `TextPolicy` clamping and specular.
//! GPU tests skip cleanly when no adapter is found and `RUNE_ALLOW_NO_GPU=1` is set.

use rune_light::{FLAG_EMISSIVE, FLAG_OCCLUDER, FLAG_UNLIT, Gi, Light, LightRenderer, Prim, Scene, Settings, TextPolicy};

const W: u32 = 128;
const H: u32 = 96;

fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x007f_ffff;
    if exp == 255 {
        return sign | 0x7c00 | if mant != 0 { 1 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (mant | 0x0080_0000) >> (1 - e + 13);
        return sign | (m as u16);
    }
    sign | ((e as u16) << 10) | ((mant >> 13) as u16)
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exp = u32::from((bits >> 10) & 0x1f);
    let mant = u32::from(bits & 0x3ff);
    if exp == 0 {
        if mant == 0 {
            return f32::from_bits(sign);
        }
        let v = (mant as f32) * 2.0f32.powi(-24);
        return if sign != 0 { -v } else { v };
    }
    if exp == 31 {
        return f32::from_bits(sign | 0x7f80_0000 | (mant << 13));
    }
    f32::from_bits(sign | ((exp + 112) << 23) | (mant << 13))
}

fn rgba16f_texture(device: &wgpu::Device, queue: &wgpu::Queue, fill: impl Fn(u32, u32) -> [f32; 4]) -> wgpu::Texture {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let mut data = Vec::with_capacity((W * H * 8) as usize);
    for y in 0..H {
        for x in 0..W {
            for c in fill(x, y) {
                data.extend_from_slice(&f32_to_f16(c).to_le_bytes());
            }
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 8), rows_per_image: Some(H) },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    tex
}

fn out_texture(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn read_rgba16f(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<[f32; 4]> {
    let row_bytes = W * 8;
    let padded = row_bytes.div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback f16"),
        size: u64::from(padded * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(H) },
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = slice.get_mapped_range().expect("mapped");
    let mut out = Vec::with_capacity((W * H) as usize);
    for y in 0..H as usize {
        let row = &data[y * padded as usize..y * padded as usize + row_bytes as usize];
        for px in row.as_chunks::<8>().0 {
            let f = |i: usize| f16_to_f32(u16::from_le_bytes([px[i], px[i + 1]]));
            out.push([f(0), f(2), f(4), f(6)]);
        }
    }
    drop(data);
    buf.unmap();
    out
}

fn rgb_hsv(rgb: [f32; 3]) -> (f32, f32, f32) {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < 1e-6 {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max > 1e-6 { d / max } else { 0.0 };
    (h, s, max)
}

fn hue_diff_deg(a: f32, b: f32) -> f32 {
    let d = (a - b).abs() % 360.0;
    if d > 180.0 { 360.0 - d } else { d }
}

fn px_at(buf: &[[f32; 4]], x: u32, y: u32) -> [f32; 3] {
    let p = buf[(y * W + x) as usize];
    [p[0], p[1], p[2]]
}

#[test]
fn prim_unlit_sets_flag_without_clearing_others() {
    let p = Prim::rect(10.0, 20.0, 80.0, 40.0)
        .raised(8.0)
        .emissive([0.06, 0.32, 1.7])
        .unlit();
    assert_eq!(p.flags() & FLAG_OCCLUDER, FLAG_OCCLUDER);
    assert_eq!(p.flags() & FLAG_EMISSIVE, FLAG_EMISSIVE);
    assert_eq!(p.flags() & FLAG_UNLIT, FLAG_UNLIT);
    assert_eq!(p.emit, [0.06, 0.32, 1.7, 0.0]);
}

#[test]
fn unlit_emissive_keeps_hue_under_strong_point_light() {
    let Some(gpu) = rune::render::Gpu::test_headless() else { return };

    // Saturated linear blue.
    let base_blue = [0.03f32, 0.16, 0.85];
    let (base_hue, base_sat, _) = rgb_hsv(base_blue);

    let base_tex = rgba16f_texture(&gpu.device, &gpu.queue, |x, y| {
        if (24..64).contains(&x) && (24..72).contains(&y) {
            [base_blue[0], base_blue[1], base_blue[2], 1.0]
        } else {
            [0.04, 0.04, 0.05, 1.0]
        }
    });
    let text_tex = rgba16f_texture(&gpu.device, &gpu.queue, |_, _| [0.0; 4]);
    let out_tex = out_texture(&gpu.device, wgpu::TextureFormat::Rgba16Float);
    let (base_v, text_v, out_v) = (
        base_tex.create_view(&Default::default()),
        text_tex.create_view(&Default::default()),
        out_tex.create_view(&Default::default()),
    );
    let mut renderer = LightRenderer::new(&gpu.device, &gpu.queue, W, H, wgpu::TextureFormat::Rgba16Float);

    let emitter = Prim::rect(24.0, 24.0, 40.0, 48.0).emissive([0.06, 0.32, 1.7]);
    let lamp = Light::point(44.0, 48.0, 58.0).intensity(2.1).radius(11.0).range(900.0);

    for (label, gi) in [("gi=off", None), ("gi=default", Some(Gi::default()))] {
        let settings = Settings { gi, ..Settings::default() };

        // Without .unlit(): strong point light pushes the saturated blue toward cyan-white.
        let lit_scene = Scene { prims: &[emitter], lights: &[lamp], settings, emission: None };
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        assert!(renderer.render(&gpu.device, &gpu.queue, &mut enc, &lit_scene, &base_v, &text_v, &out_v));
        gpu.queue.submit([enc.finish()]);
        let lit_px = px_at(&read_rgba16f(&gpu.device, &gpu.queue, &out_tex), 44, 48);
        let (lit_hue, lit_sat, _) = rgb_hsv(lit_px);
        let lit_drift = hue_diff_deg(lit_hue, base_hue);

        // With .unlit(): the same element keeps its designed hue and saturation.
        let unlit_prim = emitter.unlit();
        let unlit_scene = Scene { prims: &[unlit_prim], lights: &[lamp], settings, emission: None };
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        assert!(renderer.render(&gpu.device, &gpu.queue, &mut enc, &unlit_scene, &base_v, &text_v, &out_v));
        gpu.queue.submit([enc.finish()]);
        let unlit_px = px_at(&read_rgba16f(&gpu.device, &gpu.queue, &out_tex), 44, 48);
        let (unlit_hue, unlit_sat, _) = rgb_hsv(unlit_px);
        let unlit_drift = hue_diff_deg(unlit_hue, base_hue);

        assert!(
            lit_drift > 30.0 && lit_sat < base_sat - 0.3,
            "{label}: expected lit emissive prim to wash toward cyan-white under strong point light, got {lit_px:?} (drift={lit_drift:.2}deg, sat={lit_sat:.3})"
        );
        assert!(
            unlit_drift < 3.0 && (unlit_sat - base_sat).abs() < 0.05,
            "{label}: expected unlit emissive prim to preserve hue & saturation, got {unlit_px:?} (drift={unlit_drift:.2}deg, sat={unlit_sat:.3}, base_sat={base_sat:.3})"
        );
    }
}

#[test]
fn unlit_emissive_still_lights_neighbours_through_gi() {
    let Some(gpu) = rune::render::Gpu::test_headless() else { return };

    let base_tex = rgba16f_texture(&gpu.device, &gpu.queue, |x, y| {
        if (16..48).contains(&x) && (24..72).contains(&y) {
            [0.03, 0.16, 0.85, 1.0]
        } else {
            [0.10, 0.10, 0.10, 1.0]
        }
    });
    let text_tex = rgba16f_texture(&gpu.device, &gpu.queue, |_, _| [0.0; 4]);
    let out_tex = out_texture(&gpu.device, wgpu::TextureFormat::Rgba16Float);
    let (base_v, text_v, out_v) = (
        base_tex.create_view(&Default::default()),
        text_tex.create_view(&Default::default()),
        out_tex.create_view(&Default::default()),
    );
    let mut renderer = LightRenderer::new(&gpu.device, &gpu.queue, W, H, wgpu::TextureFormat::Rgba16Float);

    let settings = Settings {
        ambient: [0.2, 0.2, 0.2],
        gi: Some(Gi::default()),
        ..Settings::default()
    };
    let emitter = Prim::rect(16.0, 24.0, 32.0, 48.0).emissive([0.15, 0.8, 4.25]).unlit();
    let neighbour = Prim::rect(56.0, 24.0, 48.0, 48.0);

    // Baseline: neighbour with no emitter under a zero-intensity lamp.
    let dim_lamp = Light::point(120.0, 90.0, 500.0).intensity(0.0);
    let base_scene = Scene { prims: &[neighbour], lights: &[dim_lamp], settings, emission: None };
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    assert!(renderer.render(&gpu.device, &gpu.queue, &mut enc, &base_scene, &base_v, &text_v, &out_v));
    gpu.queue.submit([enc.finish()]);
    let dark_nb = px_at(&read_rgba16f(&gpu.device, &gpu.queue, &out_tex), 62, 48);

    // With unlit emitter: neighbour receives blue GI light even with no point lights.
    let gi_scene = Scene { prims: &[emitter, neighbour], lights: &[], settings, emission: None };
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    assert!(renderer.render(&gpu.device, &gpu.queue, &mut enc, &gi_scene, &base_v, &text_v, &out_v));
    gpu.queue.submit([enc.finish()]);
    let lit_nb = px_at(&read_rgba16f(&gpu.device, &gpu.queue, &out_tex), 62, 48);

    assert!(
        lit_nb[2] > dark_nb[2] + 0.05 && lit_nb[2] > lit_nb[0] * 2.0,
        "expected neighbour next to unlit blue emitter to receive blue GI light: dark={dark_nb:?} lit={lit_nb:?}"
    );
}

#[test]
fn unlit_text_ignores_text_policy_clamping_and_specular() {
    let Some(gpu) = rune::render::Gpu::test_headless() else { return };

    let base_tex = rgba16f_texture(&gpu.device, &gpu.queue, |_, _| [0.05, 0.05, 0.05, 1.0]);
    let text_color = [0.4f32, 0.5, 0.6];
    let text_tex = rgba16f_texture(&gpu.device, &gpu.queue, |x, y| {
        if (24..104).contains(&x) && (24..72).contains(&y) {
            [text_color[0], text_color[1], text_color[2], 1.0]
        } else {
            [0.0; 4]
        }
    });
    let out_tex = out_texture(&gpu.device, wgpu::TextureFormat::Rgba16Float);
    let (base_v, text_v, out_v) = (
        base_tex.create_view(&Default::default()),
        text_tex.create_view(&Default::default()),
        out_tex.create_view(&Default::default()),
    );
    let mut renderer = LightRenderer::new(&gpu.device, &gpu.queue, W, H, wgpu::TextureFormat::Rgba16Float);

    let lamp = Light::point(64.0, 48.0, 50.0).intensity(2.0).radius(10.0).range(800.0);
    // Test both ceiling < 1.0 and floor > 1.0 so clamping lightf=1.0 would visibly move the text colour.
    for (name, policy) in [
        ("ceiling<1", TextPolicy { floor: 0.3, ceiling: 0.7, lit: 1.0, specular: 0.5 }),
        ("floor>1", TextPolicy { floor: 1.25, ceiling: 1.5, lit: 1.0, specular: 0.5 }),
    ] {
        let settings = Settings { text: policy, gi: None, ..Settings::default() };

        // Lit prim: text is shifted by TextPolicy clamping + specular.
        let lit_prim = Prim::rect(24.0, 24.0, 80.0, 48.0).gloss(0.8, 0.25);
        let lit_scene = Scene { prims: &[lit_prim], lights: &[lamp], settings, emission: None };
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        assert!(renderer.render(&gpu.device, &gpu.queue, &mut enc, &lit_scene, &base_v, &text_v, &out_v));
        gpu.queue.submit([enc.finish()]);
        let lit_px = px_at(&read_rgba16f(&gpu.device, &gpu.queue, &out_tex), 64, 48);
        assert!(
            (lit_px[0] - text_color[0]).abs() > 0.05,
            "{name}: expected lit prim text to be shifted by TextPolicy, got {lit_px:?}"
        );

        // Unlit prim: text stays at its exact designed colour regardless of TextPolicy or specular.
        let unlit_prim = lit_prim.unlit();
        let unlit_scene = Scene { prims: &[unlit_prim], lights: &[lamp], settings, emission: None };
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        assert!(renderer.render(&gpu.device, &gpu.queue, &mut enc, &unlit_scene, &base_v, &text_v, &out_v));
        gpu.queue.submit([enc.finish()]);
        let unlit_px = px_at(&read_rgba16f(&gpu.device, &gpu.queue, &out_tex), 64, 48);
        for ch in 0..3 {
            assert!(
                (unlit_px[ch] - text_color[ch]).abs() < 0.005,
                "{name}: text on unlit prim channel {ch} drifted: expected {}, got {unlit_px:?}",
                text_color[ch]
            );
        }
    }
}
