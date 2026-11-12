//! Which atlas and outline texture a pool target's base bind group is made against: a target
//! waiting in the spares binds the current one only when a layer takes it back.
use super::tests::rect;
use super::*;
use crate::text::outline::{OutlineStore, WIDTH};

/// A group that needs a target of its own: half-opaque, so it isolates.
fn group(bounds: [f32; 4]) -> Vec<Instance> {
    vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5, ..Default::default() }),
        rect(bounds, [1.0; 4]), Instance::end_layer()]
}

/// `count` groups in a row, each with a target of its own.
fn groups(count: usize) -> Vec<Instance> {
    (0..count).flat_map(|index| group([index as f32 * 8.0, 4.0, 8.0, 8.0])).collect()
}

/// An outline store of `rows` rows holding a record, so text asks the renderer for Windfoil.
fn store(rows: usize) -> OutlineStore {
    OutlineStore::from_texels(vec![[0; 4]; WIDTH as usize * rows])
}

/// A target is parked when its text leaves, and the outline store can grow while it waits. Taking
/// it back for text that needs the new rows must bind the taller texture before it draws.
///
/// differs from dev: dev's `rebind_layers` walked the slots and the retained caches only, so a
/// parked target kept binding the outline texture it was made against.
#[test]
fn a_parked_target_binds_the_outline_texture_when_it_is_taken_back() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let text = group([8.0, 8.0, 32.0, 16.0]);
    // Text has drawn from outlines: Windfoil and its minimum-height outline texture exist.
    atlas.outlines = store(1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &text, (64, 64));
    let short = renderer.uber.outlines().expect("text asked for Windfoil").height();
    assert_eq!(short, crate::render::outlines::MIN_ROWS, "the store starts at its minimum");
    assert_eq!(renderer.layers.targets[0].as_ref().unwrap().outline_rows, short);

    // The text leaves: its target waits in the spares.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
    let parked = renderer.layers.spare[0].identity;

    // The store grows past the texture while the target waits: nothing binds it yet.
    atlas.outlines = store(crate::render::outlines::MIN_ROWS as usize * 2);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
    let tall = renderer.uber.outlines().expect("Windfoil stays built").height();
    assert!(tall > short, "the outline texture grew");
    assert_eq!(renderer.layers.spare[0].outline_rows, short, "a parked target waits unbound");

    // Text needing the new rows takes that same target back, and it binds before it draws.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &text, (64, 64));
    let slot = renderer.layers.targets[0].as_ref().expect("the layer has its target");
    assert_eq!(slot.identity, parked, "the layer took the parked target back");
    assert_eq!(slot.outline_rows, tall, "and draws through the taller outline store");
    assert_eq!(slot.binding, renderer.layers.bindings, "bound in this frame's generation");
}

/// Switching the outline on a layerless frame (text's first records build Windfoil) rebinds the
/// slots and retained caches, and must leave every parked target unbound: each one binds once, when
/// a layer takes it, and one no layer takes again costs nothing.
#[test]
fn a_texture_switch_binds_no_parked_target_until_one_returns() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    // Nothing has asked for outlines: the renderer is Classic, and four half-opaque groups make
    // four targets.
    let quad = groups(4);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &quad, (64, 64));
    assert!(renderer.uber.outlines().is_none(), "no outline store yet");
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
    assert_eq!(renderer.layers.spare.len(), 4, "the four targets are parked");
    let binds = renderer.layers.binds;

    // A layerless frame whose atlas grew outline records: the renderer switches to Windfoil and
    // makes its atlas pages again, rebinding every live target twice.
    atlas.outlines = store(1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
    assert!(renderer.uber.outlines().is_some(), "the frame switched to Windfoil");
    assert_eq!(renderer.layers.binds, binds, "a parked target makes no bind group");
    let generation = renderer.layers.bindings;
    assert!(renderer.layers.spare.iter().all(|t| t.binding < generation), "every spare waits stale");

    // One layer comes back and takes one of them: that target binds, once, and only it.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &quad[..3], (64, 64));
    assert_eq!(renderer.layers.binds, binds + 1, "one target returned, so one bind group");
    let slot = renderer.layers.targets[0].as_ref().expect("the layer has its target");
    assert_eq!(slot.binding, generation, "and it binds the frame's atlas and outline texture");
    assert!(renderer.layers.spare.iter().all(|t| t.binding < generation), "the others still wait");
    assert_eq!(renderer.layers.spare.len(), 3);

    // A frame that keeps that layer takes no new target and binds nothing.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &quad[..3], (64, 64));
    assert_eq!(renderer.layers.binds, binds + 1, "a steady frame binds nothing");
}
