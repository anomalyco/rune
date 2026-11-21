use super::*;

fn pie(src: &str) -> Pie {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Pie(p)) => p,
        _ => panic!("not a pie chart: {src:?}"),
    }
}

#[test]
fn slices_titles_and_show_data() {
    let p = pie("pie showData title Pets adopted\n\"Dogs\" : 386\n\"Cats\" : 85.5\nRats : 15\n\"Bad\" : -3\n\"Zero\" : 0\nnot a slice\n\"x\" : lots");
    assert_eq!(p.title.as_deref(), Some("Pets adopted"));
    assert!(p.show_data);
    assert_eq!(p.slices, [("Dogs".to_owned(), 386.0), ("Cats".to_owned(), 85.5), ("Rats".to_owned(), 15.0)]);
    let p = pie("pie\n  title Key: value\n  \"A: one\" : 1");
    assert_eq!(p.title.as_deref(), Some("Key: value"));
    assert_eq!(p.slices[0].0, "A: one", "the value is after the last colon");
    assert!(!p.show_data);
    assert!(crate::parse("pie title Empty").is_none());
}

#[test]
fn numbers_print_short() {
    assert_eq!(number(386.0), "386");
    assert_eq!(number(85.5), "85.5");
    assert_eq!(number(1.0 / 3.0), "0.33");
}

#[test]
fn draws_percentages_and_a_legend() {
    let paint = DiagramStyle::neutral(false);
    let s = crate::scene("pie showData\n\"Dogs\" : 70\n\"Cats\" : 26\n\"Fish\" : 4", &paint, None).unwrap();
    let texts: Vec<&str> = s.labels.iter().map(|l| l.text.as_str()).collect();
    for t in ["70%", "26%", "Dogs", "Cats", "Fish", "70", "4"] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
    assert!(!texts.contains(&"4%"), "a sliver has no room for its percentage: {texts:?}");
    let budget = format!("pie\n{}", (0..=MAX_ITEMS).map(|i| format!("\"s{i}\" : 1")).collect::<Vec<_>>().join("\n"));
    assert!(crate::parse(&budget).is_none());
}
