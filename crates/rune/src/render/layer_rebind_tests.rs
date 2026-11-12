//! Which outline texture a pool target's base bind group is made against: a target waiting in the
//! spares for the next layer of its size takes a taller outline store too.
use super::tests::rect;
use super::*;
use crate::text::outline::{OutlineStore, WIDTH};

/// A group that needs a target of its own: half-opaque, so it isolates.
fn group(bounds: [f32; 4]) -> Vec<Instance> {
    vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5, ..Default::default() }),
        rect(bounds, [1.0; 4]), Instance::end_layer()]
}

/// An outline store of `rows` rows, one record in it, so text asks the renderer for Windfoil.
fn store(rows: usize) -> OutlineStore {
    OutlineStore::from_texels(vec![[0; 4]; WIDTH as usize * rows])
}

/// A text layer's target is parked when its text leaves, and the outline store can grow while it
/// waits. Taking it back for text that needs the new rows must bind the taller texture, not the one
/// the target was made with.
///
/// differs from dev: dev's `rebind_layers` walked the slots and the retained caches only, so a
/// parked target kept binding the outline texture it was made against.
#[test]
fn a_parked_target_binds_the_outline_texture_the_frame_draws_with() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let bounds = [8.0, 8.0, 32.0, 16.0];
    let text = group(bounds);
    // Text has drawn from outlines: Windfoil and its 16-row outline texture exist.
    atlas.outlines = store(1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &text, (64, 64));
    let short = renderer.uber.outlines().expect("text asked for Windfoil").height();
    assert_eq!(short, crate::render::outlines::MIN_ROWS, "the store starts at its minimum");
    assert_eq!(renderer.layers.targets[0].as_ref().unwrap().outline_rows, short);

    // The text leaves: its target waits in the spares.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
    let parked = renderer.layers.spare[0].identity;
    assert_eq!(renderer.layers.spare[0].outline_rows, short);

    // The store grows past the texture while the target waits.
    atlas.outlines = store(crate::render::outlines::MIN_ROWS as usize * 2);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
    let tall = renderer.uber.outlines().expect("Windfoil stays built").height();
    assert!(tall > short, "the outline texture grew");
    for t in &renderer.layers.spare {
        assert_eq!(t.outline_rows, tall, "a parked target binds the texture this frame draws with");
    }

    // Text needing the new rows takes that same target back and draws into it.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &text, (64, 64));
    let slot = renderer.layers.targets[0].as_ref().expect("the layer has its target");
    assert_eq!(slot.identity, parked, "the layer took the parked target back");
    assert_eq!(slot.outline_rows, tall, "and draws through the taller outline store");
}
