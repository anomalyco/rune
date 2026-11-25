use crate::{Anim, LayoutAnim, Mount, NodeId, NodeSpec, Rect, Scene, Spring, prop};

#[test]
fn shrink_to_live_compacts_unmounted_nodes_and_tracks_preserving_live_state() {
    let mut scene = Scene::new();
    let root = NodeId(1);
    scene.insert(root, NodeSpec::new(Rect::new(0.0, 0.0, 900.0, 700.0)), Mount::Initial);
    let kept = [NodeId(2), NodeId(3), NodeId(4)];
    for (i, &id) in kept.iter().enumerate() {
        scene.insert(id, NodeSpec::new(Rect::new(10.0, i as f32 * 30.0, 200.0, 24.0)).parent(root), Mount::Initial);
    }
    for i in 0..2000u64 {
        let id = NodeId(100 + i);
        scene.insert(id, NodeSpec::new(Rect::new(0.0, i as f32 * 20.0, 800.0, 18.0)).parent(root), Mount::Initial);
        scene.set(id, prop::OPACITY, 0.9);
    }
    scene.frame_at(0.0);
    assert_eq!(scene.len(), 2004);
    assert!(scene.nodes.capacity() >= 2004);

    // Unmount all 2000 temporary nodes and settle.
    for i in 0..2000u64 {
        scene.remove(NodeId(100 + i));
    }
    scene.frame_at(1.0);
    assert_eq!(scene.len(), 4);
    let before: Vec<_> = std::iter::once(root)
        .chain(kept)
        .map(|id| (id, scene.world(id), scene.presented(id)))
        .collect();

    scene.shrink_to_live();
    assert!(scene.nodes.capacity() <= 64, "nodes capacity shrank: {}", scene.nodes.capacity());
    assert!(scene.rests.capacity() <= 64, "rests capacity shrank: {}", scene.rests.capacity());
    assert!(scene.free.is_empty());
    assert_eq!(scene.len(), 4);

    // Live nodes keep identical geometry, presented state, and accept new animations cleanly.
    for (id, w, p) in before {
        assert_eq!(scene.world(id), w);
        assert_eq!(scene.presented(id), p);
    }
    scene.set_frame(kept[0], Rect::new(20.0, 40.0, 220.0, 28.0), LayoutAnim::new(Anim::from(Spring::snappy(0.25, 0.0))));
    let out = scene.frame_at(1.016);
    assert!(out.animating, "compacted live node still animates cleanly");

    // Calling shrink_to_live while animating does not drop active motion.
    scene.shrink_to_live();
    assert!(scene.store.is_animating_any());
}
