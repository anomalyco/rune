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

#[test]
fn removed_scroll_viewport_forgets_offset_across_exit_and_instant_unmount() {
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Mode { Fade, PageInstant, RowInstant, TxInstant }

    struct S {
        page: usize,
        mode: Mode,
    }

    fn page(i: usize, s: &S) -> El {
        let rows = (0..30).map(|r| el().key(format!("p{i}-r{r}")).h(30.0).w_full().child(text(format!("row {r}"))));
        let body = col()
            .key(format!("p{i}-scroll"))
            .h(150.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(col().key(format!("p{i}-content")).children(rows));
        let p = col().key(format!("page{i}")).grid_cell(0).child(body);
        if s.mode == Mode::PageInstant { p.instant_changes() } else { p }
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let stack = crate::row().key("stack").equal_columns(1).w(300.0).child(page(s.page, s));
        col().child(if s.mode == Mode::RowInstant { stack.instant_changes() } else { stack })
    }

    for mode in [Mode::Fade, Mode::PageInstant, Mode::RowInstant, Mode::TxInstant] {
        let mut rt = Runtime::new(S { page: 0, mode }, view, (500.0, 400.0), Env::new(false, 1.0));
        rt.frame(0.0);
        for _ in 0..3 { rt.frame(1.0 / 60.0); }

        let scroll_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("p0-scroll")).unwrap();
        let scroll_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("p0-scroll")).unwrap().id;
        let scroll_path = rt.tree.nodes.iter().find(|n| n.id == scroll_id).unwrap().path.clone();
        rt.pointer_move(scroll_el.rect[0] + 10.0, scroll_el.rect[1] + 10.0);
        for _ in 0..2 { rt.frame(1.0 / 60.0); }
        rt.wheel(scroll_el.rect[0] + 10.0, scroll_el.rect[1] + 10.0, -200.0);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }

        let before = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("p0-content")).unwrap().scrolled;
        assert!(before[1] > 100.0, "{mode:?}: expected viewport to scroll down, got {before:?}");

        rt.update(|s, tx| {
            s.page = 1;
            if s.mode == Mode::TxInstant { tx.instant(); }
        });
        rt.frame(1.0 / 60.0);

        if matches!(mode, Mode::Fade | Mode::PageInstant) {
            assert!(rt.scroll.scrolls.contains_key(&scroll_id), "{mode:?}: exiting viewport keeps its scroll offset mid-fade");
        } else {
            assert!(!rt.scroll.scrolls.contains_key(&scroll_id), "{mode:?}: instantly unmounted viewport drops its scroll state immediately");
            assert!(!rt.tree.path_ids.contains_key(&scroll_path), "{mode:?}: instantly unmounted viewport drops its path_ids entry");
        }

        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(!rt.scroll.scrolls.contains_key(&scroll_id), "{mode:?}: removed viewport must not retain scroll state");

        rt.update(|s, tx| {
            s.page = 0;
            if s.mode == Mode::TxInstant { tx.instant(); }
        });
        for _ in 0..40 { rt.frame(1.0 / 60.0); }

        let back = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("p0-content")).unwrap().scrolled;
        assert_eq!(back, [0.0, 0.0], "{mode:?}: remounted viewport must come back at offset 0");
    }
}

#[test]
fn directly_unmounted_scroll_viewport_forgets_offset_before_next_frame() {
    fn view(present: &bool, _: &mut Cx<bool>) -> El {
        col().key("root").size(300.0, 200.0).children(present.then(|| {
            col().key("vp").size(200.0, 100.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("content").children((0..20).map(|r| el().key(format!("r{r}")).h(30.0).w_full())),
            )
        }))
    }

    let mut rt = Runtime::new(true, view, (300.0, 200.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let scroll_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("vp")).unwrap().id;
    rt.scroll.scrolls.get_mut(&scroll_id).unwrap().offset = 180.0;

    let mut instant_tx = Tx::default();
    instant_tx.instant();
    rt.state = false;
    rt.commit(&instant_tx);
    assert!(!rt.scroll.scrolls.contains_key(&scroll_id), "directly unmounted viewport drops scroll state on commit");

    rt.state = true;
    rt.commit(&Tx::default());
    let new_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("vp")).unwrap().id;
    assert_eq!(rt.scroll.scrolls[&new_id].offset, 0.0, "remounted viewport starts at offset 0");
}

#[test]
fn virtual_list_path_reused_by_unkeyed_element_prunes_and_places_children() {
    use std::borrow::Cow;

    struct S {
        list: bool,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let body = if s.list {
            col()
                .w_full()
                .min_h(0.0)
                .virtual_rows(40, 30.0, |_: &S, i| Cow::Owned(format!("row-{i}")), |_: &S, _| el().w_full().h(30.0).child(text("row")))
                .growth(GrowthSpec::default())
        } else {
            col().child(text("a")).child(text("b"))
        };
        col()
            .key("pane")
            .size(400.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: true })
            .child(col())
            .child(body)
    }

    let mut rt = Runtime::new(S { list: true }, view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.1);
    assert!(rt.commit.virtuals.contains_key("pane/#1"), "virtual list registered at unkeyed path");
    assert!(rt.commit.virtuals["pane/#1"].before > 0.0, "sticky list culled rows above the viewport");

    rt.update(|s, _| s.list = false);
    rt.frame(0.1);
    assert!(rt.commit.virtuals.is_empty(), "reused non-virtual path drops stale virtual metadata on commit");
    let first_text = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("a")).expect("replacement text mounted");
    assert!(
        (first_text.rect[1] - first_text.target[1]).abs() < 1.0,
        "replacement child must not be shifted by stale virtual before offset: rect={:?} target={:?}",
        first_text.rect,
        first_text.target,
    );

    rt.frame(0.1);
    assert!(rt.commit.virtuals.is_empty(), "stale virtual metadata stays gone on subsequent frames");

    rt.update(|s, _| s.list = true);
    rt.frame(0.1);
    assert!(rt.commit.virtuals.contains_key("pane/#1"), "virtual list re-registers when swapped back");
}

#[test]
fn virtual_list_path_reused_by_wrapper_with_inner_virtual_list_is_not_nested() {
    use std::borrow::Cow;

    struct S {
        wrapped: bool,
    }

    fn make_list() -> El {
        col()
            .w_full()
            .min_h(0.0)
            .virtual_rows(10, 20.0, |_: &S, i| Cow::Owned(format!("row-{i}")), |_: &S, _| el().w_full().h(20.0))
            .growth(GrowthSpec::default())
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let body = if s.wrapped {
            col().w_full().child(make_list().key("inner"))
        } else {
            make_list()
        };
        col()
            .key("pane")
            .size(400.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(col())
            .child(body)
    }

    let mut rt = Runtime::new(S { wrapped: false }, view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.commit.virtuals.contains_key("pane/#1"));

    rt.update(|s, _| s.wrapped = true);
    rt.frame(0.1);
    assert!(!rt.commit.virtuals.contains_key("pane/#1"), "outer stale path pruned");
    assert!(rt.commit.virtuals.contains_key("pane/#1/inner"), "inner virtual list registered");
    assert!(!rt.commit.virtuals["pane/#1/inner"].nested, "inner list is not nested in a stale non-virtual wrapper");
}
