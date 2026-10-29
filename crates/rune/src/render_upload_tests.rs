use super::*;

const SIZE: (u32, u32) = (64, 32);
const RED: [u8; 4] = [255, 0, 0, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn square(x: f32, y: f32) -> Instance {
    Instance { rect: [x, y, 8.0, 8.0], fill: [1.0, 0.0, 0.0, 1.0], clip: NO_CLIP, ..Zeroable::zeroed() }
}

/// A moving square over a still grid of small ones.
fn scene(frame: usize) -> Vec<Instance> {
    let grid = (0..48).map(|i| Instance { rect: [(i % 16) as f32 * 4.0, (i / 16) as f32 * 4.0, 1.0, 1.0], ..square(0.0, 0.0) });
    std::iter::once(square(frame as f32, 20.0)).chain(grid).collect()
}

fn live_buffers(device: &wgpu::Device) -> isize {
    device.get_internal_counters().hal.buffers.read()
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE.0 + x) * 4) as usize;
    pixels[at..at + 4].try_into().unwrap()
}

#[test]
fn a_steady_animating_frame_creates_no_buffers_and_skips_unchanged_globals() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    let mut pixels = Vec::new();
    for frame in 0..24 {
        let (buffers, globals, stats) = (live_buffers(device), renderer.uploads.globals_writes, renderer.uploads.stats());
        renderer.prepare(device, queue, &mut text.atlas, &scene(frame), SIZE);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        // Before the submit, while a queue write's staging buffer would still be alive.
        let created = live_buffers(device) - buffers;
        if frame == 23 { target.copy(&mut encoder); }
        renderer.submit(queue, encoder);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        if frame < 4 { continue; }
        let now = renderer.uploads.stats();
        assert_eq!(created, 0, "frame {frame} created buffers");
        assert_eq!(now.chunks, stats.chunks, "frame {frame} made a staging chunk");
        assert_eq!(renderer.uploads.globals_writes, globals, "frame {frame} rewrote unchanged globals");
        assert_eq!((now.writes - stats.writes, now.bytes - stats.bytes), (1, size_of::<Instance>() as u64),
            "frame {frame} writes only the instance that moved");
        assert_eq!((now.flushes - stats.flushes, now.submits - stats.submits), (1, 0), "recorded into the frame");
    }
    pixels.extend(target.read(device));
    assert_eq!(pixel(&pixels, 27, 24), RED, "the square reached its last position");
    assert_eq!(pixel(&pixels, 2, 24), CLEAR, "and left its first");
}

/// The upload has no encoder to go into when the first draw is into a caller's pass, and a
/// caller may submit without `Renderer::submit`.
#[test]
fn uploads_land_before_a_draw_into_a_callers_pass() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    for x in [0.0, 30.0] {
        renderer.prepare(device, queue, &mut text.atlas, &[square(x, 4.0)], SIZE);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            renderer.draw_range(&mut pass, OFFSCREEN_FORMAT, 0..1);
        }
        target.copy(&mut encoder);
        queue.submit([encoder.finish()]);
        let pixels = target.read(device);
        assert_eq!(pixel(&pixels, x as u32 + 4, 8), RED);
    }
}

/// Writes recorded into an encoder that is then dropped never land; the next frame sends what
/// it would otherwise skip as unchanged.
#[test]
fn a_dropped_encoder_makes_the_next_frame_resend_everything() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    let instances = [square(10.0, 10.0)];
    renderer.prepare(device, queue, &mut text.atlas, &instances, SIZE);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    drop(encoder);
    renderer.prepare(device, queue, &mut text.atlas, &instances, SIZE);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    renderer.submit(queue, encoder);
    assert_eq!(pixel(&target.read(device), 14, 14), RED);
}

/// `Frame::write_buffer`: each write lands ahead of the commands recorded after it, and a frame's
/// writes share one reused staging chunk rather than making a buffer each, as a queue write would.
#[test]
fn compositor_buffer_writes_land_in_order_and_reuse_staging() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: None, size: 16, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None, size: 32, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    for frame in 0..12u32 {
        let buffers = live_buffers(device);
        let mut encoder = device.create_command_encoder(&Default::default());
        let allocs = crate::test_alloc::allocs();
        renderer.write_buffer(device, queue, &mut encoder, &uniforms, 0, bytemuck::cast_slice(&[frame; 4]));
        let first = crate::test_alloc::allocs() - allocs;
        encoder.copy_buffer_to_buffer(&uniforms, 0, &readback, 0, 16);
        let allocs = crate::test_alloc::allocs();
        renderer.write_buffer(device, queue, &mut encoder, &uniforms, 0, bytemuck::cast_slice(&[frame + 100; 4]));
        let second = crate::test_alloc::allocs() - allocs;
        encoder.copy_buffer_to_buffer(&uniforms, 0, &readback, 16, 16);
        let created = live_buffers(device) - buffers;
        renderer.submit(queue, encoder);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let words: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
        readback.unmap();
        assert_eq!(words, [[frame; 4], [frame + 100; 4]].concat(), "frame {frame}: each write before the copy after it");
        let stats = renderer.uploads.frame_write_stats();
        assert_eq!(stats.chunks, 1, "frame {frame}: the writes share one chunk, reused once the GPU is done");
        if frame >= 2 {
            assert_eq!(created, 0, "frame {frame} created buffers");
            // A queue write allocates 6 or 7 times; the first command in a new encoder grows its list once.
            assert!(first <= 1 && second == 0, "frame {frame}: the writes allocated {first} and {second} times");
        }
    }
}

#[test]
fn a_frame_with_nothing_to_upload_never_touches_the_staging() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    renderer.upload_images(device, queue, &mut ImageAtlas::default());
    assert!(renderer.uploads.staging.is_none(), "no images, no staging");
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: None, size: 16, usage: wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.write_buffer(device, queue, &mut encoder, &uniforms, 0, bytemuck::cast_slice(&[1u32; 4]));
    let writes = renderer.uploads.frame_writes.clone().unwrap();
    assert!(writes.busy(), "recorded, its chunk still mapped");
    renderer.before_submit(&mut encoder);
    assert!(!writes.busy(), "sealed into the frame");
    let flushes = writes.stats().flushes;
    renderer.before_submit(&mut encoder);
    assert_eq!(writes.stats().flushes, flushes);
    queue.submit([encoder.finish()]);
}
