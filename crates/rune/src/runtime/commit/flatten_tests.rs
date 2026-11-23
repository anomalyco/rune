use super::*;
use crate::{Align, Color, Direction, Env, GrowthSpec, Pixels, Side, TextEdit, TextStyle, col, el, memo, row, text, text_input};

fn sample_pixels() -> Pixels {
    let px = Pixels::new();
    px.set(4, 2, vec![255; 4 * 2 * 4]);
    px
}

fn sample_tree(edit: &TextEdit, px: &Pixels) -> El {
    let hover = Anim::from(Spring::new(0.2, 0.0));
    col().key("root").size(320.0, 240.0)
        .trace_namespace("test", "flatten")
        .hover_motion_within(hover, hover)
        .child(
            col().key("stack").w_full().growth(GrowthSpec::default())
                .child(el().key("row-snap").w_full().h(20.0).child(text("snap")))
                .child(
                    el().key("row-anim").w_full().animate_layout()
                        .child(text("anim"))
                        .child(el().key("pop").size(30.0, 12.0).anchored("row-snap", Side::Left, Align::Start, 4.0)),
                ),
        )
        .child(
            row().key("rtl").dir(Direction::Rtl).instant_changes().scale(1.25).time_scale(0.5)
                .border(1.0, Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }).border_layout().corners([2.0, 4.0, 6.0, 8.0])
                .child(el().key("anchor-target").size(40.0, 20.0).animated_changes().matched("m1").help("tip"))
                .child(el().key("rtl-pop").size(24.0, 12.0).anchored("anchor-target", Side::Left, Align::Start, 6.0))
                .child(el().key("inert-box").inert().child(text("muted"))),
        )
        .child(el().key("pic-styled").w(40.0).max_w(30.0).pixels(px))
        .child(el().key("pic-measured").w_full().max_w(80.0).pixels(px))
        .child(el().key("fit").w(100.0).h(80.0).aspect_fit(2.0))
        .child(el().key("sx-one").w(120.0).h(30.0).scroll_x().child(el().key("only").w_full().h(20.0)))
        .child(
            el().key("sx-many").w(120.0).h(30.0).scroll_x()
                .child(el().key("a").w_full().h(20.0))
                .child(el().key("b").w(50.0).h(20.0)),
        )
        .child(text_input(edit, TextStyle::new(14.0, 400.0)).key("field").w(100.0).h(24.0))
}

fn run_flatten<E: FlattenEl>(el: E, foreground: Option<Color>) -> (Vec<Node>, LayoutEngine) {
    let mut taffy = LayoutEngine::with_capacity(32);
    taffy.begin_commit();
    let mut nodes = Vec::new();
    let mut path_buf = String::new();
    let mut old_nodes = Vec::new();
    let old_index = NodeIndex::default();
    let committed = HashMap::default();
    let mut path_ids = HashMap::default();
    let mut next_id = 1u64;
    let mut memo_ctx = memo_reuse::MemoFlattenCtx {
        old_nodes: &mut old_nodes,
        old_present_len: 0,
        old_node_index: &old_index,
        old_cursor: 0,
        committing: true,
        committed: &committed,
        pending: Vec::new(),
        reused: Vec::new(),
        taken_paths: Vec::new(),
        path_ids: &mut path_ids,
        next_id: &mut next_id,
        foreground,
        missed_virtual_memo: false,
    };
    let _ = flatten_node(el, None, &mut path_buf, 0, crate::tok::motion::DEFAULT, RowLayout::Free, true, None, &mut taffy, &mut nodes, 2.0, &mut memo_ctx);
    taffy.finish_flatten();
    (nodes, taffy)
}

