use super::*;

fn rect(rect: [f32; 4], fill: [f32; 4]) -> Instance {
    Instance { rect, fill, clip: NO_CLIP, ..bytemuck::Zeroable::zeroed() }
}

#[test]
fn backdrop_pool_is_lazy_reused_resized_and_follows_live_formats() {
    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let mut slot = rect([0.0, 0.0, 16.0, 16.0], [0.0, 0.0, 0.0, 1.0]);
    slot.params = [0.0, KIND_EFFECT, 0.0, 1.0];
    for instances in [&[][..], &[slot][..]] {
        renderer.prepare(device, queue, &mut text.atlas, instances, (32, 32));
        assert!(renderer.backdrops.is_empty(), "ordinary slots allocate no backdrops");
    }
    slot.params[0] = 1.0;
    renderer.prepare(device, queue, &mut text.atlas, &[slot], (32, 32));
    let original = renderer.backdrops[0].view.clone();
    renderer.prepare(device, queue, &mut text.atlas, &[slot], (32, 32));
    assert_eq!(renderer.backdrops[0].view, original);

    // Switching after prepare is legal: the next draw must already have the new format's pool.
    let format = OFFSCREEN_FORMAT_LINEAR;
    renderer.set_format(device, format);
    assert!(renderer.backdrops.iter().any(|b| b.format == format));
    for (format, size) in [(format, (48, 24)), (OFFSCREEN_FORMAT, (24, 48)), (format, (32, 32))] {
        renderer.set_format(device, format);
        renderer.prepare(device, queue, &mut text.atlas, &[slot, slot], size);
        let target = Offscreen::with_format(device, size.0, size.1, format);
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut backdrops = Vec::new();
        renderer.draw_with_backdrop(&mut encoder, &target.view, [0.0; 4], &mut |phase, site| {
            if let EffectPhase::Backdrop(_) = phase {
                let view = site.backdrop.unwrap();
                assert_eq!((view.texture().width(), view.texture().height()), size);
                assert_eq!(view.texture().format(), format);
                backdrops.push(view.clone());
            }
        });
        queue.submit([encoder.finish()]);
        assert_eq!(backdrops.len(), 2);
        assert_eq!(backdrops[0], backdrops[1], "same-format slots share a texture");
        assert_eq!(renderer.backdrops.len(), 2);
    }
}

#[test]
fn split_resets_the_hook_viewport_and_scissor_for_later_primitives() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let mut slot = rect([0.0, 0.0, 16.0, 16.0], [0.0, 0.0, 0.0, 1.0]);
    slot.params = [1.0, KIND_EFFECT, 0.0, 1.0];
    slot.clip = [0.0, 0.0, 16.0, 16.0];
    let instances = [rect([0.0, 0.0, 32.0, 32.0], [1.0, 0.0, 0.0, 1.0]), slot,
        rect([16.0, 0.0, 16.0, 32.0], [0.0, 1.0, 0.0, 1.0])];
    renderer.prepare(device, queue, &mut text.atlas, &instances, (32, 32));
    let target = Offscreen::new(device, 32, 32);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw_with(&mut encoder, &target.view, [0.0; 4], &mut |pass, site| {
        assert!(site.backdrop.is_some());
        pass.set_viewport(0.0, 0.0, 1.0, 1.0, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, 1, 1);
    });
    target.copy(&mut encoder);
    queue.submit([encoder.finish()]);
    let pixels = target.read(device);
    for (x, color) in [(8, [255, 0, 0, 255]), (24, [0, 255, 0, 255])] {
        let at = (24 * 32 + x) * 4;
        assert_eq!(&pixels[at..at + 4], &color);
    }
}

#[test]
fn backdrop_slot_degrades_when_the_target_cannot_be_copied() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let mut slot = rect([0.0, 0.0, 16.0, 16.0], [0.0, 0.0, 0.0, 1.0]);
    slot.params = [1.0, KIND_EFFECT, 0.0, 1.0];
    slot.clip = [0.0, 0.0, 16.0, 16.0];
    renderer.prepare(device, queue, &mut text.atlas, &[slot], (32, 32));
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("no copy src"),
        size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut draws = 0;
    renderer.draw_with(&mut encoder, &view, [0.0; 4], &mut |_, site| {
        assert!(site.backdrop.is_none());
        draws += 1;
    });
    queue.submit([encoder.finish()]);
    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "{error:?}");
    assert_eq!(draws, 1);
}

fn blur(rect: [f32; 4], sigma: f32) -> Instance {
    Instance { rect, fill: [0.0, 0.0, 0.0, 1.0], clip: NO_CLIP, params: [sigma, KIND_BACKDROP_BLUR, 0.0, 1.0], ..bytemuck::Zeroable::zeroed() }
}

