use super::*;

fn chart(src: &str) -> XyChart {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::XyChart(x)) => x,
        _ => panic!("not an xy chart: {src:?}"),
    }
}

#[test]
fn axes_and_series() {
    let x = chart(
        "xychart-beta\n    title \"Sales Revenue\"\n    x-axis [jan, feb, \"mar, apr\", may]\n    y-axis \"Revenue (in $)\" 4000 --> 11000\n    bar [5000, 6000, 7500, 8200]\n    line \"Trend\" [5000, 6000.5, -7500, 8200]\n    line [1, two, 3]\n    bar oops",
    );
    assert_eq!(x.title.as_deref(), Some("Sales Revenue"));
    assert!(!x.horizontal);
    assert_eq!(x.bands, ["jan", "feb", "mar, apr", "may"]);
    assert_eq!(x.y_title, "Revenue (in $)");
    assert_eq!(x.y_range, Some([4000.0, 11000.0]));
    assert_eq!(x.series.len(), 2, "series that don't read are skipped");
    assert_eq!((x.series[1].kind, x.series[1].name.as_str()), (Kind::Line, "Trend"));
    assert_eq!(x.series[1].values[2], -7500.0);

    let h = chart("xychart-beta horizontal\n    x-axis \"Score\" 10 --> 0\n    y-axis Votes\n    bar [3, 5]");
    assert!(h.horizontal);
    assert_eq!((h.x_title.as_str(), h.x_range), ("Score", Some([0.0, 10.0])));
    assert_eq!((h.y_title.as_str(), h.y_range), ("Votes", None));
    assert!(crate::parse("xychart-beta\n    title Nothing\n    x-axis [a, b]").is_none());
    let budget = format!("xychart-beta\n    bar [{}]", vec!["1"; MAX_ITEMS + 1].join(", "));
    assert!(crate::parse(&budget).is_none());
}

#[test]
fn the_value_axis_rounds_out_and_takes_in_zero() {
    let series = |v: &[f32]| vec![Series { kind: Kind::Bar, name: String::new(), values: v.to_vec() }];
    assert_eq!(scale(None, &series(&[3.0, 47.0])), ([0.0, 50.0], vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0]));
    let ([lo, hi], ticks) = scale(None, &series(&[-12.0, 30.0]));
    assert!(lo <= -12.0 && hi >= 30.0 && ticks.contains(&0.0));
    assert_eq!(scale(Some([4000.0, 11000.0]), &series(&[5000.0])).1.first(), Some(&4000.0));
}

#[test]
fn bars_rise_with_their_values_upright_and_on_their_side() {
    let paint = DiagramStyle::neutral(false);
    for (src, horizontal) in [("xychart-beta\nx-axis [low, high]\nbar [1, 9]", false), ("xychart-beta horizontal\nx-axis [low, high]\nbar [1, 9]", true)] {
        let s = crate::scene(src, &paint, None).unwrap();
        let at = |t: &str| s.labels.iter().find(|l| l.text == t).unwrap().at;
        if horizontal {
            assert!(at("low")[1] < at("high")[1], "bands run down");
        } else {
            assert!(at("low")[0] < at("high")[0], "bands run across");
        }
    }
}
