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

#[test]
fn memo_live_text_keeps_reserved_width_across_window_resize_and_memo_reuse() {
    struct State {
        value: LiveText,
        zoom: LiveText,
        elapsed: LiveText,
        status: LiveText,
        wrapped_auto: LiveText,
        cased: LiveText,
        box_w: f32,
        prefix: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut box_col = col().w(s.box_w).gap(4.0);
        if s.prefix {
            box_col = box_col.child(el().key("prefix").size(8.0, 8.0));
        }
        box_col = box_col.child(memo("toolbar", (), || {
            row()
                .key("toolbar")
                .gap(4.0)
                .child(el().size(12.0, 12.0))
                .child(text("400%").key("zoom").live_text(&s.zoom))
                .child(el().key("after_zoom").size(12.0, 12.0))
        }));
        col()
            .w_full()
            .h_full()
            .gap(4.0)
            .child(memo("label", (), || text("x").self_start().live_text(&s.value)))
            .child(text("0:00:00.0").key("elapsed").self_start().live_text(&s.elapsed))
            .child(memo("status", (), || {
                text("Compiling…").truncate().self_start().live_text(&s.status)
            }))
            .child(memo("wrapped_auto", (), || {
                text("Two line placeholder").w_full().wrap().live_text(&s.wrapped_auto)
            }))
            .child(memo("cased", (), || {
                text("abc").self_start().text_case(crate::TextCase::Uppercase).live_text(&s.cased)
            }))
            .child(box_col)
    }

    let make_state = |value: &str, zoom: &str, elapsed: &str, status: &str, wrapped_auto: &str, cased: &str| State {
        value: LiveText::new(value),
        zoom: LiveText::new(zoom),
        elapsed: LiveText::new(elapsed),
        status: LiveText::new(status),
        wrapped_auto: LiveText::new(wrapped_auto),
        cased: LiveText::new(cased),
        box_w: 300.0,
        prefix: false,
    };
    fn find<'a>(rt: &'a Runtime<State>, k: &str) -> &'a crate::runtime::types::Node {
        rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some(k)).unwrap()
    }

    let mut rt = Runtime::new(
        make_state("x", "75%", "0:00:00.0", "Compiling…", "Two line placeholder", "abc"),
        view,
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    let dt = 1.0 / 60.0;
    rt.frame(0.0);

    let label_w0 = find(&rt, "label").local.w;
    let zoom_w0 = find(&rt, "zoom").local.w;
    let after_zoom_x0 = find(&rt, "after_zoom").local.x;
    let elapsed_w0 = find(&rt, "elapsed").local.w;
    let status_w0 = find(&rt, "status").local.w;
    let cased_w0 = find(&rt, "cased").local.w;

    // Update all LiveText handles to much longer or shorter strings.
    rt.state.value.set("A much longer live label");
    rt.state.elapsed.set("9:58.2");
    rt.state.status.set("Compiling shader_cache.rs");
    rt.state.wrapped_auto.set("Line one line two line three line four line five line six line seven line eight line nine");
    rt.state.cased.set("a much longer cased string");
    rt.frame(dt);

    assert_eq!(find(&rt, "label").local.w, label_w0, "live update must keep reserved label width");
    assert_eq!(find(&rt, "label").text.as_ref().unwrap().content, "A much longer live label");
    assert_eq!(find(&rt, "elapsed").local.w, elapsed_w0, "live update must keep reserved elapsed width");
    assert_eq!(find(&rt, "status").local.w, status_w0, "live update must keep reserved status width");
    assert!(find(&rt, "status").elided.is_some(), "longer live status must elide inside reserved width");

    // Invalidate so Rune runs a Rebuild commit that reuses the memos.
    let layouts_before = rt.core().text.cached_layout_count();
    rt.invalidate();
    rt.frame(dt);
    assert_eq!(find(&rt, "label").local.w, label_w0, "memo reuse must keep reserved label width");

    // Resize the viewport (CommitKind::Relayout): every LiveText must keep its authored placeholder size.
    rt.set_viewport((530.0, 400.0), 1.0);
    rt.frame(dt);
    assert_eq!(find(&rt, "label").local.w, label_w0, "viewport resize must keep memoized LiveText reserved width");
    assert_eq!(find(&rt, "label").text.as_ref().unwrap().content, "A much longer live label");
    assert_eq!(find(&rt, "zoom").local.w, zoom_w0, "viewport resize must not shrink LiveText to shorter live value");
    assert_eq!(find(&rt, "after_zoom").local.x, after_zoom_x0, "siblings after LiveText must not shift on resize");
    assert_eq!(find(&rt, "elapsed").local.w, elapsed_w0, "viewport resize must keep non-memoized LiveText reserved width");
    assert_eq!(find(&rt, "status").local.w, status_w0, "viewport resize must keep truncated LiveText reserved width");
    assert!(find(&rt, "status").elided.is_some(), "truncated LiveText must stay elided after resize");
    assert_eq!(find(&rt, "cased").local.w, cased_w0, "viewport resize must keep cased LiveText reserved width");
    assert_eq!(find(&rt, "cased").text.as_ref().unwrap().content, "A MUCH LONGER CASED STRING");
    assert_eq!(rt.core().text.cached_layout_count(), layouts_before, "resize with LiveText must not grow shared layout cache");

    // Narrow viewport below reserved truncated width, then widen back: status must clamp while narrow
    // and return to the placeholder's reserved width (not the longer live string's width) when widened.
    rt.set_viewport((30.0, 400.0), 1.0);
    rt.frame(dt);
    assert!(find(&rt, "status").local.w <= 30.0 + 0.01, "truncated LiveText clamps to narrow viewport");
    assert!(find(&rt, "status").elided.is_some());

    // Compare wrapped_auto height at narrow width against a fresh runtime with the same placeholder.
    let mut fresh_narrow = Runtime::new(
        make_state("A much longer live label", "75%", "9:58.2", "Compiling shader_cache.rs", "Line one line two line three line four line five line six line seven line eight line nine", "a much longer cased string"),
        view,
        (30.0, 400.0),
        Env::new(true, 1.0),
    );
    fresh_narrow.frame(0.0);
    assert_eq!(
        find(&rt, "wrapped_auto").local.h,
        find(&fresh_narrow, "wrapped_auto").local.h,
        "wrapped LiveText height after resize must match placeholder height, not live value height",
    );

    rt.set_viewport((530.0, 400.0), 1.0);
    rt.frame(dt);
    assert_eq!(find(&rt, "status").local.w, status_w0, "widening viewport must restore placeholder reserved width");
    assert_eq!(find(&rt, "label").local.w, label_w0);

    // Rebuild commit changing an outer container width + rebasing memo path (!same_path):
    // reused memo must still measure the placeholder width of zoom ("400%"), not "75%".
    rt.update(|s, _| {
        s.box_w = 220.0;
        s.prefix = true;
    });
    rt.frame(dt);
    assert_eq!(find(&rt, "zoom").local.w, zoom_w0, "rebased memo reuse with container resize must keep reserved width");
    assert_eq!(find(&rt, "after_zoom").local.x, after_zoom_x0);

    // Scale change rebuilds and re-measures placeholder at the new scale while preserving live content.
    rt.set_viewport((530.0, 400.0), 2.0);
    rt.frame(dt);
    let mut fresh_2x = Runtime::new(
        make_state("A much longer live label", "75%", "9:58.2", "Compiling shader_cache.rs", "Line one", "a much longer cased string"),
        view,
        (530.0, 400.0),
        Env::new(true, 2.0),
    );
    fresh_2x.frame(0.0);
    assert_eq!(find(&rt, "label").local.w, find(&fresh_2x, "label").local.w, "scale change must keep placeholder width at 2x");
    assert_eq!(find(&rt, "label").text.as_ref().unwrap().content, "A much longer live label");
}