/// A frosted pill in a large window: the pooled copy holds the pill's region, not the window,
/// follows the pill without a new texture, and the pill still blurs what is under it.
#[test]
fn a_blur_alone_pools_a_copy_of_its_region_not_of_the_window() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let size = (1024, 768);
    let target = Offscreen::new(device, size.0, size.1);
    // Red on the left, blue on the right, the seam at x = 512 under the pill.
    let ground = [rect([0.0, 0.0, 512.0, 768.0], [1.0, 0.0, 0.0, 1.0]), rect([512.0, 0.0, 512.0, 768.0], [0.0, 0.0, 1.0, 1.0])];
    let mut frame = |renderer: &mut Renderer, pill: [f32; 4]| {
        renderer.prepare(device, queue, &mut text.atlas, &[ground[0], ground[1], blur(pill, 8.0)], size);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        target.copy(&mut encoder);
        queue.submit([encoder.finish()]);
        target.read(device)
    };
    let pixels = frame(&mut renderer, [480.0, 600.0, 64.0, 32.0]);
    assert_eq!(renderer.backdrops.len(), 1);
    let (w, h) = (renderer.backdrops[0].texture.width(), renderer.backdrops[0].texture.height());
    assert!(w * h * 4 <= size.0 * size.1, "a copy of the pill's region, not of the 1024x768 window: {w}x{h}");
    let seam = ((616 * 1024 + 512) * 4) as usize;
    assert!(pixels[seam] > 20 && pixels[seam + 2] > 20, "the pill blends red and blue across the seam: {:?}", &pixels[seam..seam + 4]);
    let away = ((100 * 1024 + 512) * 4) as usize;
    assert!(pixels[away] == 0 || pixels[away + 2] == 0, "outside the pill the seam stays sharp: {:?}", &pixels[away..away + 4]);

    // The pill moves: the same texture, at its new place.
    let kept = renderer.backdrops[0].view.clone();
    let pixels = frame(&mut renderer, [470.0, 300.0, 64.0, 32.0]);
    assert_eq!(renderer.backdrops[0].view, kept, "a moved region of the same size makes no texture");
    let seam = ((316 * 1024 + 512) * 4) as usize;
    assert!(pixels[seam] > 20 && pixels[seam + 2] > 20, "the moved pill blends across the seam: {:?}", &pixels[seam..seam + 4]);

    // An effect that reads what is below it still gets a copy of the whole target.
    let mut slot = rect([0.0, 0.0, 16.0, 16.0], [0.0, 0.0, 0.0, 1.0]);
    slot.params = [1.0, KIND_EFFECT, 0.0, 1.0];
    renderer.prepare(device, queue, &mut text.atlas, &[ground[0], slot, blur([480.0, 600.0, 64.0, 32.0], 8.0)], size);
    assert_eq!((renderer.backdrops[0].texture.width(), renderer.backdrops[0].texture.height()), size);
    assert_eq!(renderer.backdrops[0].origin, [0, 0]);
}

#[test]
fn effect_sites_match_draw_with_for_main_and_isolated_layer_slots() {
    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut rt = crate::runtime::Runtime::new(
        (),
        |_: &(), _: &mut crate::Cx<()>| {
            crate::el()
                .w(200.0)
                .h(200.0)
                .child(
                    crate::el()
                        .absolute(10.0, 12.0)
                        .w(40.0)
                        .h(30.0)
                        .effect(11)
                        .effect_data([1.0, 2.0, 3.0, 4.0]),
                )
                .child(
                    crate::el()
                        .absolute(40.0, 60.0)
                        .w(80.0)
                        .h(70.0)
                        .opacity(0.5)
                        .child(
                            crate::el()
                                .w(80.0)
                                .h(70.0)
                                .effect(22)
                                .effect_data([5.0, 6.0, 7.0, 8.0]),
                        ),
                )
                .child(
                    crate::el()
                        .absolute(0.0, 0.0)
                        .w(10.0)
                        .h(10.0)
                        .clip_rounded(0.0)
                        .child(
                            crate::el()
                                .absolute(50.0, 50.0)
                                .w(20.0)
                                .h(20.0)
                                .effect(33),
                        ),
                )
        },
        (200.0, 200.0),
        crate::Env::new(true, 1.0),
    );
    rt.frame(0.0);
    renderer.prepare(device, queue, &mut rt.text.atlas, &rt.instances, (200, 200));

    let pre: Vec<_> = renderer
        .effect_sites()
        .into_iter()
        .map(|s| (s.index, s.mark, s.data, s.scale, s.format, s.origin, s.size, s.in_layer, s.srgb_blend, s.fade_stops))
        .collect();
    assert_eq!(pre.len(), 2, "clipped-out slot 33 must be omitted: {pre:?}");
    assert_eq!(pre[0].1, 11);
    assert!(!pre[0].7, "slot 11 is in the main frame pass");
    assert_eq!(pre[0].5, [0.0, 0.0]);
    assert_eq!(pre[0].6, [200, 200]);
    assert_eq!(pre[0].4, OFFSCREEN_FORMAT);

    assert_eq!(pre[1].1, 22);
    assert!(pre[1].7, "slot 22 is inside an isolated opacity layer");
    assert!(pre[1].5[0] > 0.0 && pre[1].5[1] > 0.0, "isolated layer has non-zero window origin: {:?}", pre[1].5);
    assert_eq!(pre[1].4, wgpu::TextureFormat::Rgba16Float);

    let target = Offscreen::with_format(device, 200, 200, OFFSCREEN_FORMAT);
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut drawn = Vec::new();
    renderer.draw_with(&mut encoder, &target.view, [0.0; 4], &mut |_pass, s| {
        drawn.push((s.index, s.mark, s.data, s.scale, s.format, s.origin, s.size, s.in_layer, s.srgb_blend, s.fade_stops));
    });
    queue.submit([encoder.finish()]);

    // encode_layers_with visits layer passes before the main pass; compare by slot index.
    let mut pre_sorted = pre;
    pre_sorted.sort_by_key(|s| s.0);
    drawn.sort_by_key(|s| s.0);
    assert_eq!(pre_sorted, drawn, "pre-pass effect_sites() must match EffectSite delivered by draw_with");
}