use super::*;

fn journey(src: &str) -> Journey {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Journey(j)) => j,
        _ => panic!("not a journey: {src:?}"),
    }
}

#[test]
fn sections_tasks_scores_and_actors() {
    let j = journey(
        "journey\n    title My working day\n    section Go to work\n      Make tea: 5: Me\n      Go upstairs: 3: Me\n      Do work: 1: Me, Cat\n    section Go home\n      Go downstairs: 9: Cat\n      No score here\n      Bad: lots: Me\n      Sit down: 0",
    );
    assert_eq!(j.title.as_deref(), Some("My working day"));
    assert_eq!(j.sections, ["Go to work", "Go home"]);
    assert_eq!(j.actors, ["Me", "Cat"]);
    let got: Vec<(&str, f32, &[usize], Option<usize>)> = j.tasks.iter().map(|t| (t.name.as_str(), t.score, t.actors.as_slice(), t.section)).collect();
    assert_eq!(
        got,
        [
            ("Make tea", 5.0, &[0][..], Some(0)),
            ("Go upstairs", 3.0, &[0][..], Some(0)),
            ("Do work", 1.0, &[0, 1][..], Some(0)),
            ("Go downstairs", 5.0, &[1][..], Some(1)),
            ("Sit down", 1.0, &[][..], Some(1)),
        ]
    );
    assert!(crate::parse("journey\ntitle Empty\nsection A").is_none());
    let budget = format!("journey\n{}", (0..=MAX_ITEMS).map(|i| format!("t{i}: 3: Me")).collect::<Vec<_>>().join("\n"));
    assert!(crate::parse(&budget).is_none());
}

#[test]
fn draws_tasks_sections_and_actors() {
    let paint = DiagramStyle::neutral(false);
    let s = crate::scene("journey\nsection Day\nGood: 5: Me\nBad: 1: Me", &paint, None).unwrap();
    let texts: Vec<&str> = s.labels.iter().map(|l| l.text.as_str()).collect();
    for t in ["Day", "Good", "Bad", "Me"] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
    let low = |p: &DiagramStyle, score: f32| mood(score, p).to_oklab();
    assert!(low(&paint, 5.0)[0] > 0.5 && low(&paint, 1.0)[0] > 0.5);
}
