//! Continuous corners (negative radii) in `rune-light`'s shaders (`common.wgsl`, `glass.wgsl`) and
//! CPU reference (`cpu.rs`) must match Rune's uber shader (`clip.wgsl` `sd_rounded`).

use rune::render::{Gpu, Instance, KIND_RECT, NO_CLIP, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune_light::{Glass, GlassRenderer, GlassSettings, Light, LightRenderer, Prim, Scene, Settings, cpu};

const W: u32 = 96;
const H: u32 = 80;

fn read_rgba8(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
    let padded = (w * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = slice.get_mapped_range().expect("mapped");
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        out.extend_from_slice(&data[y * padded as usize..y * padded as usize + (w * 4) as usize]);
    }
    drop(data);
    buf.unmap();
    out
}

fn read_rgba16f(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<[f32; 4]> {
    let row_bytes = w * 8;
    let padded = row_bytes.div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback f16"),
        size: u64::from(padded * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = slice.get_mapped_range().expect("mapped");
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        let row = &data[y * padded as usize..y * padded as usize + row_bytes as usize];
        for px in row.chunks_exact(8) {
            let f = |i: usize| f16_to_f32(u16::from_le_bytes([px[i], px[i + 1]]));
            out.push([f(0), f(2), f(4), f(6)]);
        }
    }
    drop(data);
    buf.unmap();
    out
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

fn render_uber_rect(gpu: &Gpu, rect: [f32; 4], radii: [f32; 4]) -> Vec<u8> {
    let inst = Instance {
        rect,
        radii,
        fill: [1.0; 4],
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    };
    let mut atlas = rune::text::Atlas::new();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[inst], (W, H));
    let target = Offscreen::new(&gpu.device, W, H);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0; 4]);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    target.read(&gpu.device)
}

fn solid_texture(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, color: wgpu::Color) -> wgpu::Texture {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(color), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
    drop(_pass);
    queue.submit([enc.finish()]);
    tex
}

#[test]
fn light_gbuffer_field_and_cpu_match_rune_continuous_corners() {
    let Ok(gpu) = Gpu::try_new(Gpu::instance(), None) else { return eprintln!("no adapter; skipped") };
    let rect = [8.0, 8.0, 80.0, 64.0];
    let cases = [
        ("uniform-continuous", [-20.0; 4]),
        ("mixed-continuous", [-20.0, 12.0, -10.0, 0.0]),
    ];

    let base_tex = solid_texture(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba16Float, wgpu::Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 });
    let text_tex = solid_texture(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba16Float, wgpu::Color::TRANSPARENT);
    let out_tex = solid_texture(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba8Unorm, wgpu::Color::BLACK);
    let (base_view, text_view, out_view) = (
        base_tex.create_view(&Default::default()),
        text_tex.create_view(&Default::default()),
        out_tex.create_view(&Default::default()),
    );
    let mut light = LightRenderer::new(&gpu.device, &gpu.queue, W, H, wgpu::TextureFormat::Rgba8Unorm);

    for (name, radii) in cases {
        let uber = render_uber_rect(&gpu, rect, radii);
        let prim = Prim::from_rune(rect, radii).raised(16.0).emissive([1.0, 1.0, 1.0]);
        // Zero ambient + zero-intensity light isolates gbuffer surface ownership (`emissive` lights
        // `base` wherever `gbuffer.wgsl` keeps the pixel with `d <= 0.5`).
        let lights = [Light::point(48.0, 40.0, 100.0).intensity(0.0)];
        let settings = Settings {
            ambient: [0.0; 3],
            contact_ao: 0.0,
            rim: 0.0,
            albedo_floor: 0.0,
            background_gloss: 0.0,
            ..Settings::default()
        };
        let scene = Scene { prims: &[prim], lights: &lights, settings, emission: None };
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        assert!(light.render(&gpu.device, &gpu.queue, &mut enc, &scene, &base_view, &text_view, &out_view));
        gpu.queue.submit([enc.finish()]);
        let lit = read_rgba8(&gpu.device, &gpu.queue, &out_tex, W, H);

        // 1. Compare gbuffer surface silhouette inside the primitive's rect against Rune's uber
        //    shader continuous-corner mask. gbuffer keeps `d <= 0.5` (including the exact 0.5 px
        //    straight-edge outer border where uber alpha is 0), so check inside `[8..88, 8..72]`
        //    where every corner's continuous curve lies.
        let mut mismatches = 0usize;
        let mut first = None;
        for y in 8..72u32 {
            for x in 8..88u32 {
                let i = ((y * W + x) * 4) as usize;
                let uber_a = uber[i + 3];
                let lit_r = lit[i];
                let uber_inside = uber_a > 5;
                let uber_outside = uber_a == 0;
                let lit_inside = lit_r > 100;
                if (uber_inside && !lit_inside) || (uber_outside && lit_inside) {
                    mismatches += 1;
                    if first.is_none() {
                        first = Some((x, y, uber_a, lit_r));
                    }
                }
            }
        }
        assert_eq!(
            mismatches, 0,
            "{name}: rune-light gbuffer silhouette drifted from uber shader: {mismatches} pixels, first={first:?}"
        );

        // 2. Compare GPU `field` SDF against `cpu::field` AND check that the continuous corner
        //    actually cuts the corner (at `(10, 10)` inside the bounding box, distance must be positive!).
        let (fw, fh) = light.field_size();
        let field_tex = light.inspect().field.texture().clone();
        let gpu_field = read_rgba16f(&gpu.device, &gpu.queue, &field_tex, fw, fh);
        let mut max_sd_err = 0.0f32;
        let mut worst_sd = None;
        for fy in 0..fh {
            for fx in 0..fw {
                let p = [(fx as f32 + 0.5) * 2.0, (fy as f32 + 0.5) * 2.0];
                let cpu_d = cpu::field(&[prim], p)[0];
                let gpu_d = gpu_field[(fy * fw + fx) as usize][0];
                let err = (cpu_d - gpu_d).abs();
                if err > max_sd_err {
                    max_sd_err = err;
                    worst_sd = Some((fx, fy, cpu_d, gpu_d));
                }
            }
        }
        assert!(
            max_sd_err < 0.05,
            "{name}: GPU field SDF drifted from cpu::field: max_err={max_sd_err} worst={worst_sd:?}"
        );
        // Pixel (11, 11) is 3 px inside the sharp corner (8, 8): outside a continuous r=20 corner.
        let corner_cpu = cpu::field(&[prim], [11.0, 11.0])[0];
        let corner_gpu = gpu_field[(5 * fw + 5) as usize][0];
        assert!(
            corner_cpu > 3.0 && corner_gpu > 3.0,
            "{name}: continuous corner at (11,11) must have positive SDF (outside shape), got cpu={corner_cpu} gpu={corner_gpu}"
        );
    }
}

