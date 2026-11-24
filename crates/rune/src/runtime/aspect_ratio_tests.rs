//! Headless Runtime tests for `aspect_ratio` combined with min/max size clamping,
//! verified against Chromium headless shell (Blink) on every row.
use super::*;
use crate::{Cx, El, Env, col, el, row, text};
use rune_taffy::prelude::{AlignContent, BoxSizing, Style, length, percent};
use taffy::Overflow;

fn rect(rt: &Runtime<()>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
}

fn assert_rect(got: [f32; 4], want: [f32; 4], label: &str) {
    for i in 0..4 {
        assert!(
            (got[i] - want[i]).abs() < 0.01,
            "{label}: rect[{i}] = {}, want {} (got {got:?}, want {want:?})",
            got[i],
            want[i],
        );
    }
}

fn run(view: fn(&(), &mut Cx<()>) -> El) -> Runtime<()> {
    let mut rt = Runtime::new((), view, (500.0, 600.0), Env::new(false, 1.0));
    rt.frame(1.0 / 60.0);
    rt
}

fn with_style(mut e: El, f: impl FnOnce(&mut Style)) -> El {
    let mut s = e.style.to_style();
    f(&mut s);
    e.style = CompactStyle::from_style(s);
    e
}

#[test]
fn reporter_column_w_full_max_w_and_shrink_variant() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                col().key("wrap").w(400.0)
                    .child(el().key("child").w_full().max_w(64.0).aspect_ratio(1.0)),
            )
            .child(text("next").key("next"))
    }
    let rt = run(view);
    assert_rect(rect(&rt, "wrap"), [0.0, 0.0, 400.0, 64.0], "reporter wrap");
    assert_rect(rect(&rt, "child"), [0.0, 0.0, 64.0, 64.0], "reporter child");
    assert!((rect(&rt, "next")[1] - 64.0).abs() < 0.01, "next y = {}, want 64", rect(&rt, "next")[1]);

    fn view_shrink(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                col().key("wrap").w(400.0)
                    .child(with_style(el().key("child").w_full().max_w(64.0).aspect_ratio(2.0), |s| {
                        s.flex_shrink = 1.0;
                    })),
            )
            .child(el().key("next").size(40.0, 20.0))
    }
    let rt = run(view_shrink);
    assert_rect(rect(&rt, "wrap"), [0.0, 0.0, 400.0, 32.0], "shrink ratio=2 wrap");
    assert_rect(rect(&rt, "child"), [0.0, 0.0, 64.0, 32.0], "shrink ratio=2 child");
    assert_rect(rect(&rt, "next"), [0.0, 32.0, 40.0, 20.0], "shrink ratio=2 next");
}

#[test]
fn column_min_w_and_min_wins_over_max() {
    fn view_min(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                col().key("wrap").w(400.0)
                    .child(el().key("child").w(40.0).min_w(100.0).aspect_ratio(1.0)),
            )
            .child(el().key("next").size(40.0, 20.0))
    }
    let rt = run(view_min);
    assert_rect(rect(&rt, "wrap"), [0.0, 0.0, 400.0, 100.0], "min_w wrap");
    assert_rect(rect(&rt, "child"), [0.0, 0.0, 100.0, 100.0], "min_w child");
    assert_rect(rect(&rt, "next"), [0.0, 100.0, 40.0, 20.0], "min_w next");

    fn view_min_beats_max(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                col().key("wrap").w(400.0)
                    .child(el().key("child").w_full().min_w(100.0).max_w(64.0).aspect_ratio(2.0)),
            )
            .child(el().key("next").size(40.0, 20.0))
    }
    let rt = run(view_min_beats_max);
    assert_rect(rect(&rt, "wrap"), [0.0, 0.0, 400.0, 50.0], "min>max wrap");
    assert_rect(rect(&rt, "child"), [0.0, 0.0, 100.0, 50.0], "min>max child");
    assert_rect(rect(&rt, "next"), [0.0, 50.0, 40.0, 20.0], "min>max next");
}

