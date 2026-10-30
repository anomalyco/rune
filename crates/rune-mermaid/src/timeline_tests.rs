use super::*;

fn timeline(src: &str) -> Timeline {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Timeline(t)) => t,
        _ => panic!("not a timeline: {src:?}"),
    }
}

#[test]
fn periods_events_and_sections() {
    let t = timeline(
        "timeline\n    title History of Social Media\n    2002 : LinkedIn\n    2004 : Facebook : Google\n         : MySpace\n    section Later\n    2010 : Instagram\n    2012\n    accTitle: skipped",
    );
    assert_eq!(t.title.as_deref(), Some("History of Social Media"));
    assert_eq!(t.sections, ["Later"]);
    let got: Vec<(&str, Vec<&str>, Option<usize>)> =
        t.periods.iter().map(|p| (p.time.as_str(), p.events.iter().map(String::as_str).collect(), p.section)).collect();
    assert_eq!(
        got,
        [
            ("2002", vec!["LinkedIn"], None),
            ("2004", vec!["Facebook", "Google", "MySpace"], None),
            ("2010", vec!["Instagram"], Some(0)),
            ("2012", vec![], Some(0)),
        ]
    );
    assert!(!t.down);
    assert!(timeline("timeline TD\n2020 : a").down);
    assert!(crate::parse("timeline\n    title Nothing").is_none());
    let budget = format!("timeline\n{}", (0..=MAX_ITEMS).map(|i| format!("{i} : e")).collect::<Vec<_>>().join("\n"));
    assert!(crate::parse(&budget).is_none());
}

#[test]
fn runs_across_until_it_would_not_fit() {
    let paint = DiagramStyle::neutral(false);
    let across = crate::scene("timeline\n2001 : a\n2002 : b\n2003 : c", &paint, None).unwrap();
    let ys: Vec<f32> = ["2001", "2002", "2003"].iter().map(|t| across.labels.iter().find(|l| l.text == *t).unwrap().at[1]).collect();
    assert!(ys.windows(2).all(|w| (w[0] - w[1]).abs() < 0.5), "one row: {ys:?}");
    let many = format!("timeline\n{}", (2000..2012).map(|y| format!("{y} : event")).collect::<Vec<_>>().join("\n"));
    let down = crate::scene(&many, &paint, None).unwrap();
    let y = |t: &str| down.labels.iter().find(|l| l.text == t).unwrap().at[1];
    assert!(y("2001") > y("2000") && y("2011") > y("2010"), "top to bottom");
    assert!(down.size[0] <= crate::canvas::room(&paint) + 2.01);
}
