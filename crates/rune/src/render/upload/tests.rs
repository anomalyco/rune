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

#[cfg(feature = "test-gpu-counters")]
fn live_buffers(device: &wgpu::Device) -> isize {
    device.get_internal_counters().hal.buffers.read()
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE.0 + x) * 4) as usize;
    pixels[at..at + 4].try_into().unwrap()
}

#[cfg(feature = "test-gpu-counters")]
#[test]
fn a_steady_animating_frame_creates_no_buffers_and_skips_unchanged_globals() {
    let Some(gpu) = Gpu::test_headless() else { return };
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
        // Written in place where the backend can (`rune_staging::set_direct`), else staged.
        let staged = u64::from(now.direct == stats.direct);
        assert_eq!((now.flushes - stats.flushes, now.submits - stats.submits), (staged, 0), "recorded into the frame");
    }
    pixels.extend(target.read(device));
    assert_eq!(pixel(&pixels, 27, 24), RED, "the square reached its last position");
    assert_eq!(pixel(&pixels, 2, 24), CLEAR, "and left its first");
}

/// Instances that moved far apart are written alone, ones close together as one span, and every
/// write lands: the frame draws each square where it is now.
#[test]
fn instances_are_written_in_the_spans_that_changed() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    let hidden = square(-100.0, -100.0);
    // Two short of a power of two, so the instances added below fit the buffer.
    let (far, last) = (2 * BLOCK + 10, 4 * BLOCK - 3);
    let mut instances = vec![hidden; last + 1];
    let mut frame = |instances: &[Instance]| {
        let stats = renderer.uploads.stats();
        renderer.prepare(device, queue, &mut text.atlas, instances, SIZE);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        target.copy(&mut encoder);
        renderer.submit(queue, encoder);
        let now = renderer.uploads.stats();
        ((now.writes - stats.writes, (now.bytes - stats.bytes) / size_of::<Instance>() as u64), target.read(device))
    };
    frame(&instances);
    (instances[3], instances[far], instances[last]) = (square(0.0, 0.0), square(20.0, 0.0), square(40.0, 0.0));
    let (written, pixels) = frame(&instances);
    assert_eq!(written, (3, 3), "three far apart instances, written alone");
    for x in [4, 24, 44] { assert_eq!(pixel(&pixels, x, 4), RED, "the square at {x} landed"); }
    (instances[far], instances[far + 10], instances[far + 25]) = (hidden, square(20.0, 20.0), square(40.0, 20.0));
    let (written, pixels) = frame(&instances);
    assert_eq!(written, (1, 26), "close together, one span from the first to the last");
    assert_eq!(pixel(&pixels, 24, 4), CLEAR, "the square that left");
    for x in [24, 44] { assert_eq!(pixel(&pixels, x, 24), RED, "the square at {x}, 24 landed"); }
    instances.extend([square(56.0, 0.0), hidden]);
    let (written, pixels) = frame(&instances);
    assert_eq!(written, (1, 2), "added instances");
    assert_eq!(pixel(&pixels, 60, 4), RED);
    instances.iter_mut().for_each(|i| i.rect[1] += 1.0);
    assert_eq!(frame(&instances).0, (1, last as u64 + 3), "everything moved: one span");
}

/// The upload has no encoder to go into when the first draw is into a caller's pass, and a
/// caller may submit without `Renderer::submit`.
#[test]
fn uploads_land_before_a_draw_into_a_callers_pass() {
    let Some(gpu) = Gpu::test_headless() else { return };
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
    let Some(gpu) = Gpu::test_headless() else { return };
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
#[cfg(feature = "test-gpu-counters")]
#[test]
fn compositor_buffer_writes_land_in_order_and_reuse_staging() {
    let Some(gpu) = Gpu::test_headless() else { return };
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
        let allocs = crate::tooling::test_alloc::allocs();
        renderer.write_buffer(device, queue, &mut encoder, &uniforms, 0, bytemuck::cast_slice(&[frame; 4]));
        let first = crate::tooling::test_alloc::allocs() - allocs;
        encoder.copy_buffer_to_buffer(&uniforms, 0, &readback, 0, 16);
        let allocs = crate::tooling::test_alloc::allocs();
        renderer.write_buffer(device, queue, &mut encoder, &uniforms, 0, bytemuck::cast_slice(&[frame + 100; 4]));
        let second = crate::tooling::test_alloc::allocs() - allocs;
        encoder.copy_buffer_to_buffer(&uniforms, 0, &readback, 16, 16);
        let created = live_buffers(device) - buffers;
        renderer.submit(queue, encoder);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let words: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
        readback.unmap();
        assert_eq!(words, [[frame; 4], [frame + 100; 4]].concat(), "frame {frame}: each write before the copy after it");
        let stats = renderer.uploads.frame_write_stats();
        let staged = u64::from(stats.direct == 0);
        assert_eq!(stats.chunks, staged, "frame {frame}: the writes share one chunk, reused once the GPU is done");
        if frame >= 2 {
            assert_eq!(created, 0, "frame {frame} created buffers");
            // A queue write allocates 6 or 7 times; the first command in a new encoder grows its list once.
            assert!(first <= 1 && second == 0, "frame {frame}: the writes allocated {first} and {second} times");
        }
    }
}