#[test]
fn row_max_h_and_min_h() {
    fn view_max_1(_: &(), _: &mut Cx<()>) -> El {
        row().w_full().h_full()
            .child(
                row().key("wrap").h(400.0)
                    .child(el().key("child").h_full().max_h(64.0).aspect_ratio(1.0)),
            )
            .child(el().key("next").size(40.0, 20.0))
    }
    let rt = run(view_max_1);
    assert_rect(rect(&rt, "wrap"), [0.0, 0.0, 64.0, 400.0], "row max_h r=1 wrap");
    assert_rect(rect(&rt, "child"), [0.0, 0.0, 64.0, 64.0], "row max_h r=1 child");
    assert_rect(rect(&rt, "next"), [64.0, 0.0, 40.0, 20.0], "row max_h r=1 next");

    fn view_max_2(_: &(), _: &mut Cx<()>) -> El {
        row().w_full().h_full()
            .child(
                row().key("wrap").h(400.0)
                    .child(el().key("child").h_full().max_h(64.0).aspect_ratio(2.0)),
            )
            .child(el().key("next").size(40.0, 20.0))
    }
    let rt = run(view_max_2);
    assert_rect(rect(&rt, "wrap"), [0.0, 0.0, 128.0, 400.0], "row max_h r=2 wrap");
    assert_rect(rect(&rt, "child"), [0.0, 0.0, 128.0, 64.0], "row max_h r=2 child");
    assert_rect(rect(&rt, "next"), [128.0, 0.0, 40.0, 20.0], "row max_h r=2 next");

    fn view_min_h(_: &(), _: &mut Cx<()>) -> El {
        row().w_full().h_full()
            .child(
                row().key("wrap").h(400.0)
                    .child(el().key("child").h(40.0).min_h(100.0).aspect_ratio(2.0)),
            )
            .child(el().key("next").size(40.0, 20.0))
    }
    let rt = run(view_min_h);
    assert_rect(rect(&rt, "wrap"), [0.0, 0.0, 200.0, 400.0], "row min_h r=2 wrap");
    assert_rect(rect(&rt, "child"), [0.0, 0.0, 200.0, 100.0], "row min_h r=2 child");
    assert_rect(rect(&rt, "next"), [200.0, 0.0, 40.0, 20.0], "row min_h r=2 next");
}

#[test]
fn fixed_main_size_ignores_aspect_ratio() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                col().key("wrap1").w(400.0)
                    .child(el().key("c1").w_full().max_w(64.0).h(120.0).aspect_ratio(1.0)),
            )
            .child(
                col().key("wrap2").w(400.0)
                    .child(el().key("c2").w(120.0).h(40.0).aspect_ratio(1.0)),
            )
            .child(
                row().key("wrap3").h(400.0)
                    .child(el().key("c3").h_full().max_h(64.0).w(120.0).aspect_ratio(2.0)),
            )
    }
    let rt = run(view);
    assert_rect(rect(&rt, "wrap1"), [0.0, 0.0, 400.0, 120.0], "fixed main wrap1");
    assert_rect(rect(&rt, "c1"), [0.0, 0.0, 64.0, 120.0], "fixed main c1");
    assert_rect(rect(&rt, "wrap2"), [0.0, 120.0, 400.0, 40.0], "fixed both wrap2");
    assert_rect(rect(&rt, "c2"), [0.0, 120.0, 120.0, 40.0], "fixed both c2");
    assert_rect(rect(&rt, "c3"), [0.0, 160.0, 120.0, 64.0], "fixed main row c3");
}

