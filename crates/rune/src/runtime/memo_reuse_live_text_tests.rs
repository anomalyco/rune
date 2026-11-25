use crate::{Color, Cx, El, Env, LiveText, LoopSpec, Runtime, TextStyle, col, el, memo, row, text};

#[test]
fn memo_holding_live_text_reuses_nodes_copies_static_siblings_and_avoids_double_shaping() {
    const N: usize = 10;
    struct State {
        cells: Vec<LiveText>,
        tick: u64,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = TextStyle::new(12.0, 400.0).features("'tnum' 1");
        let grid = memo("grid", s.cells.len(), || {
            let mut g = col().key("grid").gap(2.0);
            for i in 0..s.cells.len() {
                g = g.child(
                    row()
                        .key(format!("cell_{i}"))
                        .gap(4.0)
                        .child(text(format!("P{i:03}")).key(format!("tag_{i}")).font(TextStyle::new(10.0, 400.0)))
                        .child(
                            text("000.00 bar")
                                .key(format!("val_{i}"))
                                .size(90.0, 18.0)
                                .font(style)
                                .live_text(&s.cells[i]),
                        ),
                );
            }
            g
        });
        let spinner = el().key("spinner").size(16.0, 16.0).fill(Color::WHITE).looping(LoopSpec::rotation(1.0));
        let status = (0..=(s.tick % 4) as usize)
            .fold(row().key("status").gap(2.0), |r, _| r.child(el().size(6.0, 6.0).fill(Color::WHITE)));
        col().w_full().h_full().padding(8.0).child(row().gap(6.0).child(spinner).child(status)).child(grid)
    }

    let cells: Vec<LiveText> = (0..N).map(|i| LiveText::new(format!("{:6.2} bar", 10.0 + i as f64))).collect();
    let handles = cells.clone();
    let mut rt = Runtime::new(State { cells, tick: 0 }, view, (400.0, 400.0), Env::new(true, 2.0));
    let dt = 1.0 / 60.0;
    rt.frame(0.0);
    rt.frame(dt);

    fn node_by_key<'a>(rt: &'a Runtime<State>, k: &str) -> &'a crate::runtime::types::Node {
        rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some(k)).unwrap()
    }
    let tag_epochs: Vec<(crate::NodeId, u32)> = (0..N).map(|i| {
        let n = node_by_key(&rt, &format!("tag_{i}"));
        (n.id, n.epoch)
    }).collect();
    let val_epochs: Vec<(crate::NodeId, u32)> = (0..N).map(|i| {
        let n = node_by_key(&rt, &format!("val_{i}"));
        (n.id, n.epoch)
    }).collect();
    let layouts_before = rt.core().text.cached_layout_count();
    let elisions_before = rt.core().text.cached_elision_count();

    // Commit with unrelated sibling change and unchanged LiveText values:
    // memo must be flatten-reused, zero texts shaped, epochs preserved, static tags copied.
    let commits0 = rt.stats.commits;
    rt.update(|s, _| s.tick += 1);
    rt.frame(dt);
    assert_eq!(rt.stats.commits, commits0 + 1);
    assert_eq!(rt.stats.shaped, 0, "reused memo with unchanged LiveText must shape 0 texts on commit");
    assert_eq!(rt.core().text.cached_layout_count(), layouts_before);
    assert_eq!(rt.core().text.cached_elision_count(), elisions_before);
    for i in 0..N {
        let tag = node_by_key(&rt, &format!("tag_{i}"));
        assert_eq!((tag.id, tag.epoch), tag_epochs[i], "static tag_{i} must keep its node id and paint epoch");
        let val = node_by_key(&rt, &format!("val_{i}"));
        assert_eq!((val.id, val.epoch), val_epochs[i], "unchanged live val_{i} must keep its node id and paint epoch");
        assert_eq!(
            val.text.as_ref().unwrap().content,
            format!("{:6.2} bar", 10.0 + i as f64),
            "latched LiveText must not revert to build-time placeholder",
        );
    }
    assert!(
        !rt.core().commit.memo.flattened.is_empty(),
        "memo holding LiveText must stay recorded as flatten-reusable across commits",
    );

    // Commit with unrelated sibling change AND all N LiveText values updated in the same frame:
    // commit keeps the latched layouts (0 shapes in commit); latch_live observes the new mask
    // ("000.00 bar") once, verifies it once (1 + 11 probes), and fast-paths the remaining cells
    // with 0 shapes (13 total, never double-shaped by commit), without growing commit caches.
    for (i, c) in handles.iter().enumerate() {
        c.set(format!("{:6.2} bar", 200.0 + i as f64));
    }
    rt.update(|s, _| s.tick += 1);
    rt.frame(dt);
    assert_eq!(rt.stats.commits, commits0 + 2);
    assert_eq!(rt.stats.shaped, 13, "new tabular mask verifies once in latch_live (never double-shaped in commit)");
    // Next commit updating all N cells within the now-verified mask shapes 0 times.
    for (i, c) in handles.iter().enumerate() {
        c.set(format!("{:6.2} bar", 210.0 + i as f64));
    }
    rt.update(|s, _| s.tick += 1);
    rt.frame(dt);
    assert_eq!(rt.stats.commits, commits0 + 3);
    assert_eq!(rt.stats.shaped, 0, "verified tabular LiveText cells shape 0 times during a commit");
    assert_eq!(rt.core().text.cached_layout_count(), layouts_before, "LiveText ticks during commit must not grow layout cache");
    assert_eq!(rt.core().text.cached_elision_count(), elisions_before, "LiveText ticks during commit must not grow elision cache");
    for (i, &expected_tag) in tag_epochs.iter().enumerate() {
        let tag = node_by_key(&rt, &format!("tag_{i}"));
        assert_eq!((tag.id, tag.epoch), expected_tag, "static tag_{i} must stay reused when LiveText ticks");
        let val = node_by_key(&rt, &format!("val_{i}"));
        assert_eq!(val.text.as_ref().unwrap().content, format!("{:6.2} bar", 210.0 + i as f64));
    }
    assert!(
        !rt.core().commit.memo.flattened.is_empty(),
        "memo holding LiveText must stay flatten-reusable when LiveText values update during a commit",
    );

    // Accessibility tree exposes the newest latched values.
    let a11y = rt.accessibility_tree();
    let ak_val_0 = a11y
        .nodes
        .iter()
        .find(|(_, n)| n.author_id() == Some("val_0"))
        .and_then(|(_, n)| n.value().or_else(|| n.label()));
    assert_eq!(ak_val_0, Some("210.00 bar"));
}