#[test]
fn glass_continuous_corners_match_rune_uber_shader() {
    let Ok(gpu) = Gpu::try_new(Gpu::instance(), None) else { return eprintln!("no adapter; skipped") };
    let rect = [8.0, 8.0, 80.0, 64.0];
    let uber = render_uber_rect(&gpu, rect, [-20.0; 4]);

    // Pure red backdrop, opaque-absorbing glass with zero env/shadow/caustic: outside glass stays
    // red (255), inside glass goes black (0), edge coverage blends linearly with `clamp(0.5 - d)`.
    let backdrop = solid_texture(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba16Float, wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
    let black = solid_texture(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba16Float, wgpu::Color::BLACK);
    let out = solid_texture(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba8Unorm, wgpu::Color::BLACK);
    let (bd_view, bl_view, out_view) = (
        backdrop.create_view(&Default::default()),
        black.create_view(&Default::default()),
        out.create_view(&Default::default()),
    );
    let mut glass = GlassRenderer::new(&gpu.device, W, H, wgpu::TextureFormat::Rgba8Unorm);
    let slab = Glass::rounded(rect[0], rect[1], rect[2], rect[3], -20.0)
        .thickness(24.0)
        .bevel(1.0)
        .gap(4.0)
        .tint([10.0, 10.0, 10.0])
        .rim_glow(0.0);
    let settings = GlassSettings { env: 0.0, shadow: 0.0, caustic: 0.0, ..GlassSettings::default() };
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    glass.render(&gpu.device, &gpu.queue, &mut enc, &[slab], &[], settings, &bd_view, &bl_view, &out_view);
    gpu.queue.submit([enc.finish()]);
    let px = read_rgba8(&gpu.device, &gpu.queue, &out, W, H);

    let mut mismatches = 0usize;
    let mut first = None;
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let uber_a = uber[i + 3];
            // Outside the continuous corner (`uber_a == 0`), glass with shadow=0 leaves the red
            // backdrop untouched (r=255). Deep inside (past the 1px bevel, so 3x3 neighbourhood is
            // all uber_a == 255), absorbing glass reduces red near zero (only ~4% Fresnel).
            let glass_cov = 255u8.saturating_sub(px[i]);
            let deep_inside = x >= 10 && x < 86 && y >= 10 && y < 70 && uber_a == 255 && {
                let mut all = true;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let ni = (((y as i32 + dy) as u32 * W + (x as i32 + dx) as u32) * 4) as usize;
                        if uber[ni + 3] != 255 {
                            all = false;
                        }
                    }
                }
                all
            };
            if (uber_a == 0 && glass_cov > 10) || (deep_inside && glass_cov < 200) {
                mismatches += 1;
                if first.is_none() {
                    first = Some((x, y, uber_a, px[i]));
                }
            }
        }
    }
    assert_eq!(
        mismatches, 0,
        "glass.wgsl continuous corner drifted from uber shader: {mismatches} pixels, first={first:?}"
    );
}