#[test]
fn explicit_flex_basis_matches_chromium() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                col().key("w_120").w(400.0).child(with_style(
                    el().key("c_120").w_full().max_w(64.0).aspect_ratio(1.0),
                    |s| s.flex_basis = length(120.0),
                )),
            )
            .child(
                col().key("w_0_auto").w(400.0).child(with_style(
                    el().key("c_0_auto").w_full().max_w(64.0).aspect_ratio(1.0),
                    |s| s.flex_basis = length(0.0),
                )),
            )
            .child(
                col().key("w_20_min0").w(400.0).child(with_style(
                    el().key("c_20_min0").w_full().max_w(64.0).min_h(0.0).aspect_ratio(1.0),
                    |s| s.flex_basis = length(20.0),
                )),
            )
            .child(
                col().key("w_0_min0").w(400.0).child(with_style(
                    el().key("c_0_min0").w_full().max_w(64.0).min_h(0.0).aspect_ratio(1.0),
                    |s| s.flex_basis = length(0.0),
                )),
            )
            .child(
                col().key("w_0_hidden").w(400.0).child(with_style(
                    el().key("c_0_hidden").w_full().max_w(64.0).aspect_ratio(1.0),
                    |s| {
                        s.flex_basis = length(0.0);
                        s.overflow.y = Overflow::Hidden;
                    },
                )),
            )
            .child(
                col().key("w_pct").w(400.0).child(with_style(
                    el().key("c_pct").w_full().max_w(64.0).aspect_ratio(1.0),
                    |s| s.flex_basis = percent(0.5),
                )),
            )
            .child(
                col().key("w_grow").w(400.0).h(200.0)
                    .child(el().key("c_grow").w_full().max_w(64.0).grow(1.0).aspect_ratio(1.0)),
            )
    }
    let rt = run(view);
    assert_eq!(&rect(&rt, "w_120")[2..], &[400.0, 120.0]);
    assert_eq!(&rect(&rt, "c_120")[2..], &[64.0, 120.0]);
    assert_eq!(&rect(&rt, "w_0_auto")[2..], &[400.0, 64.0]);
    assert_eq!(&rect(&rt, "c_0_auto")[2..], &[64.0, 64.0]);
    assert_eq!(&rect(&rt, "w_20_min0")[2..], &[400.0, 20.0]);
    assert_eq!(&rect(&rt, "c_20_min0")[2..], &[64.0, 20.0]);
    assert_eq!(&rect(&rt, "w_0_min0")[2..], &[400.0, 0.0]);
    assert_eq!(&rect(&rt, "c_0_min0")[2..], &[64.0, 0.0]);
    assert_eq!(&rect(&rt, "w_0_hidden")[2..], &[400.0, 0.0]);
    assert_eq!(&rect(&rt, "c_0_hidden")[2..], &[64.0, 0.0]);
    assert_eq!(&rect(&rt, "w_pct")[2..], &[400.0, 64.0]);
    assert_eq!(&rect(&rt, "c_pct")[2..], &[64.0, 64.0]);
    assert_eq!(&rect(&rt, "w_grow")[2..], &[400.0, 200.0]);
    assert_eq!(&rect(&rt, "c_grow")[2..], &[64.0, 200.0]);
}

#[test]
fn wrapping_row_and_stretched_cross_match_chromium() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                with_style(row().key("wrap_h").w(200.0).h(200.0).flex_wrap(), |s| {
                    s.align_content = AlignContent::FLEX_START;
                })
                .child(el().key("a").h_full().max_h(64.0).aspect_ratio(2.0))
                .child(el().key("b").h_full().max_h(64.0).aspect_ratio(2.0)),
            )
            .child(
                row().key("wrap_w").w(200.0).flex_wrap()
                    .child(el().key("wa").w_full().max_w(80.0).max_h(100.0).aspect_ratio(2.0))
                    .child(el().key("wb").w_full().max_w(80.0).max_h(100.0).aspect_ratio(2.0))
                    .child(el().key("wc").w_full().max_w(80.0).max_h(100.0).aspect_ratio(2.0)),
            )
            .child(
                row().key("stretch_row").w(200.0).h(100.0)
                    .child(el().key("sr").w_full().max_w(80.0).aspect_ratio(2.0)),
            )
    }
    let rt = run(view);
    assert_rect(rect(&rt, "a"), [0.0, 0.0, 128.0, 64.0], "wrap_h a");
    assert_rect(rect(&rt, "b"), [0.0, 64.0, 128.0, 64.0], "wrap_h b");
    assert_rect(rect(&rt, "wrap_w"), [0.0, 200.0, 200.0, 80.0], "wrap_w");
    assert_rect(rect(&rt, "wa"), [0.0, 200.0, 80.0, 40.0], "wrap_w a");
    assert_rect(rect(&rt, "wb"), [80.0, 200.0, 80.0, 40.0], "wrap_w b");
    assert_rect(rect(&rt, "wc"), [0.0, 240.0, 80.0, 40.0], "wrap_w c");
    assert_rect(rect(&rt, "sr"), [0.0, 280.0, 80.0, 100.0], "stretch row keeps auto cross stretch");
}

