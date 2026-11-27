//! Flex alignment builders place children where CSS would.
use rune::{Cx, El, Env, Runtime, col, el, row};

fn rect<S: 'static>(rt: &Runtime<S>, key: &str) -> [f32; 4] {
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

#[test]
fn w_fit_hugs_content_without_fixing_alignment() {
    use rune::text;
    const LONG: &str = "This sentence is much longer than two hundred points so it must wrap across multiple lines.";

    fn pill(key: &str) -> El {
        row().key(key).w_fit().px(10.0).h(24.0).child(el().size(60.0, 24.0))
    }

    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut mut_w_fit = row().key("in_place").w_full().h(24.0).child(el().size(50.0, 24.0));
        mut_w_fit.set_w_fit();

        col().w(300.0).gap(4.0)
            // Inside a default stretching column: w_fit hugs content (60 + 20 = 80), while a
            // default sibling still stretches to 300.
            .child(col().key("stretch_col").w(300.0)
                .child(el().key("default_bar").h(20.0))
                .child(pill("stretch_pill"))
                .child(mut_w_fit)
                .child(row().key("empty_fit").w_fit().h(16.0)))
            // Same control inside a horizontally centred column: hugs and centres horizontally.
            .child(col().key("center_col").w(300.0).items_center().child(pill("center_pill")))
            // Same control inside a right-aligned column: hugs and aligns to end.
            .child(col().key("end_col").w(300.0).items_end().child(pill("end_pill")))
            // Same control inside a vertically centred row: hugs and centres vertically.
            .child(row().key("center_row").size(300.0, 80.0).center_y().child(pill("row_pill")))
            // Long wrapping content clamps to the column width (CSS fit-content).
            .child(col().w(200.0).child(row().key("wrap_fit").w_fit().child(text(LONG).key("wrap_fit_text").wrap().shrink())))
            .child(text("Short").key("one_line"))
    }

    let mut rt = Runtime::new((), view, (400.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r = |key: &str| rect(&rt, key);

    assert!((r("default_bar")[2] - 300.0).abs() < 0.01, "default child still stretches: {:?}", r("default_bar"));
    let sp = r("stretch_pill");
    assert!((sp[2] - 80.0).abs() < 0.01 && sp[0].abs() < 0.01, "w_fit hugs in stretching col: {sp:?}");
    let ip = r("in_place");
    assert!((ip[2] - 50.0).abs() < 0.01, "set_w_fit overrides w_full in place: {ip:?}");
    let ef = r("empty_fit");
    assert!(ef[2].abs() < 0.01, "empty w_fit has zero width: {ef:?}");

    let cp = r("center_pill");
    assert!((cp[2] - 80.0).abs() < 0.01 && (cp[0] - 110.0).abs() < 0.01, "w_fit centres in items_center col: {cp:?}");
    let ep = r("end_pill");
    assert!((ep[2] - 80.0).abs() < 0.01 && (ep[0] - 220.0).abs() < 0.01, "w_fit aligns end in items_end col: {ep:?}");

    let row_box = r("center_row");
    let rp = r("row_pill");
    assert!((rp[2] - 80.0).abs() < 0.01, "w_fit keeps content width in row: {rp:?}");
    assert!(((rp[1] - row_box[1]) - 28.0).abs() < 0.01, "w_fit centres vertically in row().center_y(): {rp:?} vs {row_box:?}");

    let wf = r("wrap_fit");
    let wt = r("wrap_fit_text");
    let one_line = r("one_line")[3];
    assert!(wf[2] <= 200.5, "w_fit clamps long wrapping content to container: {wf:?}");
    assert!(wt[3] >= one_line * 2.0 - 0.5, "w_fit text wraps across lines: {wt:?}");
}

#[test]
fn h_fit_hugs_content_height_and_respects_parent_alignment() {
    fn chip(key: &str) -> El {
        col().key(key).h_fit().py(6.0).child(el().size(40.0, 20.0))
    }

    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut mut_h_fit = col().key("in_place").h_full().child(el().size(40.0, 24.0));
        mut_h_fit.set_h_fit();

        col().gap(4.0)
            // Default stretching row: h_fit hugs to 20 + 12 = 32, default sibling stretches to 100.
            .child(row().key("stretch_row").size(300.0, 100.0)
                .child(el().key("default_bar").w(20.0))
                .child(chip("stretch_chip"))
                .child(mut_h_fit)
                .child(col().key("empty_fit").h_fit().w(16.0)))
            // Centred row: h_fit hugs and centres vertically.
            .child(row().key("center_row").size(300.0, 100.0).center_y().child(chip("center_chip")))
            // End-aligned row: h_fit hugs and aligns to bottom.
            .child(row().key("end_row").size(300.0, 100.0).items_end().child(chip("end_chip")))
    }

    let mut rt = Runtime::new((), view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r = |key: &str| rect(&rt, key);

    assert!((r("default_bar")[3] - 100.0).abs() < 0.01, "default child still stretches vertically: {:?}", r("default_bar"));
    let sc = r("stretch_chip");
    assert!((sc[3] - 32.0).abs() < 0.01 && sc[1].abs() < 0.01, "h_fit hugs in stretching row: {sc:?}");
    let ip = r("in_place");
    assert!((ip[3] - 24.0).abs() < 0.01, "set_h_fit overrides h_full in place: {ip:?}");
    let ef = r("empty_fit");
    assert!(ef[3].abs() < 0.01, "empty h_fit has zero height: {ef:?}");

    let crow = r("center_row");
    let cc = r("center_chip");
    assert!((cc[3] - 32.0).abs() < 0.01 && ((cc[1] - crow[1]) - 34.0).abs() < 0.01, "h_fit centres in center_y row: {cc:?} vs {crow:?}");
    let erow = r("end_row");
    let ec = r("end_chip");
    assert!((ec[3] - 32.0).abs() < 0.01 && ((ec[1] - erow[1]) - 68.0).abs() < 0.01, "h_fit aligns bottom in items_end row: {ec:?} vs {erow:?}");
}

#[test]
fn self_auto_restores_parent_alignment() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut reset_in_place = el().key("in_place").size(20.0, 20.0).self_end();
        reset_in_place.set_self_auto();

        col().w(200.0).gap(4.0)
            // Restores row().center_y() after self_start.
            .child(row().key("center_row").size(200.0, 60.0).center_y()
                .child(el().key("kept_start").size(20.0, 20.0).self_start())
                .child(el().key("reset_center").size(20.0, 20.0).self_start().self_auto())
                .child(reset_in_place))
            // Restores default column stretch after self_start.
            .child(col().w(200.0)
                .child(el().key("stretched_again").h(16.0).self_start().self_auto()))
            // Restores col().items_center() after self_end.
            .child(col().w(200.0).items_center()
                .child(el().key("col_centered").size(40.0, 16.0).self_end().self_auto()))
    }

    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r = |key: &str| rect(&rt, key);

    let row_y = r("center_row")[1];
    assert!((r("kept_start")[1] - row_y).abs() < 0.01, "self_start pins to top");
    assert!(((r("reset_center")[1] - row_y) - 20.0).abs() < 0.01, "self_auto restores row center_y: {:?}", r("reset_center"));
    assert!(((r("in_place")[1] - row_y) - 20.0).abs() < 0.01, "set_self_auto restores row center_y: {:?}", r("in_place"));
    assert!((r("stretched_again")[2] - 200.0).abs() < 0.01, "self_auto restores default col stretch: {:?}", r("stretched_again"));
    assert!((r("col_centered")[0] - 80.0).abs() < 0.01, "self_auto restores col items_center: {:?}", r("col_centered"));
}

