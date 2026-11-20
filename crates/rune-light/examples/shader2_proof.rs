//! Throwaway proof program (never merged) exercising all four shaders audited by SHADER-2
//! on a single headless GPU device:
//! 1. `backdrop_composite.wgsl` (shape SDF, `clip_alpha`, pyramid UV in Rgba16Float + cached vs uncached)
//! 2. `csd_corners.rs` (left/right symmetry + exact `distance == radius - 0.5` boundary)
//! 3. `rune-fx` `common.wgsl` (`clip_alpha`, `bd`) + 7 effect shaders (`distort`, `extras`, `glow`,
//!    `pulse`, `electric`, `fire`, `motion`), including shifted-origin `draw_layer_in`
//! 4. `rune-light` `gbuffer.wgsl` (`d == 0.5` outer discard boundary + symmetry)

use bytemuck::Zeroable;
use rune::render::csd_corners::CsdCorners;
use rune::render::{
    Blend, Gpu, Instance, KIND_BACKDROP_BLUR, KIND_RECT, LayerSpec, NO_CLIP, OFFSCREEN_FORMAT,
    Offscreen, Renderer,
};
use rune::text::Atlas;
use rune_fx::{
    AuroraParams, BlurMode, DistortParams, ElectricParams, FireParams, Fx, FxBlend, FxFrame,
    FxLayer, FxOptions, FxRenderer, FxSlot, GlowParams, GrainParams, LensParams,
    MeshGradientParams, MotionBlurParams, MotionSamples, PlasticParams, Pose, PulseParams, Rect,
    Shape, ShimmerParams, Stops, rgb,
};
use rune_light::{Light, LightRenderer, Prim, Scene, Settings};

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

fn read_rgba8(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<[u8; 4]> {
    let padded = (w * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read8"),
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
        let row = &data[y * padded as usize..y * padded as usize + (w * 4) as usize];
        for px in row.chunks_exact(4) {
            out.push([px[0], px[1], px[2], px[3]]);
        }
    }
    drop(data);
    buf.unmap();
    out
}

fn read_rgba16f_bits(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<[u16; 4]> {
    let row_bytes = w * 8;
    let padded = row_bytes.div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read16f"),
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
            out.push([
                u16::from_le_bytes([px[0], px[1]]),
                u16::from_le_bytes([px[2], px[3]]),
                u16::from_le_bytes([px[4], px[5]]),
                u16::from_le_bytes([px[6], px[7]]),
            ]);
        }
    }
    drop(data);
    buf.unmap();
    out
}