#[test]
fn nested_columns_plain_and_padded() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                col().key("outer").w(400.0)
                    .child(
                        col().key("inner")
                            .child(el().key("child").w_full().max_w(64.0).aspect_ratio(1.0)),
                    )
                    .child(el().key("next").size(40.0, 20.0)),
            )
            .child(
                col().key("p_outer").w(400.0).padding(20.0)
                    .child(
                        col().key("p_inner").padding(10.0)
                            .child(with_style(el().key("p_child").w_full().aspect_ratio(1.0), |s| {
                                s.max_size.width = percent(0.5);
                            })),
                    ),
            )
    }
    let rt = run(view);
    assert_rect(rect(&rt, "outer"), [0.0, 0.0, 400.0, 84.0], "nested outer");
    assert_rect(rect(&rt, "inner"), [0.0, 0.0, 400.0, 64.0], "nested inner");
    assert_rect(rect(&rt, "child"), [0.0, 0.0, 64.0, 64.0], "nested child");
    assert_rect(rect(&rt, "next"), [0.0, 64.0, 40.0, 20.0], "nested next");
    assert_rect(rect(&rt, "p_outer"), [0.0, 84.0, 400.0, 230.0], "padded outer");
    assert_rect(rect(&rt, "p_inner"), [20.0, 104.0, 360.0, 190.0], "padded inner");
    assert_rect(rect(&rt, "p_child"), [30.0, 114.0, 170.0, 170.0], "padded child");
}

#[test]
fn percentage_against_indefinite_size() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                row().w(500.0).h(100.0).items_start()
                    .child(
                        col().key("indef_w")
                            .child(el().key("indef_w_child").w_full().max_w(64.0).aspect_ratio(1.0)),
                    ),
            )
            .child(
                col().key("indef_main").w(400.0)
                    .child(el().key("indef_main_child").w_full().max_w(64.0).h_full().aspect_ratio(1.0)),
            )
            .child(
                col().w(500.0).items_start()
                    .child(
                        row().key("indef_h")
                            .child(el().key("indef_h_child").h_full().max_h(64.0).aspect_ratio(2.0)),
                    ),
            )
    }
    let rt = run(view);
    assert_eq!(&rect(&rt, "indef_w")[2..], &[0.0, 0.0]);
    assert_eq!(&rect(&rt, "indef_w_child")[2..], &[0.0, 0.0]);
    assert_eq!(&rect(&rt, "indef_main")[2..], &[400.0, 64.0]);
    assert_eq!(&rect(&rt, "indef_main_child")[2..], &[64.0, 64.0]);
    assert_eq!(&rect(&rt, "indef_h")[2..], &[0.0, 0.0]);
    assert_eq!(&rect(&rt, "indef_h_child")[2..], &[0.0, 0.0]);
}

