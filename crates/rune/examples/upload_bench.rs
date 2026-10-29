//! Headless per-frame upload cost: `prepare`, `draw` and `submit` of a 2,000-instance frame,
//! with a few instances moving (`steady`) or all of them (`scroll`). Prints the median CPU
//! time to record and submit a frame, the median time until the GPU is done, and the buffers
//! created per frame. Run with `--profile perf`; it leaves out presentation, and the GPU copy
//! is counted only in `frame_us`.

use std::time::Instant;

use bytemuck::Zeroable;
use rune::render::{Gpu, Instance, NO_CLIP, Offscreen, Renderer};

const SIZE: (u32, u32) = (1280, 800);
const COUNT: usize = 2000;
const FRAMES: usize = 600;
const WARMUP: usize = 60;

fn scene(frame: usize, scroll: bool) -> Vec<Instance> {
    (0..COUNT)
        .map(|i| {
            let (mut x, mut y) = ((i % 50) as f32 * 25.0, (i / 50) as f32 * 20.0);
            if scroll {
                y -= (frame % 200) as f32;
            } else if i % 700 == 0 {
                x += (frame % 100) as f32;
            }
            Instance { rect: [x, y, 20.0, 14.0], fill: [0.2, 0.4, 0.8, 1.0], radii: [3.0; 4], clip: NO_CLIP, ..Zeroable::zeroed() }
        })
        .collect()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut text = rune::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    for (name, scroll) in [("steady", false), ("scroll", true)] {
        let mut renderer = Renderer::new(device, rune::render::OFFSCREEN_FORMAT);
        let (mut cpu, mut frame, mut created) = (Vec::new(), Vec::new(), 0isize);
        for i in 0..FRAMES + WARMUP {
            let instances = scene(i, scroll);
            let buffers = device.get_internal_counters().hal.buffers.read();
            let start = Instant::now();
            renderer.prepare(device, queue, &mut text.atlas, &instances, SIZE);
            let mut encoder = device.create_command_encoder(&Default::default());
            renderer.draw(&mut encoder, &target.view, [0.0; 4]);
            let made = device.get_internal_counters().hal.buffers.read() - buffers;
            renderer.submit(queue, encoder);
            let recorded = start.elapsed();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            if i >= WARMUP {
                cpu.push(recorded.as_secs_f64() * 1e6);
                frame.push(start.elapsed().as_secs_f64() * 1e6);
                created += made;
            }
        }
        println!(
            "scene={name} cpu_us={:.1} frame_us={:.1} buffers_per_frame={:.2}",
            median(cpu),
            median(frame),
            created as f64 / FRAMES as f64
        );
    }
}
