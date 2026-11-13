//! How a layer draws (`Disposition`) when the frame finds out more about its parent than the
//! planning walk knew at the layer's start.
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

/// A cache hit composites from a texture the miss drew upright, so it must carry the same turn the
/// miss did: an unturned hit snaps the scope back to where its bounds sit.
#[test]
fn a_cache_hit_composites_with_its_layer_turn() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 64, 64);
    let mut atlas = crate::text::TextSystem::default().atlas;
    // A wide scope holding ink in its left third, turned about its centre: the turn moves it.
    let bounds = [16.0, 22.0, 32.0, 12.0];
    let turned = |angle: Option<f32>| {
        let mut cache = Instance::begin_layer(bounds, NO_CLIP, LayerSpec::default());
        if let Some(angle) = angle { cache = cache.turned(angle, [32.0, 28.0]); }
        cache.params[3] = 1.0;
        cache.radii = pack_u64_f32(31);
        cache.uv = pack_u64_f32(1);
        vec![cache, rect([bounds[0] + 2.0, bounds[1] + 2.0, 8.0, 6.0]), Instance::end_layer()]
    };
    // The miss draws the turned scope into its target and composites it turned.
    let instances = turned(Some(0.6));
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (64, 64));
    assert!(matches!(renderer.layers.planner.dispositions[0], Disposition::Render { .. }), "the first frame draws it");
    let miss = pixels(&gpu, &renderer, &target);
    // The hit composites the texture it kept, and must land the same pixels.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (64, 64));
    assert!(matches!(renderer.layers.planner.dispositions[0], Disposition::Cached { .. }), "the second frame hits");
    let hit = pixels(&gpu, &renderer, &target);
    let differing = hit.iter().zip(&miss).filter(|(hit, miss)| hit != miss).count();
    assert_eq!(differing, 0, "a hit composites the turned quad the miss drew");
    // And the turn is what moves them: the same scope upright draws elsewhere.
    let upright = turned(None);
    for _ in 0..2 { renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &upright, (64, 64)); }
    assert_ne!(pixels(&gpu, &renderer, &target), hit, "the turn is in the composite, not only the render");
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
    assert!(matches!(renderer.layers.planner.dispositions[1], Disposition::Render { .. }));

    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    assert!(matches!(renderer.layers.planner.dispositions[0], Disposition::Cached { .. }));
    assert_eq!(renderer.layers.planner.dispositions[1], Disposition::Skip);

    // The kept texture no longer fits, but stays in the pool for this frame's walk to find.
    renderer.layers.cache_budget = 1;
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    near(&pixels(&gpu, &renderer, &target)[at..at + 4], rgba([0.5; 4]));
    assert!(matches!(renderer.layers.planner.dispositions[1], Disposition::Render { .. }));
}

/// A cached scope (node 41) at `bounds`: at each of `children`, from its corner, a plain group
/// holding a half-faded layer, which needs a target of its own.
fn cached_children(bounds: [f32; 4], clip: [f32; 4], turn: Option<f32>, children: &[[f32; 2]]) -> Vec<Instance> {
    let mut cache = Instance::begin_layer(bounds, clip, LayerSpec::default());
    if let Some(angle) = turn { cache = cache.turned(angle, [bounds[0] + bounds[2] * 0.5, bounds[1] + bounds[3] * 0.5]); }
    cache.params[3] = 1.0;
    cache.radii = pack_u64_f32(41);
    cache.uv = pack_u64_f32(1);
    let mut out = vec![cache];
    for at in children {
        let child = [bounds[0] + at[0], bounds[1] + at[1], 8.0, 8.0];
        out.extend([Instance::begin_layer(child, NO_CLIP, LayerSpec::default()),
            Instance::begin_layer(child, NO_CLIP, LayerSpec { opacity: 0.5, ..Default::default() }),
            rect(child), Instance::end_layer(), Instance::end_layer()]);
    }
    out.push(Instance::end_layer());
    out
}

/// What `second` shows as a hit after `first` drew the cache, beside what it shows drawn fresh.
fn hit_and_fresh(first: &[Instance], second: &[Instance]) -> (Vec<u8>, Vec<u8>) {
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 64, 64);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let mut fresh = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    fresh.prepare(&gpu.device, &gpu.queue, &mut atlas, second, (64, 64));
    let fresh = pixels(&gpu, &fresh, &target);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, first, (64, 64));
    // Drawing the first frame is what fills the cache's texture.
    pixels(&gpu, &renderer, &target);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, second, (64, 64));
    assert!(matches!(renderer.layers.planner.dispositions[0], Disposition::Cached { .. }), "the second frame hits");
    (pixels(&gpu, &renderer, &target), fresh)
}

/// A cache's texture keeps all of its content, the layers in it too, so a hit that turns a child
/// the window couldn't see into view shows it as a fresh miss does, both ways round.
#[test]
fn a_cache_hit_turned_shows_a_child_the_window_could_not_see() {
    // Half a turn about (64, 32) swaps the child at 96 (off the window upright) and the one at 16.
    let scene = |turn| cached_children([0.0, 0.0, 128.0, 64.0], NO_CLIP, turn, &[[16.0, 24.0], [96.0, 24.0]]);
    let (upright, turned) = (scene(None), scene(Some(std::f32::consts::PI)));
    let (hit, fresh) = hit_and_fresh(&upright, &turned);
    let at = (36 * 64 + 28) * 4;
    near(&fresh[at..at + 4], rgba([0.5; 4]));
    same(&hit, &fresh, "turned into view");
    let (hit, fresh) = hit_and_fresh(&turned, &upright);
    let at = (28 * 64 + 20) * 4;
    near(&fresh[at..at + 4], rgba([0.5; 4]));
    same(&hit, &fresh, "turned back into view");
}

/// A cache's clip applies to its composite: a hit that moves a child its clip hid into the clip
/// shows it as a fresh miss does.
#[test]
fn a_cache_hit_moved_shows_a_child_its_clip_hid() {
    let clip = [0.0, 0.0, 32.0, 64.0];
    let first = cached_children([0.0, 0.0, 64.0, 64.0], clip, None, &[[40.0, 24.0]]);
    let second = cached_children([-32.0, 0.0, 64.0, 64.0], clip, None, &[[40.0, 24.0]]);
    let (hit, fresh) = hit_and_fresh(&first, &second);
    let at = (28 * 64 + 12) * 4;
    near(&fresh[at..at + 4], rgba([0.5; 4]));
    same(&hit, &fresh, "moved into the clip");
}

/// Asserts the hit landed every pixel the fresh miss did, naming the first that differs.
fn same(hit: &[u8], fresh: &[u8], what: &str) {
    let differing = hit.chunks(4).zip(fresh.chunks(4)).position(|(hit, fresh)| hit != fresh);
    if let Some(at) = differing {
        panic!("{what}: ({}, {}) is {:?} on the hit, {:?} fresh", at % 64, at / 64, &hit[at * 4..at * 4 + 4], &fresh[at * 4..at * 4 + 4]);
    }
}
