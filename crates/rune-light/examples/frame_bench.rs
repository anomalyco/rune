//! CPU cost of recording rune-light and rune-fx frames, headless. Prints one line per measure: the median in µs over
//! its samples (and, for `gi off textures`, the textures a renderer holds after a frame without GI). It leaves out GPU
//! execution, upload and presentation.
//!
//!     cargo run -p rune-light --profile perf --example frame_bench
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use std::time::Instant;

use rune_fx::{Fx, FxFrame, FxOptions, FxRenderer, GlowParams, LensParams, Shape};
use rune_light::{Gi, Glass, GlassRenderer, GlassSettings, Light, LightRenderer, Prim, Scene, Settings};

const W: u32 = 2560;
const H: u32 = 1600;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("device");
    let view = |format| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let (base, text, target, out) = (view(wgpu::TextureFormat::Rgba16Float), view(wgpu::TextureFormat::Rgba16Float), view(FORMAT), view(FORMAT));
    let lights = [Light::point(400.0, 300.0, 200.0), Light::point(1800.0, 900.0, 160.0)];
    let prims = |n: usize| -> Vec<Prim> {
        (0..n).map(|i| Prim::rect((i % 20) as f32 * 120.0, (i / 20) as f32 * 60.0, 100.0, 44.0).radius(10.0).raised(6.0)).collect()
    };
    let small = prims(16);
    let big = prims(300);
    let scene = |prims, gi| Scene { prims, lights: &lights, settings: Settings { gi, ..Settings::default() }, emission: None };
    let finish = |encoder: wgpu::CommandEncoder| {
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    };
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };

    // A frame that outgrows the 256-prim buffer, on a fresh renderer each sample.
    let mut grow = Vec::new();
    for _ in 0..30 {
        let mut light = LightRenderer::new(&device, &queue, W, H, FORMAT);
        let mut encoder = device.create_command_encoder(&Default::default());
        light.render(&device, &queue, &mut encoder, &scene(&small, None), &base, &text, &target);
        finish(encoder);
        let mut encoder = device.create_command_encoder(&Default::default());
        let t = Instant::now();
        light.render(&device, &queue, &mut encoder, &scene(&big, None), &base, &text, &target);
        grow.push(t.elapsed().as_secs_f64() * 1e6);
        finish(encoder);
    }
    println!("grow past 256 prims  {:8.1} us", median(grow));

    let before = device.get_internal_counters().hal.textures.read();
    let mut light = LightRenderer::new(&device, &queue, W, H, FORMAT);
    let mut encoder = device.create_command_encoder(&Default::default());
    light.render(&device, &queue, &mut encoder, &scene(&small, None), &base, &text, &target);
    finish(encoder);
    println!("gi off textures      {:8}", device.get_internal_counters().hal.textures.read() - before);

    let mut glass = GlassRenderer::new(&device, W, H, FORMAT);
    let panes = [Glass::rounded(200.0, 200.0, 600.0, 400.0, 40.0), Glass::rounded(1200.0, 600.0, 500.0, 300.0, 30.0)];
    for (name, gi) in [("light steady gi off", None), ("light steady gi on", Some(Gi::default()))] {
        let (mut l, mut g) = (Vec::new(), Vec::new());
        for frame in 0..320 {
            let mut encoder = device.create_command_encoder(&Default::default());
            let t = Instant::now();
            light.render(&device, &queue, &mut encoder, &scene(&small, gi), &base, &text, &target);
            let lit = t.elapsed().as_secs_f64() * 1e6;
            let fluence = light.inspect().fluence.clone();
            let t = Instant::now();
            glass.render(&device, &queue, &mut encoder, &panes, &[], GlassSettings::default(), &target, &fluence, &out);
            let glassed = t.elapsed().as_secs_f64() * 1e6;
            finish(encoder);
            if frame >= 20 {
                l.push(lit);
                g.push(glassed);
            }
        }
        println!("{name:<20} {:8.1} us", median(l));
        println!("glass steady ({})  {:8.1} us", if gi.is_some() { "gi on " } else { "gi off" }, median(g));
    }

    let mut fx = FxRenderer::new(&device, FORMAT, FxOptions::default());
    let layers: Vec<_> = (0..16)
        .map(|i| {
            let shape = Shape::rounded(100.0 + (i % 4) as f32 * 300.0, 100.0 + (i / 4) as f32 * 200.0, 200.0, 120.0, 20.0);
            let fx = if i % 4 == 0 { Fx::lens(LensParams { frost: 0.6, ..Default::default() }) } else { Fx::glow(GlowParams { radius: 12.0, intensity: 0.8, ..Default::default() }) };
            fx.on(shape).at(0.5)
        })
        .collect();
    let mut f = Vec::new();
    for frame in 0..320 {
        let mut encoder = device.create_command_encoder(&Default::default());
        let t = Instant::now();
        fx.prepare(&device, &queue, &FxFrame { viewport: [W, H], scale: 1.0, layers: &layers, backdrop: Some(&base), sources: &[] });
        for i in (0..layers.len()).step_by(4) {
            fx.blur_backdrop(&device, &queue, &mut encoder, &base, i);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            fx.draw_all(&mut pass, &layers, None);
        }
        let us = t.elapsed().as_secs_f64() * 1e6;
        finish(encoder);
        if frame >= 20 {
            f.push(us);
        }
    }
    println!("fx steady 16 layers  {:8.1} us", median(f));
}
