use super::*;
use super::prune::REVEALS_KEPT;
use crate::{Env, GrowthSpec, ScrollSpec, col, el, text};

#[test]
fn static_scales_entry_is_pruned_when_exit_finishes() {
    fn view(present: &bool, _: &mut Cx<bool>) -> El {
        col().key("root").size(200.0, 200.0).children(
            present.then(|| el().key("scaled").size(40.0, 40.0).scale(1.5)),
        )
    }
    let mut rt = Runtime::new(true, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("scaled")).unwrap().id;
    assert!(rt.paint.tracks_static_scale(id), "scaled node is tracked in static_scales");

    rt.update(|on, _| *on = false);
    for _ in 0..180 {
        rt.frame(1.0 / 60.0);
        if !rt.tree.nodes.iter().any(|n| n.id == id) { break; }
    }
    assert!(!rt.tree.nodes.iter().any(|n| n.id == id), "scaled node finished exiting");
    assert!(!rt.paint.tracks_static_scale(id), "exited node's static_scales entry must be pruned without waiting for another commit");
}

#[test]
fn launch_for_exited_flight_is_dropped_when_exit_finishes() {
    #[derive(Default)]
    struct S { show_target: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("root").size(300.0, 200.0)
            .child(text("hello").key("src"))
            .children(s.show_target.then(|| {
                el().key("dst").absolute(150.0, 120.0).size(80.0, 24.0).child(text("hello"))
            }))
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, tx| {
        s.show_target = true;
        tx.launch("src", "dst");
    });
    rt.frame(1.0 / 60.0);
    let dst_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("dst")).unwrap().id;
    assert!(rt.motion.flying.iter().any(|f| f.0 == dst_id), "launched node is flying");
    assert!(rt.motion.launches().flying(dst_id), "launch state tracks airborne flight");

    // Remove the flying destination with an animated exit while still airborne.
    rt.update(|s, _| s.show_target = false);
    rt.frame(1.0 / 60.0);
    assert!(rt.tree.nodes.iter().any(|n| n.id == dst_id), "destination stays mounted while exiting");
    assert!(rt.motion.flying.iter().any(|f| f.0 == dst_id), "exiting flight stays lifted");

    // Let the exit finish with no further commit.
    for _ in 0..180 {
        rt.frame(1.0 / 60.0);
        if !rt.tree.nodes.iter().any(|n| n.id == dst_id) { break; }
    }
    assert!(!rt.tree.nodes.iter().any(|n| n.id == dst_id), "destination finished exiting");
    assert!(!rt.motion.flying.iter().any(|f| f.0 == dst_id), "exited flight left flying");
    assert!(!rt.motion.launches().flying(dst_id), "airborne launch entry must be dropped when its exiting flight is pruned");
}

#[test]
fn reveals_stay_bounded_across_instant_unmounts_in_commit_apply() {
    #[derive(Default)]
    struct S { n: usize, present: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("root").w(300.0).children(
            s.present.then(|| col().key(format!("p{}", s.n)).child(text("a b c").key("t").w_full().wrap().reveal(0.2))),
        )
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let mounted = rt.tree.path_ids.len();
    let mut instant_tx = Tx::default();
    instant_tx.instant();
    let count = REVEALS_KEPT * 2;
    for n in 0..count {
        rt.state.n = n;
        rt.state.present = true;
        rt.commit(&Tx::default());
        rt.state.present = false;
        rt.commit(&instant_tx);
    }
    assert_eq!(rt.motion.reveals_map().len(), REVEALS_KEPT, "commit apply must cap orphan reveals even without prune_exited");
    assert!(rt.motion.reveals_kept_len() <= REVEALS_KEPT, "{} orphans tracked", rt.motion.reveals_kept_len());
    // At most the mounted paths, the REVEALS_KEPT orphan reveal paths, and the last unmounted parent still waiting for prune_exited.
    assert!(rt.tree.path_ids.len() <= mounted + REVEALS_KEPT + 1, "{} paths kept", rt.tree.path_ids.len());
}

#[test]
fn resident_caches_and_node_flags_clear_when_exit_finishes() {
    fn view(present: &bool, _: &mut Cx<bool>) -> El {
        col().key("root").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).children(
            present.then(|| {
                col().key("section").w_full().h(300.0)
                    .child(el().key("hdr").w_full().h(24.0).sticky_top(0.0).cache())
            }),
        )
    }
    let mut rt = Runtime::new(true, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let hdr_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("hdr")).unwrap().id;
    assert!(rt.tree.has_sticky, "sticky header sets has_sticky");
    assert!(rt.tree.has_cache, "cached header sets has_cache");
    // Simulate a resident GPU cache entry with no CPU-side node_caches entry.
    rt.paint.node_caches.clear();
    rt.text.atlas.resident_caches.insert(hdr_id.0, 42);

    rt.update(|on, _| *on = false);
    for _ in 0..180 {
        rt.frame(1.0 / 60.0);
        if !rt.tree.nodes.iter().any(|n| n.id == hdr_id) { break; }
    }
    assert!(!rt.tree.nodes.iter().any(|n| n.id == hdr_id), "section finished exiting");
    assert!(!rt.tree.has_sticky, "has_sticky clears when the last sticky node finishes exiting");
    assert!(!rt.tree.has_cache, "has_cache clears when the last cached node finishes exiting");
    assert!(!rt.text.atlas.resident_caches.contains_key(&hdr_id.0), "resident_caches clears even if node_caches was empty");
}

#[test]
fn virtual_list_stays_during_exit_and_prunes_when_exit_finishes() {
    fn view(present: &bool, _: &mut Cx<bool>) -> El {
        col().key("root").size(200.0, 200.0).children(
            present.then(|| {
                col().key("vp").size(200.0, 120.0).scroll(ScrollSpec { stick_bottom: false }).child(
                    col().key("list").w_full().growth(GrowthSpec::default()).virtualize(20.0)
                        .children((0..10).map(|i| el().key(format!("r{i}")).w_full().h(20.0))),
                )
            }),
        )
    }
    let mut rt = Runtime::new(true, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let list_path = "root/vp/list";
    assert!(rt.commit.virtuals.contains_key(list_path), "virtual list registered");

    rt.update(|on, _| *on = false);
    rt.frame(1.0 / 60.0);
    assert!(rt.tree.nodes.iter().any(|n| n.path == list_path), "list stays in nodes mid-exit");
    assert!(rt.commit.virtuals.contains_key(list_path), "virtual metadata stays while exiting");

    for _ in 0..180 {
        rt.frame(1.0 / 60.0);
        if !rt.tree.nodes.iter().any(|n| n.path == list_path) { break; }
    }
    assert!(!rt.tree.nodes.iter().any(|n| n.path == list_path), "list finished exiting");
    assert!(rt.commit.virtuals.is_empty(), "virtual metadata pruned when exit finishes");
}
