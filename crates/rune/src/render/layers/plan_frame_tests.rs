//! What a frame costs the plan: a frame with no layers plans nothing at all, a frame reads no spare
//! target dev didn't, the plan's scratch stays warm without keeping a burst's size, and the scratch
//! a layer stops needing goes before a later layer makes a texture, so the frame's peak is dev's.
use super::tests::rect;
use super::*;

/// A group that needs a target of its own: half-opaque, and blurred when `sigma` is not zero.
fn group(bounds: [f32; 4], sigma: f32) -> Vec<Instance> {
    vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5, blur: sigma, ..Default::default() }),
        rect(bounds, [1.0; 4]), Instance::end_layer()]
}

/// 1,024 small groups, each with a target of its own.
fn burst() -> Vec<Instance> {
    (0..1024).flat_map(|index| group([(index % 32) as f32 * 8.0, (index / 32) as f32 * 8.0, 8.0, 8.0], 0.0)).collect()
}

/// Whether every one of the plan's scratch buffers holds no more than an ordinary frame keeps.
fn warm_sized(renderer: &Renderer) -> bool {
    renderer.layers.planner.capacities().iter().all(|&capacity| capacity <= 64)
}

/// A frame with no layers draws nothing, so it must plan nothing: dev walked no target and no layer
/// there, and neither may the plan, however many layers closed on the frame before.
#[test]
fn a_layerless_frame_plans_nothing() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &burst(), (256, 256));
    assert_eq!(renderer.layer_stats().targets, 1024, "a frame's worth of layers, every one with a target");

    // They close: their targets stay parked for the next layer of their size, and the plan's own
    // buffers must not be left behind at a burst's size for every frame after them.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (256, 256));
    let stats = renderer.layer_stats();
    assert_eq!((stats.targets, stats.bytes, stats.extra_passes), (0, 0, 0), "nothing is drawn");
    assert!(renderer.layers.planner.dispositions.is_empty());
    assert!(renderer.layers.planner.moves.is_empty());
    assert!(warm_sized(&renderer), "a burst's planning buffers go with it");
}

/// One layer left of a burst keeps its own target, which still fits: dev never looked at the spare
/// pool again, and neither may the plan, frame after frame.
#[test]
fn one_layer_left_of_a_burst_reads_no_spare() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let burst = burst();
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &burst, (256, 256));
    let visits = renderer.layers.planner.parked.visits;
    for frame in 0..4 {
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &burst[..3], (256, 256));
        assert_eq!(renderer.layers.spare.len(), 1023, "the burst's other targets are parked");
        assert_eq!(renderer.layers.planner.parked.visits, visits, "frame {frame} read a spare");
        assert!(warm_sized(&renderer), "a burst's planning buffers go once it ends");
    }
}

/// A menu that opens and closes again and again: once warm, neither the plan's scratch nor a
/// target is made again, as dev reused its vectors and the target it parked.
#[test]
fn a_warm_menu_opens_and_closes_without_allocating() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let menu = group([4.0, 4.0, 40.0, 24.0], 0.0);
    // The first opening makes the target; the second takes it back from the spares.
    for instances in [&menu[..], &[], &menu[..]] {
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, instances, (64, 64));
    }
    let (warm, allocations) = (renderer.layers.planner.capacities(), renderer.layer_stats().allocations);
    for toggle in 0..4 {
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
        assert_eq!(renderer.layers.planner.capacities(), warm, "closing {toggle}");
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &menu, (64, 64));
        assert_eq!(renderer.layers.planner.capacities(), warm, "opening {toggle}");
        assert_eq!(renderer.layer_stats().allocations, allocations, "opening {toggle} made a target");
    }
}

/// The frame a menu closes on forgets its plan; the empty frames after it have nothing to forget.
#[test]
fn an_empty_frame_after_an_empty_frame_does_nothing() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &group([4.0, 4.0, 40.0, 24.0], 0.0), (64, 64));
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
    assert_eq!(renderer.layers.planner.settles, 1, "the frame the menu closed on");
    for frame in 0..3 {
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
        assert_eq!(renderer.layers.planner.settles, 1, "empty frame {frame} tidied the plan");
    }
}

/// Two viewport-sized half-opaque siblings each fill a 2,048 bucket: 32 MiB of texture. The first
/// blurs at sigma 1, whose scratch is one more texture at the same size, and dev dropped that
/// scratch before the sibling's texture existed: at most 64 MiB is ever live at once.
#[test]
fn a_frame_never_holds_more_layer_textures_than_dev() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let (one, two) = ([4.0, 4.0, 2000.0, 2000.0], [8.0, 8.0, 2000.0, 2000.0]);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &group(one, 1.0), (2048, 2048));
    assert_eq!(renderer.layers.peak_bytes(), 64 << 20, "the layer's texture and its blur scratch");

    // It stops blurring while a sibling appears: its scratch is released before the sibling's
    // texture is made, as dev released it in the same layer's own step.
    let mut instances = group(one, 0.0);
    instances.extend(group(two, 0.0));
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (2048, 2048));
    assert_eq!(renderer.layers.peak_bytes(), 64 << 20, "two textures, with the scratch already gone");
}
