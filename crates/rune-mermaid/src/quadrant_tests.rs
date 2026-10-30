use super::*;

fn quadrant(src: &str) -> Quadrant {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Quadrant(q)) => q,
        _ => panic!("not a quadrant chart: {src:?}"),
    }
}

#[test]
fn axes_quadrants_points_and_classes() {
    let q = quadrant(
        "quadrantChart\n    title Reach and engagement\n    x-axis Low Reach --> High Reach\n    y-axis Engagement\n    quadrant-1 We should expand\n    quadrant-4 May be improved\n    quadrant-9 Nope\n    Campaign A: [0.3, 0.6]\n    Campaign B: [0.45, 0.23] radius: 12, color: #ff3300, stroke-color: #10f0f0, stroke-width: 5px\n    Point C:::hot: [1.4, -0.2]\n    Broken: [x, 1]\n    classDef hot color: #109060, radius : 10",
    );
    assert_eq!(q.title.as_deref(), Some("Reach and engagement"));
    assert_eq!(q.x, ["Low Reach".to_owned(), "High Reach".to_owned()]);
    assert_eq!(q.y, ["Engagement".to_owned(), String::new()]);
    assert_eq!(q.names[0], "We should expand");
    assert_eq!(q.names[3], "May be improved");
    assert_eq!(q.names[1], "");
    let names: Vec<&str> = q.points.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Campaign A", "Campaign B", "Point C"]);
    assert_eq!(q.points[1].look.radius, Some(12.0));
    assert_eq!(q.points[1].look.stroke_width, Some(5.0));
    assert!(q.points[1].look.color.is_some() && q.points[1].look.stroke.is_some());
    assert_eq!(q.points[2].at, [1.0, 0.0], "clamped into the chart");
    assert_eq!(q.points[2].look.radius, Some(10.0), "from its class, defined later");
    assert!(crate::parse("quadrantChart\n    title Only a title").is_none());
    let budget = format!("quadrantChart\n{}", (0..=MAX_ITEMS).map(|i| format!("p{i}: [0.5, 0.5]")).collect::<Vec<_>>().join("\n"));
    assert!(crate::parse(&budget).is_none());
}

#[test]
fn points_land_by_their_values() {
    let paint = DiagramStyle::neutral(false);
    let s = crate::scene("quadrantChart\nquadrant-1 Q1\nquadrant-3 Q3\nHigh: [0.9, 0.9]\nLow: [0.1, 0.1]", &paint, None).unwrap();
    let at = |t: &str| s.labels.iter().find(|l| l.text == t).unwrap().at;
    assert!(at("High")[0] > at("Low")[0] && at("High")[1] < at("Low")[1]);
    assert!(at("Q1")[0] > at("Q3")[0] && at("Q1")[1] < at("Q3")[1]);
}
