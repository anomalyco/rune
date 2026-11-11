//! What a frame costs the plan: a frame with no layers plans nothing at all, and the captions a
//! layer stops needing go before a later layer makes a texture, so the frame's peak is dev's.
use super::tests::rect;
use super::*;

/// A group that needs a target of its own: half-opaque, and blurred when `sigma` is not zero.
fn group(bounds: [f32; 4], sigma: f32) -> Vec<Instance> {
    vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5, blur: sigma, ..Default::default() }),
        rect(bounds, [1.0; 4]), Instance::end_layer()]
}

/// A frame with no layers draws nothing, so it must plan nothing: dev walked no target and no layer
/// there, and neither may the plan, however many layers closed on the frame before.
#[test]
fn a_layerless_frame_plans_nothing() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let mut instances = Vec::new();
    for index in 0..1024 {
        let at = [(index % 32) as f32 * 8.0, (index / 32) as f32 * 8.0];
        instances.extend(group([at[0], at[1], 8.0, 8.0], 0.0));
    }
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (256, 256));
    assert!(renderer.layers.slots.len() == 1024, "a frame's worth of layers, every one with a target");

    // They close: their targets stay parked for the next layer of their size, and the plan's own
    // buffers must not be left behind for every frame after them, nor copied again for nothing.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (256, 256));
    let stats = renderer.layer_stats();
    assert_eq!((stats.targets, stats.bytes, stats.extra_passes), (0, 0, 0), "nothing is drawn");
    assert!(renderer.layers.kept.is_empty(), "no kept target is read for a frame with no layers");
    assert!(renderer.layers.parked.is_empty(), "no spare is read for a frame with no layers");
    assert!(renderer.layers.slots.is_empty());
    assert!(renderer.layers.dispositions.is_empty());
    assert!(renderer.layers.moves.is_empty());
    assert_eq!(renderer.layers.slots.capacity(), 0, "a burst's planning buffers go with it");
    assert_eq!(renderer.layers.dispositions.capacity(), 0);
    assert_eq!(renderer.layers.moves.capacity(), 0);
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