fn make_tex(device: &wgpu::Device, w: u32, h: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("proof tex"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn rect_inst(rect: [f32; 4], fill: [f32; 4]) -> Instance {
    Instance {
        rect,
        fill,
        params: [0.0, KIND_RECT, 0.0, 1.0],
        clip: NO_CLIP,
        ..Instance::zeroed()
    }
}

fn probe_backdrop_composite(gpu: &Gpu) {
    println!("=== PROBE 1: backdrop_composite.wgsl ===");
    let (w, h) = (40u32, 28u32);
    let mut atlas = Atlas::new();
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba16Float);

    // 1a: Direct Rgba16Float 40x28 symmetric scene: blue bg [0,0,40,28], centered red [12,10,16,8],
    // centered backdrop_blur [8,6,24,16] (top y=6, bottom y=21, left x=8, right x=31).
    let run_f16 = |renderer: &mut Renderer, atlas: &mut Atlas, bd_rect: [f32; 4], clip: [f32; 4]| -> Vec<[u16; 4]> {
        let insts = [
            rect_inst([0.0, 0.0, 40.0, 28.0], [0.0, 0.0, 1.0, 1.0]),
            rect_inst([12.0, 10.0, 16.0, 8.0], [1.0, 0.0, 0.0, 1.0]),
            Instance {
                rect: bd_rect,
                fill: [0.0, 0.0, 0.0, 1.0],
                params: [4.0, KIND_BACKDROP_BLUR, 0.0, 1.0],
                clip,
                ..Instance::zeroed()
            },
        ];
        renderer.prepare(&gpu.device, &gpu.queue, atlas, &insts, (w, h));
        let tex = make_tex(&gpu.device, w, h, wgpu::TextureFormat::Rgba16Float);
        let view = tex.create_view(&Default::default());
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &view, [0.0; 4]);
        renderer.submit(&gpu.queue, enc);
        read_rgba16f_bits(&gpu.device, &gpu.queue, &tex, w, h)
    };

    for (label, bd_rect, clip) in [
        ("1a-sd_rounded-sym", [8.0, 6.0, 24.0, 16.0], NO_CLIP),
        ("1a2-sd_rounded-10", [10.0, 10.0, 20.0, 8.0], NO_CLIP),
        ("1c-clip_alpha", [0.0, 0.0, 40.0, 28.0], [8.0, 6.0, 32.0, 22.0]),
    ] {
        let px = run_f16(&mut renderer, &mut atlas, bd_rect, clip);
        let (x0, y0, x1, y1) = if clip != NO_CLIP {
            (clip[0] as u32, clip[1] as u32, clip[2] as u32, clip[3] as u32)
        } else {
            (bd_rect[0] as u32, bd_rect[1] as u32, (bd_rect[0] + bd_rect[2]) as u32, (bd_rect[1] + bd_rect[3]) as u32)
        };
        let mut tb_diffs = 0usize;
        let mut lr_diffs = 0usize;
        let mut sample_tb = None;
        let mut sample_lr = None;
        for x in (x0 + 1)..(x1 - 1) {
            let top = px[(y0 * w + x) as usize];
            let bot = px[((y1 - 1) * w + x) as usize];
            if top != bot {
                tb_diffs += 1;
                if sample_tb.is_none() {
                    sample_tb = Some((x, top.map(f16_to_f32), bot.map(f16_to_f32), top, bot));
                }
            }
        }
        for y in (y0 + 1)..(y1 - 1) {
            let left = px[(y * w + x0) as usize];
            let right = px[(y * w + (x1 - 1)) as usize];
            if left != right {
                lr_diffs += 1;
                if sample_lr.is_none() {
                    sample_lr = Some((y, left.map(f16_to_f32), right.map(f16_to_f32), left, right));
                }
            }
        }
        println!("  {label}: top!=bot {tb_diffs}/{}, left!=right {lr_diffs}/{}", x1 - x0 - 2, y1 - y0 - 2);
        if let Some((x, t, b, th, bh)) = sample_tb {
            println!("    sample tb x={x}: top={t:?} ({th:#06x?}) bot={b:?} ({bh:#06x?})");
        }
        if let Some((y, l, r, lh, rh)) = sample_lr {
            println!("    sample lr y={y}: left={l:?} ({lh:#06x?}) right={r:?} ({rh:#06x?})");
        }
    }

    // 1b: Cached Rgba16Float isolated layer [6,6,40,28] vs direct Rgba8UnormSrgb in 64x48 viewport
    // (exact #1289 pattern: red layer bg, 1px blue edge under backdrop_blur [10,10,24,8]).
    let (vw, vh) = (64u32, 48u32);
    let mut ren_srgb = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let render_srgb = |ren: &mut Renderer, atlas: &mut Atlas, isolated: bool| -> Vec<[u8; 4]> {
        let mut insts = Vec::new();
        if isolated {
            insts.push(Instance::begin_layer(
                [6.0, 6.0, 40.0, 28.0],
                [6.0, 6.0, 46.0, 34.0],
                LayerSpec { opacity: 1.0, blend: Blend::SourceOver, blur: 0.0, clip_radius: 1.0 },
            ));
        }
        insts.push(rect_inst([6.0, 6.0, 40.0, 28.0], [1.0, 0.0, 0.0, 1.0]));
        // 1px blue strips directly underneath all 4 edges of [10,10,24,8]
        insts.push(rect_inst([10.0, 10.0, 24.0, 1.0], [0.0, 0.0, 1.0, 1.0]));
        insts.push(rect_inst([10.0, 17.0, 24.0, 1.0], [0.0, 0.0, 1.0, 1.0]));
        insts.push(rect_inst([10.0, 10.0, 1.0, 8.0], [0.0, 0.0, 1.0, 1.0]));
        insts.push(rect_inst([33.0, 10.0, 1.0, 8.0], [0.0, 0.0, 1.0, 1.0]));
        insts.push(Instance {
            rect: [10.0, 10.0, 24.0, 8.0],
            fill: [0.0, 0.0, 0.0, 1.0],
            params: [16.0, KIND_BACKDROP_BLUR, 0.0, 1.0],
            clip: NO_CLIP,
            ..Instance::zeroed()
        });
        if isolated {
            insts.push(Instance::end_layer());
        }
        ren.prepare(&gpu.device, &gpu.queue, atlas, &insts, (vw, vh));
        let target = Offscreen::with_format(&gpu.device, vw, vh, OFFSCREEN_FORMAT);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        ren.draw(&mut enc, &target.view, [0.0; 4]);
        target.copy(&mut enc);
        ren.submit(&gpu.queue, enc);
        target.read(&gpu.device).chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect()
    };
    let direct = render_srgb(&mut ren_srgb, &mut atlas, false);
    let iso = render_srgb(&mut ren_srgb, &mut atlas, true);
    let mut diffs = 0usize;
    let mut top_left_diffs = 0usize;
    let mut bot_right_diffs = 0usize;
    let mut first = None;
    for y in 10..18u32 {
        for x in 10..34u32 {
            let a = iso[(y * vw + x) as usize];
            let b = direct[(y * vw + x) as usize];
            if a != b {
                diffs += 1;
                if y == 10 || x == 10 { top_left_diffs += 1; }
                if y == 17 || x == 33 { bot_right_diffs += 1; }
                if first.is_none() { first = Some((x, y, a, b)); }
            }
        }
    }
    println!("  1b-isolated-vs-direct-srgb: total_diffs={diffs} (top/left={top_left_diffs}, bot/right={bot_right_diffs}), first={first:?}");
}

