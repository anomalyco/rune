use std::rc::Rc;

use rune::{Cx, El, Env, Runtime, col};

use super::line::runs;
use super::*;

#[test]
fn scale_rounds_out_and_takes_in_zero() {
    let s = Scale::new([3.0, 17.0, 42.0], None);
    assert_eq!((s.lo, s.hi), (0.0, 60.0));
    assert_eq!(s.ticks, vec![0.0, 20.0, 40.0, 60.0]);
    let s = Scale::new([-7.0, 12.0], None);
    assert_eq!((s.lo, s.hi), (-10.0, 15.0));
    assert_eq!(s.ticks, vec![-10.0, -5.0, 0.0, 5.0, 10.0, 15.0]);
    assert_eq!(s.frac(0.0), 0.4);
}

#[test]
fn scale_survives_flat_empty_and_non_finite_data() {
    let s = Scale::new([], None);
    assert_eq!((s.lo, s.hi), (0.0, 1.0));
    let s = Scale::new([f32::NAN, f32::INFINITY], None);
    assert_eq!((s.lo, s.hi), (0.0, 1.0));
    let s = Scale::new([5.0], Some([10.0, 0.0]));
    assert_eq!((s.lo, s.hi), (0.0, 10.0));
    assert_eq!(s.frac(20.0), 1.0);
}

#[test]
fn plain_trims_zeros() {
    assert_eq!(plain(20.0), "20");
    assert_eq!(plain(1.5), "1.5");
    assert_eq!(plain(-0.001), "0");
    assert_eq!(plain(f32::NAN), "–");
}

#[test]
fn runs_split_at_gaps() {
    let v = [1.0, 2.0, f32::NAN, 3.0, f32::NAN, f32::NAN, 4.0, 5.0];
    let got: Vec<Vec<(usize, f32)>> = runs(&v).map(Iterator::collect).collect();
    assert_eq!(got, vec![vec![(0, 1.0), (1, 2.0)], vec![(3, 3.0)], vec![(6, 4.0), (7, 5.0)]]);
}

#[test]
fn label_stride_keeps_labels_apart() {
    assert_eq!(label_stride(5, 12), 1);
    assert_eq!(label_stride(30, 12), 3);
    assert_eq!(label_stride(0, 12), 1);
}

#[derive(Default)]
struct Harness {
    hover: Option<usize>,
    bar_hover: Option<usize>,
}

fn view(s: &Harness, _: &mut Cx<Harness>) -> El {
    let k = Kit::neutral(true);
    let line = LineChart::new("line")
        .labels(["a", "b", "c", "d"])
        .series("one", [1.0, 4.0, 2.0, 3.0])
        .series("two", [2.0, f32::NAN, 1.0, 5.0])
        .hovered(s.hover)
        .on_hover(|s: &mut Harness, i| s.hover = i);
    let bars = BarChart::new("bars")
        .bars([("x", 3.0), ("y", 7.0)])
        .horizontal()
        .hovered(s.bar_hover)
        .on_hover(|s: &mut Harness, i| s.bar_hover = i);
    col()
        .padding(20.0)
        .gap(20.0)
        .child(line.build(&k))
        .child(bars.build(&k))
        .child(BarChart::new("up").bar("p", -2.0).bar("q", 5.0).build(&k))
}

fn rect_of(rt: &Runtime<Harness>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing {key:?}")).rect
}

#[test]
fn bands_split_the_plot_and_hover_follows_the_pointer() {
    let mut rt = Runtime::new(Harness::default(), view, (420.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    let plot = rect_of(&rt, "line-plot");
    let b0 = rect_of(&rt, "line-band-0");
    let b3 = rect_of(&rt, "line-band-3");
    assert!((b0[2] - plot[2] / 4.0).abs() < 0.5, "a quarter of the plot each: {b0:?} in {plot:?}");
    assert!((b3[0] + b3[2] - (plot[0] + plot[2])).abs() < 0.5, "the last band ends with the plot");

    // 0 to 6 in steps of 2: the top label centres on the plot's top, the last on its bottom.
    let mid = |r: [f32; 4]| r[1] + r[3] / 2.0;
    assert!((mid(rect_of(&rt, "y-0")) - plot[1]).abs() < 0.5);
    assert!((mid(rect_of(&rt, "y-3")) - (plot[1] + plot[3])).abs() < 0.5);

    let y = plot[1] + plot[3] / 2.0;
    rt.pointer_move(b3[0] + b3[2] / 2.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hover, Some(3));
    rt.pointer_move(b0[0] + b0[2] / 2.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hover, Some(0));
    rt.pointer_move(plot[0] + plot[2] / 2.0, 590.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hover, None, "leaving the plot clears the hover");

    let r = rect_of(&rt, "bars-band-1");
    rt.pointer_move(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.bar_hover, Some(1));
}

fn gaps(_: &Harness, _: &mut Cx<Harness>) -> El {
    let k = Kit::neutral(true);
    let pct = |v: f32| format!("{v:.0}%");
    col()
        .child(
            LineChart::new("line")
                .labels(["a", "b"])
                .series("cpu", [1.0, f32::NAN])
                .format(pct)
                .hovered(Some(1))
                .on_hover(|_: &mut Harness, _| {})
                .build(&k),
        )
        .child(
            BarChart::new("bars")
                .bar("x", f32::NAN)
                .format(pct)
                .hovered(Some(0))
                .on_hover(|_: &mut Harness, _| {})
                .build(&k),
        )
}

#[test]
fn gaps_read_as_a_dash_whatever_the_format() {
    assert_eq!(show(&(Rc::new(|v: f32| format!("{v:.0}%")) as Format), f32::NAN), "–");
    let mut rt = Runtime::new(Harness::default(), gaps, (420.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    let tree = serde_json::to_string(&rt.accessibility_json()).unwrap();
    assert!(!tree.contains("NaN"), "no format sees a gap: {tree}");
    assert!(tree.contains("\"cpu –\""), "the hovered readout dashes the gap: {tree}");
    assert!(tree.contains("\"x: –\"") && tree.contains("\"–\""), "so do a bar's item and readout: {tree}");
}
