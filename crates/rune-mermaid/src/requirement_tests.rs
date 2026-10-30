use super::*;

fn reqs(src: &str) -> Requirements {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Requirement(r)) => r,
        _ => panic!("not a requirement diagram: {src:?}"),
    }
}

#[test]
fn blocks_fields_and_relationships() {
    let d = reqs(
        "requirementDiagram\n    direction LR\n    requirement test_req {\n    id: 1\n    text: the test text.\n    risk: high\n    verifymethod: test\n    }\n    functionalRequirement \"Fast start\":::hot {\n    id: 1.1\n    risk: LOW }\n    element test_entity {\n    type: simulation\n    docref: reqs/test_entity\n    }\n    test_entity - satisfies -> test_req\n    test_req - contains -> \"Fast start\"\n    test_req <- copies - other\n    a - likes -> b\n    classDef hot fill:#f96",
    );
    assert_eq!(d.dir, Dir::Right);
    let names: Vec<&str> = d.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["test_req", "Fast start", "test_entity", "other"]);
    assert_eq!(d.nodes[0].fields, ["ID: 1", "Text: the test text.", "Risk: High", "Verification: Test"]);
    assert_eq!((d.nodes[1].kind.as_str(), d.nodes[1].fields.as_slice()), ("Functional Requirement", &["ID: 1.1".to_owned(), "Risk: Low".to_owned()][..]));
    assert_eq!(d.nodes[2].fields, ["Type: simulation", "Doc Ref: reqs/test_entity"]);
    assert_eq!(d.nodes[3].kind, "", "a name only related is a bare box");
    assert!(d.nodes[1].style.fill.is_some());
    assert_eq!(d.edges.len(), 3, "an unknown relationship is skipped");
    assert_eq!((d.edges[0].from, d.edges[0].to, d.edges[0].label.as_deref()), (2, 0, Some("«satisfies»")));
    assert_eq!((d.edges[1].start, d.edges[1].line), (Head::Contains, Line::Solid));
    assert_eq!((d.edges[2].from, d.edges[2].to, d.edges[2].end), (3, 0, Head::Arrow), "`<-` points back");
    assert!(crate::parse("requirementDiagram\n    direction TB").is_none());
}

#[test]
fn draws_each_box_with_its_fields() {
    let s = crate::scene("requirementDiagram\nrequirement r {\nid: 7\n}\nelement e {\ntype: code\n}\ne - verifies -> r", &DiagramStyle::neutral(false), None).unwrap();
    for t in ["«Requirement»", "r", "ID: 7", "«Element»", "Type: code", "«verifies»"] {
        assert!(s.labels.iter().any(|l| l.text == t), "{t}");
    }
}
