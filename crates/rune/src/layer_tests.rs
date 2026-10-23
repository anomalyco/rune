use super::*;
use crate::{Color, Cx, El, Env, Live, Runtime, el};

fn rect(bounds: [f32; 4], color: [f32; 4]) -> Instance {
    Instance { rect: bounds, fill: color, clip: NO_CLIP, params: [0.0, KIND_RECT, 0.0, 1.0], ..Instance::zeroed() }
}

fn group(bounds: [f32; 4], opacity: f32, color: [f32; 4]) -> Vec<Instance> {
    vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity }),
        rect(bounds, color), rect([bounds[0] + bounds[2] * 0.25, bounds[1], bounds[2] * 0.5, bounds[3]], color),
        Instance::end_layer()]
}

fn pixels(gpu: &Gpu, renderer: &Renderer, target: &Offscreen) -> Vec<u8> {
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0; 4]);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    target.read(&gpu.device)
}

fn rgba(linear: [f32; 4]) -> [u8; 4] {
    std::array::from_fn(|i| {
        let v = linear[i];
        let v = if i == 3 { v } else if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

fn near(got: &[u8], want: [u8; 4]) {
    assert!(got.iter().zip(want).all(|(&a,b)| a.abs_diff(b) <= 1), "{got:?} != {want:?}");
}

#[test]
fn isolated_layers_match_scalar_overlap_crossfade_and_nesting() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let bounds = [4.0, 4.0, 24.0, 8.0];
    let mut instances = group(bounds, 0.5, [1.0; 4]);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    let out = pixels(&gpu, &renderer, &target);
    for x in [6, 16, 26] { near(&out[(8 * 32 + x) * 4..(8 * 32 + x + 1) * 4], rgba([0.5; 4])); }
    assert_eq!(renderer.layer_stats().targets, 1);
    assert!(renderer.layer_stats().bytes < 32 * 16 * 8, "bounded allocation");
    let allocations = renderer.layer_stats().allocations;

    // Two opaque states, each with internal overlap. Source-over crossfade is B(1-t) over
    // A(t): alpha = 1-t+t², NOT 1. Internal child overlap must never alter that oracle.
    for t in [0.25, 0.5, 0.75] {
        instances = group(bounds, t, [1.0, 0.0, 0.0, 1.0]);
        instances.extend(group(bounds, 1.0 - t, [0.0, 0.0, 1.0, 1.0]));
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
        let out = pixels(&gpu, &renderer, &target);
        let want = rgba([t * t, 0.0, 1.0 - t, 1.0 - t + t * t]);
        for x in [6, 16, 26] { near(&out[(8 * 32 + x) * 4..(8 * 32 + x + 1) * 4], want); }
    }
    assert_eq!(renderer.layer_stats().allocations, allocations + 1, "reuse targets while opacity changes");

    instances = vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5 })];
    instances.extend(group(bounds, 0.5, [1.0; 4]));
    instances.push(Instance::end_layer());
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(8 * 32 + 16) * 4..(8 * 32 + 17) * 4], rgba([0.25; 4]));

    instances = group(bounds, 0.5, [1.0; 4]);
    for i in &mut instances[..3] { i.clip = [8.0, 5.0, 24.0, 11.0]; }
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    assert_eq!(renderer.layer_stats().bytes, 16 * 6 * 8, "clip bounds the linear RGBA target");
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(8 * 32 + 6) * 4..(8 * 32 + 7) * 4], [0; 4]);
    near(&out[(8 * 32 + 16) * 4..(8 * 32 + 17) * 4], rgba([0.5; 4]));

    for alpha in [1.0, 0.0] {
        instances = group(bounds, alpha, [1.0; 4]);
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
        let out = pixels(&gpu, &renderer, &target);
        near(&out[(8 * 32 + 16) * 4..(8 * 32 + 17) * 4], rgba([alpha; 4]));
        assert_eq!(renderer.layer_stats().targets, 0, "release on both endpoints");
        assert_eq!(renderer.layer_stats().extra_passes, 0);
    }
    // Fully opaque streams allocate no planner storage and issue only the ordinary pass.
    let mut ordinary = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    ordinary.prepare(&gpu.device, &gpu.queue, &mut atlas, &[rect(bounds, [1.0; 4])], (32, 16));
    assert_eq!(ordinary.layer_stats(), LayerStats::default());
    assert_eq!(ordinary.layers.plan.capacity(), 0);
    assert_eq!(ordinary.layers.targets.capacity(), 0);
    assert_eq!(ordinary.layers.stack.capacity(), 0);
    assert_eq!(ordinary.layers.pipelines.capacity(), 0);
}

