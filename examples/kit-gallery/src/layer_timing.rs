//! Manual, headless disabled-control timing and legacy per-instance pixel comparison.
use super::*;
use rune::render::{Gpu, Instance, Renderer, Offscreen, OFFSCREEN_FORMAT, KIND_LAYER, KIND_LAYER_END, KIND_GRADIENT};

fn disabled_controls(_: &G, _: &mut Cx<G>) -> El {
    let mut s = init();
    let k = kit(&s, true);
    let mut overlays = Vec::new();
    let mut columns = rune::row().gap(24.0);
    for story in ["ui-radio--states", "ui-text-input--states", "ui-select--disabled"] {
        s.story = story;
        columns = columns.child(col().w(300.0).child(stories::render(&s, k, &mut overlays)));
    }
    let segmented = rune_kit::SegmentedControl::new(&["List", "Grid", "Board"], Some(0))
        .segment_state(0, rune_kit::State::Disabled)
        .segment_state(1, rune_kit::State::Disabled)
        .segment_state(2, rune_kit::State::Disabled).build(k);
    rune_kit::root(col().fill(k.c.page).padding(24.0).gap(24.0).child(columns).child(segmented), overlays)
}

fn legacy(instances: &[Instance]) -> Vec<Instance> {
    let mut alpha = 1.0;
    let mut stack = Vec::new();
    let mut out = Vec::new();
    for i in instances {
        if i.params[1] == KIND_LAYER { stack.push(alpha); alpha *= i.fill[3]; }
        else if i.params[1] == KIND_LAYER_END { alpha = stack.pop().unwrap(); }
        else {
            let mut i = *i;
            i.fill = i.fill.map(|v| v * alpha);
            i.border = i.border.map(|v| v * alpha);
            if i.params[1] == KIND_GRADIENT { i.uv = i.uv.map(|v| v * alpha); }
            out.push(i);
        }
    }
    assert!(stack.is_empty());
    out
}

#[test]
#[ignore = "manual byte-for-byte disabled styling check; provide RUNE_KIT_PIXEL_BASELINE"]
fn disabled_controls_match_legacy_pixels() {
    // Record against the pre-dimming kit source (reconstructing dev's per-instance opacity),
    // then run without RECORD on the new source. Keep the directory outside HiveMind's checkout.
    let directory = std::path::PathBuf::from(std::env::var_os("RUNE_KIT_PIXEL_BASELINE").unwrap());
    let record = std::env::var_os("RUNE_KIT_RECORD_BASELINE").is_some();
    std::fs::create_dir_all(&directory).unwrap();
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    for dark in [false, true] {
        for scale in [1.0, 2.0] {
            let target = Offscreen::new(&gpu.device, (1024.0 * scale) as u32, (800.0 * scale) as u32);
            for story in ["ui-button--all-states", "ui-icon-button--all-states", "ui-radio--states",
                "ui-text-input--states", "ui-textarea--states", "ui-select--disabled",
                "ui-switch--states", "ui-menu--basic", "ui-segmented-control--basic"] {
                let Some((_, setup)) = stories::SCENES.iter().find(|(id, _)| *id == story) else { panic!("missing {story}") };
                let mut s = init(); setup(&mut s);
                let text = rune::text::TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
                let mut rt = rune::Runtime::with_text(s, view, (1024.0, 800.0), rune::Env { scale, system_dark: dark }, text);
                rt.frame(0.0); rt.settle_all(); rt.frame(0.0);
                if record && story == "ui-button--all-states" {
                    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_LAYER),
                        "record the legacy baseline with the pre-dimming kit source");
                }
                let instances = if record { legacy(&rt.instances) } else { rt.instances.clone() };
                renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &instances, ((1024.0 * scale) as u32, (800.0 * scale) as u32));
                let mut encoder = gpu.device.create_command_encoder(&Default::default());
                renderer.draw(&mut encoder, &target.view, [0.0; 4]); target.copy(&mut encoder);
                gpu.queue.submit([encoder.finish()]);
                let pixels = target.read(&gpu.device);
                let path = directory.join(format!("{story}-{dark}-{scale}.rgba"));
                if record { std::fs::write(path, pixels).unwrap(); }
                else {
                    let before = std::fs::read(path).unwrap();
                    let differences = pixels.iter().zip(&before).filter(|(a, b)| a != b).count();
                    assert_eq!(pixels.len(), before.len());
                    assert_eq!(differences, 0, "{story} dark={dark} scale={scale}");
                }
            }
        }
    }
}