#[test]
fn a_frame_with_nothing_to_upload_never_touches_the_staging() {
    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
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
    assert!(writes.busy() || writes.stats().direct == 1, "recorded, its chunk still mapped, or written in place");
    renderer.before_submit(&mut encoder);
    assert!(!writes.busy(), "sealed into the frame");
    let flushes = writes.stats().flushes;
    renderer.before_submit(&mut encoder);
    assert_eq!(writes.stats().flushes, flushes);
    queue.submit([encoder.finish()]);
}

/// A compositor asking for `IMMEDIATES` gets the adapter's immediate size and can colour a
/// draw with `set_immediates`, no buffer written.
#[test]
fn a_device_asked_for_immediates_draws_with_them() {
    let Some(gpu) = Gpu::test_with_features(wgpu::Features::IMMEDIATES) else { return };
    let device = &gpu.device;
    if !device.features().contains(wgpu::Features::IMMEDIATES) { return; }
    assert!(device.limits().max_immediate_size >= 16, "the limit came with the feature");
    let Some(plain) = Gpu::test_headless() else { return };
    if crate::gpu::backend::native(&plain.device) {
        assert!(plain.device.features().contains(wgpu::Features::IMMEDIATES), "free on the native backend, so always there");
    } else {
        assert_eq!(plain.device.limits().max_immediate_size, 0, "only when asked");
    }
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl("
            // A struct: naga's HLSL output has no immediates of other types (wgpu#5683).
            struct Fill { colour: vec4<f32> }
            var<immediate> fill: Fill;
            @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                return vec4<f32>(vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
            }
            @fragment fn fs() -> @location(0) vec4<f32> { return fill.colour; }
        ".into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[], immediate_size: 16 });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&layout),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState {
            module: &shader, entry_point: Some("fs"), compilation_options: Default::default(),
            targets: &[Some(OFFSCREEN_FORMAT.into())],
        }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_immediates(0, bytemuck::cast_slice(&[1.0f32, 0.0, 0.0, 1.0]));
        pass.draw(0..3, 0..1);
    }
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    assert_eq!(pixel(&target.read(device), 10, 10), RED);
}

#[test]
fn same_finds_one_changed_bit_in_any_word() {
    let a = scene(0);
    assert!(same(&a, &a.clone()));
    for word in 0..size_of::<Instance>() / 4 {
        let mut b = a.clone();
        let words: &mut [u32] = bytemuck::cast_slice_mut(&mut b[a.len() - 1..]);
        words[word] ^= 1 << 31;
        assert!(!same(&a, &b), "missed a change in word {word}");
    }
}

/// Lists at any 4-byte offset compare, not only 8-aligned ones: a stack array or a Vec may land
/// either way.
#[test]
fn same_takes_lists_at_any_word_offset() {
    let n = size_of::<Instance>() / 4;
    let words: Vec<u32> = (0..3 * n as u32 + 1).collect();
    let mut other = words.clone();
    for skip in [0, 1] {
        let a: &[Instance] = bytemuck::cast_slice(&words[skip..skip + 3 * n]);
        assert!(same(a, a), "offset {skip}");
        other[skip + 2 * n] ^= 1;
        assert!(!same(a, bytemuck::cast_slice(&other[skip..skip + 3 * n])), "offset {skip}");
        other[skip + 2 * n] ^= 1;
    }
}

