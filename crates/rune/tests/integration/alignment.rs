//! Flex alignment builders place children where CSS would.
use rune::{Cx, El, Env, Runtime, col, el, row};

fn rect(rt: &Runtime<u8>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().target
}

fn view(case: &u8, _: &mut Cx<u8>) -> El {
    let kids = || [el().key("a").size(20.0, 10.0), el().key("b").size(20.0, 30.0)];
    let parent = row().key("parent").size(200.0, 60.0);
    match case {
        0 => parent.items_end().children(kids()),
        1 => parent.items_center().children(kids()),
        2 => parent.children([el().key("a").size(20.0, 10.0).self_end(), el().key("b").w(20.0).self_stretch()]),
        3 => parent.justify_center().children(kids()),
        4 => parent.justify_around().children(kids()),
        5 => parent.justify_evenly().children(kids()),
        6 => col().key("parent").size(200.0, 60.0).items_end().justify_center().justify_start().children(kids()),
        _ => unreachable!(),
    }
}

#[test]
fn alignment_builders_match_css() {
    let expect: [(&str, [f32; 2], [f32; 2]); 7] = [
        ("items_end", [0.0, 50.0], [20.0, 30.0]),
        ("items_center", [0.0, 25.0], [20.0, 15.0]),
        ("self_end / self_stretch", [0.0, 50.0], [20.0, 0.0]),
        ("justify_center", [80.0, 0.0], [100.0, 0.0]),
        ("justify_around", [40.0, 0.0], [140.0, 0.0]),
        ("justify_evenly", [53.333, 0.0], [126.667, 0.0]),
        ("column items_end + justify_start", [180.0, 0.0], [180.0, 10.0]),
    ];
    for (case, (name, a, b)) in expect.into_iter().enumerate() {
        let mut rt = Runtime::new(case as u8, view, (200.0, 60.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for (key, at) in [("a", a), ("b", b)] {
            let r = rect(&rt, key);
            assert!((r[0] - at[0]).abs() < 0.01 && (r[1] - at[1]).abs() < 0.01, "{name}: {key} at {r:?}, want {at:?}");
        }
        if case == 2 { assert_eq!(rect(&rt, "b")[3], 60.0, "self_stretch fills the cross axis"); }
    }
}

#[test]
fn non_stretched_items_in_column_clamp_to_fit_content_width() {
    use rune::text;
    const SHORT: &str = "Short label";
    const LONG: &str = "This sentence is much longer than two hundred and forty points so it must wrap across multiple lines.";
    const UNBROKEN: &str = "SupercalifragilisticexpialidociousSupercalifragilisticexpialidocious tail";

    fn fit_view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .gap(4.0)
            .child(col().w(240.0).items_start().child(text(SHORT).key("short_leaf").wrap()))
            .child(col().w(240.0).items_start().child(text(LONG).key("start_leaf").wrap()))
            .child(col().w(240.0).items_center().child(text(LONG).key("center_leaf").wrap()))
            .child(col().w(240.0).items_end().child(text(LONG).key("end_leaf").wrap()))
            .child(col().w(240.0).child(text(LONG).key("self_start_leaf").wrap().self_start()))
            .child(col().w(240.0).items_start().child(row().key("start_row").child(text(LONG).key("start_row_text").wrap().shrink())))
            .child(col().w(240.0).items_center().child(row().key("center_row").child(text(LONG).key("center_row_text").wrap().shrink())))
            .child(col().w(240.0).items_end().child(row().key("end_row").child(text(LONG).key("end_row_text").wrap().shrink())))
            .child(col().w(240.0).child(row().key("self_start_row").self_start().child(text(LONG).key("self_start_row_text").wrap().shrink())))
            .child(col().w(240.0).items_start().child(row().key("short_row").child(text(SHORT).wrap().shrink())))
            .child(col().w(240.0).items_start().child(row().key("wrap_row").flex_wrap().child(text(LONG).key("wrap_row_text").wrap().shrink())))
            .child(col().w(240.0).items_start().child(row().key("explicit_row").w(320.0).child(text(LONG).wrap().shrink())))
            .child(col().w(240.0).items_start().child(row().key("unbroken_row").child(text(UNBROKEN).wrap().shrink())))
            .child(col().w(240.0).items_start().child(col().key("nested_col").child(row().key("nested_row").child(text(LONG).key("nested_text").wrap().shrink()))))
            .child(row().h(60.0).items_start().child(col().key("mirrored_col").child(el().w(40.0).h(120.0).shrink().min_h(0.0))))
    }

    let mut rt = Runtime::new((), fit_view, (800.0, 1200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r = |key: &str| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().target
    };
    let one_line = r("short_leaf")[3];

    for key in ["start_leaf", "center_leaf", "end_leaf", "self_start_leaf"] {
        let b = r(key);
        assert!(b[2] <= 240.5, "{key} overflows 240: {b:?}");
        assert!(b[3] >= one_line * 2.0 - 0.5, "{key} did not wrap: {b:?}");
    }
    for (row_k, txt_k) in [
        ("start_row", "start_row_text"),
        ("center_row", "center_row_text"),
        ("end_row", "end_row_text"),
        ("self_start_row", "self_start_row_text"),
        ("nested_col", "nested_text"),
    ] {
        let rb = r(row_k);
        let tb = r(txt_k);
        assert!(rb[0] >= -0.5 && rb[0] + rb[2] <= 240.5, "{row_k} overflows 240: {rb:?}");
        assert!(tb[2] <= 240.5, "{txt_k} overflows 240: {tb:?}");
        assert!(tb[3] >= one_line * 2.0 - 0.5, "{txt_k} did not wrap: {tb:?}");
        assert!(rb[3] >= tb[3] - 0.5, "{row_k} did not grow for wrapped text: {rb:?} vs {tb:?}");
    }
    let short_row = r("short_row");
    assert!(short_row[2] < 200.0, "short auto-width row stays compact: {short_row:?}");
    let wrap_row = r("wrap_row");
    let wrap_row_text = r("wrap_row_text");
    assert!(wrap_row[2] <= 240.5, "flex_wrap row clamps to 240: {wrap_row:?}");
    assert!(wrap_row_text[3] >= one_line * 2.0 - 0.5, "flex_wrap row wraps child: {wrap_row_text:?}");
    let explicit_row = r("explicit_row");
    assert!((explicit_row[2] - 320.0).abs() < 0.5, "explicit width is preserved: {explicit_row:?}");
    let unbroken = r("unbroken_row");
    assert!(unbroken[2] > 240.0, "unbroken min-content wider than container floors fit-content: {unbroken:?}");
    assert!(unbroken[3] >= one_line * 2.0 - 0.5, "tail word after unbroken word wraps onto second line: {unbroken:?}");
    let mirrored = r("mirrored_col");
    assert!((mirrored[3] - 120.0).abs() < 0.5, "auto-height column in fixed-height row keeps content height: {mirrored:?}");
}