#[test]
fn memo_holding_wrapped_truncated_clamped_live_text_reuses_and_relayouts_on_width_and_path_change() {
    struct State {
        wrapped: LiveText,
        trunc: LiveText,
        clamped: LiveText,
        box_w: f32,
        prefix: bool,
        tick: u64,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let panel = memo("panel", 1u32, || {
            col()
                .key("panel")
                .w_full()
                .gap(4.0)
                .child(text("init").key("wrapped").w_full().h(60.0).wrap().live_text(&s.wrapped))
                .child(text("init").key("trunc").w_full().h(20.0).truncate().live_text(&s.trunc))
                .child(text("init").key("clamped").w_full().h(44.0).max_lines(2).live_text(&s.clamped))
        });
        let mut box_col = col().w(s.box_w).h(300.0);
        if s.prefix {
            box_col = box_col.child(el().key("banner").size(10.0, 10.0));
        }
        col().w_full().h_full().child(box_col.child(el().key(format!("t{}", s.tick)).size(6.0, 6.0)).child(panel))
    }

    let w_long = "A long wrapped live text that breaks across multiple lines inside ninety points";
    let t_long = "A very long single-line live ticker that overflows ninety points";
    let c_long = "Line one and line two and line three and line four overflowing two lines";
    let state = State {
        wrapped: LiveText::new(w_long),
        trunc: LiveText::new(t_long),
        clamped: LiveText::new(c_long),
        box_w: 90.0,
        prefix: false,
        tick: 0,
    };
    let mut rt = Runtime::new(state, view, (500.0, 300.0), Env::new(true, 1.0));
    let dt = 1.0 / 60.0;
    rt.frame(0.0);

    fn find<'a>(rt: &'a Runtime<State>, k: &str) -> &'a crate::runtime::types::Node {
        rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some(k)).unwrap()
    }
    let lines_of = |rt: &Runtime<State>, k: &str| {
        find(rt, k).text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap().len()
    };
    assert!(lines_of(&rt, "wrapped") > 1);
    assert!(find(&rt, "trunc").elided.is_some());
    assert_eq!(lines_of(&rt, "clamped"), 2);
    assert!(find(&rt, "clamped").elided.is_some());

    let layouts_before = rt.core().text.cached_layout_count();
    let elisions_before = rt.core().text.cached_elision_count();

    // Unrelated commit at the same width: 0 shapes, wrapped/truncated/clamped layouts preserved.
    rt.update(|s, _| s.tick += 1);
    rt.frame(dt);
    assert_eq!(rt.stats.shaped, 0, "same-width commit must not reshape wrapped/truncated/clamped LiveText");
    assert!(lines_of(&rt, "wrapped") > 1);
    assert!(find(&rt, "trunc").elided.is_some());
    assert!(find(&rt, "clamped").elided.is_some());
    assert_eq!(rt.core().text.cached_layout_count(), layouts_before);
    assert_eq!(rt.core().text.cached_elision_count(), elisions_before);

    // Rebased (!same_path) commit at the same width: inserting a prefix sibling shifts the memo path,
    // keeps latched text intact with 0 reshapes, and later LiveText::set updates immediately.
    rt.update(|s, _| { s.prefix = true; s.tick += 1; });
    rt.frame(dt);
    assert_eq!(rt.stats.shaped, 0, "rebased same-width memo reuse must not reshape LiveText");
    assert_eq!(find(&rt, "wrapped").text.as_ref().unwrap().content, w_long);
    assert!(find(&rt, "trunc").elided.is_some());

    // Widening the parent container during a commit must re-lay out wrapped/truncated/clamped LiveText
    // at the new width without growing the shared commit caches.
    rt.update(|s, _| { s.box_w = 500.0; s.tick += 1; });
    rt.frame(dt);
    assert_eq!(lines_of(&rt, "wrapped"), 1, "widening container must unwrap reused LiveText");
    assert!(find(&rt, "trunc").elided.is_none(), "widening container must clear truncation on reused LiveText");
    assert!(find(&rt, "clamped").elided.is_none(), "widening container must clear clamp elision on reused LiveText");
    assert_eq!(rt.core().text.cached_layout_count(), layouts_before);
    assert_eq!(rt.core().text.cached_elision_count(), elisions_before);

    // Subsequent LiveText update after rebase + resize still lands on that very frame.
    rt.state.trunc.set("updated after rebase");
    rt.frame(dt);
    assert_eq!(find(&rt, "trunc").text.as_ref().unwrap().content, "updated after rebase");
}
