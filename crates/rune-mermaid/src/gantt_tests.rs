use super::*;
use crate::date::days_from_civil;

fn gantt(src: &str) -> Gantt {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Gantt(g)) => g,
        _ => panic!("not a gantt chart: {src:?}"),
    }
}

fn day(y: i64, m: u32, d: u32) -> f64 {
    days_from_civil(y, m, d) as f64 * DAY
}

fn span_of<'a>(g: &'a Gantt, name: &str) -> (f64, f64, &'a Task) {
    let t = g.tasks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no task {name}"));
    (t.start, t.end, t)
}

#[test]
fn tasks_links_tags_and_sections() {
    let g = gantt(
        "gantt\n\
         title Release\n\
         dateFormat YYYY-MM-DD\n\
         axisFormat %b %d\n\
         section Build\n\
         Design :done, des, 2024-01-01, 2024-01-05\n\
         Code :active, code, after des, 5d\n\
         Test :crit, 3d\n\
         section Ship\n\
         Review :rev, after code test2, 2d\n\
         Launch :milestone, after des, until rev\n\
         Freeze :vert, 2024-01-08, 0d",
    );
    assert_eq!(g.title.as_deref(), Some("Release"));
    assert_eq!(g.sections, ["Build", "Ship"]);
    assert_eq!(g.axis, "%b %d");
    let (s, e, t) = span_of(&g, "Design");
    assert_eq!((s, e, t.tags.done, t.section), (day(2024, 1, 1), day(2024, 1, 5), true, Some(0)));
    let (s, e, t) = span_of(&g, "Code");
    assert_eq!((s, e, t.tags.active), (day(2024, 1, 5), day(2024, 1, 10), true));
    let (s, e, t) = span_of(&g, "Test");
    assert_eq!((s, e, t.tags.crit), (day(2024, 1, 10), day(2024, 1, 13), true), "follows the task before it");
    // `test2` names nothing, so `after` reads only `code`.
    assert_eq!(span_of(&g, "Review").0, day(2024, 1, 10));
    assert_eq!(span_of(&g, "Review").2.section, Some(1));
    let (s, e, t) = span_of(&g, "Launch");
    assert_eq!((s, e, t.tags.milestone), (day(2024, 1, 5), day(2024, 1, 10), true));
    assert!(span_of(&g, "Freeze").2.tags.vert);
}

#[test]
fn forward_links_resolve_and_loops_drop() {
    let g = gantt("gantt\nB :b, after a, 1d\nA :a, 2024-03-01, 2d\nC :c, after d, 1d\nD :d, after c, 1d");
    assert_eq!(span_of(&g, "B").0, day(2024, 3, 3));
    assert_eq!(g.tasks.len(), 2, "c and d wait on each other");
}

#[test]
fn excludes_push_durations_past_weekends_and_dates() {
    // 2024-01-05 is a Friday.
    let g = gantt("gantt\nexcludes weekends, 2024-01-09\nA :2024-01-05, 3d\nB :2024-01-05, 2024-01-08");
    assert_eq!(span_of(&g, "A").1, day(2024, 1, 11), "Fri, (Sat, Sun skipped), Mon, (Tue skipped), Wed");
    assert_eq!(span_of(&g, "B").1, day(2024, 1, 8), "a fixed end stays put");
    assert!(g.excludes.weekdays[0] && g.excludes.weekdays[6]);
    let g = gantt("gantt\nweekend friday\nexcludes weekends\nA :2024-01-04, 2d");
    assert_eq!(span_of(&g, "A").1, day(2024, 1, 8), "Thu, (Fri, Sat skipped), Sun");
}

#[test]
fn formats_inclusive_ends_and_bad_lines() {
    let g = gantt("gantt\ndateFormat DD/MM/YYYY\ninclusiveEndDates\nnot a task\nA :01/02/2024, 03/02/2024\nB :what, ever\nC :after nobody, 1d");
    assert_eq!(g.tasks.len(), 1);
    assert_eq!(span_of(&g, "A").1, day(2024, 2, 4), "the end date is the task's last day");
    assert!(crate::parse("gantt\ntitle Nothing\nsection Empty").is_none());
}

#[test]
fn draws_bars_labels_and_an_axis() {
    let paint = DiagramStyle::neutral(true);
    let s = crate::scene("gantt\ntitle Plan\nsection Work\nShort :2024-01-01, 1d\nA much longer task name :2024-01-02, 20d", &paint, None).unwrap();
    assert_eq!(s.title.as_deref(), Some("Plan"));
    let texts: Vec<&str> = s.labels.iter().map(|l| l.text.as_str()).collect();
    assert!(texts.contains(&"Work") && texts.contains(&"Short") && texts.contains(&"A much longer task name"), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("2024-01-")), "axis ticks: {texts:?}");
    let late = crate::scene("gantt\nA :a, 2024-01-01, 20d\nA long milestone name at the very end :milestone, after a, 0d", &paint, None).unwrap();
    assert!(late.size[0] <= crate::canvas::room(&paint) + 2.01, "a milestone at the end keeps its label inside: {}", late.size[0]);
    let budget = format!("gantt\n{}", (0..=MAX_ITEMS).map(|i| format!("T{i} :2024-01-01, 1d")).collect::<Vec<_>>().join("\n"));
    assert!(crate::parse(&budget).is_none());
}