#[test]
fn live_group_opacity_changes_scopes_without_view_or_layout_work() {
    fn view(alpha: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(32.0, 16.0).live_opacity(alpha)
            .child(el().size(24.0, 12.0).fill(Color::WHITE))
            .child(el().size(24.0, 12.0).pin(Some(4.0), Some(0.0), None, None).fill(Color::WHITE))
    }
    let alpha = Live::new(1.0);
    let mut rt = Runtime::new(alpha.clone(), view, (32.0, 16.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    let commits = rt.stats.commits;
    let original = bytemuck::cast_slice::<Instance, u8>(&rt.instances).to_vec();
    alpha.set(0.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    assert_eq!(rt.instances.first().unwrap().params[1], KIND_LAYER);
    assert_eq!(rt.instances.last().unwrap().params[1], KIND_LAYER_END);
    assert_eq!(rt.instances[1].fill[3], 1.0);
    alpha.set(0.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.instances.is_empty());
    alpha.set(1.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    assert_eq!(bytemuck::cast_slice::<Instance, u8>(&rt.instances), original);
}

#[test]
fn isolated_exit_descendants_stay_inside_their_fading_parent() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        let mut fading = el().key("fading").size(24.0, 16.0).opacity(0.5)
            .child(el().key("kept").size(16.0, 16.0).fill(Color::WHITE));
        if *show { fading = fading.child(el().key("leaving").size(16.0, 16.0).fill(Color::WHITE)); }
        el().size(32.0, 16.0).child(fading)
            .child(el().key("sibling").size(8.0, 16.0).pin(Some(24.0), Some(0.0), None, None).fill(Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }))
    }
    let mut rt = Runtime::new(true, view, (32.0, 16.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    rt.state = false; rt.invalidate(); rt.frame(0.0);
    let start = rt.instances.iter().position(|i| i.params[1] == KIND_LAYER).unwrap();
    let end = rt.instances.iter().position(|i| i.params[1] == KIND_LAYER_END).unwrap();
    assert_eq!(end - start, 3, "both retained descendants share the scope");
    assert_eq!(rt.instances[end + 1].fill, [0.0, 0.0, 1.0, 1.0], "sibling paints outside the group");
}

#[test]
fn single_leaf_groups_keep_the_direct_path_and_springs_stay_live() {
    fn leaf(alpha: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(32.0, 16.0).live_opacity(alpha).child(el().size(16.0, 16.0).fill(Color::WHITE))
    }
    fn group_view(alpha: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(32.0, 16.0).live_opacity(alpha.smooth(crate::Spring::DEFAULT))
            .child(el().size(16.0, 16.0).fill(Color::WHITE))
            .child(el().size(16.0, 16.0).pin(Some(8.0), Some(0.0), None, None).fill(Color::WHITE))
    }
    let alpha = Live::new(0.5);
    let env = Env { scale: 1.0, system_dark: true };
    let mut rt = Runtime::new(alpha.clone(), leaf, (32.0, 16.0), env);
    rt.frame(0.0);
    assert_eq!(rt.instances.len(), 1);
    assert_eq!(rt.instances[0].fill[3], 0.5);
    alpha.set(1.0);
    let mut rt = Runtime::new(alpha.clone(), group_view, (32.0, 16.0), env);
    rt.frame(0.0);
    let commits = rt.stats.commits;
    alpha.set(0.5);
    rt.frame(1.0 / 60.0);
    let a = rt.instances[0].fill[3];
    rt.frame(1.0 / 60.0);
    let b = rt.instances[0].fill[3];
    assert_eq!(rt.instances[0].params[1], KIND_LAYER);
    assert!(b < a && b > 0.5, "spring samples change the group each presentation: {a} -> {b}");
    assert_eq!(rt.stats.commits, commits);
}

#[test]
fn opacity_motion_samples_group_alpha_until_the_terminal_frame() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        el().size(32.0, 16.0).opacity(if *show { 1.0 } else { 0.0 })
            .opacity_motion(rune_motion::Curve::ease_in_out(0.2))
            .child(el().size(16.0, 16.0).fill(Color::WHITE))
            .child(el().size(16.0, 16.0).pin(Some(8.0), Some(0.0), None, None).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(true, view, (32.0, 16.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    rt.state = false; rt.invalidate(); rt.frame(0.0);
    let commits = rt.stats.commits;
    rt.frame(0.05);
    let first = rt.instances[0].fill[3];
    assert_eq!(rt.instances[0].params[1], KIND_LAYER);
    rt.frame(0.05);
    assert!(rt.instances[0].fill[3] < first);
    assert_eq!(rt.instances[1].fill[3], 1.0);
    for _ in 0..24 { rt.frame(1.0 / 60.0); }
    assert!(rt.instances.is_empty());
    assert_eq!(rt.stats.commits, commits);
}

#[test]
fn isolated_layers_preserve_image_video_spring_and_list_order() {
    use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceDraw, SurfaceFit};
    use crate::gpu_springs::{Node, Track};
    use crate::pixels::Pixels;
    let gpu = Gpu::headless();
    let bounds = [4.0, 4.0, 24.0, 8.0];
    let mut content = group(bounds, 0.5, [1.0, 0.0, 0.0, 1.0]);
    // A glyph, a vector and a page-1 image inside the nonzero-origin layer. Absolute spring
    // indices differ from local draw indices, so a shifted subrange would use the wrong node.
    let mut atlas = crate::text::TextSystem::default().atlas;
    atlas.data[0] = 255; atlas.dirty = Some((0, 1));
    content.insert(3, Instance { rect: [9.0, 6.0, 1.0, 1.0], fill: [1.0; 4], clip: NO_CLIP,
        params: [0.0, KIND_GLYPH, 0.0, 1.0], ..Instance::zeroed() });
    let px = Pixels::new(); px.set(1, 1, vec![0, 255, 0, 255]);
    let mut images = crate::pixels::ImageAtlas::default();
    let full_page = Pixels::new(); full_page.set(1024, 1024, vec![255; 1024 * 1024 * 4]);
    images.place(&full_page, [1024.0; 2]).unwrap();
    let image = images.place(&px, [2.0, 2.0]).unwrap();
    assert_eq!(image.page, 1);
    content.insert(4, Instance { rect: [15.0, 6.0, 2.0, 2.0], fill: [0.0, 0.0, 0.0, 1.0],
        params: [image.page as f32, KIND_IMAGE, 0.0, 1.0], uv: image.rect.map(|v| v as f32), clip: NO_CLIP, ..Instance::zeroed() });
    let prefix = [rect([0.0, 0.0, 2.0, 2.0], [1.0; 4])];
    let zero = Track::constant(0.0);
    let nodes = [Node { x: zero, y: zero, opacity: Track::constant(1.0) },
        Node { x: Track::constant(1.0), y: zero, opacity: Track::constant(1.0) }];
    let mut ids = vec![0; prefix.len() + content.len()];
    ids[4] = 1; // glyph at absolute index 4, translated from x=9 to x=10
    let mut renderer = Renderer::with_springs(&gpu.device, OFFSCREEN_FORMAT, &nodes, &ids);
    let ranges = renderer.prepare_lists(&gpu.device, &gpu.queue, &mut atlas, &[&prefix, &content], (32, 16));
    renderer.upload_images(&gpu.device, &gpu.queue, &mut images);
    let surface = NativeSurface::new();
    surface.push_bytes(ByteFrame { width: 2, height: 2, format: Format::Bgra, color: ColorInfo::default(),
        planes: [vec![255, 0, 0, 255].repeat(4), vec![]] }, 1).unwrap();
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &[SurfaceDraw { surface, fit: SurfaceFit::Fill,
        before_instance: 4, rect: [22.0, 6.0, 2.0, 2.0], radii: [0.0; 4], clip: NO_CLIP, fade: [0.0; 4], opacity: 1.0 }], (32, 16));
    let format = wgpu::TextureFormat::Rgba8Unorm;
    renderer.add_format(&gpu.device, format);
    let target = Offscreen::with_format(&gpu.device, 32, 16, format);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.encode_layers(&mut enc);
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None, color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        for range in ranges { renderer.draw_range(&mut pass, format, range); }
    }
    target.copy(&mut enc); renderer.submit(&gpu.queue, enc);
    let out = target.read(&gpu.device);
    for (x, y, want) in [(0,0,[255;4]), (9,6,[128,0,0,128]), (10,6,[128;4]), (15,6,[0,128,0,128]), (22,6,[0,0,128,128])] {
        near(&out[(y*32+x)*4..(y*32+x+1)*4], want);
    }
}

#[test]
#[ignore = "manual GPU timing and two headless review stills"]
fn layer_gpu_timing_and_stills() {
    let gpu = Gpu::headless();
    assert!(gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY));
    eprintln!("adapter: {:?}", gpu.adapter.get_info());
    let mut atlas = crate::text::TextSystem::default().atlas;
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 1600, 1000);
    for (name, bounds) in [("400x120pt@2x", [400.0, 380.0, 800.0, 240.0]), ("full-window", [0.0, 0.0, 1600.0, 1000.0])] {
        for isolated in [false, true] {
            let content = group(bounds, 0.5, [1.0; 4]);
            let content = if isolated { &content[..] } else { &content[1..3] };
            renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, content, (1600, 1000));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            for _ in 0..8 { renderer.draw(&mut enc, &target.view, [0.0; 4]); }
            gpu.queue.submit([enc.finish()]);
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            const N: u32 = 60;
            let queries = gpu.device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: N * 2 });
            let resolve = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(N) * 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
            let read = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(N) * 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            // One completed presentation per sample. Batching many frames into one command
            // buffer lets Metal overlap their vertex/fragment stages and inflates pass spans.
            for i in 0..N {
                let mut enc = gpu.device.create_command_encoder(&Default::default());
                renderer.draw_timed(&mut enc, &target.view, [0.0; 4], Some(wgpu::RenderPassTimestampWrites {
                    query_set: &queries, beginning_of_pass_write_index: Some(i * 2), end_of_pass_write_index: Some(i * 2 + 1),
                }));
                gpu.queue.submit([enc.finish()]);
                gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            }
            // Resolve after completion: Metal counter end samples can race a same-buffer resolve.
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            enc.resolve_query_set(&queries, 0..N * 2, &resolve, 0);
            enc.copy_buffer_to_buffer(&resolve, 0, &read, 0, u64::from(N) * 16);
            gpu.queue.submit([enc.finish()]);
            read.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let bytes = read.slice(..).get_mapped_range().unwrap();
            let mut ms: Vec<f64> = bytes.chunks_exact(16).map(|b| {
                let a = u64::from_le_bytes(b[..8].try_into().unwrap());
                let z = u64::from_le_bytes(b[8..].try_into().unwrap());
                assert!(z >= a && z != 0, "invalid GPU counters");
                (z - a) as f64 * f64::from(gpu.queue.get_timestamp_period()) / 1e6
            }).collect();
            ms.sort_by(f64::total_cmp);
            eprintln!("{name} isolated={isolated}: n={N} median={:.6}ms p95={:.6}ms {:?}", ms[N as usize / 2], ms[(N as usize * 95 / 100) - 1], renderer.layer_stats());
            drop(bytes); read.unmap();
        }
    }
    let output = std::env::var_os("LEASE_OUT").map(std::path::PathBuf::from).unwrap_or_else(|| "artifacts/group-layers".into());
    let target = Offscreen::new(&gpu.device, 880, 320);
    for t in [0.25, 0.75] {
        let mut all = vec![rect([0.0, 0.0, 880.0, 320.0], [0.025, 0.025, 0.035, 1.0])];
        for (alpha, color, bar_width) in [(t, [0.03, 0.28, 0.65, 1.0], 360.0), (1.0 - t, [0.45, 0.08, 0.28, 1.0], 480.0)] {
            all.push(Instance::begin_layer([40.0, 40.0, 800.0, 240.0], NO_CLIP, LayerSpec { opacity: alpha }));
            all.push(Instance { radii: [120.0; 4], ..rect([40.0, 40.0, 800.0, 240.0], color) });
            all.push(Instance { radii: [48.0; 4], ..rect([100.0, 112.0, 96.0, 96.0], [1.0; 4]) });
            for y in [120.0, 158.0, 196.0] {
                all.push(Instance { radii: [9.0; 4], ..rect([240.0, y, bar_width, 18.0], [1.0; 4]) });
            }
            all.push(Instance::end_layer());
        }
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &all, (880, 320));
        let out = pixels(&gpu, &renderer, &target);
        write_png(&output.join(format!("crossfade-{}.png", (t * 100.0) as u32)), 880, 320, &out).unwrap();
    }
}

