//! Measuring only (FRAME-PERF, #1140's split limit): what splitting a span into runs of one kind costs when the
//! kinds alternate, on the GPU and in the encoder. Three offscreen frames at 1440x960 drawn by the real renderer:
//! a terminal's full redraw (each cell's background, then its glyph), a code block with a chip behind each token
//! (a chip, then its glyphs), and a selection drawn as one rectangle per line before that line's glyphs. Each
//! frame is drawn 200 times per arm: `RUNE_PINNED` unset (the PR's limit), `all` (every span split) and `off`
//! (the general shader only); the arm comes from the environment, so run it three times. Frame-profile build.
//!   RUNE_PINNED=all cargo run -p rune --example split_cost --profile perf --features frame-profile
use bytemuck::Zeroable;
use rune::render::{Gpu, Instance, NO_CLIP, Offscreen, Renderer, OFFSCREEN_FORMAT};
use rune::text::TextSystem;
use std::time::Instant;

const KIND_RECT: f32 = 0.0;
const KIND_GLYPH: f32 = 2.0;

fn rect(x: f32, y: f32, w: f32, h: f32, fill: [f32; 4]) -> Instance {
    Instance { rect: [x, y, w, h], fill, params: [0.0, KIND_RECT, 0.0, 1.0], clip: NO_CLIP, ..Zeroable::zeroed() }
}

fn glyph(x: f32, y: f32) -> Instance {
    Instance { rect: [x, y, 8.0, 14.0], fill: [0.9, 0.9, 0.9, 1.0], params: [0.0, KIND_GLYPH, 0.0, 1.0], uv: [0.0, 0.0, 8.0, 14.0], clip: NO_CLIP, ..Zeroable::zeroed() }
}

fn main() {
    let gpu = Gpu::headless();
    let (d, q) = (&gpu.device, &gpu.queue);
    let arm = std::env::var("RUNE_PINNED").unwrap_or_else(|_| "limit".into());
    let (w, h) = (1440u32, 960u32);
    let target = Offscreen::new(d, w, h);
    let mut renderer = Renderer::new(d, OFFSCREEN_FORMAT);
    let mut atlas = TextSystem::default().atlas;
    atlas.data.resize((rune::text::ATLAS_SIZE * rune::text::ATLAS_SIZE) as usize, 128);
    let bg = [0.10, 0.10, 0.12, 1.0];
    // A terminal: 160 x 48 cells of 9 x 20 px, each a background then a glyph.
    let terminal: Vec<Instance> = std::iter::once(rect(0.0, 0.0, w as f32, h as f32, bg))
        .chain((0..48).flat_map(|r| (0..160).flat_map(move |c| {
            let (x, y) = (c as f32 * 9.0, r as f32 * 20.0);
            [rect(x, y, 9.0, 20.0, [0.12 + (c % 7) as f32 * 0.02, 0.12, 0.14, 1.0]), glyph(x + 0.5, y + 3.0)]
        }))).collect();
    // A highlighted code block: 45 lines of 25 tokens, a chip behind each token, then its 4 glyphs.
    let chips: Vec<Instance> = std::iter::once(rect(0.0, 0.0, w as f32, h as f32, bg))
        .chain((0..45).flat_map(|r| (0..25).flat_map(move |t| {
            let (x, y) = (8.0 + t as f32 * 56.0, 8.0 + r as f32 * 21.0);
            std::iter::once(rect(x, y, 40.0, 18.0, [0.2, 0.25, 0.35, 0.6])).chain((0..4).map(move |k| glyph(x + 2.0 + k as f32 * 9.0, y + 2.0)))
        }))).collect();
    // A selection: 45 lines, one rectangle each, then that line's 150 glyphs.
    let selection: Vec<Instance> = std::iter::once(rect(0.0, 0.0, w as f32, h as f32, bg))
        .chain((0..45).flat_map(|r| {
            let y = 8.0 + r as f32 * 21.0;
            std::iter::once(rect(8.0, y, 1350.0, 19.0, [0.2, 0.4, 0.8, 0.35])).chain((0..150).map(move |k| glyph(8.0 + k as f32 * 9.0, y + 2.0)))
        })).collect();
    println!("adapter {:?}, arm {arm}", gpu.adapter.get_info().name);
    for (name, instances) in [("terminal", &terminal), ("chips", &chips), ("selection", &selection)] {
        let frame = |renderer: &mut Renderer, atlas: &mut rune::text::Atlas| {
            renderer.prepare(d, q, atlas, instances, (w, h));
            let mut enc = d.create_command_encoder(&Default::default());
            let started = Instant::now();
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
            let encode = started.elapsed();
            let submitted = Instant::now();
            q.submit([enc.finish()]);
            d.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            (encode.as_secs_f64() * 1e6, submitted.elapsed().as_secs_f64() * 1e6)
        };
        // Warm: the pinned pipelines arrive from their thread a few frames in.
        for _ in 0..30 { frame(&mut renderer, &mut atlas); }
        std::thread::sleep(std::time::Duration::from_millis(500));
        for _ in 0..10 { frame(&mut renderer, &mut atlas); }
        let mut runs: Vec<(f64, f64)> = (0..200).map(|_| frame(&mut renderer, &mut atlas)).collect();
        let p = |v: &mut Vec<f64>, at: f64| { v.sort_by(f64::total_cmp); v[((v.len() as f64 - 1.0) * at) as usize] };
        let (mut enc, mut gpu_ms): (Vec<f64>, Vec<f64>) = runs.drain(..).unzip();
        println!("{name} ({} instances) {arm}: GPU (submit to done) p50 {:.0} us p90 {:.0} us; encode p50 {:.0} us p90 {:.0} us",
            instances.len(), p(&mut gpu_ms, 0.5), p(&mut gpu_ms, 0.9), p(&mut enc, 0.5), p(&mut enc, 0.9));
    }
}