/// Plain rects between images and glyphs keep the batch they're in; only image and glyph pages
/// start one.
#[test]
fn batches_break_only_where_the_bound_page_changes() {
    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let with = |kind: f32, page: f32| Instance { params: [page, kind, 0.0, 0.0], ..square(0.0, 0.0) };
    let glyph = |page: f32| Instance { params: [0.0, KIND_GLYPH, page, 0.0], ..square(0.0, 0.0) };
    let instances = [
        square(0.0, 0.0), with(KIND_IMAGE, 1.0), square(0.0, 0.0), with(KIND_SHADOW, 0.0),
        glyph(2.0), with(KIND_GRADIENT, 0.0), glyph(0.0), square(0.0, 0.0), with(KIND_IMAGE, 1.0),
    ];
    renderer.prepare(device, queue, &mut text.atlas, &instances, SIZE);
    let batches: Vec<_> = renderer.image_batches.iter().map(|(range, bind)| (range.clone(), match bind {
        BatchBind::Image(page) => ('i', *page),
        BatchBind::Glyph(page) => ('g', *page),
    })).collect();
    assert_eq!(batches, [(0..1, ('i', 0)), (1..4, ('i', 1)), (4..6, ('g', 1)), (6..8, ('i', 0)), (8..9, ('i', 1))]);
}

/// Frames whose instances only move keep their batches, and their effects see where they moved;
/// a changed `params`, a longer list or a lost staging batches again.
#[test]
fn only_a_changed_params_or_length_batches_again() {
    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let frame = |x: f32, page: f32, extra: usize| {
        let mut list = vec![square(x, 0.0), Instance { params: [page, KIND_IMAGE, 0.0, 0.0], ..square(x, 8.0) },
            Instance { params: [0.0, KIND_EFFECT, 0.0, 0.0], ..square(x, 16.0) }];
        list.extend((0..extra).map(|i| square(i as f32, 24.0)));
        list
    };
    let mut prepare = |renderer: &mut Renderer, list: &[Instance]| {
        let runs = renderer.uploads.batch_runs;
        renderer.prepare(device, queue, &mut text.atlas, list, SIZE);
        let pages: Vec<_> = renderer.image_batches.iter().map(|(r, b)| (r.clone(), matches!(b, BatchBind::Image(1)))).collect();
        (renderer.uploads.batch_runs - runs, pages, renderer.effects.iter().map(|(at, i)| (*at, i.rect[0])).collect::<Vec<_>>())
    };
    assert_eq!(prepare(&mut renderer, &frame(0.0, 1.0, 0)), (1, vec![(0..1, false), (1..3, true)], vec![(2, 0.0)]));
    assert_eq!(prepare(&mut renderer, &frame(5.0, 1.0, 0)), (0, vec![(0..1, false), (1..3, true)], vec![(2, 5.0)]), "moved only");
    assert_eq!(prepare(&mut renderer, &frame(5.0, 0.0, 0)), (1, vec![(0..3, false)], vec![(2, 5.0)]), "image page changed");
    assert_eq!(prepare(&mut renderer, &frame(5.0, 0.0, 2)).0, 1, "longer");
    renderer.uploads.forget_instances();
    assert_eq!(prepare(&mut renderer, &frame(5.0, 0.0, 2)).0, 1, "forgotten");
    assert_eq!(prepare(&mut renderer, &[]), (1, vec![], vec![]), "emptied");
}

