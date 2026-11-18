use super::*;

fn blocks(src: &str) -> BlockDiagram {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Block(b)) => b,
        _ => panic!("not a block diagram: {src:?}"),
    }
}

fn find<'a>(d: &'a BlockDiagram, id: &str) -> &'a Block {
    d.blocks.iter().find(|b| b.id == id).unwrap()
}

#[test]
fn rows_spans_groups_and_arrows() {
    let d = blocks(
        "block-beta\n  columns 3\n  a[\"Front end\"] b:2\n  space c((DB))\n  block:back:2\n    columns 2\n    d e\n  end\n  f<[\"go\"]>(right)\n  a --> b\n  b -- \"calls\" --> c\n  style a fill:#f96\n  classDef hot stroke:#f00\n  class c hot\n  g[unclosed",
    );
    assert_eq!(d.blocks[0].kind, Kind::Group { columns: Some(3), children: vec![1, 2, 3, 4, 5, 8, 9] });
    assert_eq!(find(&d, "a").label, "Front end");
    assert_eq!(find(&d, "b").span, 2);
    assert_eq!(d.blocks[3].kind, Kind::Space);
    assert_eq!(find(&d, "c").shape, NodeShape::Circle);
    assert_eq!(find(&d, "back").kind, Kind::Group { columns: Some(2), children: vec![6, 7] });
    assert_eq!(find(&d, "f").kind, Kind::Arrow(Point::Right));
    assert_eq!(find(&d, "g").label, "unclosed", "an unclosed shape keeps its text");
    assert_eq!(d.edges.len(), 2);
    assert_eq!(d.edges[1].label.as_deref(), Some("calls"));
    assert!(find(&d, "a").style.fill.is_some() && find(&d, "c").style.stroke.is_some());
    assert!(crate::parse("block-beta\n  columns 2").is_none());
    let deep = format!("block-beta\n{}x\n", "block\n".repeat(MAX_DEPTH + 1));
    assert!(crate::parse(&deep).is_none());
    let budget = format!("block-beta\n{}", (0..MAX_ITEMS).map(|i| format!("b{i}")).collect::<Vec<_>>().join(" "));
    assert!(crate::parse(&budget).is_none());
}

#[test]
fn blocks_wrap_at_their_columns() {
    let paint = DiagramStyle::neutral(false);
    let s = crate::scene("block-beta\ncolumns 2\nOne Two\nThree", &paint, None).unwrap();
    let at = |t: &str| s.labels.iter().find(|l| l.text == t).unwrap().at;
    assert!(at("One")[0] < at("Two")[0] && (at("One")[1] - at("Two")[1]).abs() < 1.0);
    assert!(at("Three")[1] > at("One")[1] + 10.0, "the third block starts a new row");
    let auto = crate::scene("block\nOne Two Three", &paint, None).unwrap();
    let ys: Vec<f32> = auto.labels.iter().map(|l| l.at[1]).collect();
    assert!(ys.iter().all(|y| (y - ys[0]).abs() < 1.0), "without columns, one row");
}