#[test]
fn memo_single_line_live_text_resize_keeps_reserved_width_with_zero_reshapes() {
    fn view(value: &LiveText, _: &mut Cx<LiveText>) -> El {
        col()
            .w_full()
            .h_full()
            .child(memo("label", (), || text("x").self_start().live_text(value)))
    }
    let width = |rt: &Runtime<LiveText>| {
        rt.core()
            .tree
            .nodes
            .iter()
            .find(|n| n.key.as_deref() == Some("label"))
            .unwrap()
            .local
            .w
    };
    let mut rt = Runtime::new(LiveText::new("x"), view, (600.0, 400.0), Env::new(true, 1.0));
    let dt = 1.0 / 60.0;
    rt.frame(0.0);
    let w0 = width(&rt);

    rt.state.set("A much longer live label");
    rt.frame(dt);
    assert_eq!(width(&rt), w0);

    let layouts_before = rt.core().text.cached_layout_count();
    rt.invalidate();
    rt.frame(dt);
    assert_eq!(width(&rt), w0);
    assert_eq!(rt.stats.shaped, 0, "memo reuse must shape 0 times");

    rt.set_viewport((530.0, 400.0), 1.0);
    rt.frame(dt);
    assert_eq!(width(&rt), w0, "window resize must preserve reserved width of placeholder");
    assert_eq!(rt.stats.shaped, 0, "window resize of single-line LiveText must shape 0 times");
    assert_eq!(rt.core().text.cached_layout_count(), layouts_before);
}
