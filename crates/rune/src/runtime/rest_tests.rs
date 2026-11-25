use crate::{Cx, El, Env, Runtime, col, el, memo, text};

struct State {
    large: bool,
    tick: u32,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    if s.large {
        col().key("doc").size(800.0, 600.0).children((0..1500).map(|i| {
            memo(format!("row-{i}"), i, move || {
                el().key(format!("r{i}")).h(20.0).child(text(format!("Line number {i} in document")))
            })
        }))
    } else {
        col()
            .key("start")
            .size(800.0, 600.0)
            .child(text(format!("Start page ({})", s.tick)).key("title"))
    }
}

#[test]
fn runtime_rest_shrinks_retained_tables_prunes_exited_paths_and_preserves_paint_keep() {
    let mut rt = Runtime::new(State { large: true, tick: 0 }, view, (800.0, 600.0), Env::new(true, 1.0));
    #[cfg(debug_assertions)]
    rt.set_paint_keep_strict(true);
    rt.a11y_activate("Test");
    rt.frame(0.0);
    let _ = rt.a11y_update();
    assert!(rt.tree.nodes.len() > 1500);
    assert!(rt.tree.path_ids.len() > 1500);

    // Switch to the small start page and let exits finish.
    rt.update(|s, _| s.large = false);
    for _ in 0..120 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    let _ = rt.a11y_update();
    assert!(rt.tree.nodes.len() < 10);

    rt.rest();
    assert!(rt.tree.path_ids.len() <= 16, "exited paths pruned: {}", rt.tree.path_ids.len());
    assert!(rt.tree.nodes.capacity() <= 128, "nodes capacity shrank: {}", rt.tree.nodes.capacity());
    assert!(rt.tree.spare_nodes.capacity() <= 128, "spare_nodes shrank: {}", rt.tree.spare_nodes.capacity());
    assert!(rt.paint.inh_scratch.capacity() <= 128, "inh_scratch shrank: {}", rt.paint.inh_scratch.capacity());
    assert!(rt.text.cached_layout_count() <= 2, "only start-page layout kept: {}", rt.text.cached_layout_count());

    // Subsequent frame must render cleanly with zero PaintKeep self-check mismatches (strict mode).
    let before_inst = rt.instances.len();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.instances.len(), before_inst);

    // Mutating the start page after rest still commits and paints cleanly.
    rt.update(|s, _| s.tick += 1);
    rt.frame(1.0 / 60.0);
    assert!(!rt.instances.is_empty());
}