fn assert_nodes_match(owned: &[Node], borrowed: &[Node], taffy_owned: Option<&LayoutEngine>, taffy_borrowed: Option<&LayoutEngine>) {
    assert_eq!(owned.len(), borrowed.len(), "node count");
    for (i, (a, b)) in owned.iter().zip(borrowed.iter()).enumerate() {
        assert_eq!(a.id, b.id, "node {i} id");
        assert_eq!(a.path, b.path, "node {i} path");
        assert_eq!(a.key, b.key, "node {i} key");
        assert_eq!(a.role, b.role, "node {i} role");
        assert_eq!(a.parent, b.parent, "node {i} parent");
        assert_eq!(a.text.as_ref().map(|t| (&t.content, t.color, t.dir, t.shape)), b.text.as_ref().map(|t| (&t.content, t.color, t.dir, t.shape)), "node {i} text");
        assert_eq!(a.input.as_ref().map(|x| (x.placeholder_color, x.caret, x.selection)), b.input.as_ref().map(|x| (x.placeholder_color, x.caret, x.selection)), "node {i} input");
        assert_eq!(a.vis.radii(), b.vis.radii(), "node {i} radii");
        assert_eq!(a.vis.border_width(), b.vis.border_width(), "node {i} border_width");
        assert_eq!(a.vis_flags, b.vis_flags, "node {i} vis_flags");
        assert_eq!(a.hover_within.as_deref(), b.hover_within.as_deref(), "node {i} hover_within");
        assert_eq!(a.motion, b.motion, "node {i} motion");
        assert_eq!(a.static_scale, b.static_scale, "node {i} static_scale");
        assert_eq!(a.time_scale, b.time_scale, "node {i} time_scale");
        assert_eq!(a.in_row, b.in_row, "node {i} in_row");
        assert_eq!(a.width_leads, b.width_leads, "node {i} width_leads");
        assert_eq!(a.instant_changes, b.instant_changes, "node {i} instant_changes");
        assert_eq!(a.animated_changes, b.animated_changes, "node {i} animated_changes");
        assert_eq!(a.streaming, b.streaming, "node {i} streaming");
        assert_eq!(a.whole, b.whole, "node {i} whole");
        assert_eq!(a.inert, b.inert, "node {i} inert");
        assert_eq!(a.flow, b.flow, "node {i} flow");
        assert_eq!(a.directed, b.directed, "node {i} directed");
        assert_eq!(a.pin, b.pin, "node {i} pin");
        assert_eq!(a.ime_option_as_alt, b.ime_option_as_alt, "node {i} ime_option_as_alt");
        assert_eq!(a.cold.is_empty(), b.cold.is_empty(), "node {i} cold empty");
        assert_eq!(a.trace_namespace, b.trace_namespace, "node {i} trace_namespace");
        assert_eq!(a.above.as_ref().map(|x| (&x.key, x.side, x.align, x.gap)), b.above.as_ref().map(|x| (&x.key, x.side, x.align, x.gap)), "node {i} above");
        assert_eq!(a.matched, b.matched, "node {i} matched");
        assert_eq!(a.pixel_aspect, b.pixel_aspect, "node {i} pixel_aspect");
        assert_eq!(a.aspect_fit.as_ref().map(|x| (x.0, &x.1)), b.aspect_fit.as_ref().map(|x| (x.0, &x.1)), "node {i} aspect_fit");
        assert_eq!(a.help, b.help, "node {i} help");
        assert_eq!(a.scroll_axes, b.scroll_axes, "node {i} scroll_axes");
        if let (Some(ta), Some(tb)) = (taffy_owned, taffy_borrowed) {
            assert_eq!(ta.style(a.taffy), tb.style(b.taffy), "node {i} style");
        } else {
            assert_eq!(a.local, b.local, "node {i} local");
            assert_eq!(a.target, b.target, "node {i} target");
        }
    }
}

#[test]
fn owned_and_borrowed_flatten_produce_identical_nodes() {
    let edit = TextEdit::new("hello");
    let px = sample_pixels();
    let tree = sample_tree(&edit, &px);
    let fg = Some(Color { r: 0.2, g: 0.4, b: 0.6, a: 1.0 });
    let (borrowed, taffy_borrowed) = run_flatten(&tree, fg);
    let (owned, taffy_owned) = run_flatten(tree, fg);
    assert_nodes_match(&owned, &borrowed, Some(&taffy_owned), Some(&taffy_borrowed));
    // Verify RTL actually mirrored the anchor on both paths.
    let pop = owned.iter().find(|n| n.key.as_deref() == Some("rtl-pop")).unwrap();
    assert_eq!(pop.above.as_ref().unwrap().side, Side::Right);
    assert_eq!(pop.above.as_ref().unwrap().align, Align::Start);
}

#[test]
fn memo_children_and_plain_children_flatten_to_matching_node_tables() {
    struct S {
        edit: TextEdit,
        px: Pixels,
        use_memo: bool,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        cx.foreground(Color { r: 0.2, g: 0.4, b: 0.6, a: 1.0 });
        if s.use_memo {
            let extra = el().key("tail").size(20.0, 10.0).child(text("extra"));
            memo("root", 1u32, || sample_tree(&s.edit, &s.px)).child(extra)
        } else {
            sample_tree(&s.edit, &s.px).child(el().key("tail").size(20.0, 10.0).child(text("extra")))
        }
    }
    let env = Env::new(true, 2.0);
    let mut plain = Runtime::new(S { edit: TextEdit::new("hello"), px: sample_pixels(), use_memo: false }, view, (320.0, 240.0), env);
    let mut memoed = Runtime::new(S { edit: TextEdit::new("hello"), px: sample_pixels(), use_memo: true }, view, (320.0, 240.0), env);
    plain.frame(0.0);
    memoed.frame(0.0);
    assert_nodes_match(&plain.tree.nodes, &memoed.tree.nodes, None, None);
}