/// Idle rest (`give_back(false)`) drops GPU textures (and layer image bind groups) for image
/// pages `ImageAtlas::rest` emptied while keeping visible pages resident; hidden/covered rest
/// (`give_back(true)`) drops every image GPU texture, and the first drawn frame on wake
/// recreates and uploads them with pixel-exact output.
#[test]
fn give_back_drops_freed_image_pages_at_rest_and_all_pages_when_hidden_and_wakes_cleanly() {
    use std::sync::Arc;
    use crate::el::{El, el};
    use crate::image::ImageCache;
    use crate::pixels::Pixels;
    use crate::runtime::{Cx, Env, Runtime};

    struct State {
        small: Pixels,
        large: Pixels,
        show_small: bool,
        show_large: bool,
    }
    // Wrap in an opacity layer so layer targets also hold image bind groups referencing the
    // atlas textures: pruning/clearing must drop those bind groups too so HAL textures free.
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut inner = el().size(16.0, 16.0);
        if s.show_small {
            inner = inner.child(el().size(8.0, 8.0).pixels(&s.small));
        }
        if s.show_large {
            inner = inner.child(el().size(8.0, 8.0).pixels(&s.large));
        }
        el().size(16.0, 16.0).opacity(0.95).child(inner)
    }

    let cache = ImageCache::new(Arc::new(|| {}));
    let small_img = cache.rgba(4, 4, [240, 30, 80, 255].repeat(16));
    small_img.wait().unwrap();
    let large = Pixels::new();
    large.set(1025, 64, [20, 210, 90, 255].repeat(1025 * 64));

    let mut rt = Runtime::new(
        State { small: small_img.pixels().clone(), large, show_small: true, show_large: true },
        view,
        (16.0, 16.0),
        Env::new(true, 1.0),
    );
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 16, 16);

    #[cfg(feature = "test-gpu-counters")]
    let (base_tex, base_mem) = {
        let c = gpu.device.get_internal_counters().hal;
        (c.textures.read(), c.texture_memory.read())
    };

    let draw_read = |rt: &mut Runtime<State>, renderer: &mut Renderer| -> Vec<u8> {
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (16, 16));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        target.read(&gpu.device)
    };

    // Frame 1: small image on shelf page 0 (1024x128 = 524,288 B) + large standalone page 1 (1025x64 = 262,400 B).
    let page0_bytes = 1024u64 * 128 * 4;
    let page1_bytes = 1025u64 * 64 * 4;
    rt.frame(0.0);
    let _ = draw_read(&mut rt, &mut renderer);
    assert_eq!(rt.images.pages.len(), 2);
    assert_eq!(renderer.image_texture_count(), 2);
    assert_eq!(renderer.image_texture_bytes(), page0_bytes + page1_bytes);

    // Frame 2: remove small image while keeping large image; shelf page 0 stays warm until rest.
    rt.state.show_small = false;
    rt.invalidate();
    rt.frame(0.0);
    let before_hide = draw_read(&mut rt, &mut renderer);
    assert_eq!(renderer.image_texture_count(), 2);
    assert_eq!(renderer.image_texture_bytes(), page0_bytes + page1_bytes);
    #[cfg(feature = "test-gpu-counters")]
    let (tex_before_rest, mem_before_rest) = {
        let c = gpu.device.get_internal_counters().hal;
        (c.textures.read(), c.texture_memory.read())
    };

    // Idle rest (Host::give_back(false)): interior page 0 is emptied by rest() and pruned from GPU;
    // visible page 1 stays resident.
    rt.images.rest();
    assert_eq!(rt.images.pages.len(), 2);
    assert!(rt.images.pages[0].data.is_empty());
    assert!(!rt.images.pages[1].data.is_empty());
    renderer.give_back(false, &mut rt.text.atlas.resident_caches, &rt.images);
    assert!(renderer.has_images(), "visible image page stays resident at idle rest");
    assert_eq!(renderer.image_texture_count(), 1);
    assert_eq!(renderer.image_texture_bytes(), page1_bytes);
    #[cfg(feature = "test-gpu-counters")]
    {
        let c = gpu.device.get_internal_counters().hal;
        assert_eq!(c.textures.read(), tex_before_rest - 1, "idle rest frees emptied page 0 HAL texture");
        if mem_before_rest > base_mem {
            assert!(c.texture_memory.read() < mem_before_rest, "idle rest reclaims page 0 HAL texture memory");
        }
    }

    // Hidden / covered (Host::give_back(true)): every image texture (and scratch layer target) drops.
    rt.images.rest();
    renderer.give_back(true, &mut rt.text.atlas.resident_caches, &rt.images);
    gpu.queue.submit(std::iter::empty());
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    renderer.trim_uploads();
    assert!(!renderer.has_images(), "give_back(true) drops all GPU image textures");
    assert_eq!(renderer.image_texture_count(), 0);
    assert_eq!(renderer.image_texture_bytes(), 0);
    #[cfg(feature = "test-gpu-counters")]
    {
        let c = gpu.device.get_internal_counters().hal;
        assert_eq!(c.textures.read(), base_tex, "give_back(true) restores baseline HAL texture count");
        assert_eq!(c.texture_memory.read(), base_mem, "give_back(true) restores baseline HAL texture memory");
    }

    // Wake from give_back(true): first frame draws identically with no rt.frame or manual invalidate.
    let after_wake = draw_read(&mut rt, &mut renderer);
    assert_eq!(after_wake, before_hide, "waking from give_back(true) draws identically on first frame");
    assert_eq!(renderer.image_texture_count(), 1);
    assert_eq!(renderer.image_texture_bytes(), page1_bytes);
    assert!(small_img.is_ready(), "content-addressed ImageCache entry remains untouched");
}