#[test]
#[ignore = "provide RUNE_BASELINE_SHADER from the base revision for the byte-identity gate"]
fn opaque_shader_is_byte_identical_to_base_revision() {
    let source = std::env::var("RUNE_BASELINE_SHADER").expect("provide the base revision's crates/rune/src/shader.wgsl");
    let gpu = Gpu::headless();
    let mut old = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    old.shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("base revision"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    old.pipeline = Renderer::pipeline(&gpu.device, &old.layout, &old.shader, OFFSCREEN_FORMAT, "vs");
    let mut new = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 96, 64);
    let mut atlas = crate::text::TextSystem::default().atlas;
    for y in 0..16 { for x in 0..16 { atlas.data[y * ATLAS_SIZE as usize + x] = ((x * 17 + y * 31) % 256) as u8; } }
    for seed in 0..24 {
        let mut all = Vec::new();
        for kind in 0..=6 {
            let mut i = rect([6.25 + kind as f32 * 7.0, 8.5 + seed as f32 * 0.25, 24.0, 28.0], [0.25, 0.1, 0.3, 0.5]);
            i.params = [2.0, kind as f32, 2.0, 1.2];
            i.radii = [4.0, 7.0, 2.0, 9.0];
            i.border = [0.1, 0.2, 0.05, 0.5];
            i.clip = [4.25, 5.75, 88.5, 58.25];
            i.fade = [3.0, 0.0, 4.0, 2.0];
            match kind {
                2 => { i.rect[2] = 16.0; i.rect[3] = 16.0; i.params[2] = 0.0; },
                3 => { i.uv = [2.0, 3.0, 0.0, 0.0]; },
                4 => { i.uv = [0.01, 0.2, 0.1, 0.3]; },
                5 => { i.radii = [i.rect[0], i.rect[1], i.rect[0] + 24.0, i.rect[1] + 28.0]; i.params[2] = 1.0; },
                6 => { i.radii = [i.rect[0], i.rect[1], i.rect[0] + 24.0, i.rect[1] + 28.0]; i.uv = [i.rect[0], i.rect[1] + 28.0, 0.0, 0.0]; i.params[0] = 7.0; },
                _ => {},
            }
            all.push(i);
        }
        atlas.dirty = Some((0, 16)); old.prepare(&gpu.device, &gpu.queue, &mut atlas, &all, (96, 64));
        let before = pixels(&gpu, &old, &target);
        atlas.dirty = Some((0, 16)); new.prepare(&gpu.device, &gpu.queue, &mut atlas, &all, (96, 64));
        assert_eq!(before, pixels(&gpu, &new, &target), "opaque baseline seed {seed}");
        assert_eq!(new.layer_stats(), LayerStats::default());
    }
    eprintln!("24 opaque GPU fixtures byte-identical to the base shader (kinds 0–6)");
}