#[test]
fn cold_packing_handles_el_only_cold_meta_only_and_shared_clones() {
    let px = sample_pixels();
    let own_spring = Spring::new(0.15, 0.0);
    let tree = col().key("root").size(200.0, 120.0)
        // Plain node: empty ElCold, no metadata -> empty NodeCold.
        .child(el().key("plain").size(20.0, 20.0))
        // Non-empty ElCold containing only fields that do not live in NodeCold -> still empty NodeCold.
        .child(el().key("el-only").size(20.0, 20.0).motion(own_spring).animated_changes().time_scale(0.5).focusable(true).ime_option_as_alt(true))
        // Empty ElCold with pixel_aspect metadata only -> packs NodeCold from metadata alone.
        .child(el().key("pic-only").w(40.0).pixels(&px))
        // Matched element with its own motion override -> preserves (id, Some(own_spring)).
        .child(el().key("matched-motion").size(20.0, 20.0).motion(own_spring).matched("card").label("Card").help("Open card"));

    let cloned = tree.clone();
    let (borrowed, taffy_borrowed) = run_flatten(&tree, None);
    let (owned_shared, taffy_shared) = run_flatten(cloned, None);
    let (owned_unique, taffy_unique) = run_flatten(tree, None);

    assert_nodes_match(&owned_shared, &borrowed, Some(&taffy_shared), Some(&taffy_borrowed));
    assert_nodes_match(&owned_unique, &borrowed, Some(&taffy_unique), Some(&taffy_borrowed));

    fn by_key<'a>(nodes: &'a [Node], k: &str) -> &'a Node {
        nodes.iter().find(|n| n.key.as_deref() == Some(k)).unwrap()
    }
    assert!(by_key(&owned_unique, "plain").cold.is_empty());
    assert!(by_key(&owned_unique, "el-only").cold.is_empty());
    assert_eq!(by_key(&owned_unique, "el-only").motion, own_spring);
    assert!(by_key(&owned_unique, "el-only").animated_changes);
    assert_eq!(by_key(&owned_unique, "el-only").time_scale, 0.5);
    assert!(by_key(&owned_unique, "el-only").ime_option_as_alt);
    assert!(!by_key(&owned_unique, "pic-only").cold.is_empty());
    assert_eq!(by_key(&owned_unique, "pic-only").pixel_aspect, Some(2.0));
    let matched = by_key(&owned_unique, "matched-motion");
    assert_eq!(matched.matched.as_ref().map(|(id, m)| (&**id, *m)), Some(("card", Some(own_spring))));
    assert_eq!(matched.label.as_deref(), Some("Card"));
    assert_eq!(matched.help.as_deref(), Some("Open card"));
}

#[test]
fn push_usize_formats_single_and_multi_digit_indices() {
    for n in [0usize, 1, 9, 10, 42, 999, 1_234_567, usize::MAX] {
        let mut buf = String::from("root/#");
        push_usize(&mut buf, n);
        assert_eq!(buf, format!("root/#{n}"));
    }
}

#[test]
fn path_segment_encoding_is_injective_and_preserves_unambiguous_paths() {
    use super::path::*;

    // Unambiguous keys stay verbatim.
    for plain in ["root", "ancestor:a", "row:R-94#dup1", "width:50%", "C:\\dir\\file", "#header", "#fff", "#"] {
        assert_eq!(node_path("#0", Some(plain), 0), format!("#0/{plain}"));
        assert_eq!(decode_key_segment(plain).as_deref(), Some(plain));
        assert!(segment_matches_key(plain, plain));
    }

    // Distinct keys and positional indices never collide, and round-trip through decode.
    let tricky = [
        "", "#0", "#12", "#!", "#!foo", "#!a%2Fb", "a/b", "a/b/c",
        "ancestor:a/#0", "ancestor:a%2F#0", "#!ancestor:a%2F#0", "a/b%25c", "café/50%/%2F",
    ];
    let mut encoded = std::collections::HashSet::new();
    for &k in &tricky {
        let p = node_path("#0", Some(k), 0);
        let mut buf = String::from("#0");
        push_segment(&mut buf, Some(k), 0);
        assert_eq!(p, buf, "node_path and push_segment agree for {k:?}");
        let seg = p.strip_prefix("#0/").unwrap();
        assert!(!seg.is_empty() && !seg.contains('/'), "encoded segment {seg:?} for {k:?}");
        assert!(!is_unkeyed_segment(seg), "escaped key {seg:?} must not read as unkeyed");
        assert!(encoded.insert(seg.to_owned()), "collision for {k:?} -> {seg:?}");
        assert_eq!(decode_key_segment(seg).as_deref(), Some(k), "round-trip for {k:?}");
        assert!(segment_matches_key(seg, k));
        assert!(in_scope(&p, Some(k)));
    }
    assert!(encoded.insert("#0".to_owned()), "unkeyed #0 must not collide with any escaped key");
    assert_eq!(decode_key_segment("#0"), None);

    // Child path vs sibling key containing '/'.
    let child_path = node_path(&node_path("#0", Some("ancestor:a"), 0), None, 0);
    let sibling_path = node_path("#0", Some("ancestor:a/#0"), 1);
    assert_eq!(child_path, "#0/ancestor:a/#0");
    assert_ne!(child_path, sibling_path);

    // Keyed suffix matching across unkeyed nodes and escaped '/' keys.
    let row_field = node_path(&node_path("#0/viewport/rows", Some("row/0"), 0), Some("field"), 0);
    assert!(keyed_suffix_matches(&row_field, "row/0/field"));
    assert!(keyed_suffix_matches(&row_field, "rows/row/0/field"));
    assert!(crate::flight::selects("row/0/field", &row_field, Some("field")));
    assert!(!keyed_suffix_matches(&row_field, "viewport/field"));
}