#[test]
fn h_frac_sizes_height_as_fraction_of_parent() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut mutated = el().key("in_place").w(20.0).h(10.0);
        mutated.set_h_frac(0.3);

        row().gap(4.0)
            // Bounded 200 pt parent: 40% = 80 pt, 30% (in place) = 60 pt, 0% = 0 pt, 100% = 200 pt, 150% = 300 pt.
            .child(col().size(40.0, 200.0)
                .child(el().key("frac_40").h_frac(0.4))
                .child(mutated)
                .child(el().key("frac_0").h_frac(0.0)))
            .child(col().size(40.0, 200.0)
                .child(el().key("split_25").h_frac(0.25))
                .child(el().key("split_75").h_frac(0.75)))
            .child(col().size(40.0, 200.0).child(el().key("frac_100").h_frac(1.0)))
            .child(col().size(40.0, 200.0).child(el().key("frac_150").h_frac(1.5)))
            // Indefinite-height parent (not stretched to the root row): CSS percentage height resolves as auto (content height).
            .child(col().w(40.0).self_start().child(col().key("indefinite").h_frac(0.5).child(el().size(20.0, 34.0))))
            // Absolute child resolves against parent padding box (200 pt height, 10 pt vertical padding).
            .child(col().key("abs_parent").size(40.0, 200.0).py(10.0)
                .child(el().key("abs_half").absolute(0.0, 0.0).w(10.0).h_frac(0.5)))
    }

    let mut rt = Runtime::new((), view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r = |key: &str| rect(&rt, key);

    assert!((r("frac_40")[3] - 80.0).abs() < 0.01, "h_frac(0.4) of 200 = 80: {:?}", r("frac_40"));
    assert!((r("in_place")[3] - 60.0).abs() < 0.01, "set_h_frac(0.3) of 200 = 60: {:?}", r("in_place"));
    assert!(r("frac_0")[3].abs() < 0.01, "h_frac(0.0) = 0: {:?}", r("frac_0"));
    assert!((r("split_25")[3] - 50.0).abs() < 0.01 && (r("split_75")[3] - 150.0).abs() < 0.01, "25/75 split");
    assert!((r("split_75")[1] - 50.0).abs() < 0.01, "split_75 starts right below split_25");
    assert!((r("frac_100")[3] - 200.0).abs() < 0.01, "h_frac(1.0) = 200: {:?}", r("frac_100"));
    assert!((r("frac_150")[3] - 300.0).abs() < 0.01, "h_frac(1.5) = 300: {:?}", r("frac_150"));
    assert!((r("indefinite")[3] - 34.0).abs() < 0.01, "h_frac in indefinite-height parent falls back to content height: {:?}", r("indefinite"));
    assert!((r("abs_half")[3] - 100.0).abs() < 0.01, "absolute h_frac(0.5) resolves against parent padding box: {:?}", r("abs_half"));
}