fn probe_csd_corners(gpu: &Gpu) {
    println!("=== PROBE 2: csd_corners.rs ===");
    for &(w, h, radius) in &[
        (40u32, 30u32, 6.0f32),
        (100, 70, 13.5),
        (1280, 720, 12.0),
        (1366, 768, 13.5),
        (1440, 900, 15.0),
        (1920, 1080, 18.0),
    ] {
        let band = (radius.ceil() as u32 + 2).min(w / 2).min(h);
        // Test Rgba16Float (highest sensitivity to tiny outside > 0 leaks) and Rgba8UnormSrgb
        let tex16 = make_tex(&gpu.device, w, h, wgpu::TextureFormat::Rgba16Float);
        let view16 = tex16.create_view(&Default::default());
        let mut pass16 = CsdCorners::new(&gpu.device, wgpu::TextureFormat::Rgba16Float);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        {
            let _c = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view16,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::RED), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
        }
        pass16.prepare(&gpu.queue, (w, h), radius);
        pass16.draw(&mut enc, &view16);
        gpu.queue.submit([enc.finish()]);
        let px16 = read_rgba16f_bits(&gpu.device, &gpu.queue, &tex16, w, h);

        let mut sym16_diffs = 0usize;
        let mut first_sym16 = None;
        for y in (h - band)..h {
            for x in 0..band {
                let l = px16[(y * w + x) as usize];
                let r = px16[(y * w + (w - 1 - x)) as usize];
                if l != r {
                    sym16_diffs += 1;
                    if first_sym16.is_none() {
                        first_sym16 = Some((x, y, l.map(f16_to_f32), r.map(f16_to_f32), l, r));
                    }
                }
            }
        }
        let mut leak16 = 0usize;
        let mut first_leak16 = None;
        let lc = (radius, h as f32 - radius);
        let rc = (w as f32 - radius, h as f32 - radius);
        for y in (h - band)..h {
            for x in (0..band).chain((w - band)..w) {
                let cx = x as f32 + 0.5;
                let cy = y as f32 + 0.5;
                let in_left = cx <= radius && cy >= h as f32 - radius;
                let in_right = cx >= w as f32 - radius && cy >= h as f32 - radius;
                let exact_d = if in_left {
                    ((cx - lc.0).powi(2) + (cy - lc.1).powi(2)).sqrt()
                } else if in_right {
                    ((cx - rc.0).powi(2) + (cy - rc.1).powi(2)).sqrt()
                } else {
                    0.0
                };
                if exact_d <= radius - 0.5 + 1e-6 {
                    let got = px16[(y * w + x) as usize];
                    if got[3] != 0x3c00 {
                        leak16 += 1;
                        if first_leak16.is_none() {
                            first_leak16 = Some((x, y, exact_d, got.map(f16_to_f32), got));
                        }
                    }
                }
            }
        }

        // Also Rgba8UnormSrgb symmetry
        let tex8 = make_tex(&gpu.device, w, h, OFFSCREEN_FORMAT);
        let view8 = tex8.create_view(&Default::default());
        let mut pass8 = CsdCorners::new(&gpu.device, OFFSCREEN_FORMAT);
        let mut enc8 = gpu.device.create_command_encoder(&Default::default());
        {
            let _c = enc8.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view8,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::RED), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
        }
        pass8.prepare(&gpu.queue, (w, h), radius);
        pass8.draw(&mut enc8, &view8);
        gpu.queue.submit([enc8.finish()]);
        let px8 = read_rgba8(&gpu.device, &gpu.queue, &tex8, w, h);
        let mut sym8_diffs = 0usize;
        let mut first_sym8 = None;
        for y in (h - band)..h {
            for x in 0..band {
                let l = px8[(y * w + x) as usize];
                let r = px8[(y * w + (w - 1 - x)) as usize];
                if l != r {
                    sym8_diffs += 1;
                    if first_sym8.is_none() { first_sym8 = Some((x, y, l, r)); }
                }
            }
        }
        println!("  size=({w},{h}) r={radius}: f16_sym_diffs={sym16_diffs} f16_boundary_leaks={leak16} srgb8_sym_diffs={sym8_diffs}");
        if let Some(s) = first_sym16 { println!("    f16_sym first={s:?}"); }
        if let Some(l) = first_leak16 { println!("    f16_leak first={l:?}"); }
        if let Some(s) = first_sym8 { println!("    srgb8_sym first={s:?}"); }
    }
}