/// Idle rest (`give_back(false)`) truncates `Renderer::images` and frees the GPU texture when
/// `ImageAtlas::rest` truncates trailing empty pages (such as a warm first shelf page).
#[test]
fn give_back_truncates_trailing_empty_image_pages_at_rest() {
    use crate::el::{El, el};
    use crate::pixels::Pixels;
    use crate::runtime::{Cx, Env, Runtime};

    struct State {
        px: Pixels,
        show: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = el().size(8.0, 8.0);
        if s.show {
            root = root.child(el().size(4.0, 4.0).pixels(&s.px));
        }
        root
    }

    let px = Pixels::new();
    px.set(4, 4, [180, 90, 220, 255].repeat(16));
    let mut rt = Runtime::new(State { px, show: true }, view, (8.0, 8.0), Env::new(true, 1.0));
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 8, 8);

    #[cfg(feature = "test-gpu-counters")]
    let (base_tex, base_mem) = {
        let c = gpu.device.get_internal_counters().hal;
        (c.textures.read(), c.texture_memory.read())
    };

    let draw_once = |rt: &mut Runtime<State>, renderer: &mut Renderer| {
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (8, 8));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0; 4]);
        renderer.submit(&gpu.queue, enc);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    };

    // Frame 1: shelf page 0 uploaded (1024x128 = 524,288 B).
    let page0_bytes = 1024u64 * 128 * 4;
    rt.frame(0.0);
    draw_once(&mut rt, &mut renderer);
    assert_eq!(renderer.image_texture_count(), 1);
    assert_eq!(renderer.image_texture_bytes(), page0_bytes);
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(gpu.device.get_internal_counters().hal.textures.read(), base_tex + 1);

    // Frame 2: image removed and settled, first shelf page stays warm until rest.
    rt.state.show = false;
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.0);
    draw_once(&mut rt, &mut renderer);
    assert_eq!(rt.images.pages.len(), 1);
    assert_eq!(renderer.image_texture_count(), 1);

    // Idle rest: rest() truncates rt.images.pages to 0, give_back(false) truncates and drops GPU page.
    rt.images.rest();
    assert!(rt.images.pages.is_empty());
    renderer.give_back(false, &mut rt.text.atlas.resident_caches, &rt.images);
    assert!(!renderer.has_images(), "give_back(false) drops truncated trailing page GPU texture");
    assert_eq!(renderer.image_texture_count(), 0);
    assert_eq!(renderer.image_texture_bytes(), 0);
    #[cfg(feature = "test-gpu-counters")]
    {
        let c = gpu.device.get_internal_counters().hal;
        assert_eq!(c.textures.read(), base_tex, "truncated page HAL texture freed at idle rest");
        assert_eq!(c.texture_memory.read(), base_mem, "truncated page HAL texture memory reclaimed at idle rest");
    }
}