#[test]
fn auto_fit_and_auto_fill_columns_lay_out_in_single_pass_and_resize() {
    use rune::{ScrollSpec, text};

    // 1. Single-pass responsive layout inside a column (no `on_layout` round-trip) + live resize.
    fn responsive(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().gap(10.0)
            .child(
                el().key("grid").auto_fit_columns(240.0).gap(12.0)
                    .children((0..5).map(|k| col().key(format!("c{k}")).h(60.0))),
            )
            .child(el().key("after").h(20.0))
    }

    let mut rt = Runtime::new((), responsive, (980.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Frame 1 at 980 pt: floor((980 + 12) / (240 + 12)) = 3 columns of (980 - 24) / 3 = 318.67 pt.
    let col3_w = (980.0 - 24.0) / 3.0;
    let g = rect(&rt, "grid");
    assert!((g[2] - 980.0).abs() < 0.01 && (g[3] - 132.0).abs() < 0.01, "grid at 980: {g:?}");
    assert!((rect(&rt, "after")[1] - 142.0).abs() < 0.01, "sibling sits below 2-row grid");
    for (k, (ex, ey)) in [
        (0, (0.0, 0.0)),
        (1, (col3_w + 12.0, 0.0)),
        (2, ((col3_w + 12.0) * 2.0, 0.0)),
        (3, (0.0, 72.0)),
        (4, (col3_w + 12.0, 72.0)),
    ] {
        let r = rect(&rt, &format!("c{k}"));
        assert!(
            (r[0] - ex).abs() < 0.05 && (r[1] - ey).abs() < 0.05 && (r[2] - col3_w).abs() < 0.05 && (r[3] - 60.0).abs() < 0.01,
            "980 c{k}: got {r:?}, want x={ex}, y={ey}, w={col3_w}, h=60"
        );
    }

    // Resize to 500 pt without rebuilding view: 2 columns of (500 - 12) / 2 = 244 pt across 3 rows.
    rt.set_viewport((500.0, 600.0), 1.0);
    rt.frame(0.0);
    let g500 = rect(&rt, "grid");
    assert!((g500[2] - 500.0).abs() < 0.01 && (g500[3] - 204.0).abs() < 0.01, "grid at 500: {g500:?}");
    assert!((rect(&rt, "c0")[2] - 244.0).abs() < 0.05);
    assert!((rect(&rt, "c1")[0] - 256.0).abs() < 0.05 && rect(&rt, "c1")[1].abs() < 0.01);
    assert!(rect(&rt, "c2")[0].abs() < 0.01 && (rect(&rt, "c2")[1] - 72.0).abs() < 0.05);
    assert!(rect(&rt, "c4")[0].abs() < 0.01 && (rect(&rt, "c4")[1] - 144.0).abs() < 0.05);

    // Resize to 1200 pt: 4 columns of (1200 - 36) / 4 = 291 pt across 2 rows.
    rt.set_viewport((1200.0, 600.0), 1.0);
    rt.frame(0.0);
    assert!((rect(&rt, "grid")[3] - 132.0).abs() < 0.01);
    assert!((rect(&rt, "c3")[0] - 909.0).abs() < 0.05 && rect(&rt, "c3")[1].abs() < 0.01);
    assert!(rect(&rt, "c4")[0].abs() < 0.01 && (rect(&rt, "c4")[1] - 72.0).abs() < 0.05);

    // Resize below min_width (180 < 240): 1 column of 240 pt (CSS minmax(240px, 1fr)), 5 rows.
    rt.set_viewport((180.0, 600.0), 1.0);
    rt.frame(0.0);
    assert!((rect(&rt, "grid")[3] - 348.0).abs() < 0.01, "5 rows of 60 + 4 gaps of 12 = 348");
    assert!((rect(&rt, "c0")[2] - 240.0).abs() < 0.05);
    assert!(rect(&rt, "c1")[0].abs() < 0.01 && (rect(&rt, "c1")[1] - 72.0).abs() < 0.05);

    // 2. auto_fit collapses empty trailing tracks; auto_fill preserves them; padding, scroll,
    // in-place setters, wrapping text, and non-positive/non-finite fallbacks.
    fn variants(_: &(), _: &mut Cx<()>) -> El {
        let mut in_place_fit = el().key("ip_fit").w(980.0).gap(12.0)
            .child(el().key("ip_fit0").h(40.0))
            .child(el().key("ip_fit1").h(40.0));
        in_place_fit.set_auto_fit_columns(240.0);

        let mut in_place_fill = el().key("ip_fill").w(980.0).gap(12.0)
            .child(el().key("ip_fill0").h(40.0))
            .child(el().key("ip_fill1").h(40.0));
        in_place_fill.set_auto_fill_columns(240.0);

        col().w(980.0).gap(10.0)
            .child(in_place_fit)
            .child(in_place_fill)
            // Padding reduces available track space: inner width = 980 - 40 = 940 -> 3 cols of (940 - 24) / 3.
            .child(
                el().key("padded").w(980.0).padding(20.0).auto_fit_columns(240.0).gap(12.0)
                    .children((0..3).map(|k| el().key(format!("p{k}")).h(30.0))),
            )
            // Scroll directly on the auto-fit container (exercises take_box_placement).
            .child(
                el().key("scroll_self").w(980.0).h(100.0).scroll(ScrollSpec { stick_bottom: false })
                    .auto_fit_columns(240.0).gap(12.0)
                    .children((0..4).map(|k| el().key(format!("ss{k}")).h(60.0))),
            )
            // Wrapping text inside cards stays within the 1fr track width.
            .child(
                el().key("text_grid").w(980.0).auto_fit_columns(240.0).gap(12.0)
                    .children((0..3).map(|k| {
                        col().key(format!("t{k}")).child(
                            text("Long card copy that wraps cleanly inside its responsive grid column without widening the track.").wrap(),
                        )
                    })),
            )
            // Non-positive and non-finite min_width fall back to 1 full-width column (1fr).
            .child(el().w(400.0).auto_fit_columns(0.0).child(el().key("zero").h(20.0)).child(el().key("zero2").h(20.0)))
            .child(el().w(400.0).auto_fit_columns(-50.0).child(el().key("neg").h(20.0)))
            .child(el().w(400.0).auto_fit_columns(f32::NAN).child(el().key("nan").h(20.0)))
            .child(el().w(400.0).auto_fill_columns(f32::INFINITY).child(el().key("inf").h(20.0)))
            // Existing equal_columns(3) stays unchanged.
            .child(el().w(300.0).equal_columns(3).child(el().key("eq0").h(20.0)).child(el().key("eq1").h(20.0)))
    }

    let mut rt2 = Runtime::new((), variants, (980.0, 1200.0), Env::new(true, 1.0));
    rt2.frame(0.0);

    // 2 items in 980 pt: auto_fit -> 2 cols of 484 pt; auto_fill -> 3-col width of 318.67 pt.
    assert!((rect(&rt2, "ip_fit0")[2] - 484.0).abs() < 0.05 && (rect(&rt2, "ip_fit1")[0] - 496.0).abs() < 0.05);
    assert!((rect(&rt2, "ip_fill0")[2] - col3_w).abs() < 0.05 && (rect(&rt2, "ip_fill1")[0] - (col3_w + 12.0)).abs() < 0.05);

    // Padded container: inner width 940 -> (940 - 24) / 3 = 305.33 pt.
    let pad_w = (940.0 - 24.0) / 3.0;
    assert!((rect(&rt2, "p0")[0] - 20.0).abs() < 0.05 && (rect(&rt2, "p0")[2] - pad_w).abs() < 0.05);
    assert!((rect(&rt2, "p2")[0] - (20.0 + (pad_w + 12.0) * 2.0)).abs() < 0.05);

    // Scroll-self grid still lays out 3 columns on row 1 and 1 on row 2.
    assert!((rect(&rt2, "ss0")[2] - col3_w).abs() < 0.05);
    assert!((rect(&rt2, "ss2")[0] - (col3_w + 12.0) * 2.0).abs() < 0.05);
    assert!(rect(&rt2, "ss3")[0].abs() < 0.01 && (rect(&rt2, "ss3")[1] - rect(&rt2, "ss0")[1] - 72.0).abs() < 0.05);

    // Wrapping text cards keep equal 3-column widths.
    for k in 0..3 {
        let tr = rect(&rt2, &format!("t{k}"));
        assert!((tr[2] - col3_w).abs() < 0.05 && tr[3] > 0.0, "t{k}: {tr:?}");
    }

    // Invalid min_width falls back to 1 column of 400 pt.
    for k in ["zero", "neg", "nan", "inf"] {
        assert!((rect(&rt2, k)[2] - 400.0).abs() < 0.05, "{k}: {:?}", rect(&rt2, k));
    }
    assert!((rect(&rt2, "zero2")[1] - rect(&rt2, "zero")[1] - 20.0).abs() < 0.05, "1 column stacks vertically");

    // Existing equal_columns(3) default preserved.
    assert!((rect(&rt2, "eq0")[2] - 100.0).abs() < 0.05 && (rect(&rt2, "eq1")[0] - 100.0).abs() < 0.05);
}