fn probe_rune_fx(gpu: &Gpu) {
    println!("=== PROBE 3: rune-fx (common.wgsl + 7 effect shaders) ===");
    let fmt = wgpu::TextureFormat::Rgba16Float;

    // 3a: common.wgsl:129 clip_alpha(in.px) on Rgba16Float 40x28 blue bg, pure-red MeshGradient fill larger than clip [10,10,34,18]
    {
        let (w, h) = (40u32, 28u32);
        let mut fx = FxRenderer::new(&gpu.device, fmt, FxOptions { dither: false });
        let layers = [Fx::fill(MeshGradientParams {
            colors: Stops::hex(&[0xff0000, 0xff0000, 0xff0000]),
            ..MeshGradientParams::dusk()
        })
        .on(Shape::rounded(6.0, 6.0, 32.0, 20.0, 0.0))];
        let slots = [FxSlot::default().clip([10.0, 10.0, 34.0, 18.0], [0; 4])];
        fx.prepare_with_slots(
            &gpu.device,
            &gpu.queue,
            &FxFrame { viewport: [w, h], scale: 1.0, layers: &layers, backdrop: None, sources: &[] },
            &slots,
        );
        let tex = make_tex(&gpu.device, w, h, fmt);
        let view = tex.create_view(&Default::default());
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLUE), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            fx.draw_layer(&mut pass, 0);
        }
        gpu.queue.submit([enc.finish()]);
        let px = read_rgba16f_bits(&gpu.device, &gpu.queue, &tex, w, h);
        // Any pixel inside [10..34, 10..18] should have B == 0.0 (no blue leak from dst)
        let mut tl_leaks = 0usize;
        let mut br_leaks = 0usize;
        let mut first = None;
        for y in 10..18u32 {
            for x in 10..34u32 {
                let p = px[(y * w + x) as usize];
                let b = f16_to_f32(p[2]);
                if b > 0.0 {
                    if y == 10 || x == 10 { tl_leaks += 1; } else { br_leaks += 1; }
                    if first.is_none() { first = Some((x, y, p.map(f16_to_f32), p)); }
                }
            }
        }
        println!("  3a-common-clip_alpha: top/left_blue_leaks={tl_leaks}/31, other_leaks={br_leaks}, first={first:?}");
    }

    // 3b: distort.wgsl:102 Lens coverage on 40x28 Rgba16Float
    {
        let (w, h) = (40u32, 28u32);
        let mut fx = FxRenderer::new(&gpu.device, fmt, FxOptions { dither: false });
        let bd_tex = make_tex(&gpu.device, w, h, fmt);
        let bd_view = bd_tex.create_view(&Default::default());
        let mut enc0 = gpu.device.create_command_encoder(&Default::default());
        {
            let _c = enc0.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &bd_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLUE), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
        }
        gpu.queue.submit([enc0.finish()]);
        let layers = [Fx::lens(LensParams {
            depth: 0.0,
            shadow: 0.0,
            rim: 0.0,
            frost: 0.0,
            magnification: 1.0,
            presence: 1.0,
            tint: rgb(0xff0000),
            tint_amount: 1.0,
            ..Default::default()
        })
        .on(Shape::rounded(10.0, 10.0, 24.0, 8.0, 0.0))];
        fx.prepare(
            &gpu.device,
            &gpu.queue,
            &FxFrame { viewport: [w, h], scale: 1.0, layers: &layers, backdrop: Some(&bd_view), sources: &[] },
        );
        let tex = make_tex(&gpu.device, w, h, fmt);
        let view = tex.create_view(&Default::default());
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLUE), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            fx.draw_layer(&mut pass, 0);
        }
        gpu.queue.submit([enc.finish()]);
        let px = read_rgba16f_bits(&gpu.device, &gpu.queue, &tex, w, h);
        // Inside [10..34, 10..18], exact coverage == 1.0 => col = blue * red = 0 => B == 0.0
        let mut tl_leaks = 0usize;
        let mut other_leaks = 0usize;
        let mut first = None;
        for y in 10..18u32 {
            for x in 10..34u32 {
                let p = px[(y * w + x) as usize];
                let b = f16_to_f32(p[2]);
                if b > 0.0 {
                    if y == 10 || x == 10 { tl_leaks += 1; } else { other_leaks += 1; }
                    if first.is_none() { first = Some((x, y, p.map(f16_to_f32), p)); }
                }
            }
        }
        println!("  3b-distort-lens-cov: top/left_blue_leaks={tl_leaks}/31, other_leaks={other_leaks}, first={first:?}");
    }

    // 3c: Exact shifted-origin draw_layer_in invariance + texel-centre bleed check across ALL 7
    // remaining rune-fx shaders (glow, pulse, electric, fire, distort, extras, motion) and
    // common.wgsl:152 bd(in.px, 1.0).
    // Draw A renders the window-space layer into a 56x40 target at origin=[0,0] (cropped [16..56, 12..40]).
    // Draw B renders the EXACT same layer into a 40x28 target via draw_layer_in at origin=[16,12].
    // Both must be bit-for-bit identical in Rgba16Float AND hit exact checkerboard texel centres!
    {
        let (vw, vh) = (56u32, 40u32);
        let (tw, th) = (40u32, 28u32);
        let (ox, oy) = (16u32, 12u32);
        let origin = [ox as f32, oy as f32];
        let mut fx = FxRenderer::new(&gpu.device, fmt, FxOptions { dither: false });

        // Sharp 1px black/white checkerboard (56x40)
        let bd_tex = make_tex(&gpu.device, vw, vh, wgpu::TextureFormat::Rgba8Unorm);
        let mut data = vec![0u8; (vw * vh * 4) as usize];
        for y in 0..vh {
            for x in 0..vw {
                let v = if ((x + y) & 1) == 0 { 255u8 } else { 0u8 };
                let idx = ((y * vw + x) * 4) as usize;
                data[idx..idx + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &bd_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(vw * 4), rows_per_image: Some(vh) },
            wgpu::Extent3d { width: vw, height: vh, depth_or_array_layers: 1 },
        );
        let bd_v = bd_tex.create_view(&Default::default());

        let render_pair = |fx: &mut FxRenderer, layer: FxLayer, blend: FxBlend| -> (Vec<[u16; 4]>, Vec<[u16; 4]>) {
            let layers = [layer];
            let slots = [FxSlot::default().blend(blend)];
            let sources = [&bd_v];
            fx.prepare_with_slots(
                &gpu.device,
                &gpu.queue,
                &FxFrame { viewport: [vw, vh], scale: 1.0, layers: &layers, backdrop: Some(&bd_v), sources: &sources },
                &slots,
            );
            let run = |w: u32, h: u32, org: [f32; 2]| -> Vec<[u16; 4]> {
                let tex = make_tex(&gpu.device, w, h, fmt);
                let view = tex.create_view(&Default::default());
                let mut enc = gpu.device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                        })],
                        ..Default::default()
                    });
                    fx.draw_layer_in(&mut pass, fmt, org, 0);
                }
                gpu.queue.submit([enc.finish()]);
                read_rgba16f_bits(&gpu.device, &gpu.queue, &tex, w, h)
            };
            let full = run(vw, vh, [0.0, 0.0]);
            let mut crop = Vec::with_capacity((tw * th) as usize);
            for y in oy..oy + th {
                for x in ox..ox + tw {
                    crop.push(full[(y * vw + x) as usize]);
                }
            }
            let shifted = run(tw, th, origin);
            (crop, shifted)
        };

        let shape = Shape::rounded(24.0, 18.0, 24.0, 14.0, 4.0);
        let sharp_shape = Shape::rounded(24.0, 18.0, 24.0, 14.0, 0.0);
        let motion_rest = {
            let r = Rect::new(24.0, 18.0, 24.0, 14.0);
            let p = MotionBlurParams::default();
            let ms = MotionSamples::from_track(&p, r, 1.0, 0.0, |_| Pose::default());
            Fx::motion_blur(p, ms).on(sharp_shape).source(0, Rect::new(0.0, 0.0, vw as f32, vh as f32))
        };

        let motion_dir = {
            let r = Rect::new(24.0, 18.0, 24.0, 14.0);
            let p = MotionBlurParams { mode: BlurMode::Directional, ..Default::default() };
            let ms = MotionSamples::from_track(&p, r, 1.0, 0.5, |t| Pose { x: t * 200.0, y: t * 120.0, ..Default::default() });
            Fx::motion_blur(p, ms).on(sharp_shape).source(0, Rect::new(0.0, 0.0, vw as f32, vh as f32))
        };

        let cases: Vec<(&str, FxLayer, FxBlend)> = vec![
            ("glow", Fx::glow(GlowParams::default()).on(shape).at(0.5), FxBlend::Normal),
            ("common-bd-multiply", Fx::fill(PlasticParams { color: rgb(0xffffff), roughness: 1.0, dome: 0.0, bevel: 0.0, ..PlasticParams::matte() }).on(sharp_shape), FxBlend::Multiply),
            ("pulse", Fx::pulse(PulseParams::default()).on(shape).at(0.5), FxBlend::Normal),
            ("electric", Fx::electric(ElectricParams::default()).on(shape).at(0.5), FxBlend::Normal),
            ("fire-plasma", Fx::fire(FireParams::plasma()).on(shape).at(0.5), FxBlend::Normal),
            ("distort-chroma", Fx::distort(DistortParams::chroma_split([1e-6, 0.0])).on(sharp_shape).at(0.5), FxBlend::Normal),
            ("extras-shimmer", Fx::shimmer(ShimmerParams { base_alpha: 1.0, ..Default::default() }).on(sharp_shape).at(0.5), FxBlend::Normal),
            ("extras-aurora", Fx::aurora(AuroraParams::default()).on(shape).at(0.5), FxBlend::Normal),
            ("extras-grain", Fx::grain(GrainParams { amount: 0.2, fps: 0.0, feather: 0.0 }).on(sharp_shape).at(0.5), FxBlend::Normal),
            ("motion-rest", motion_rest, FxBlend::Normal),
            ("motion-dir", motion_dir, FxBlend::Normal),
        ];

        for (name, layer, blend) in cases {
            let (crop, shifted) = render_pair(&mut fx, layer, blend);
            let mut diffs = 0usize;
            let mut first = None;
            for y in 0..th {
                for x in 0..tw {
                    let pa = crop[(y * tw + x) as usize];
                    let pb = shifted[(y * tw + x) as usize];
                    if pa != pb {
                        diffs += 1;
                        if first.is_none() {
                            first = Some((x, y, pa.map(f16_to_f32), pb.map(f16_to_f32)));
                        }
                    }
                }
            }
            // For checkerboard samplers (motion-rest, common-bd-multiply), also check texel-centre bleed!
            let mut bleed = 0usize;
            let mut first_bleed = None;
            if name == "motion-rest" || name == "common-bd-multiply" {
                for y in 6..20u32 {
                    for x in 8..32u32 {
                        let p = crop[(y * tw + x) as usize];
                        let r = f16_to_f32(p[0]);
                        let expected_white = ((x + ox + y + oy) & 1) == 0;
                        let bad = if expected_white { r < 0.999 } else { r > 0.001 };
                        if bad {
                            bleed += 1;
                            if first_bleed.is_none() {
                                first_bleed = Some((x + ox, y + oy, expected_white, r, p));
                            }
                        }
                    }
                }
            }
            println!("  3c-{name}: origin_diffs={diffs}, first={first:?}, checker_bleed={bleed}, first_bleed={first_bleed:?}");
        }
    }
}