#[test]
#[ignore = "manual disabled-controls GPU comparison; run through HiveMind"]
fn disabled_controls_gpu_cost() {
    let gpu = Gpu::headless();
    assert!(gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY));
    eprintln!("adapter: {:?}", gpu.adapter.get_info());
    let text = rune::text::TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
    let mut rt = rune::Runtime::with_text(init(), disabled_controls, (1024.0, 640.0),
        rune::Env { scale: 2.0, system_dark: true }, text);
    rt.frame(0.0); rt.settle_all(); rt.frame(0.0);
    assert!(rt.instances.iter().all(|i| i.params[1] != KIND_LAYER && i.params[1] != KIND_LAYER_END));
    let old = legacy(&rt.instances);
    let mut renderers = [Renderer::new(&gpu.device, OFFSCREEN_FORMAT), Renderer::new(&gpu.device, OFFSCREEN_FORMAT)];
    for (renderer, instances) in renderers.iter_mut().zip([&old, &rt.instances]) {
        rt.text.atlas.dirty = Some((0, rune::text::ATLAS_SIZE));
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, instances, (2048, 1280));
    }
    assert_eq!(renderers[1].layer_stats().targets, 0, "disabled styling must stay on the direct path");
    let target = Offscreen::new(&gpu.device, 2048, 1280);
    for _ in 0..8 {
        for renderer in &renderers {
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0; 4]);
            gpu.queue.submit([enc.finish()]);
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
    }
    const N: u32 = 60;
    let queries = gpu.device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: N * 4 });
    let resolve = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(N) * 32,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let read = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(N) * 32,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    for sample in 0..N {
        // Alternate paired order to reduce drift; each sample is one completed frame.
        for offset in 0..2 {
            let mode = (sample + offset) % 2;
            let query = sample * 4 + mode * 2;
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderers[mode as usize].draw_timed(&mut enc, &target.view, [0.0; 4], Some(wgpu::RenderPassTimestampWrites {
                query_set: &queries, beginning_of_pass_write_index: Some(query), end_of_pass_write_index: Some(query + 1),
            }));
            gpu.queue.submit([enc.finish()]);
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
    }
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    enc.resolve_query_set(&queries, 0..N * 4, &resolve, 0);
    enc.copy_buffer_to_buffer(&resolve, 0, &read, 0, u64::from(N) * 32);
    gpu.queue.submit([enc.finish()]);
    read.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = read.slice(..).get_mapped_range().unwrap();
    for (mode, name) in ["flat-reference", "disabled-leaves"].into_iter().enumerate() {
        let mut ms: Vec<f64> = bytes.chunks_exact(32).map(|b| {
            let b = &b[mode * 16..mode * 16 + 16];
            let a = u64::from_le_bytes(b[..8].try_into().unwrap());
            let z = u64::from_le_bytes(b[8..].try_into().unwrap());
            assert!(z >= a && z != 0);
            (z - a) as f64 * f64::from(gpu.queue.get_timestamp_period()) / 1e6
        }).collect();
        ms.sort_by(f64::total_cmp);
        eprintln!("disabled-controls {name}: n={N} median={:.6}ms p95={:.6}ms {:?}",
            ms[N as usize / 2], ms[N as usize * 95 / 100 - 1], renderers[mode].layer_stats());
    }
}