/// A steady scene showing more than `IMAGE_ATLAS_MAX_PAGES` (16) pages on every frame must keep
/// all overflow pages resident with zero CPU re-copies and zero GPU texture uploads per frame,
/// and still compact back under the bound once the visible set shrinks.
#[test]
fn steady_overflow_image_atlas_uploads_zero_pages_per_frame() {
    use crate::pixels::{IMAGE_ATLAS_MAX_PAGES, ImageAtlas, Pixels};

    let Some(gpu) = Gpu::test_headless() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, 8, 8);

    let count = IMAGE_ATLAS_MAX_PAGES + 4;
    let (w, h) = (900u32, 900u32);
    let imgs: Vec<Pixels> = (0..count)
        .map(|i| {
            let p = Pixels::new();
            p.set(w, h, [10 + i as u8 * 10, 120, 200, 255].repeat((w * h) as usize));
            p
        })
        .collect();

    let mut images = ImageAtlas::default();
    // Frame 0: initial placement and GPU upload of all 20 pages.
    let mut prev_placed: Vec<_> = imgs.iter().map(|p| images.place(p, [w as f32, h as f32]).unwrap()).collect();
    images.end_frame();
    renderer.upload_images(device, queue, &mut images);
    assert_eq!(images.pages.len(), count);
    assert_eq!(renderer.image_texture_count(), count);

    #[cfg(feature = "test-gpu-counters")]
    let steady_hal_textures = device.get_internal_counters().hal.textures.read();

    // Frames 1..=5: steady scene past the 16-page / 64 MiB bound.
    for frame in 1..=5usize {
        let mut slots_moved = 0usize;
        for (i, img) in imgs.iter().enumerate() {
            let placed = images.place(img, [w as f32, h as f32]).unwrap();
            if placed != prev_placed[i] {
                slots_moved += 1;
                prev_placed[i] = placed;
            }
        }
        images.end_frame();
        let dirty_pages = images.pages.iter().filter(|p| p.dirty.is_some()).count();
        let stats_before = renderer.uploads.stats();
        renderer.upload_images(device, queue, &mut images);
        let stats_after = renderer.uploads.stats();
        let upload_writes = stats_after.writes - stats_before.writes;
        let upload_bytes = stats_after.bytes - stats_before.bytes;
        assert_eq!(slots_moved, 0, "frame {frame}: steady overflow must move 0 slots");
        assert_eq!(dirty_pages, 0, "frame {frame}: steady overflow must dirty 0 pages");
        assert_eq!(upload_writes, 0, "frame {frame}: steady overflow must perform 0 GPU texture writes");
        assert_eq!(upload_bytes, 0, "frame {frame}: steady overflow must upload 0 bytes to GPU");
        #[cfg(feature = "test-gpu-counters")]
        assert_eq!(
            device.get_internal_counters().hal.textures.read(),
            steady_hal_textures,
            "frame {frame}: steady overflow must not churn HAL textures",
        );
    }

    // Shrink to 10 visible images (6 from 0..16 + the 4 overflow images 16..20): overflow images
    // relocate onto freed pages < 16 and GPU texture count drops from 20 to 10.
    let kept: Vec<usize> = (0..6).chain(IMAGE_ATLAS_MAX_PAGES..count).collect();
    for _ in 0..2 {
        for &i in &kept {
            images.place(&imgs[i], [w as f32, h as f32]).unwrap();
        }
        images.end_frame();
        renderer.upload_images(device, queue, &mut images);
    }
    assert!(images.pages.len() <= IMAGE_ATLAS_MAX_PAGES);
    assert_eq!(renderer.image_texture_count(), kept.len());

    // Draw one of the relocated overflow images (imgs[16]) and verify exact pixels on GPU.
    let relocated = images.place(&imgs[IMAGE_ATLAS_MAX_PAGES], [w as f32, h as f32]).unwrap();
    assert!(relocated.page < IMAGE_ATLAS_MAX_PAGES);
    images.end_frame();
    renderer.upload_images(device, queue, &mut images);
    let inst = Instance {
        rect: [0.0, 0.0, 8.0, 8.0],
        radii: [0.0; 4],
        fill: [0.0, 0.0, 0.0, 1.0],
        border: [0.0; 4],
        params: [relocated.page as f32, KIND_IMAGE, 0.0, 1.0],
        uv: relocated.rect.map(|v| v as f32),
        clip: NO_CLIP,
        fade: [0; 4],
    };
    renderer.prepare(device, queue, &mut text.atlas, std::slice::from_ref(&inst), (8, 8));
    let mut enc = device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0; 4]);
    target.copy(&mut enc);
    renderer.submit(queue, enc);
    let out = target.read(device);
    let want = [10 + IMAGE_ATLAS_MAX_PAGES as u8 * 10, 120, 200, 255];
    for (got, want) in out[..4].iter().zip(want) {
        assert!(got.abs_diff(want) <= 1, "relocated pixel {got} != {want}");
    }
}