fn probe_gbuffer(gpu: &Gpu) {
    println!("=== PROBE 4: rune-light gbuffer.wgsl ===");
    for &(w, h, rect) in &[
        (40u32, 28u32, [10.0f32, 10.0, 20.0, 10.0]),
        (100, 70, [10.0, 10.0, 40.0, 30.0]),
        (96, 80, [8.0, 8.0, 80.0, 64.0]),
        (111, 73, [11.0, 9.0, 41.0, 29.0]),
        (1280, 720, [101.0, 73.0, 200.0, 120.0]),
    ] {
        let solid = |format: wgpu::TextureFormat, color: wgpu::Color| -> wgpu::Texture {
            let tex = make_tex(&gpu.device, w, h, format);
            let view = tex.create_view(&Default::default());
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            let _p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(color), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            drop(_p);
            gpu.queue.submit([enc.finish()]);
            tex
        };
        let base_tex = solid(wgpu::TextureFormat::Rgba16Float, wgpu::Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 });
        let text_tex = solid(wgpu::TextureFormat::Rgba16Float, wgpu::Color::TRANSPARENT);
        let out_tex = solid(wgpu::TextureFormat::Rgba8Unorm, wgpu::Color::BLACK);
        let (bv, tv, ov) = (
            base_tex.create_view(&Default::default()),
            text_tex.create_view(&Default::default()),
            out_tex.create_view(&Default::default()),
        );
        let mut light = LightRenderer::new(&gpu.device, &gpu.queue, w, h, wgpu::TextureFormat::Rgba8Unorm);
        let prim = Prim::from_rune(rect, [0.0; 4]).raised(16.0).emissive([1.0, 1.0, 1.0]);
        let lights = [Light::point(w as f32 * 0.5, h as f32 * 0.5, 100.0).intensity(0.0)];
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
        assert!(light.render(&gpu.device, &gpu.queue, &mut enc, &scene, &bv, &tv, &ov));
        gpu.queue.submit([enc.finish()]);
        let px = read_rgba8(&gpu.device, &gpu.queue, &out_tex, w, h);

        let (x0, y0, rw, rh) = (rect[0] as u32, rect[1] as u32, rect[2] as u32, rect[3] as u32);
        let top_kept = (x0..x0 + rw).filter(|&x| px[((y0 - 1) * w + x) as usize][0] > 100).count();
        let bot_kept = (x0..x0 + rw).filter(|&x| px[((y0 + rh) * w + x) as usize][0] > 100).count();
        let left_kept = (y0..y0 + rh).filter(|&y| px[(y * w + (x0 - 1)) as usize][0] > 100).count();
        let right_kept = (y0..y0 + rh).filter(|&y| px[(y * w + (x0 + rw)) as usize][0] > 100).count();
        let tb_asym = (x0..x0 + rw).filter(|&x| px[((y0 - 1) * w + x) as usize] != px[((y0 + rh) * w + x) as usize]).count();
        let lr_asym = (y0..y0 + rh).filter(|&y| px[(y * w + (x0 - 1)) as usize] != px[(y * w + (x0 + rw)) as usize]).count();
        let top_sample = px[((y0 - 1) * w + x0) as usize];
        let bot_sample = px[((y0 + rh) * w + x0) as usize];
        let left_sample = px[(y0 * w + (x0 - 1)) as usize];
        let right_sample = px[(y0 * w + (x0 + rw)) as usize];
        println!(
            "  size=({w},{h}) rect={rect:?}: d=0.5 kept top={top_kept}/{rw} bot={bot_kept}/{rw} left={left_kept}/{rh} right={right_kept}/{rh} | asym tb={tb_asym}/{rw} lr={lr_asym}/{rh} | samples top={top_sample:?} bot={bot_sample:?} left={left_sample:?} right={right_sample:?}"
        );
    }
}

fn main() {
    let gpu = Gpu::headless();
    let info = gpu.adapter.get_info();
    println!("adapter: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
    probe_backdrop_composite(&gpu);
    probe_csd_corners(&gpu);
    probe_rune_fx(&gpu);
    probe_gbuffer(&gpu);
    println!("DONE");
}