#[test]
fn box_sizing_with_padding_matches_chromium() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                col().key("bb_wrap").w(400.0)
                    .child(el().key("bb").w_full().max_w(64.0).padding(10.0).aspect_ratio(2.0)),
            )
            .child(
                col().key("cb_wrap").w(400.0).child(with_style(
                    el().key("cb").w_full().max_w(64.0).padding(10.0).aspect_ratio(2.0),
                    |s| s.box_sizing = BoxSizing::ContentBox,
                )),
            )
            .child(
                col().key("bb_pad_floor_wrap").w(400.0)
                    .child(el().key("bb_pad_floor").w_full().max_w(64.0).padding(20.0).aspect_ratio(4.0)),
            )
            .child(
                col().key("bb_w_floor_wrap").w(400.0)
                    .child(el().key("bb_w_floor").w_full().max_w(10.0).px(20.0).aspect_ratio(1.0)),
            )
            .child(
                col().key("cb_w_wrap").w(400.0).child(with_style(
                    el().key("cb_w").w_full().max_w(10.0).px(20.0).aspect_ratio(1.0),
                    |s| s.box_sizing = BoxSizing::ContentBox,
                )),
            )
    }
    let rt = run(view);
    assert_eq!(&rect(&rt, "bb_wrap")[2..], &[400.0, 32.0]);
    assert_eq!(&rect(&rt, "bb")[2..], &[64.0, 32.0]);
    assert_eq!(&rect(&rt, "cb_wrap")[2..], &[400.0, 52.0]);
    assert_eq!(&rect(&rt, "cb")[2..], &[84.0, 52.0]);
    assert_eq!(&rect(&rt, "bb_pad_floor_wrap")[2..], &[400.0, 40.0]);
    assert_eq!(&rect(&rt, "bb_pad_floor")[2..], &[64.0, 40.0]);
    assert_eq!(&rect(&rt, "bb_w_floor_wrap")[2..], &[400.0, 40.0]);
    assert_eq!(&rect(&rt, "bb_w_floor")[2..], &[40.0, 40.0]);
    assert_eq!(&rect(&rt, "cb_w_wrap")[2..], &[400.0, 10.0]);
    assert_eq!(&rect(&rt, "cb_w")[2..], &[50.0, 10.0]);
}

#[test]
fn grid_and_main_cross_limits_match_chromium() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full()
            .child(
                el().key("grid_leaf").equal_columns(1).w(400.0)
                    .child(el().key("gl").w_full().max_w(64.0).aspect_ratio(1.0)),
            )
            .child(
                el().key("grid_cb").equal_columns(1).w(400.0).child(with_style(
                    el().key("gcb").w_full().max_w(64.0).padding(10.0).aspect_ratio(2.0),
                    |s| s.box_sizing = BoxSizing::ContentBox,
                )),
            )
            .child(
                el().key("grid_container").equal_columns(1).w(400.0).child(
                    col().key("gc").w_full().max_w(64.0).max_h(100.0).aspect_ratio(1.0)
                        .child(el().h(10.0)),
                ),
            )
            .child(
                col().key("both_max").w(400.0)
                    .child(el().key("bm").w_full().max_w(200.0).max_h(80.0).aspect_ratio(1.0)),
            )
            .child(
                col().key("cross_max_main_min").w(400.0)
                    .child(el().key("cm").w_full().max_w(64.0).min_h(100.0).aspect_ratio(1.0)),
            )
            .child(
                col().key("non_leaf").w(400.0)
                    .child(
                        col().key("nl").w_full().max_w(64.0).aspect_ratio(1.0)
                            .child(el().w(20.0).h(100.0)),
                    ),
            )
    }
    let rt = run(view);
    assert_eq!(&rect(&rt, "grid_leaf")[2..], &[400.0, 64.0]);
    assert_eq!(&rect(&rt, "gl")[2..], &[64.0, 64.0]);
    assert_eq!(&rect(&rt, "grid_cb")[2..], &[400.0, 52.0]);
    assert_eq!(&rect(&rt, "gcb")[2..], &[84.0, 52.0]);
    assert_eq!(&rect(&rt, "grid_container")[2..], &[400.0, 64.0]);
    assert_eq!(&rect(&rt, "gc")[2..], &[64.0, 64.0]);
    assert_eq!(&rect(&rt, "both_max")[2..], &[400.0, 80.0]);
    assert_eq!(&rect(&rt, "bm")[2..], &[200.0, 80.0]);
    assert_eq!(&rect(&rt, "cross_max_main_min")[2..], &[400.0, 100.0]);
    assert_eq!(&rect(&rt, "cm")[2..], &[64.0, 100.0]);
    assert_eq!(&rect(&rt, "non_leaf")[2..], &[400.0, 64.0]);
    assert_eq!(&rect(&rt, "nl")[2..], &[64.0, 64.0]);
}
