//! How a layer draws (`Show`) when the frame finds out more about its parent than the planning
//! walk knew at the layer's start.
use super::*;
use super::tests::{near, pixels, rgba};

fn rect(bounds: [f32; 4]) -> Instance {
    Instance { rect: bounds, fill: [1.0; 4], clip: NO_CLIP, params: [0.0, KIND_RECT, 0.0, 1.0], ..Instance::zeroed() }
}

/// A cached scope (`El::cache`, node 7) holding a half-faded group of two overlapping rects.
fn cached_card(bounds: [f32; 4]) -> Vec<Instance> {
    let mut cache = Instance::begin_layer(bounds, NO_CLIP, LayerSpec::default());
    cache.params[3] = 1.0;
    cache.radii = pack_u64_f32(7);
    cache.uv = pack_u64_f32(1);
    let inner = [bounds[0] + 4.0, bounds[1] + 2.0, bounds[2] - 8.0, bounds[3] - 4.0];
    vec![cache, Instance::begin_layer(inner, NO_CLIP, LayerSpec { opacity: 0.5, ..Default::default() }),
        rect(inner), rect([inner[0] + 4.0, inner[1], inner[2] - 8.0, inner[3]]), Instance::end_layer(), Instance::end_layer()]
}

/// The planning walk takes a kept cache to be a hit and covers what's inside it; when the cache
/// then doesn't fit the budget, the scope draws as a plain group, and its faded child with it.
#[test]
fn a_layer_inside_a_cache_that_misses_its_budget_still_draws() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let instances = cached_card([4.0, 2.0, 24.0, 12.0]);
    let at = (8 * 32 + 16) * 4;
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    near(&pixels(&gpu, &renderer, &target)[at..at + 4], rgba([0.5; 4]));
    assert_eq!(renderer.layers.plan[1].show, Show::Render);

    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    assert_eq!((renderer.layers.plan[0].show, renderer.layers.plan[1].show), (Show::Cached, Show::Covered));

    // The kept texture no longer fits, but stays in the pool for this frame's walk to find.
    renderer.layers.cache_budget = 1;
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    near(&pixels(&gpu, &renderer, &target)[at..at + 4], rgba([0.5; 4]));
    assert_eq!(renderer.layers.plan[1].show, Show::Render);
}
